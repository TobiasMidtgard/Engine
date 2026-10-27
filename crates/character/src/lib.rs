#![doc = include_str!("../README.md")]

pub mod api;
pub mod appearance;
pub mod build;
pub mod clothes;
pub mod demo;
pub mod eyes;
pub mod face;
pub mod feature;
pub mod hair_motion;
pub mod hands;
pub mod headgear;
pub mod hood;
pub mod lod;
pub mod mesh;
pub mod morph;
pub mod mouth;
pub mod pack;
pub mod rig;
pub mod shoes;
pub mod skirt;
pub mod sliders;
pub mod strands;
pub mod subdiv;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use engine_core::prelude::*;
use engine_render::RenderAppExt;
pub use engine_world::character::CharacterQuality;
use engine_world::character::{AppearanceBodies, BodyFit};

pub use appearance::{
    Ancestry, AncestryExt, Appearance, AppearanceExt, AppearanceRequest, Beard, BeardStyle, Brows,
    Eyes, Hair, HairColor, HairStyle, Skin,
};
pub use build::{BuildOptions, CharacterMeshes};
pub use feature::{CharacterFeature, CharacterPipelinesReady, GpuCharacter};
pub use lod::CharacterLod;
pub use mesh::Region;
pub use pack::HumanPack;

/// How a character is built: how much of the body, how smooth, and whether it has hair.
/// Without it, the shared [`CharacterQuality`] override picks the level (close-up by default).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterBuild {
    /// `body` (the whole person, dressed), `bust` (head, neck, shoulders) or `head`.
    pub region: RegionName,
    /// Skin subdivision levels, 0..=2 (2 for close-ups).
    pub levels: u32,
    /// Grow hair strands.
    pub hair: bool,
}

/// Serializable [`Region`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegionName {
    Head,
    Bust,
    #[default]
    Body,
}

impl Default for CharacterBuild {
    fn default() -> Self {
        Self {
            region: RegionName::Body,
            levels: 2,
            hair: true,
        }
    }
}

impl CharacterBuild {
    /// A close-up of head and shoulders (the creator's portraits).
    pub fn portrait() -> Self {
        Self {
            region: RegionName::Bust,
            ..Self::default()
        }
    }

    /// The build for a shared detail level.
    pub fn for_quality(q: CharacterQuality) -> Self {
        match q {
            CharacterQuality::CloseUp => Self::default(),
            CharacterQuality::Mid => Self {
                levels: 1,
                ..Self::default()
            },
            CharacterQuality::Crowd => Self {
                levels: 0,
                hair: false,
                ..Self::default()
            },
        }
    }

    /// The build options this asks for.
    pub fn options(&self) -> BuildOptions {
        BuildOptions {
            region: match self.region {
                RegionName::Head => Region::Head,
                RegionName::Bust => Region::Bust,
                RegionName::Body => Region::Body,
            },
            levels: self.levels.min(2),
            hair: self.hair,
            face: self.levels > 0,
        }
    }
}

/// The meshes built for an entity's [`Appearance`] (replaced whenever it changes).
#[derive(Component, Clone)]
pub struct BuiltCharacter {
    pub meshes: Arc<CharacterMeshes>,
    /// Increases with every rebuild.
    pub version: u64,
    /// Colors and landmarks for the shaders.
    pub uniform: GpuCharacter,
    /// The appearance it was built from.
    pub appearance: Appearance,
    /// How it was built (a different region or detail level rebuilds it).
    pub options: BuildOptions,
}

/// A build running in the background for an entity.
#[derive(Component)]
struct PendingBuild {
    rx: Mutex<Receiver<(Appearance, BuildOptions, CharacterMeshes)>>,
}

/// Debug views of the character shaders: 0 off, 1 region masks, 2 normals, 3 curvature.
#[derive(Resource, Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterDebug {
    pub view: u8,
}

/// Whether skin uses the renderer's subsurface pass (when `RenderSettings::subsurface_width`
/// is above 0; on by default) instead of the skin shader's own imitation of it.
#[derive(Resource, Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterSubsurface {
    pub pass: bool,
}

impl Default for CharacterSubsurface {
    fn default() -> Self {
        Self { pass: true }
    }
}

/// Where the human pack stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackStatus {
    Loading,
    Ready,
    /// The pack couldn't be read; the message says what to do. Characters aren't drawn.
    Missing(String),
}

/// The human pack and its region masks, loaded in the background at startup.
/// A loaded pack and its regions, or why loading failed.
type LoadResult = Result<(HumanPack, mesh::Regions), String>;

#[derive(Resource)]
pub struct HumanAssets {
    pub pack: Option<Arc<HumanPack>>,
    pub regions: Option<Arc<mesh::Regions>>,
    pub status: PackStatus,
    rx: Mutex<Option<Receiver<LoadResult>>>,
}

impl HumanAssets {
    /// Assets already in memory (tests, tools).
    pub fn ready(pack: HumanPack) -> Self {
        let regions = mesh::Regions::new(&pack);
        Self {
            pack: Some(Arc::new(pack)),
            regions: Some(Arc::new(regions)),
            status: PackStatus::Ready,
            rx: Mutex::new(None),
        }
    }

    fn loading(path: PathBuf) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = std::fs::read(&path)
                .map_err(|e| {
                    format!(
                        "the human pack {} could not be read ({e}). Run `cargo run -p fetch-assets -- character` to download it.",
                        path.display()
                    )
                })
                .and_then(|bytes| HumanPack::parse(&bytes))
                .map(|pack| {
                    let regions = mesh::Regions::new(&pack);
                    (pack, regions)
                });
            let _ = tx.send(result);
        });
        Self {
            pack: None,
            regions: None,
            status: PackStatus::Loading,
            rx: Mutex::new(Some(rx)),
        }
    }
}

/// System sets of the character module (in `Update`).
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CharacterSet;

/// Characters with faces and bodies from a morphable human model (MakeHuman's CC0 base mesh
/// and shape targets), drawn with skin, eye and hair shading. Add an [`Appearance`] (and a
/// `Transform`) to an entity to give it a person; edit the appearance to change them.
pub struct CharacterPlugin;

impl Plugin for CharacterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CharacterDebug>()
            .init_resource::<CharacterSubsurface>()
            .register_component::<CharacterBuild>(
                "How a character is built: region (body, bust or head), levels (skin subdivision 0..2), hair (grow strands).",
            )
            .register_resource::<CharacterDebug>(
                "Character shader debug view: 0 off, 1 region masks, 2 normals, 3 curvature.",
            )
            .register_resource::<CharacterSubsurface>(
                "Skin through the renderer's subsurface pass (pass: true) or the skin shader's imitation of it.",
            )
            .add_systems(Startup, load_pack)
            .add_systems(
                Update,
                (
                    answer_requests,
                    poll_pack,
                    start_builds,
                    finish_builds,
                    hair_motion::publish_hair_guides,
                )
                    .chain()
                    .in_set(CharacterSet),
            )
            .add_render_feature(CharacterFeature::default());
        // Every person is drawn here, skinned to the animation module's pose, once the pack
        // has loaded (`poll_pack` says so with `AppearanceBodies`): without it nothing would
        // be drawn, so other modules keep drawing their own bodies.
        api::register(app);
    }
}

fn load_pack(world: &mut World) {
    if world.contains_resource::<HumanAssets>() {
        return;
    }
    let root = world
        .get_resource::<engine_assets::AssetServer>()
        .map(|s| s.root())
        .unwrap_or_else(|| PathBuf::from("assets"));
    world.insert_resource(HumanAssets::loading(root.join(pack::PACK_PATH)));
}

fn poll_pack(mut commands: Commands, assets: Option<ResMut<HumanAssets>>) {
    let Some(mut assets) = assets else {
        return;
    };
    let result = {
        let rx = assets.rx.get_mut().unwrap_or_else(|e| e.into_inner());
        match rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(r)) => {
                *rx = None;
                Some(r)
            }
            Some(Err(TryRecvError::Disconnected)) => {
                *rx = None;
                Some(Err(
                    "the human pack loader stopped unexpectedly.".to_string()
                ))
            }
            _ => None,
        }
    };
    match result {
        Some(Ok((pack, regions))) => {
            tracing::info!(
                "character: human pack loaded ({} vertices, {} targets)",
                pack.positions.len(),
                pack.targets.len()
            );
            assets.pack = Some(Arc::new(pack));
            assets.regions = Some(Arc::new(regions));
            assets.status = PackStatus::Ready;
            commands.insert_resource(AppearanceBodies {
                drawn_by: "character".into(),
            });
        }
        Some(Err(e)) => {
            tracing::warn!("character: {e} Characters are not drawn until then.");
            assets.status = PackStatus::Missing(e);
            commands.remove_resource::<AppearanceBodies>();
        }
        None => {}
    }
}

/// Gives every [`AppearanceRequest`] without an [`Appearance`] the person it asks for.
fn answer_requests(
    mut commands: Commands,
    q: Query<(Entity, &AppearanceRequest), Without<Appearance>>,
) {
    for (entity, req) in &q {
        commands
            .entity(entity)
            .insert(Appearance::from_request(req));
    }
}

#[allow(clippy::type_complexity)]
fn start_builds(
    mut commands: Commands,
    assets: Option<Res<HumanAssets>>,
    q: Query<(
        Entity,
        &Appearance,
        Option<&CharacterBuild>,
        Option<&CharacterQuality>,
        Option<&BuiltCharacter>,
        Has<PendingBuild>,
    )>,
) {
    let Some(assets) = assets else {
        return;
    };
    let (Some(pack), Some(regions)) = (assets.pack.clone(), assets.regions.clone()) else {
        return;
    };
    for (entity, appearance, build, quality, built, pending) in &q {
        let opts = build
            .copied()
            .unwrap_or_else(|| CharacterBuild::for_quality(quality.copied().unwrap_or_default()))
            .options();
        if pending || built.is_some_and(|b| b.appearance == *appearance && b.options == opts) {
            continue;
        }
        let a = appearance.clone();
        let (pack, regions) = (pack.clone(), regions.clone());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let meshes = build::build(&pack, &regions, &a, opts);
            let _ = tx.send((a, opts, meshes));
        });
        commands
            .entity(entity)
            .insert(PendingBuild { rx: Mutex::new(rx) });
    }
}

fn finish_builds(
    mut commands: Commands,
    assets: Option<Res<HumanAssets>>,
    q: Query<(
        Entity,
        &PendingBuild,
        Option<&BuiltCharacter>,
        Option<&BodyFit>,
    )>,
) {
    let rest = assets
        .as_ref()
        .and_then(|a| a.regions.as_ref())
        .map(|r| r.landmarks);
    for (entity, pending, built, fit) in &q {
        let msg = pending
            .rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_recv();
        match msg {
            Ok((appearance, options, meshes)) => {
                let Some(rest) = rest else {
                    continue;
                };
                let version = built.map_or(1, |b| b.version + 1);
                let mut uniform = GpuCharacter::new(&appearance, &rest, 0);
                if let Some([thigh, knee]) = meshes.thigh {
                    (uniform.thigh, uniform.knee) = (thigh, knee);
                }
                if let Some(hands) = meshes.hands {
                    uniform.hands = hands.map(|h| h.gpu());
                }
                uniform.lids = meshes.lids;
                if let Some(g) = &meshes.hair_shadow {
                    uniform.hair_grid = g.placement();
                }
                let mut e = commands.entity(entity);
                // The person's own limb lengths, for posers that fit a skeleton to them.
                match meshes.rig.as_ref().map(|r| r.body_fit()) {
                    Some(f) if fit != Some(&f) => {
                        e.insert(f);
                    }
                    Some(_) => {}
                    None => {
                        e.remove::<BodyFit>();
                    }
                }
                e.remove::<PendingBuild>().insert(BuiltCharacter {
                    meshes: Arc::new(meshes),
                    version,
                    uniform,
                    appearance,
                    options,
                });
            }
            Err(TryRecvError::Disconnected) => {
                tracing::warn!("character: a build stopped unexpectedly; retrying");
                commands.entity(entity).remove::<PendingBuild>();
            }
            Err(TryRecvError::Empty) => {}
        }
    }
}
