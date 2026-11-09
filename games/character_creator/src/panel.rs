//! The creator's side panel: presets, a random button, and sliders by tab (body and
//! proportions, skin, eyes, hair, face, pose, hands). Sliders write into `UiData` under
//! `creator`, a JSON copy of the subject's [`Appearance`], and `creator_pose`, a copy of its
//! [`PoseEdit`]; [`sync`] carries changes both ways. Designs (look and pose) save to
//! `designs/slot<n>.json`; every change can be undone.

use bevy_ecs::message::MessageCursor;
use engine::prelude::*;
use engine_character::limbs::LIMBS;
use engine_character::sliders::SLIDERS;
use engine_character::{
    Appearance, AppearanceExt, BeardStyle, CharacterBuild, HairStyle, HumanAssets, PackStatus,
    anatomy, hands,
};
use engine_ui::input::read_events;
use engine_ui::style::{Overflow, Position};
use engine_ui::widgets::{self, Slider};
use engine_ui::{El, Screen, UiData, UiEvent, el, screens};
use serde_json::Value;

use crate::Subject;
use crate::posing::{Design, History, PoseEdit};
use crate::studio::{CameraFocus, Orbit};

const TABS: [&str; 7] = ["Body", "Skin", "Eyes", "Hair", "Face", "Pose", "Hands"];

impl CreatorUi {
    /// The open tab's name.
    pub fn tab_name(&self) -> &'static str {
        TABS[self.tab.min(TABS.len() - 1)]
    }
}

/// Save slots.
const SLOTS: usize = 4;

/// Face slider groups, in the order the Face tab lists them.
const FACE_GROUPS: [&str; 11] = [
    "head", "forehead", "eyebrows", "eyes", "nose", "mouth", "ears", "cheek", "chin", "neck",
    "torso",
];

#[derive(Resource, Debug)]
pub struct CreatorUi {
    tab: usize,
    group: usize,
    /// The Pose tab's bone group ([`anatomy::GROUPS`]).
    bones: usize,
    /// The Hands tab's hand (0 left, 1 right).
    pub hand: usize,
    /// The joint picked in the view (its sliders come first).
    pub selected: Option<String>,
    body: Option<Entity>,
    pub rebuild: bool,
    last: Option<Appearance>,
    last_pose: Option<PoseEdit>,
    data_version: u64,
    /// What the last save, load or undo did.
    note: String,
}

pub fn setup(world: &mut World) {
    world.insert_resource(CreatorUi {
        tab: 0,
        group: 4,
        bones: 1,
        hand: 1,
        selected: None,
        body: None,
        rebuild: true,
        last: None,
        last_pose: None,
        data_version: 0,
        note: String::new(),
    });
    world.init_resource::<History>();
    let panel = El::new().class("screen").child(
        El::new()
            .class("panel")
            .with_style(|s| {
                s.position(Position::Absolute)
                    .left(20.0)
                    .top(20.0)
                    .bottom(20.0)
                    .width(460.0)
                    .gap(10.0)
            })
            .child(El::text("Character creator").class("title"))
            .child(El::text("{creator_status}").class("caption"))
            .child(button_row(&[
                ("Undo".to_string(), "creator.undo".to_string()),
                ("Redo".to_string(), "creator.redo".to_string()),
            ]))
            .child(button_row(
                &(1..=SLOTS)
                    .map(|i| (format!("Save {i}"), format!("creator.save:{i}")))
                    .collect::<Vec<_>>(),
            ))
            .child(button_row(
                &(1..=SLOTS)
                    .map(|i| (format!("Load {i}"), format!("creator.load:{i}")))
                    .collect::<Vec<_>>(),
            ))
            .child(button_row(
                &["Astrid", "Kofi", "Mei"].map(|n| (n.to_string(), format!("creator.preset:{n}"))),
            ))
            .child(button_row(&[
                ("Tomas".into(), "creator.preset:Tomas".into()),
                ("Ruth".into(), "creator.preset:Ruth".into()),
                ("Arjun".into(), "creator.preset:Arjun".into()),
                ("Random".into(), "creator.random".into()),
            ]))
            .child(
                El::new()
                    .class("row")
                    .children(TABS.iter().enumerate().map(|(i, t)| {
                        El::button(*t, format!("creator.tab:{i}"))
                            .class("chip")
                            .id(format!("creator_tab_{i}"))
                    })),
            )
            .child(
                El::new()
                    .class("scroll")
                    .id("creator_body")
                    .with_style(|s| s.grow(1.0).overflow(Overflow::Scroll).gap(4.0)),
            ),
    );
    let subject = {
        let mut q = world.query_filtered::<Entity, With<Subject>>();
        q.iter(world).next()
    };
    // `--tab Pose`: start on that tab.
    let tab = std::env::args()
        .skip_while(|a| a != "--tab")
        .nth(1)
        .and_then(|t| TABS.iter().position(|n| n.eq_ignore_ascii_case(&t)))
        .unwrap_or(0);
    world.resource_mut::<CreatorUi>().tab = tab;
    if let Some(e) = subject {
        show(world, e, TABS[tab], 1);
    }
    match screens::spawn_screen(
        world,
        Screen {
            key: "creator".into(),
            layer: 5,
            ..Default::default()
        },
        &panel,
    ) {
        Ok(sp) => world.resource_mut::<CreatorUi>().body = sp.get("creator_body"),
        Err(e) => tracing::warn!("creator panel: {e}"),
    }
}

fn button_row(buttons: &[(String, String)]) -> El {
    El::new().class("row").children(
        buttons
            .iter()
            .map(|(label, action)| El::button(label.clone(), action.clone()).class("ghost")),
    )
}

fn slider(label: &str, bind: &str, min: f32, max: f32, step: f32) -> El {
    widgets::slider_row(
        label,
        Slider {
            value: if min < 0.0 { 0.0 } else { min },
            min,
            max,
            step,
            bind: format!("creator.{bind}"),
            decimals: if step >= 1.0 { 0 } else { 2 },
            ..Default::default()
        },
    )
}

/// A slider bound anywhere in `UiData` (whole path), in degrees or other units.
fn bound_slider(label: &str, bind: String, min: f32, max: f32, step: f32) -> El {
    widgets::slider_row(
        label,
        Slider {
            value: 0.0f32.clamp(min, max),
            min,
            max,
            step,
            bind,
            decimals: if step >= 1.0 { 0 } else { 2 },
            ..Default::default()
        },
    )
}

/// One bone's sliders: its movements by name, in degrees over its range of motion.
fn joint_sliders(out: &mut Vec<El>, bone: &str) {
    let Some(ms) = anatomy::movements(bone) else {
        return;
    };
    out.push(heading(&anatomy::label(bone)));
    for (k, m) in ms.iter().enumerate() {
        if let Some(m) = m {
            out.push(bound_slider(
                m.name,
                format!("creator_pose.joints.{bone}.{k}"),
                m.min,
                m.max,
                1.0,
            ));
        }
    }
}

fn heading(text: &str) -> El {
    El::text(text.to_uppercase())
        .class("label_caps")
        .with_style(|s| s.margin([10.0, 4.0, 2.0, 4.0]))
}

/// "nose-hump" in group "nose" → "Hump".
fn slider_label(group: &str, target: &str) -> String {
    let t = target
        .strip_prefix(group)
        .map(|t| t.trim_start_matches('-'))
        .filter(|t| !t.is_empty())
        .unwrap_or(target)
        .replace(['-', '_'], " ");
    let mut c = t.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => t,
    }
}

fn tab_content(ui: &CreatorUi, a: &Appearance, p: &PoseEdit) -> Vec<El> {
    let (tab, group) = (ui.tab, ui.group);
    let mut out = Vec::new();
    match TABS[tab] {
        "Pose" => {
            if let Some(b) = ui
                .selected
                .as_deref()
                .filter(|b| engine_character::skin::finger_of(b).is_none())
            {
                out.push(heading("Picked joint (drag its dot to turn it)"));
                joint_sliders(&mut out, b);
            }
            out.push(heading("Base pose"));
            for row in crate::pose::POSES.chunks(4) {
                out.push(styles(
                    row.iter().map(|n| (*n, *n == p.base)),
                    "creator.base",
                ));
            }
            out.push(
                El::new().class("row").children([
                    El::button("Mirror left and right", "creator.mirror")
                        .class("chip")
                        .selected(p.mirror),
                    El::button("Move", "creator.moving")
                        .class("chip")
                        .selected(p.moving),
                    El::button("Reset pose", "creator.reset").class("ghost"),
                ]),
            );
            let groups: Vec<El> = anatomy::GROUPS
                .iter()
                .enumerate()
                .map(|(i, (g, _))| {
                    El::button(*g, format!("creator.bones:{i}"))
                        .class("chip")
                        .selected(i == ui.bones)
                })
                .collect();
            for chunk in groups.chunks(4) {
                out.push(El::new().class("row").children(chunk.to_vec()));
            }
            let (name, bones) = anatomy::GROUPS[ui.bones.min(anatomy::GROUPS.len() - 1)];
            if bones.is_empty() {
                // A hand: every finger joint.
                let s = if name.starts_with("Left") { "l" } else { "r" };
                for f in engine_character::skin::FINGERS {
                    for k in 1..=3 {
                        joint_sliders(&mut out, &format!("{f}_0{k}_{s}"));
                    }
                }
            } else {
                for b in bones {
                    joint_sliders(&mut out, b);
                }
            }
        }
        "Hands" => {
            out.push(styles(
                ["Left", "Right"]
                    .iter()
                    .enumerate()
                    .map(|(i, n)| (*n, i == ui.hand)),
                "creator.hand",
            ));
            let h = p.hands[ui.hand];
            out.push(heading("Hand shape"));
            // Two rows of four, so every shape shows in the panel's width.
            for row in hands::SHAPES.chunks(4) {
                out.push(styles(
                    row.iter().map(|s| (hands::shape_name(*s), *s == h.shape)),
                    "creator.shape",
                ));
            }
            out.push(heading("Holds"));
            out.push(styles(
                crate::posing::Held::ALL
                    .iter()
                    .map(|x| (x.name(), *x == h.held)),
                "creator.held",
            ));
            out.push(bound_slider(
                "Into the shape",
                format!("creator_pose.hands.{}.weight", ui.hand),
                0.0,
                1.0,
                0.02,
            ));
            out.push(bound_slider(
                "Handle radius (cm)",
                format!("creator_pose.hands.{}.handle_cm", ui.hand),
                0.5,
                5.0,
                0.1,
            ));
            let s = ["l", "r"][ui.hand];
            out.push(
                El::new().class("row").children([
                    El::button("Mirror left and right", "creator.mirror")
                        .class("chip")
                        .selected(p.mirror),
                    El::button("Reset fingers", format!("creator.reset_hand:{}", ui.hand))
                        .class("ghost"),
                ]),
            );
            out.push(heading("Wrist"));
            for (k, m) in anatomy::movements(&format!("hand_{s}"))
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                if let Some(m) = m {
                    out.push(bound_slider(
                        m.name,
                        format!("creator_pose.joints.hand_{s}.{k}"),
                        m.min,
                        m.max,
                        1.0,
                    ));
                }
            }
            // Each finger joint on top of the shape (degrees added to the shape's angles); a
            // finger picked in the view comes first.
            let picked = ui
                .selected
                .as_deref()
                .and_then(engine_character::skin::finger_of)
                .map(|(f, _)| f);
            let order: Vec<usize> = picked
                .into_iter()
                .chain((0..5).filter(|f| Some(*f) != picked))
                .collect();
            for (f, finger) in order
                .into_iter()
                .map(|f| (f, engine_character::skin::FINGERS[f]))
            {
                out.push(heading(&format!(
                    "{} (added to the shape)",
                    capital(finger)
                )));
                for k in 1..=3 {
                    let bone = format!("{finger}_0{k}_{s}");
                    let Some(ms) = anatomy::movements(&bone) else {
                        continue;
                    };
                    for (i, m) in ms.iter().enumerate() {
                        let Some(m) = m else {
                            continue;
                        };
                        let joint = if f == 0 {
                            ["Base", "Knuckle", "End joint"][k - 1]
                        } else {
                            ["Knuckle", "Middle joint", "End joint"][k - 1]
                        };
                        let what = if i == 0 {
                            "bend".to_string()
                        } else {
                            m.name.to_lowercase()
                        };
                        let span = m.max - m.min;
                        out.push(bound_slider(
                            &format!("{joint}: {what}"),
                            format!("creator_pose.joints.{bone}.{i}"),
                            -span * 0.75,
                            span * 0.75,
                            1.0,
                        ));
                    }
                }
            }
        }
        "Body" => {
            out.push(heading("Body"));
            out.push(slider("Female · male", "sex", 0.0, 1.0, 0.02));
            out.push(slider("Age", "age", 1.0, 90.0, 1.0));
            out.push(slider("Muscle", "muscle", 0.0, 1.0, 0.02));
            out.push(slider("Weight", "weight", 0.0, 1.0, 0.02));
            out.push(slider("Height", "height", 0.0, 1.0, 0.02));
            out.push(slider("Proportions", "proportions", 0.0, 1.0, 0.02));
            out.push(heading("Ancestry (face and body shape)"));
            out.push(slider("African", "ancestry.african", 0.0, 1.0, 0.02));
            out.push(slider("Asian", "ancestry.asian", 0.0, 1.0, 0.02));
            out.push(slider("European", "ancestry.european", 0.0, 1.0, 0.02));
            out.push(heading("Proportions"));
            for l in LIMBS {
                out.push(slider(
                    l.label,
                    &format!("shape.{}", l.key),
                    -1.0,
                    1.0,
                    0.02,
                ));
            }
            // Hide groups: what armor or a long coat covers is left out of the body.
            out.push(heading("Hidden under armor"));
            for row in engine_character::cover::REGIONS.chunks(4) {
                out.push(El::new().class("row").children(row.iter().map(|r| {
                    El::button(
                        slider_label("", &r.replace('_', " ")),
                        format!("creator.hide:{r}"),
                    )
                    .class("chip")
                    .selected(a.hide.iter().any(|h| h == r))
                })));
            }
        }
        "Skin" => {
            out.push(heading("Skin"));
            out.push(slider("Melanin", "skin.melanin", 0.0, 1.0, 0.01));
            out.push(slider("Redness", "skin.redness", 0.0, 1.0, 0.02));
            out.push(slider("Cool · warm", "skin.undertone", -1.0, 1.0, 0.02));
            out.push(slider("Freckles", "skin.freckles", 0.0, 1.0, 0.02));
            out.push(slider("Moles", "skin.moles", 0.0, 1.0, 0.02));
            out.push(slider("Oiliness", "skin.oiliness", 0.0, 1.0, 0.02));
            out.push(slider("Wrinkles", "skin.wrinkles", -1.0, 1.0, 0.02));
            out.push(slider("Pores", "skin.pores", 0.0, 1.0, 0.02));
        }
        "Eyes" => {
            out.push(heading("Iris"));
            out.push(slider("Blue · brown", "eyes.melanin", 0.0, 1.0, 0.01));
            out.push(slider("Green-gold", "eyes.lipochrome", 0.0, 1.0, 0.02));
            out.push(slider("Limbal ring", "eyes.limbal_ring", 0.0, 1.0, 0.02));
            out.push(slider("Pupil", "eyes.pupil", 0.0, 1.0, 0.02));
            out.push(slider("Redness", "eyes.redness", 0.0, 1.0, 0.02));
        }
        "Hair" => {
            out.push(heading("Hair style"));
            out.push(styles(
                HairStyle::ALL
                    .iter()
                    .map(|s| (s.name(), *s == a.hair_style())),
                "creator.hair",
            ));
            out.push(slider("Length", "hair.length", 0.5, 1.5, 0.02));
            out.push(slider("Curl", "hair.curl", 0.0, 1.0, 0.02));
            out.push(slider("Hairline", "hair.recession", 0.0, 1.0, 0.02));
            out.push(heading("Hair color"));
            out.push(slider(
                "Blonde · black",
                "hair.color.melanin",
                0.0,
                1.0,
                0.01,
            ));
            out.push(slider("Red", "hair.color.red", 0.0, 1.0, 0.02));
            out.push(slider("Grey", "hair.color.grey", 0.0, 1.0, 0.02));
            out.push(heading("Brows"));
            out.push(slider("Thickness", "brows.thickness", 0.0, 1.0, 0.02));
            out.push(heading("Beard"));
            out.push(styles(
                BeardStyle::ALL
                    .iter()
                    .map(|s| (s.name(), *s == a.beard_style())),
                "creator.beard",
            ));
            out.push(slider("Density", "beard.density", 0.0, 1.0, 0.02));
        }
        _ => {
            let groups: Vec<El> = FACE_GROUPS
                .iter()
                .enumerate()
                .map(|(i, g)| {
                    El::button(slider_label("", g), format!("creator.group:{i}"))
                        .class("chip")
                        .selected(i == group)
                })
                .collect();
            for chunk in groups.chunks(4) {
                out.push(El::new().class("row").children(chunk.to_vec()));
            }
            let g = FACE_GROUPS[group.min(FACE_GROUPS.len() - 1)];
            for s in SLIDERS.iter().filter(|s| s.group == g) {
                let min = if s.low.is_empty() { 0.0 } else { -1.0 };
                let bind = format!("shape.{}", s.key());
                out.push(slider(&slider_label(g, s.target), &bind, min, 1.0, 0.02));
            }
        }
    }
    out
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn styles<'a>(items: impl Iterator<Item = (&'a str, bool)>, action: &str) -> El {
    El::new().class("row").children(items.map(|(name, on)| {
        El::button(slider_label("", name), format!("{action}:{name}"))
            .class("chip")
            .selected(on)
    }))
}

fn subject(world: &mut World) -> Option<Entity> {
    let mut q = world.query_filtered::<Entity, With<Subject>>();
    q.iter(world).next()
}

/// Buttons: presets, random, tabs, face groups and hair and beard styles.
pub fn apply_actions(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let actions: Vec<String> = read_events(world, &mut seen)
        .into_iter()
        .filter_map(|e| match e {
            UiEvent::Action { action, .. } => Some(action),
            _ => None,
        })
        .collect();
    let Some(e) = subject(world) else {
        return;
    };
    for action in actions {
        let Some(rest) = action.strip_prefix("creator.") else {
            continue;
        };
        let (verb, arg) = rest.split_once(':').unwrap_or((rest, ""));
        let mut a = world.get::<Appearance>(e).cloned().unwrap_or_default();
        let mut p = world.get::<PoseEdit>(e).cloned().unwrap_or_default();
        let before = p.clone();
        match verb {
            "base" => {
                if crate::pose::POSES.contains(&arg) {
                    p.base = arg.to_string();
                }
            }
            "mirror" => p.mirror = !p.mirror,
            "moving" => p.moving = !p.moving,
            "reset" => p.reset(None),
            "reset_hand" => {
                let side = arg.parse().unwrap_or(0usize).min(1);
                let s = ["l", "r"][side];
                for (b, v) in p.joints.iter_mut() {
                    if (b.starts_with("hand_") || engine_character::skin::finger_of(b).is_some())
                        && b.ends_with(&format!("_{s}"))
                    {
                        *v = [0.0; 3];
                    }
                }
                p.hands[side] = Default::default();
            }
            "pick" => {
                let bone = arg.to_string();
                let finger = engine_character::skin::finger_of(&bone).is_some();
                let side = usize::from(bone.ends_with("_r"));
                let tab = if finger { "Hands" } else { "Pose" };
                let group = anatomy::GROUPS
                    .iter()
                    .position(|(_, bones)| bones.contains(&bone.as_str()));
                let switch = {
                    let mut ui = world.resource_mut::<CreatorUi>();
                    ui.selected = Some(bone);
                    if let Some(g) = group {
                        ui.bones = g;
                    }
                    if finger {
                        ui.hand = side;
                    }
                    ui.rebuild = true;
                    let switch = ui.tab_name() != tab;
                    ui.tab = TABS.iter().position(|t| *t == tab).unwrap_or(0);
                    switch
                };
                if switch {
                    show(world, e, tab, side);
                }
            }
            "bones" => {
                let mut ui = world.resource_mut::<CreatorUi>();
                ui.bones = arg.parse().unwrap_or(0).min(anatomy::GROUPS.len() - 1);
                ui.rebuild = true;
            }
            "hand" => {
                let side = if arg == "Left" { 0 } else { 1 };
                let mut ui = world.resource_mut::<CreatorUi>();
                ui.hand = side;
                ui.rebuild = true;
                world.resource_mut::<CameraFocus>().0 = Some(["hand_l", "hand_r"][side].into());
            }
            "hide" => {
                if engine_character::cover::REGIONS.contains(&arg) {
                    if let Some(i) = a.hide.iter().position(|h| h == arg) {
                        a.hide.remove(i);
                    } else {
                        a.hide.push(arg.to_string());
                    }
                }
            }
            "held" => {
                if let Some(h) = crate::posing::Held::ALL
                    .into_iter()
                    .find(|h| h.name() == arg)
                {
                    let side = world.resource::<CreatorUi>().hand;
                    let hand = &mut p.hands[side];
                    hand.held = h;
                    // The hand closes round the handle (and opens again when it lets go).
                    match h.handle_cm() {
                        Some(cm) => {
                            hand.shape = engine::world::character::HandShape::PowerGrip;
                            hand.weight = 1.0;
                            hand.handle_cm = cm;
                        }
                        None => hand.shape = engine::world::character::HandShape::Relaxed,
                    }
                }
            }
            "shape" => {
                if let Some(s) = hands::SHAPES.iter().find(|s| hands::shape_name(**s) == arg) {
                    let side = world.resource::<CreatorUi>().hand;
                    p.hands[side].shape = *s;
                    p.hands[side].weight = 1.0;
                }
            }
            "undo" | "redo" => {
                let now = Design {
                    appearance: a.clone(),
                    pose: p.clone(),
                };
                let mut h = world.resource_mut::<History>();
                let back = if verb == "undo" {
                    h.undo(&now)
                } else {
                    h.redo(&now)
                };
                if let Some(d) = back {
                    a = d.appearance;
                    p = d.pose;
                }
            }
            "save" => {
                let design = Design {
                    appearance: a.clone(),
                    pose: p.clone(),
                };
                let note = match crate::posing::save(&format!("slot{arg}"), &design) {
                    Ok(path) => format!("Saved {}", path.display()),
                    Err(e) => format!("Not saved: {e}"),
                };
                world.resource_mut::<CreatorUi>().note = note;
            }
            "load" => match crate::posing::load(&format!("slot{arg}")) {
                Ok(d) => {
                    a = d.appearance;
                    p = d.pose;
                    world.resource_mut::<CreatorUi>().note = format!("Loaded slot {arg}");
                }
                Err(e) => world.resource_mut::<CreatorUi>().note = format!("Not loaded: {e}"),
            },
            "preset" => {
                if let Some(p) = Appearance::preset(arg) {
                    a = p;
                }
            }
            "random" => {
                let seed = world.resource::<Time>().frame_count() ^ a.seed.rotate_left(17);
                a = Appearance::random(seed);
            }
            "tab" => {
                let tab = arg.parse().unwrap_or(0).min(TABS.len() - 1);
                let side = {
                    let mut ui = world.resource_mut::<CreatorUi>();
                    ui.tab = tab;
                    ui.rebuild = true;
                    ui.hand
                };
                show(world, e, TABS[tab], side);
            }
            "group" => {
                let mut ui = world.resource_mut::<CreatorUi>();
                ui.group = arg.parse().unwrap_or(0).min(FACE_GROUPS.len() - 1);
                ui.rebuild = true;
            }
            "hair" => {
                if let Some(s) = HairStyle::ALL.into_iter().find(|s| s.name() == arg) {
                    a.hair.style = s.name().into();
                }
            }
            "beard" => {
                if let Some(s) = BeardStyle::ALL.into_iter().find(|s| s.name() == arg) {
                    a.beard.style = s.name().into();
                }
            }
            _ => {}
        }
        if world.get::<Appearance>(e) != Some(&a) {
            world.entity_mut(e).insert(a);
            world.resource_mut::<CreatorUi>().rebuild = true;
        }
        p.mirrored_from(&before);
        p.tidy();
        if world.get::<PoseEdit>(e).is_some_and(|q| *q != p) {
            world.entity_mut(e).insert(p);
            world.resource_mut::<CreatorUi>().rebuild = true;
        }
    }
}

/// What the camera and the body show for a tab: the whole person, standing, for pose and
/// hands (the hands' close-up on the chosen hand), a portrait otherwise.
fn show(world: &mut World, e: Entity, tab: &str, side: usize) {
    let whole = matches!(tab, "Pose" | "Hands" | "Body");
    let build = if whole {
        CharacterBuild::default()
    } else {
        CharacterBuild::portrait()
    };
    if world.get::<CharacterBuild>(e) != Some(&build) {
        world.entity_mut(e).insert(build);
    }
    if whole {
        if world.get::<PoseEdit>(e).is_none() {
            world.entity_mut(e).insert(PoseEdit::default());
        }
    } else {
        world.entity_mut(e).remove::<PoseEdit>();
    }
    let (orbit, focus) = match tab {
        "Hands" => (Orbit::HAND, Some(["hand_l", "hand_r"][side].to_string())),
        "Pose" | "Body" => (Orbit::BODY, None),
        _ => (Orbit::CLOSE_UP, None),
    };
    world.resource_mut::<CameraFocus>().0 = focus;
    let mut q = world.query::<&mut Orbit>();
    for mut o in q.iter_mut(world) {
        *o = orbit;
    }
}

/// Keeps `UiData.creator` and the subject's appearance in step, and rebuilds the tab's
/// content when needed.
pub fn sync(world: &mut World) {
    let status = match world.get_resource::<HumanAssets>().map(|h| &h.status) {
        Some(PackStatus::Ready) => {
            let note = &world.resource::<CreatorUi>().note;
            if note.is_empty() {
                "Drag to turn, scroll to zoom.".to_string()
            } else {
                note.clone()
            }
        }
        Some(PackStatus::Missing(e)) => e.clone(),
        _ => "Loading the human model…".to_string(),
    };
    world.resource_mut::<UiData>().set("creator_status", status);
    let Some(e) = subject(world) else {
        return;
    };
    let current = world.get::<Appearance>(e).cloned().unwrap_or_default();
    let pose = world.get::<PoseEdit>(e).cloned();
    let version = world.resource::<UiData>().version;
    let (last, last_pose, data_version) = {
        let ui = world.resource::<CreatorUi>();
        (ui.last.clone(), ui.last_pose.clone(), ui.data_version)
    };
    if last.as_ref() != Some(&current) {
        // Changed outside the sliders (a preset, a style button, undo, the API): show it.
        let value = serde_json::to_value(&current).unwrap_or(Value::Null);
        world.resource_mut::<UiData>().set("creator", value);
        world.resource_mut::<CreatorUi>().last = Some(current.clone());
    } else if version != data_version {
        let edited = world
            .resource::<UiData>()
            .get("creator")
            .cloned()
            .and_then(|v| serde_json::from_value::<Appearance>(v).ok());
        if let Some(mut a) = edited {
            let shape = std::mem::take(&mut a.shape);
            for (k, v) in shape {
                a.set_slider(&k, v);
            }
            if a != current {
                world.entity_mut(e).insert(a.clone());
                world.resource_mut::<CreatorUi>().last = Some(a);
            }
        }
    }
    if last_pose != pose {
        let value = serde_json::to_value(&pose).unwrap_or(Value::Null);
        world.resource_mut::<UiData>().set("creator_pose", value);
        // Chips (base pose, mirror, hand shapes) aren't bound to the data: a pose changed
        // outside the sliders (a load, undo, a shot's design) rebuilds the panel to show them.
        let chips = |p: &Option<PoseEdit>| {
            p.as_ref()
                .map(|p| (p.base.clone(), p.mirror, p.moving, p.hands))
        };
        let mut ui = world.resource_mut::<CreatorUi>();
        if chips(&last_pose) != chips(&pose) {
            ui.rebuild = true;
        }
        ui.last_pose = pose.clone();
    } else if version != data_version
        && let Some(current_pose) = &pose
    {
        let edited = world
            .resource::<UiData>()
            .get("creator_pose")
            .cloned()
            .and_then(|v| serde_json::from_value::<PoseEdit>(v).ok());
        if let Some(mut p) = edited
            && p != *current_pose
        {
            p.mirrored_from(current_pose);
            p.tidy();
            world.entity_mut(e).insert(p.clone());
            // Mirrored joints show on the other side's sliders too.
            let value = serde_json::to_value(&p).unwrap_or(Value::Null);
            world.resource_mut::<UiData>().set("creator_pose", value);
            world.resource_mut::<CreatorUi>().last_pose = Some(p);
        }
    }
    let v = world.resource::<UiData>().version;
    world.resource_mut::<CreatorUi>().data_version = v;
    // Undo history.
    let design = Design {
        appearance: world.get::<Appearance>(e).cloned().unwrap_or_default(),
        pose: world.get::<PoseEdit>(e).cloned().unwrap_or_default(),
    };
    world.resource_mut::<History>().track(&design);
    let (rebuild, body) = {
        let ui = world.resource::<CreatorUi>();
        (ui.rebuild, ui.body)
    };
    if rebuild && let Some(body) = body {
        let a = world.get::<Appearance>(e).cloned().unwrap_or_default();
        let p = world.get::<PoseEdit>(e).cloned().unwrap_or_default();
        let content = tab_content(world.resource::<CreatorUi>(), &a, &p);
        if let Err(err) = el::replace_children(world, body, &content) {
            tracing::warn!("creator panel: {err}");
        }
        world.resource_mut::<CreatorUi>().rebuild = false;
    }
}
