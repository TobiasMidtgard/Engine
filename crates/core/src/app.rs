//! [`App`]: the world, its schedules, and the plugins that fill them.
//!
//! ```no_run
//! use engine_core::prelude::*;
//!
//! #[derive(Component, Serialize, Deserialize, Default)]
//! struct Spin(f32);
//!
//! fn spin(time: Res<Time>, mut q: Query<(&Spin, &mut Transform)>) {
//!     for (spin, mut t) in &mut q {
//!         t.rotate_y(spin.0 * time.delta_secs());
//!     }
//! }
//!
//! struct SpinPlugin;
//! impl Plugin for SpinPlugin {
//!     fn build(&self, app: &mut App) {
//!         app.register_component::<Spin>("Rotates the entity around +Y, radians per second.")
//!             .add_systems(Update, spin);
//!     }
//! }
//!
//! App::new().add_plugins(SpinPlugin).run();
//! ```

use std::any::TypeId;
use std::collections::HashSet;

use bevy_ecs::message::{
    Message, MessageCursor, MessageRegistry, Messages, message_update_condition,
    message_update_system, signal_message_update_system,
};
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, ScheduleLabel, Schedules};
use bevy_ecs::system::ScheduleSystem;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::config::EngineConfig;
use crate::diagnostics::{self, Diagnostics};
use crate::registry::{ApiRegistry, TypeRegistry};
use crate::schedule::*;
use crate::time::{self, FixedTime, LocalTimeScale, Time, TimeDilation, TimeSource};
use crate::transform::{self, Transform, TransformInterpolation};
use crate::window::{Window, WindowCloseRequested, WindowResized};

/// Send this message to stop the app at the end of the frame.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub enum AppExit {
    Success,
    Error(u8),
}

impl AppExit {
    pub fn is_success(&self) -> bool {
        matches!(self, AppExit::Success)
    }
}

/// A packaged feature. Plugins add resources, components, systems and API functions.
///
/// `build` runs immediately when the plugin is added. `finish` runs once, after every plugin
/// has been built and before the first frame, for work that needs other plugins to exist.
pub trait Plugin: Send + Sync + 'static {
    fn build(&self, app: &mut App);
    fn finish(&self, _app: &mut App) {}
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }
}

type Runner = Box<dyn FnOnce(App) -> AppExit>;

/// The engine application: an ECS [`World`] plus the schedules that run on it each frame.
pub struct App {
    pub world: World,
    plugins: Vec<Box<dyn Plugin>>,
    plugin_names: HashSet<String>,
    finished: bool,
    started: bool,
    exit_cursor: MessageCursor<AppExit>,
    runner: Option<Runner>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

const MAIN_SCHEDULES: &[fn() -> Schedule] = &[
    || Schedule::new(Startup),
    || Schedule::new(First),
    || Schedule::new(PreUpdate),
    || Schedule::new(FixedPreUpdate),
    || Schedule::new(FixedUpdate),
    || Schedule::new(FixedPostUpdate),
    || Schedule::new(Update),
    || Schedule::new(PostUpdate),
    || Schedule::new(Extract),
    || Schedule::new(Render),
    || Schedule::new(Last),
];

impl App {
    /// A new app with the engine's core resources, schedules and systems installed.
    pub fn new() -> Self {
        crate::tasks::init_task_pools();
        let mut world = World::new();
        let mut schedules = Schedules::new();
        for make in MAIN_SCHEDULES {
            schedules.insert(make());
        }
        world.insert_resource(schedules);

        let mut app = App {
            world,
            plugins: Vec::new(),
            plugin_names: HashSet::new(),
            finished: false,
            started: false,
            exit_cursor: MessageCursor::default(),
            runner: Some(Box::new(default_runner)),
        };

        app.init_resource::<EngineConfig>()
            .init_resource::<Time>()
            .init_resource::<FixedTime>()
            .init_resource::<TimeDilation>()
            .init_resource::<TypeRegistry>()
            .init_resource::<ApiRegistry>()
            .init_resource::<Diagnostics>()
            .init_resource::<crate::profile::ProfileWindow>()
            .init_resource::<Window>()
            .insert_resource(crate::log::log_buffer())
            .add_message::<AppExit>()
            .add_message::<WindowResized>()
            .add_message::<WindowCloseRequested>();

        app.configure_sets(
            FixedUpdate,
            (
                SimSet::Input,
                SimSet::Gameplay,
                SimSet::Physics,
                SimSet::PostPhysics,
                SimSet::Environment,
            )
                .chain(),
        )
        .configure_sets(Update, (UpdateSet::Logic, UpdateSet::Camera).chain())
        .configure_sets(
            PostUpdate,
            (UpdateSet::Propagate, UpdateSet::AfterPropagate).chain(),
        );

        app.add_systems(
            First,
            (
                time::time_system,
                message_update_system.run_if(message_update_condition),
                diagnostics::frame_diagnostics_system,
            )
                .chain(),
        )
        .add_systems(FixedPreUpdate, transform::snapshot_interpolation_system)
        .add_systems(
            Last,
            crate::profile::profile_report_system.run_if(crate::profile::enabled),
        )
        .add_systems(FixedPostUpdate, signal_message_update_system)
        .add_systems(
            PostUpdate,
            transform::propagate_transforms_system.in_set(UpdateSet::Propagate),
        );

        app.register_component::<Transform>(
            "Position (f64 meters), rotation and scale relative to the parent (or world).",
        )
        .register_component::<TransformInterpolation>(
            "Smooths rendering of entities moved in FixedUpdate. Add to physics-driven entities.",
        )
        .register_component::<LocalTimeScale>(
            "Per-entity time scale. `factor` multiplies this entity's delta; \
             `ignore_world_dilation` lets it move at full speed through global slow motion.",
        )
        .register_component::<Name>("Human-readable entity name.")
        .register_resource::<Window>("Primary window: size, focus, cursor mode, title.")
        .register_resource::<EngineConfig>("Startup configuration.");

        crate::registry::register_core_api(&mut app);
        time::register_api(&mut app);
        app
    }

    // ---------------------------------------------------------------- plugins

    /// Add one plugin, a plugin group, or a tuple of them.
    pub fn add_plugins<M>(&mut self, plugins: impl Plugins<M>) -> &mut Self {
        plugins.add_to_app(self);
        self
    }

    /// Add an already-boxed plugin. Adding the same plugin type twice keeps the first copy: the
    /// second one, settings included, is ignored with a warning. To configure a plugin that a
    /// group already adds, replace it in the group with [`PluginGroup::set`].
    pub fn add_boxed_plugin(&mut self, plugin: Box<dyn Plugin>) -> &mut Self {
        let name = plugin.name().to_string();
        if !self.plugin_names.insert(name.clone()) {
            tracing::warn!(
                "plugin {name} was added twice; the second copy and its settings are ignored. \
                 To configure a plugin that a group (like DefaultPlugins) already adds, replace \
                 it in the group with `DefaultPlugins.set(...)`, or add your copy before the \
                 group"
            );
            return self;
        }
        tracing::debug!("building plugin {name}");
        plugin.build(self);
        self.plugins.push(plugin);
        self
    }

    /// True if a plugin with this type has been added.
    pub fn has_plugin<P: Plugin>(&self) -> bool {
        self.plugin_names.contains(std::any::type_name::<P>())
    }

    /// Names of all added plugins, in the order they were added.
    pub fn plugin_names(&self) -> Vec<String> {
        self.plugins.iter().map(|p| p.name().to_string()).collect()
    }

    // ---------------------------------------------------------------- systems

    pub fn add_systems<M>(
        &mut self,
        schedule: impl ScheduleLabel,
        systems: impl IntoScheduleConfigs<ScheduleSystem, M>,
    ) -> &mut Self {
        self.world
            .resource_mut::<Schedules>()
            .add_systems(schedule, systems);
        self
    }

    pub fn configure_sets<M>(
        &mut self,
        schedule: impl ScheduleLabel,
        sets: impl IntoScheduleConfigs<bevy_ecs::schedule::InternedSystemSet, M>,
    ) -> &mut Self {
        self.world
            .resource_mut::<Schedules>()
            .configure_sets(schedule, sets);
        self
    }

    /// Access a schedule directly (for advanced configuration such as executor kind).
    pub fn edit_schedule(
        &mut self,
        label: impl ScheduleLabel,
        f: impl FnOnce(&mut Schedule),
    ) -> &mut Self {
        let mut schedules = self.world.resource_mut::<Schedules>();
        f(schedules.entry(label));
        self
    }

    // ---------------------------------------------------------------- data

    pub fn insert_resource<R: Resource>(&mut self, resource: R) -> &mut Self {
        self.world.insert_resource(resource);
        self
    }

    pub fn init_resource<R: Resource + FromWorld>(&mut self) -> &mut Self {
        self.world.init_resource::<R>();
        self
    }

    pub fn insert_non_send_resource<R: 'static>(&mut self, resource: R) -> &mut Self {
        self.world.insert_non_send(resource);
        self
    }

    /// Register a buffered message type (read with `MessageReader<T>`, write with
    /// `MessageWriter<T>`). Messages live for two frames.
    pub fn add_message<T: Message>(&mut self) -> &mut Self {
        if !self.world.contains_resource::<Messages<T>>() {
            MessageRegistry::register_message::<T>(&mut self.world);
        }
        self
    }

    /// Make a component visible to scenes, the AI bridge, scripting and inspectors by name.
    pub fn register_component<T>(&mut self, doc: &str) -> &mut Self
    where
        T: Component<Mutability = bevy_ecs::component::Mutable> + Serialize + DeserializeOwned,
    {
        self.world
            .resource_mut::<TypeRegistry>()
            .register_component::<T>(doc);
        self
    }

    /// Make a resource visible to the AI bridge, scripting and inspectors by name.
    pub fn register_resource<T>(&mut self, doc: &str) -> &mut Self
    where
        T: Resource + Serialize + DeserializeOwned,
    {
        self.world
            .resource_mut::<TypeRegistry>()
            .register_resource::<T>(doc);
        self
    }

    /// Expose a named function to the AI bridge, console and scripts.
    ///
    /// `args_example` is a JSON example of the arguments (shown to AI agents).
    pub fn register_api(
        &mut self,
        name: &str,
        summary: &str,
        args_example: &str,
        f: impl Fn(&mut World, serde_json::Value) -> Result<serde_json::Value, String>
        + Send
        + Sync
        + 'static,
    ) -> &mut Self {
        self.world
            .resource_mut::<ApiRegistry>()
            .register(name, summary, args_example, f);
        self
    }

    // ---------------------------------------------------------------- running

    /// Replace how [`App::run`] drives frames (the platform plugin installs a window loop).
    pub fn set_runner(&mut self, runner: impl FnOnce(App) -> AppExit + 'static) -> &mut Self {
        self.runner = Some(Box::new(runner));
        self
    }

    /// Call every plugin's `finish` once. Called automatically before the first frame.
    pub fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let mut i = 0;
        // Plugins may add plugins during finish; walk until no new ones appear.
        while i < self.plugins.len() {
            let plugins = std::mem::take(&mut self.plugins);
            for plugin in &plugins[i..] {
                plugin.finish(self);
            }
            i = plugins.len();
            let added = std::mem::replace(&mut self.plugins, plugins);
            self.plugins.extend(added);
        }
    }

    /// Run exactly one frame (running `Startup` first if this is the first frame).
    pub fn update(&mut self) {
        self.finish();
        if !self.started {
            self.started = true;
            self.world.run_schedule(Startup);
        }
        run_frame(&mut self.world);
        self.world.clear_trackers();
    }

    /// Run `n` frames. Useful for tests, headless tools and AI-driven stepping.
    pub fn update_n(&mut self, n: usize) {
        for _ in 0..n {
            self.update();
        }
    }

    /// If an [`AppExit`] was sent since the last check, return it.
    pub fn should_exit(&mut self) -> Option<AppExit> {
        let messages = self.world.get_resource::<Messages<AppExit>>()?;
        self.exit_cursor.read(messages).last().cloned()
    }

    /// Run the app with its runner until it exits.
    pub fn run(&mut self) -> AppExit {
        let mut app = std::mem::replace(self, App::empty());
        let runner = app
            .runner
            .take()
            .unwrap_or_else(|| Box::new(default_runner));
        app.finish();
        runner(app)
    }

    /// A bare app with no resources or schedules (used internally by [`App::run`]).
    pub fn empty() -> App {
        App {
            world: World::new(),
            plugins: Vec::new(),
            plugin_names: HashSet::new(),
            finished: true,
            started: true,
            exit_cursor: MessageCursor::default(),
            runner: None,
        }
    }
}

/// One frame: see [`crate::schedule`] for the order.
pub fn run_frame(world: &mut World) {
    world.run_schedule(First);
    world.run_schedule(PreUpdate);

    let (steps, budget) = {
        let fixed = world.resource::<FixedTime>();
        let realtime = matches!(world.resource::<Time>().source(), TimeSource::Realtime);
        let budget = realtime.then(|| std::time::Duration::from_secs_f64(fixed.step_budget_secs()));
        (fixed.steps_this_frame(), budget)
    };
    let started = std::time::Instant::now();
    for i in 0..steps {
        time::enter_fixed_step(world, i);
        world.run_schedule(FixedPreUpdate);
        world.run_schedule(FixedUpdate);
        world.run_schedule(FixedPostUpdate);
        if i + 1 < steps && budget.is_some_and(|b| started.elapsed() >= b) {
            world.resource_mut::<FixedTime>().stop_early(i + 1);
            break;
        }
    }
    time::exit_fixed_steps(world);

    world.run_schedule(Update);
    world.run_schedule(PostUpdate);
    world.run_schedule(Extract);
    world.run_schedule(Render);
    world.run_schedule(Last);
}

/// Runs frames as fast as possible until an [`AppExit`] is sent. Used when no platform
/// runner is installed (headless servers, tools).
fn default_runner(mut app: App) -> AppExit {
    loop {
        app.update();
        if let Some(exit) = app.should_exit() {
            return exit;
        }
    }
}

// -------------------------------------------------------------------- plugin sets

/// Anything that can be passed to [`App::add_plugins`]: a plugin, a group, or a tuple.
pub trait Plugins<Marker> {
    fn add_to_app(self, app: &mut App);
}

#[doc(hidden)]
pub struct PluginMarker;
#[doc(hidden)]
pub struct PluginGroupMarker;
#[doc(hidden)]
pub struct PluginsTupleMarker;

impl<P: Plugin> Plugins<PluginMarker> for P {
    fn add_to_app(self, app: &mut App) {
        app.add_boxed_plugin(Box::new(self));
    }
}

impl<G: PluginGroup> Plugins<PluginGroupMarker> for G {
    fn add_to_app(self, app: &mut App) {
        self.build().finish(app);
    }
}

macro_rules! impl_plugins_tuples {
    ($(($P:ident, $M:ident, $p:ident)),*) => {
        impl<$($P: Plugins<$M>, $M),*> Plugins<(PluginsTupleMarker, $($M,)*)> for ($($P,)*) {
            fn add_to_app(self, app: &mut App) {
                let ($($p,)*) = self;
                $($p.add_to_app(app);)*
            }
        }
    };
}
impl_plugins_tuples!((P0, M0, p0));
impl_plugins_tuples!((P0, M0, p0), (P1, M1, p1));
impl_plugins_tuples!((P0, M0, p0), (P1, M1, p1), (P2, M2, p2));
impl_plugins_tuples!((P0, M0, p0), (P1, M1, p1), (P2, M2, p2), (P3, M3, p3));
impl_plugins_tuples!(
    (P0, M0, p0),
    (P1, M1, p1),
    (P2, M2, p2),
    (P3, M3, p3),
    (P4, M4, p4)
);
impl_plugins_tuples!(
    (P0, M0, p0),
    (P1, M1, p1),
    (P2, M2, p2),
    (P3, M3, p3),
    (P4, M4, p4),
    (P5, M5, p5)
);
impl_plugins_tuples!(
    (P0, M0, p0),
    (P1, M1, p1),
    (P2, M2, p2),
    (P3, M3, p3),
    (P4, M4, p4),
    (P5, M5, p5),
    (P6, M6, p6)
);
impl_plugins_tuples!(
    (P0, M0, p0),
    (P1, M1, p1),
    (P2, M2, p2),
    (P3, M3, p3),
    (P4, M4, p4),
    (P5, M5, p5),
    (P6, M6, p6),
    (P7, M7, p7)
);
impl_plugins_tuples!(
    (P0, M0, p0),
    (P1, M1, p1),
    (P2, M2, p2),
    (P3, M3, p3),
    (P4, M4, p4),
    (P5, M5, p5),
    (P6, M6, p6),
    (P7, M7, p7),
    (P8, M8, p8)
);
impl_plugins_tuples!(
    (P0, M0, p0),
    (P1, M1, p1),
    (P2, M2, p2),
    (P3, M3, p3),
    (P4, M4, p4),
    (P5, M5, p5),
    (P6, M6, p6),
    (P7, M7, p7),
    (P8, M8, p8),
    (P9, M9, p9)
);
impl_plugins_tuples!(
    (P0, M0, p0),
    (P1, M1, p1),
    (P2, M2, p2),
    (P3, M3, p3),
    (P4, M4, p4),
    (P5, M5, p5),
    (P6, M6, p6),
    (P7, M7, p7),
    (P8, M8, p8),
    (P9, M9, p9),
    (P10, M10, p10),
    (P11, M11, p11)
);

/// A named, configurable list of plugins (for example `DefaultPlugins`).
///
/// ```ignore
/// App::new().add_plugins(DefaultPlugins.disable::<AudioPlugin>().set(WindowPlugin { .. }));
/// ```
pub trait PluginGroup: Sized {
    fn build(self) -> PluginGroupBuilder;

    /// Replace the plugin of type `P` in this group with `plugin`.
    fn set<P: Plugin>(self, plugin: P) -> PluginGroupBuilder {
        self.build().set(plugin)
    }

    /// Leave the plugin of type `P` out of this group.
    fn disable<P: Plugin>(self) -> PluginGroupBuilder {
        self.build().disable::<P>()
    }
}

/// Ordered, editable list of plugins produced by a [`PluginGroup`].
#[derive(Default)]
pub struct PluginGroupBuilder {
    entries: Vec<(TypeId, Box<dyn Plugin>, bool)>,
}

impl PluginGroupBuilder {
    pub fn start() -> Self {
        Self::default()
    }

    /// Append a plugin (replacing an earlier one of the same type).
    #[allow(clippy::should_implement_trait)] // builder method, same name as Bevy's
    pub fn add<P: Plugin>(mut self, plugin: P) -> Self {
        let id = TypeId::of::<P>();
        self.entries.retain(|(t, _, _)| *t != id);
        self.entries.push((id, Box::new(plugin), true));
        self
    }

    /// Append every plugin of another group.
    pub fn add_group(mut self, group: impl PluginGroup) -> Self {
        for entry in group.build().entries {
            self.entries.retain(|(t, _, _)| *t != entry.0);
            self.entries.push(entry);
        }
        self
    }

    /// Replace the plugin of type `P`, keeping its position. Panics if it is not in the group.
    pub fn set<P: Plugin>(mut self, plugin: P) -> Self {
        let id = TypeId::of::<P>();
        let entry = self
            .entries
            .iter_mut()
            .find(|(t, _, _)| *t == id)
            .unwrap_or_else(|| {
                panic!(
                    "{} is not part of this plugin group",
                    std::any::type_name::<P>()
                )
            });
        entry.1 = Box::new(plugin);
        self
    }

    /// Leave the plugin of type `P` out.
    pub fn disable<P: Plugin>(mut self) -> Self {
        let id = TypeId::of::<P>();
        for entry in &mut self.entries {
            if entry.0 == id {
                entry.2 = false;
            }
        }
        self
    }

    /// Re-enable a plugin that was disabled.
    pub fn enable<P: Plugin>(mut self) -> Self {
        let id = TypeId::of::<P>();
        for entry in &mut self.entries {
            if entry.0 == id {
                entry.2 = true;
            }
        }
        self
    }

    pub fn finish(self, app: &mut App) {
        for (_, plugin, enabled) in self.entries {
            if enabled {
                app.add_boxed_plugin(plugin);
            }
        }
    }
}

impl PluginGroup for PluginGroupBuilder {
    fn build(self) -> PluginGroupBuilder {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct Counter(u32);

    struct CountPlugin;
    impl Plugin for CountPlugin {
        fn build(&self, app: &mut App) {
            app.init_resource::<Counter>()
                .add_systems(Update, |mut c: ResMut<Counter>| c.0 += 1);
        }
    }

    #[test]
    fn plugin_systems_run_each_frame() {
        let mut app = App::new();
        app.add_plugins(CountPlugin);
        app.update_n(3);
        assert_eq!(app.world.resource::<Counter>().0, 3);
    }

    #[test]
    fn duplicate_plugins_are_ignored() {
        let mut app = App::new();
        // Watched, like every test that makes the duplicate warning, so its first use never
        // happens unobserved on a parallel test thread (see `warnings_during`).
        let warnings = warnings_during(|| {
            app.add_plugins((CountPlugin, CountPlugin));
        });
        app.update();
        assert_eq!(app.world.resource::<Counter>().0, 1);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn exit_message_is_seen() {
        let mut app = App::new();
        app.add_systems(Update, |mut w: MessageWriter<AppExit>| {
            w.write(AppExit::Success);
        });
        app.update();
        assert_eq!(app.should_exit(), Some(AppExit::Success));
    }

    struct Group;
    impl PluginGroup for Group {
        fn build(self) -> PluginGroupBuilder {
            PluginGroupBuilder::start().add(CountPlugin)
        }
    }

    #[test]
    fn groups_can_disable() {
        let mut app = App::new();
        app.add_plugins(Group.disable::<CountPlugin>());
        assert!(!app.has_plugin::<CountPlugin>());
    }

    #[derive(Resource)]
    struct Setting(u32);

    /// A plugin with settings, like `AiBridgePlugin::listen(..)`.
    struct SettingPlugin(u32);
    impl Plugin for SettingPlugin {
        fn build(&self, app: &mut App) {
            app.insert_resource(Setting(self.0));
        }
    }

    /// A group that includes `SettingPlugin` with its default settings, like `DefaultPlugins`.
    struct SettingGroup;
    impl PluginGroup for SettingGroup {
        fn build(self) -> PluginGroupBuilder {
            PluginGroupBuilder::start()
                .add(CountPlugin)
                .add(SettingPlugin(1))
        }
    }

    /// Messages of the warnings logged on this thread while `f` runs.
    ///
    /// Tests run on parallel threads, and `tracing` caches per log statement whether anyone
    /// listens. If another thread uses the statement first, while this subscriber is being set
    /// up, the cache can say "nobody" and the warning is dropped; so the cache is rebuilt once
    /// the subscriber is in place.
    fn warnings_during(f: impl FnOnce()) -> Vec<String> {
        use std::sync::{Arc, Mutex};
        use tracing::field::{Field, Visit};
        use tracing_subscriber::layer::{Context, SubscriberExt};

        struct Message<'a>(&'a mut String);
        impl Visit for Message<'_> {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    *self.0 = format!("{value:?}");
                }
            }
        }

        #[derive(Clone, Default)]
        struct Warnings(Arc<Mutex<Vec<String>>>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Warnings {
            fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
                if *event.metadata().level() == tracing::Level::WARN {
                    let mut text = String::new();
                    event.record(&mut Message(&mut text));
                    self.0.lock().unwrap().push(text);
                }
            }
        }

        let warnings = Warnings::default();
        let subscriber = tracing_subscriber::registry().with(warnings.clone());
        tracing::subscriber::with_default(subscriber, || {
            tracing::callsite::rebuild_interest_cache();
            f()
        });
        warnings.0.lock().unwrap().clone()
    }

    #[test]
    fn group_plugins_are_configured_with_set() {
        let mut app = App::new();
        app.add_plugins(SettingGroup.set(SettingPlugin(2)));
        assert_eq!(app.world.resource::<Setting>().0, 2);
        assert_eq!(
            app.plugin_names()
                .iter()
                .filter(|n| n.ends_with("SettingPlugin"))
                .count(),
            1
        );
    }

    #[test]
    fn adding_a_group_plugin_again_says_how_to_configure_it() {
        let mut app = App::new();
        app.add_plugins(SettingGroup);
        let warnings = warnings_during(|| {
            app.add_plugins(SettingPlugin(2));
        });
        assert_eq!(
            app.world.resource::<Setting>().0,
            1,
            "the second copy is ignored"
        );
        let [warning] = warnings.as_slice() else {
            panic!("expected one warning, got {warnings:?}");
        };
        for needed in [
            "SettingPlugin",
            "settings are ignored",
            "DefaultPlugins.set(",
            "before the group",
        ] {
            assert!(
                warning.contains(needed),
                "the warning should mention {needed:?}: {warning}"
            );
        }
    }
}
