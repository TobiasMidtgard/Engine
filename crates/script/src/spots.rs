//! Choices made by walking somewhere (scripting-research.md §28). An option set whose options
//! carry `#spot:<name>` tags is answered by where the player stands, not from a menu:
//!
//! ```text
//! Captain: Who speaks for the company?
//! <<spots 12>>
//! -> Hold your place in the rank #spot:rank #fallback
//! -> Step out past him #spot:flank
//! -> Whisper him an idea #spot:ear
//! ```
//!
//! A spot is an entity with [`StorySpot`] (often a child of the person it belongs to, so "at
//! the captain's ear" moves with him). While the choice is open the player keeps walking:
//!
//! * a glowing ring marks each spot, with its option over it and the prompt over the speaker
//!   (UI markers, when the UI module is there; the conversation screen would take the controls);
//! * standing in a ring for [`DWELL_SECS`] (it fills meanwhile) or pressing E there (each ring
//!   is an [`Interactable`] with the option as its verb) picks that option; a spot the player
//!   stands in when the choice opens counts only once they step out and back, or press E;
//! * `<<spots 12>>` gives 12 real seconds, after which the `#fallback` option is taken;
//! * the view blends to one from above over the player, the spots and the speaker, keeping the
//!   game camera's heading (so up still walks up the screen), and back when the choice lands;
//! * the world slows (`<<spots 12 0.35>>`, 1 for none) while the player keeps full speed
//!   ([`LocalTimeScale::ignore_world_dilation`], set each fixed step after input).
//!
//! The Luau half is in `story.luau` (`Options:pick`); the conversation polls the choice each
//! frame. `story.choose` still picks directly.

use std::collections::HashSet;

use bevy_ecs::message::{MessageCursor, Messages};
use bevy_ecs::prelude::*;
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::{ChildOf, Name, Time, Transform, Window};
use engine_core::registry::call_api;
use engine_core::time::{LocalTimeScale, TimeDilation};
use engine_render::{
    Camera, ExtraView, MeshRenderer, PointLight, Projection, Shape, StandardMaterial,
};
use engine_world::{Interact, Interactable, Interactor};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::cutscene::{CutsceneView, VIEW_ORDER, blend, smooth, world_pose};

/// The `TimeDilation` key of a walk-to choice's slow motion.
pub const SLOW_KEY: &str = "story spots";
/// Real seconds standing in a spot before it is picked.
pub const DWELL_SECS: f32 = 0.7;
/// World time scale while a choice is open, unless `<<spots secs slow>>` says otherwise.
pub const DEFAULT_SLOW: f32 = 0.35;
/// Real seconds the view takes to go up, and to come back down.
const BLEND_IN: f64 = 0.8;
const BLEND_OUT: f64 = 0.6;
/// How steeply the view looks down, degrees below the horizon.
const PITCH_DEG: f32 = 64.0;
/// Room left around what the view shows, meters.
const MARGIN_M: f64 = 1.1;

/// Gold for a choice, a cool silver-blue for the one taken if the player does nothing.
const WARM: [f32; 3] = [1.0, 0.68, 0.26];
const COOL: [f32; 3] = [0.5, 0.72, 1.0];

/// A place to walk to in a conversation: options tagged `#spot:<name>` are picked by
/// standing in the spot of that name (the one nearest the player when several share it).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StorySpot {
    pub name: String,
    /// How far from its middle counts as standing in it, meters.
    pub radius_m: f32,
}

impl Default for StorySpot {
    fn default() -> Self {
        Self {
            name: String::new(),
            radius_m: 0.75,
        }
    }
}

/// A part of a spot's ring, shown while its choice is open.
#[derive(Component, Debug, Clone, Copy)]
pub struct SpotRing {
    pub choice: u64,
    pub option: usize,
}

/// The camera a walk-to choice looks down through (spawned and removed by it).
#[derive(Component, Debug, Clone, Copy)]
pub struct SpotView {
    pub choice: u64,
}

/// One option of a walk-to choice.
#[derive(Debug, Clone, Default)]
pub struct SpotOption {
    pub text: String,
    /// Its `#spot:` name (none: only the fallback, or `story.choose`, takes it).
    pub spot_name: Option<String>,
    pub spot: Option<Entity>,
    pub fallback: bool,
    /// Where the spot is now and how big.
    pub at: DVec3,
    pub radius: f32,
    /// 0..1 while the player stands in it.
    pub progress: f32,
    pub inside: bool,
    /// Off while the player has stood in it since the choice opened.
    armed: bool,
    ring: Option<Entity>,
    fill: Option<Entity>,
    light: Option<Entity>,
    label: Option<Entity>,
    label_text: String,
}

/// A walk-to choice: open while the conversation waits on it, then closing (the rings go, the
/// view comes back down) for a moment after the pick.
#[derive(Debug, Clone)]
pub struct SpotChoice {
    pub id: u64,
    /// The conversation's id (`StoryRuns`).
    pub run: u64,
    pub options: Vec<SpotOption>,
    /// Who walks: the player's body (an `Interactor`) or the conversation's `chooser`.
    pub chooser: Entity,
    pub speaker: Option<Entity>,
    pub prompt_speaker: String,
    pub prompt: String,
    /// Real seconds to choose (0: no limit).
    pub time_limit: f32,
    /// Real seconds since it opened.
    pub age: f32,
    pub slow: f32,
    /// The option taken (0-based).
    pub picked: Option<usize>,
    /// Real seconds since the pick (closing).
    pub closing: Option<f32>,
    /// Spots the player stepped into since the conversation last looked.
    entered: Vec<String>,
    view: Option<Entity>,
    look: Option<(Transform, Projection)>,
    center: Option<DVec3>,
    bubble: Option<Entity>,
    /// The chooser's own clock before the choice (None: it had none, so it goes again).
    spared: Option<Option<LocalTimeScale>>,
}

impl SpotChoice {
    pub fn is_open(&self) -> bool {
        self.closing.is_none()
    }

    /// Seconds left before the fallback is taken (None: no time limit).
    pub fn seconds_left(&self) -> Option<f32> {
        (self.time_limit > 0.0).then(|| (self.time_limit - self.age).max(0.0))
    }
}

/// The walk-to choices open (and closing) now.
#[derive(Resource, Default)]
pub struct SpotChoices {
    pub choices: Vec<SpotChoice>,
    next_id: u64,
    cursor: MessageCursor<Interact>,
    /// Changes whenever a choice opens, moves on or closes.
    pub version: u64,
}

impl SpotChoices {
    pub fn get(&self, id: u64) -> Option<&SpotChoice> {
        self.choices.iter().find(|c| c.id == id)
    }

    /// The open choice of conversation `run`.
    pub fn of_run(&self, run: u64) -> Option<&SpotChoice> {
        self.choices.iter().find(|c| c.run == run && c.is_open())
    }
}

// ------------------------------------------------------------------------------- opening

/// What `story.luau` asks for: `{ options = { {text, spot, fallback} }, prompt = {speaker,
/// text}, time, slow }`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Spec {
    pub options: Vec<SpecOption>,
    pub prompt: SpecPrompt,
    pub time: Option<f32>,
    pub slow: Option<f32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SpecOption {
    pub text: String,
    pub spot: Option<String>,
    pub fallback: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SpecPrompt {
    pub speaker: Option<String>,
    pub text: String,
}

/// The player's body (the first `Interactor`), or `given` while it exists.
pub fn find_chooser(world: &mut World, given: Option<Entity>) -> Option<Entity> {
    if let Some(e) = given.filter(|e| world.get_entity(*e).is_ok()) {
        return Some(e);
    }
    let mut q = world.query_filtered::<Entity, With<Interactor>>();
    q.iter(world).next()
}

/// The spot named `name` nearest `near`: (entity, where, radius).
pub fn find_spot(world: &mut World, name: &str, near: DVec3) -> Option<(Entity, DVec3, f32)> {
    let mut q = world.query::<(Entity, &StorySpot)>();
    let list: Vec<(Entity, f32)> = q
        .iter(world)
        .filter(|(_, s)| s.name == name)
        .map(|(e, s)| (e, s.radius_m.max(0.2)))
        .collect();
    list.into_iter()
        .filter_map(|(e, r)| Some((e, world_pose(world, e)?.translation, r)))
        .min_by(|a, b| {
            a.1.distance_squared(near)
                .total_cmp(&b.1.distance_squared(near))
        })
}

/// The world position of `e` (its transform under its parent's).
fn pos(world: &World, e: Entity) -> Option<DVec3> {
    world_pose(world, e).map(|t| t.translation)
}

fn flat_distance(a: DVec3, b: DVec3) -> f64 {
    DVec3::new(a.x - b.x, 0.0, a.z - b.z).length()
}

/// Whether `chooser` (the player when None) stands in a spot named `name` now.
pub fn standing_at(world: &mut World, chooser: Option<Entity>, name: &str) -> bool {
    let Some(c) = find_chooser(world, chooser) else {
        return false;
    };
    let Some(at) = pos(world, c) else {
        return false;
    };
    find_spot(world, name, at).is_some_and(|(_, s, r)| flat_distance(at, s) <= r as f64)
}

/// Opens a walk-to choice for conversation `run`; its id, or why it can't (the options then
/// show as a menu).
pub fn begin(
    world: &mut World,
    run: u64,
    spec: Spec,
    chooser: Option<Entity>,
    owner: Option<Entity>,
) -> Result<u64, String> {
    let chooser = find_chooser(world, chooser).ok_or(
        "nobody to walk: no Interactor (the player's body) in the world, and no chooser given",
    )?;
    let from = pos(world, chooser).ok_or("the chooser has no Transform")?;
    let mut options = Vec::new();
    for o in &spec.options {
        let mut opt = SpotOption {
            text: o.text.clone(),
            spot_name: o.spot.clone(),
            fallback: o.fallback,
            armed: true,
            ..Default::default()
        };
        if let Some(name) = &o.spot {
            let (e, at, r) = find_spot(world, name, from)
                .ok_or_else(|| format!("#spot:{name}: no StorySpot named '{name}' in the world"))?;
            opt.spot = Some(e);
            opt.at = at;
            opt.radius = r;
            opt.inside = flat_distance(from, at) <= r as f64;
            opt.armed = !opt.inside;
        }
        options.push(opt);
    }
    if options.iter().all(|o| o.spot.is_none()) {
        return Err("none of the options has a spot".into());
    }
    let speaker = spec
        .prompt
        .speaker
        .as_deref()
        .and_then(|s| named(world, s))
        .or(owner);
    let slow = spec.slow.unwrap_or(DEFAULT_SLOW).clamp(0.02, 1.0);
    let id = {
        let mut choices = world.get_resource_or_insert_with(SpotChoices::default);
        choices.next_id += 1;
        choices.version += 1;
        choices.next_id
    };
    let mut choice = SpotChoice {
        id,
        run,
        options,
        chooser,
        speaker,
        prompt_speaker: spec.prompt.speaker.clone().unwrap_or_default(),
        prompt: spec.prompt.text.clone(),
        time_limit: spec.time.unwrap_or(0.0).max(0.0),
        age: 0.0,
        slow,
        picked: None,
        closing: None,
        entered: Vec::new(),
        view: None,
        look: None,
        center: None,
        bubble: None,
        spared: None,
    };
    for i in 0..choice.options.len() {
        spawn_ring(world, &mut choice, i);
    }
    show_labels(world, &mut choice);
    if slow < 0.999 {
        let had = world.get::<LocalTimeScale>(chooser).copied();
        if had.is_none() {
            world.entity_mut(chooser).insert(LocalTimeScale::default());
        }
        choice.spared = Some(had);
        if let Some(mut d) = world.get_resource_mut::<TimeDilation>() {
            d.hold(SLOW_KEY, slow, 0.35);
        }
    }
    world.resource_mut::<SpotChoices>().choices.push(choice);
    Ok(id)
}

/// The entity named `name`.
fn named(world: &mut World, name: &str) -> Option<Entity> {
    let mut q = world.query::<(Entity, &Name)>();
    q.iter(world)
        .find(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
}

/// The ring of option `i`: a glowing torus on the ground, a faint disc, a fill that grows
/// while the player stands in it, and a small light; the root is what E uses.
fn spawn_ring(world: &mut World, choice: &mut SpotChoice, i: usize) {
    let o = &choice.options[i];
    if o.spot.is_none() {
        return;
    }
    let color = if o.fallback { COOL } else { WARM };
    let r = o.radius;
    let id = choice.id;
    let root = world
        .spawn((
            Name::new(format!("Spot ring: {}", o.text)),
            Transform::from_translation(o.at),
            SpotRing {
                choice: id,
                option: i,
            },
            Interactable {
                verb: o.text.clone(),
                reach_m: r + 0.25,
                offset: Vec3::ZERO,
                enabled: true,
            },
        ))
        .id();
    let part = |world: &mut World, y: f64, shape: Shape, material: StandardMaterial| {
        world
            .spawn((
                Transform::from_xyz(0.0, y, 0.0),
                MeshRenderer::new(shape, material).without_shadows(),
                SpotRing {
                    choice: id,
                    option: i,
                },
                ChildOf(root),
            ))
            .id()
    };
    let dark = |k: f32| StandardMaterial::color(color[0] * k, color[1] * k, color[2] * k);
    part(
        world,
        0.04,
        Shape::Torus {
            radius: r,
            tube: 0.035,
            segments: 72,
        },
        dark(0.3).with_emissive(color, 7.0),
    );
    part(
        world,
        0.012,
        Shape::Cylinder {
            radius: r,
            height: 0.004,
            segments: 48,
        },
        dark(0.2).with_emissive(color, 0.5).with_alpha(0.14),
    );
    let fill = part(
        world,
        0.02,
        Shape::Cylinder {
            radius: r * 0.94,
            height: 0.004,
            segments: 48,
        },
        dark(0.4).with_emissive(color, 2.6).with_alpha(0.42),
    );
    set_fill(world, fill, 0.0);
    let light = world
        .spawn((
            Transform::from_xyz(0.0, 0.4, 0.0),
            PointLight {
                color,
                intensity_lm: 40.0,
                range: r * 3.0 + 1.5,
                radius: 0.05,
            },
            SpotRing {
                choice: id,
                option: i,
            },
            ChildOf(root),
        ))
        .id();
    let o = &mut choice.options[i];
    o.ring = Some(root);
    o.fill = Some(fill);
    o.light = Some(light);
}

/// The fill disc at `k` (0..1) of the ring's size.
fn set_fill(world: &mut World, fill: Entity, k: f32) {
    let k = k.clamp(0.0, 1.0);
    if let Some(mut t) = world.get_mut::<Transform>(fill) {
        t.scale = Vec3::new(k.max(0.01), 1.0, k.max(0.01));
    }
    if let Some(mut m) = world.get_mut::<MeshRenderer>(fill) {
        let show = k > 0.02;
        if m.visible != show {
            m.visible = show;
        }
    }
}

/// A UI marker (the UI module's `hud.marker`), when there is one.
fn marker(world: &mut World, args: Json) -> Option<Entity> {
    let has = world
        .get_resource::<engine_core::registry::ApiRegistry>()
        .is_some_and(|r| r.get("hud.marker").is_some());
    if !has {
        return None;
    }
    match call_api(world, "hud.marker", args) {
        Ok(out) => out["entity"].as_u64().and_then(Entity::try_from_bits),
        Err(e) => {
            engine_core::log::warn!("story spots: {e}");
            None
        }
    }
}

fn despawn(world: &mut World, e: Option<Entity>) {
    if let Some(e) = e
        && let Ok(ent) = world.get_entity_mut(e)
    {
        ent.despawn();
    }
}

/// What the fallback's label says under its option.
fn fallback_note(choice: &SpotChoice) -> String {
    match choice.seconds_left() {
        Some(s) => format!("if you wait · {} s", s.ceil() as i32),
        None => "where you stand".into(),
    }
}

/// The options over their rings, and the prompt over the speaker.
fn show_labels(world: &mut World, choice: &mut SpotChoice) {
    for i in 0..choice.options.len() {
        update_label(world, choice, i);
    }
    let speaker_name = choice
        .speaker
        .and_then(|e| world.get::<Name>(e).map(|n| n.as_str().to_string()));
    let title = if choice.prompt_speaker.is_empty() {
        speaker_name.clone().unwrap_or_default()
    } else {
        choice.prompt_speaker.clone()
    };
    let mut args = json!({
        "title": title,
        "subtitle": choice.prompt,
        "icon": "dot",
        "color": "#f2ead8",
        "distance": false,
        "always": true,
    });
    match (speaker_name, choice.speaker.and_then(|e| pos(world, e))) {
        (Some(name), _) => {
            args["entity"] = json!(name);
            args["offset"] = json!([0.0, 2.35, 0.0]);
        }
        (None, Some(at)) => args["position"] = json!([at.x, at.y + 2.35, at.z]),
        _ => return,
    }
    despawn(world, choice.bubble.take());
    choice.bubble = marker(world, args);
}

/// Option `i`'s label, put up again when its text changed (the fallback's countdown).
fn update_label(world: &mut World, choice: &mut SpotChoice, i: usize) {
    let o = &choice.options[i];
    if o.spot.is_none() {
        return;
    }
    let note = if o.fallback {
        fallback_note(choice)
    } else {
        String::new()
    };
    if choice.options[i].label.is_some() && choice.options[i].label_text == note {
        return;
    }
    let o = &choice.options[i];
    let color = if o.fallback { "#9cc4ff" } else { "#ffc266" };
    let args = json!({
        "title": o.text,
        "subtitle": note,
        "icon": "dot",
        "color": color,
        "position": [o.at.x, o.at.y + 0.3, o.at.z],
        "distance": false,
        "always": true,
    });
    let old = choice.options[i].label.take();
    despawn(world, old);
    let label = marker(world, args);
    let o = &mut choice.options[i];
    o.label = label;
    o.label_text = note;
}

// ------------------------------------------------------------------------------- running

/// What the conversation sees each frame: the option picked (from 1), and the spots stepped
/// into since it last asked.
pub fn poll(world: &mut World, id: u64) -> (Option<usize>, Vec<String>) {
    let Some(mut choices) = world.get_resource_mut::<SpotChoices>() else {
        return (None, Vec::new());
    };
    let Some(c) = choices.choices.iter_mut().find(|c| c.id == id) else {
        return (None, Vec::new());
    };
    let entered = std::mem::take(&mut c.entered);
    (c.picked.map(|i| i + 1), entered)
}

/// The conversation has its pick (`index` from 1, when it came another way such as
/// `story.choose`): the choice closes.
pub fn end(world: &mut World, id: u64, index: Option<usize>) {
    let Some(mut choices) = world.get_resource_mut::<SpotChoices>() else {
        return;
    };
    choices.version += 1;
    if let Some(c) = choices.choices.iter_mut().find(|c| c.id == id) {
        if c.picked.is_none() {
            c.picked = index.and_then(|i| i.checked_sub(1));
        }
        if c.closing.is_none() {
            c.closing = Some(0.0);
        }
    }
}

/// The speaker says `text` while the choice is open (a script's `on_spot_near` answer).
pub fn say(world: &mut World, id: u64, text: String) {
    world.resource_scope(|world, mut choices: Mut<SpotChoices>| {
        if let Some(c) = choices
            .choices
            .iter_mut()
            .find(|c| c.id == id && c.is_open())
        {
            c.prompt = text;
            show_labels(world, c);
        }
        choices.version += 1;
    });
}

/// Every frame (`Update`, after the conversations): the player's place against the spots, the
/// picks, the rings, labels and view, and closing choices going away.
pub(crate) fn run_spots(world: &mut World) {
    if !world.contains_resource::<SpotChoices>() {
        return;
    }
    if world.contains_resource::<crate::ScriptsHeld>() {
        reset(world);
        return;
    }
    let dt = world
        .get_resource::<Time>()
        .map(|t| t.real_delta_secs())
        .unwrap_or(1.0 / 60.0)
        .min(0.1);
    world.resource_scope(|world, mut choices: Mut<SpotChoices>| {
        // E on a ring picks its option.
        let pressed: Vec<Entity> = match world.get_resource::<Messages<Interact>>() {
            Some(m) => choices.cursor.read(m).map(|i| i.target).collect(),
            None => Vec::new(),
        };
        let mut changed = false;
        for c in choices.choices.iter_mut() {
            changed |= step(world, c, dt, &pressed);
        }
        let mut gone = Vec::new();
        choices.choices.retain(|c| {
            let keep = c.closing.is_none_or(|t| t < BLEND_OUT.max(0.45) as f32);
            if !keep {
                gone.push(c.clone());
            }
            keep
        });
        for c in gone {
            close(world, &c);
            changed = true;
        }
        let slowing = choices
            .choices
            .iter()
            .any(|c| c.is_open() && c.slow < 0.999);
        if !slowing
            && let Some(mut d) = world.get_resource_mut::<TimeDilation>()
            && d.is_active(SLOW_KEY)
        {
            d.release(SLOW_KEY);
        }
        if changed {
            choices.version += 1;
        }
    });
}

/// One frame of choice `c`; true when what the editor shows changed.
fn step(world: &mut World, c: &mut SpotChoice, dt: f32, pressed: &[Entity]) -> bool {
    if let Some(t0) = c.closing {
        if t0 == 0.0 {
            // Just picked: the labels go now, the rings shrink, the view comes down.
            for o in c.options.iter_mut() {
                despawn(world, o.label.take());
            }
            despawn(world, c.bubble.take());
            restore_clock(world, c);
        }
        let t = t0 + dt;
        c.closing = Some(t);
        let k = (t / 0.45).clamp(0.0, 1.0);
        for (i, o) in c.options.iter().enumerate() {
            let Some(ring) = o.ring else { continue };
            let s = if Some(i) == c.picked {
                1.0 + 0.3 * k
            } else {
                (1.0 - k).max(0.01)
            };
            if let Some(mut tr) = world.get_mut::<Transform>(ring) {
                tr.scale = Vec3::new(s, 1.0, s);
            }
            if Some(i) == c.picked
                && let Some(f) = o.fill
            {
                set_fill(world, f, 1.0 - k);
            }
        }
        view(world, c, dt as f64);
        return true;
    }
    c.age += dt;
    let mut changed = false;
    let Some(at) = pos(world, c.chooser) else {
        // The chooser went: the fallback (or the first) is taken.
        c.picked = Some(c.options.iter().position(|o| o.fallback).unwrap_or(0));
        return true;
    };
    for i in 0..c.options.len() {
        let Some(spot) = c.options[i].spot else {
            continue;
        };
        if let Some(p) = pos(world, spot) {
            c.options[i].at = p;
        }
        let o = &mut c.options[i];
        let inside = flat_distance(at, o.at) <= o.radius as f64;
        if !inside {
            o.armed = true;
        }
        if inside && !o.inside && o.armed {
            if let Some(name) = &o.spot_name {
                c.entered.push(name.clone());
            }
            changed = true;
        }
        o.inside = inside;
        let before = o.progress;
        o.progress = if inside && o.armed {
            (o.progress + dt / DWELL_SECS).min(1.0)
        } else {
            (o.progress - dt * 3.0 / DWELL_SECS).max(0.0)
        };
        changed |= (o.progress > 0.0) != (before > 0.0);
        if o.progress >= 1.0 && c.picked.is_none() {
            c.picked = Some(i);
        }
        let ring = o.ring;
        if pressed.iter().any(|p| Some(*p) == ring) && c.picked.is_none() {
            c.picked = Some(i);
        }
        let (fill, light, progress, age) = (o.fill, o.light, o.progress, c.age);
        if let Some(r) = ring
            && let Some(mut t) = world.get_mut::<Transform>(r)
        {
            t.translation = c.options[i].at;
        }
        if let Some(f) = fill {
            set_fill(world, f, progress);
        }
        if let Some(l) = light
            && let Some(mut pl) = world.get_mut::<PointLight>(l)
        {
            let breathe = 0.5 + 0.5 * (age * 2.6 + i as f32).sin();
            pl.intensity_lm = 30.0 + 25.0 * breathe + 160.0 * progress;
        }
    }
    if c.picked.is_none()
        && let Some(left) = c.seconds_left()
    {
        if left <= 0.0 {
            c.picked = Some(
                c.options
                    .iter()
                    .position(|o| o.fallback)
                    .or_else(|| nearest(c, at))
                    .unwrap_or(0),
            );
        } else if let Some(i) = c.options.iter().position(|o| o.fallback) {
            update_label(world, c, i);
        }
    }
    if c.picked.is_some() {
        changed = true;
    }
    view(world, c, dt as f64);
    changed
}

/// The option whose spot is nearest `at`.
fn nearest(c: &SpotChoice, at: DVec3) -> Option<usize> {
    c.options
        .iter()
        .enumerate()
        .filter(|(_, o)| o.spot.is_some())
        .min_by(|a, b| flat_distance(at, a.1.at).total_cmp(&flat_distance(at, b.1.at)))
        .map(|(i, _)| i)
}

/// The chooser's clock as it was.
fn restore_clock(world: &mut World, c: &mut SpotChoice) {
    match c.spared.take() {
        Some(Some(before)) => {
            if let Some(mut l) = world.get_mut::<LocalTimeScale>(c.chooser) {
                l.ignore_world_dilation = before.ignore_world_dilation;
            }
        }
        Some(None) => {
            if let Ok(mut e) = world.get_entity_mut(c.chooser) {
                e.remove::<LocalTimeScale>();
            }
        }
        None => {}
    }
}

/// Everything the choice spawned goes.
fn close(world: &mut World, c: &SpotChoice) {
    for o in &c.options {
        despawn(world, o.ring);
        despawn(world, o.label);
    }
    despawn(world, c.bubble);
    despawn(world, c.view);
    let mut c = c.clone();
    restore_clock(world, &mut c);
}

/// Each fixed step after input, before gameplay: the chooser of an open choice moves at full
/// speed through its slow motion (a character's own systems may have reset its clock).
pub(crate) fn spare_choosers(
    choices: Option<Res<SpotChoices>>,
    mut clocks: Query<&mut LocalTimeScale>,
) {
    let Some(choices) = choices else {
        return;
    };
    for c in &choices.choices {
        if c.is_open()
            && c.spared.is_some()
            && let Ok(mut l) = clocks.get_mut(c.chooser)
            && !l.ignore_world_dilation
        {
            l.ignore_world_dilation = true;
        }
    }
}

/// Drops every choice at once (the editor's Stop, scripts held).
pub fn reset(world: &mut World) {
    let Some(mut choices) = world.get_resource_mut::<SpotChoices>() else {
        return;
    };
    if choices.choices.is_empty() {
        return;
    }
    let all = std::mem::take(&mut choices.choices);
    choices.version += 1;
    for c in &all {
        close(world, c);
    }
    if let Some(mut d) = world.get_resource_mut::<TimeDilation>() {
        d.release(SLOW_KEY);
    }
}

// ---------------------------------------------------------------------------------- view

/// The game's camera: the active one in front that isn't a cutscene's, a choice's or an
/// extra view, below [`VIEW_ORDER`].
fn game_camera(world: &mut World) -> Option<(Transform, Camera)> {
    let mut q = world.query_filtered::<(Entity, &Camera), (
        Without<CutsceneView>,
        Without<SpotView>,
        Without<ExtraView>,
    )>();
    let (e, cam) = q
        .iter(world)
        .filter(|(_, c)| c.active && c.order < VIEW_ORDER)
        .max_by_key(|(_, c)| c.order)
        .map(|(e, c)| (e, *c))?;
    Some((world_pose(world, e)?, cam))
}

/// Where the view from above is: over the player, the spots and the speaker, with the game
/// camera's heading, far enough to show them all.
fn overhead(world: &World, c: &SpotChoice, yaw: f32, fov_y: f32, center: DVec3) -> Transform {
    let turn = Quat::from_rotation_y(yaw);
    let right = (turn * Vec3::X).as_dvec3();
    let ahead = (turn * Vec3::NEG_Z).as_dvec3();
    let aspect = world
        .get_resource::<Window>()
        .filter(|w| w.width > 0 && w.height > 0)
        .map_or(16.0 / 9.0, |w| w.width as f64 / w.height as f64);
    let (mut w, mut d) = (0.0f64, 0.0f64);
    for (p, r) in points(world, c) {
        let o = p - center;
        w = w.max(o.dot(right).abs() + r);
        d = d.max(o.dot(ahead).abs() + r);
    }
    let pitch = PITCH_DEG.to_radians();
    let half = (fov_y.to_radians() as f64 * 0.5).tan();
    let by_depth = (d + MARGIN_M) * (pitch as f64).sin() / half;
    let by_width = (w + MARGIN_M) / (half * aspect);
    let dist = by_depth.max(by_width).max(5.0);
    let rotation = turn * Quat::from_rotation_x(-pitch);
    let forward = (rotation * Vec3::NEG_Z).as_dvec3();
    let mut t = Transform::from_translation(center - forward * dist);
    t.rotation = rotation;
    t
}

/// What the view keeps in frame: the chooser, each spot (with its size) and the speaker.
fn points(world: &World, c: &SpotChoice) -> Vec<(DVec3, f64)> {
    let mut out = Vec::new();
    if let Some(p) = pos(world, c.chooser) {
        out.push((p, 0.6));
    }
    for o in &c.options {
        if o.spot.is_some() {
            out.push((o.at, o.radius as f64 + 0.4));
        }
    }
    if let Some(p) = c.speaker.and_then(|e| pos(world, e)) {
        out.push((p, 0.6));
    }
    out
}

/// The middle of what the view keeps in frame (on the ground).
fn middle(world: &World, c: &SpotChoice) -> Option<DVec3> {
    let pts = points(world, c);
    if pts.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
    for (p, _) in &pts {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let mut m = (lo + hi) * 0.5;
    m.y = lo.y;
    Some(m)
}

/// Puts the choice's camera where it belongs this frame: going up, over the player, or
/// coming back down to the game's camera.
fn view(world: &mut World, c: &mut SpotChoice, dt: f64) {
    let game = game_camera(world);
    let (game_pose, game_cam) = match &game {
        Some((t, cam)) => (Some(*t), *cam),
        None => (None, Camera::default()),
    };
    let fov = match game_cam.projection {
        Projection::Perspective { fov_y_deg, .. } => fov_y_deg,
        Projection::Orthographic { .. } => 55.0,
    };
    let lens = match game_cam.projection {
        p @ Projection::Perspective { .. } => p,
        Projection::Orthographic { .. } => Projection::Perspective {
            fov_y_deg: 55.0,
            near: 0.1,
        },
    };
    let pose = if let Some(t) = c.closing {
        let Some((from, from_lens)) = c.look else {
            return;
        };
        let k = smooth(t as f64 / BLEND_OUT);
        match game_pose {
            Some(g) => blend(&(from, from_lens), &(g, lens), k),
            None => (from, from_lens),
        }
    } else {
        let Some(target) = middle(world, c) else {
            return;
        };
        let center = match c.center {
            Some(m) => m.lerp(target, 1.0 - (-dt / 0.4).exp()),
            None => target,
        };
        c.center = Some(center);
        let yaw = match (c.look, game_pose) {
            // Keep the heading the view took when it went up.
            (Some((t, _)), _) => heading(&t),
            (None, Some(g)) => heading(&g),
            (None, None) => 0.0,
        };
        let above = (overhead(world, c, yaw, fov, center), lens);
        let k = smooth(c.age as f64 / BLEND_IN);
        match game_pose {
            Some(g) if k < 1.0 => blend(&(g, lens), &above, k),
            _ => above,
        }
    };
    c.look = Some(pose);
    let camera = Camera {
        projection: pose.1,
        exposure: game_cam.exposure,
        active: true,
        order: VIEW_ORDER,
    };
    match c.view.filter(|v| world.get_entity(*v).is_ok()) {
        Some(v) => {
            if let Some(mut t) = world.get_mut::<Transform>(v) {
                *t = pose.0;
            }
            if let Some(mut cam) = world.get_mut::<Camera>(v) {
                *cam = camera;
            }
        }
        None => {
            c.view = Some(
                world
                    .spawn((
                        Name::new("Choice view"),
                        SpotView { choice: c.id },
                        pose.0,
                        camera,
                    ))
                    .id(),
            );
        }
    }
}

/// The heading (yaw about up) a camera looks along.
fn heading(t: &Transform) -> f32 {
    let f = t.rotation * Vec3::NEG_Z;
    (-f.x).atan2(-f.z)
}

// -------------------------------------------------------------------------------- for agents

/// The open choices as `story.state` lists them.
pub fn describe(world: &World) -> Json {
    let Some(choices) = world.get_resource::<SpotChoices>() else {
        return json!([]);
    };
    let list: Vec<Json> = choices
        .choices
        .iter()
        .filter(|c| c.is_open())
        .map(|c| {
            json!({
                "id": c.id,
                "conversation": c.run,
                "prompt": c.prompt,
                "seconds_left": c.seconds_left(),
                "options": c.options.iter().map(|o| json!({
                    "text": o.text,
                    "spot": o.spot_name,
                    "fallback": o.fallback,
                    "at": o.spot.map(|_| [o.at.x, o.at.y, o.at.z]),
                    "inside": o.inside,
                    "progress": o.progress,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!(list)
}

/// The spot names in the world, for checks (the editor marks options whose spot is missing).
pub fn spot_names(world: &mut World) -> HashSet<String> {
    let mut q = world.query::<&StorySpot>();
    q.iter(world).map(|s| s.name.clone()).collect()
}

/// The Luau side's helpers: `__story_spots_begin`, `_poll`, `_end`, `_say`, and
/// `__story_standing_at`.
pub(crate) fn install(lua: &mlua::Lua) -> mlua::Result<()> {
    use crate::access::with_world_as;
    use crate::entity::EntityRef;
    use mlua::Value;
    let g = lua.globals();
    g.set(
        "__story_spots_begin",
        lua.create_function(
            |lua,
             (run, spec, chooser, owner): (
                u64,
                Value,
                Option<EntityRef>,
                Option<EntityRef>,
            )| {
                let spec: Spec = serde_json::from_value(crate::convert::to_json(&spec)?)
                    .map_err(|e| mlua::Error::runtime(format!("spots: {e}")))?;
                with_world_as(lua, "dialogue", |w| {
                    Ok(
                        match begin(w, run, spec, chooser.map(|c| c.0), owner.map(|o| o.0)) {
                            Ok(id) => (Some(id), None),
                            Err(why) => (None, Some(why)),
                        },
                    )
                })
            },
        )?,
    )?;
    g.set(
        "__story_spots_poll",
        lua.create_function(|lua, id: u64| {
            with_world_as(lua, "dialogue", |w| {
                let (pick, entered) = poll(w, id);
                Ok((pick, (!entered.is_empty()).then_some(entered)))
            })
        })?,
    )?;
    g.set(
        "__story_spots_end",
        lua.create_function(|lua, (id, index): (u64, Option<usize>)| {
            with_world_as(lua, "dialogue", |w| {
                end(w, id, index);
                Ok(())
            })
        })?,
    )?;
    g.set(
        "__story_spots_say",
        lua.create_function(|lua, (id, text): (u64, String)| {
            with_world_as(lua, "dialogue", |w| {
                say(w, id, text);
                Ok(())
            })
        })?,
    )?;
    g.set(
        "__story_standing_at",
        lua.create_function(|lua, (chooser, name): (Option<EntityRef>, String)| {
            with_world_as(lua, "dialogue", |w| {
                Ok(standing_at(w, chooser.map(|c| c.0), &name))
            })
        })?,
    )?;
    Ok(())
}
