//! The creator's side panel: presets, a random button, and sliders by tab (body, skin, eyes,
//! hair, face). Sliders write into `UiData` under `creator`, a JSON copy of the subject's
//! [`Appearance`]; [`sync`] carries changes both ways.

use bevy_ecs::message::MessageCursor;
use engine::prelude::*;
use engine_character::sliders::SLIDERS;
use engine_character::{Appearance, AppearanceExt, BeardStyle, HairStyle, HumanAssets, PackStatus};
use engine_ui::input::read_events;
use engine_ui::style::{Overflow, Position};
use engine_ui::widgets::{self, Slider};
use engine_ui::{El, Screen, UiData, UiEvent, el, screens};
use serde_json::Value;

use crate::Subject;

const TABS: [&str; 5] = ["Body", "Skin", "Eyes", "Hair", "Face"];

/// Face slider groups, in the order the Face tab lists them.
const FACE_GROUPS: [&str; 11] = [
    "head", "forehead", "eyebrows", "eyes", "nose", "mouth", "ears", "cheek", "chin", "neck",
    "torso",
];

#[derive(Resource, Debug)]
pub struct CreatorUi {
    tab: usize,
    group: usize,
    body: Option<Entity>,
    rebuild: bool,
    last: Option<Appearance>,
    data_version: u64,
}

pub fn setup(world: &mut World) {
    world.insert_resource(CreatorUi {
        tab: 0,
        group: 4,
        body: None,
        rebuild: true,
        last: None,
        data_version: 0,
    });
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
                            .class("tab")
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

fn tab_content(tab: usize, group: usize, a: &Appearance) -> Vec<El> {
    let mut out = Vec::new();
    match TABS[tab] {
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
        match verb {
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
                let mut ui = world.resource_mut::<CreatorUi>();
                ui.tab = arg.parse().unwrap_or(0).min(TABS.len() - 1);
                ui.rebuild = true;
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
    }
}

/// Keeps `UiData.creator` and the subject's appearance in step, and rebuilds the tab's
/// content when needed.
pub fn sync(world: &mut World) {
    let status = match world.get_resource::<HumanAssets>().map(|h| &h.status) {
        Some(PackStatus::Ready) => "Drag to turn, scroll to zoom.".to_string(),
        Some(PackStatus::Missing(e)) => e.clone(),
        _ => "Loading the human model…".to_string(),
    };
    world.resource_mut::<UiData>().set("creator_status", status);
    let Some(e) = subject(world) else {
        return;
    };
    let current = world.get::<Appearance>(e).cloned().unwrap_or_default();
    let version = world.resource::<UiData>().version;
    let (last, data_version) = {
        let ui = world.resource::<CreatorUi>();
        (ui.last.clone(), ui.data_version)
    };
    if last.as_ref() != Some(&current) {
        // Changed outside the sliders (a preset, a style button, the API): show it.
        let value = serde_json::to_value(&current).unwrap_or(Value::Null);
        let mut data = world.resource_mut::<UiData>();
        data.set("creator", value);
        let v = data.version;
        let mut ui = world.resource_mut::<CreatorUi>();
        ui.last = Some(current);
        ui.data_version = v;
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
        world.resource_mut::<CreatorUi>().data_version = version;
    }
    let (rebuild, body, tab, group) = {
        let ui = world.resource::<CreatorUi>();
        (ui.rebuild, ui.body, ui.tab, ui.group)
    };
    if rebuild && let Some(body) = body {
        let a = world.get::<Appearance>(e).cloned().unwrap_or_default();
        if let Err(err) = el::replace_children(world, body, &tab_content(tab, group, &a)) {
            tracing::warn!("creator panel: {err}");
        }
        world.resource_mut::<CreatorUi>().rebuild = false;
    }
}
