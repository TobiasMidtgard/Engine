//! Joint handles in the view: a dot on every joint of the subject (body joints on the Pose
//! tab, the chosen hand's finger joints on the Hands tab). Click a dot to pick that joint's
//! sliders; drag it to bend the joint (up and down) and move it sideways (left and right).

use engine::prelude::*;
use engine_character::rig::BoneKind;
use engine_character::{BuiltCharacter, anatomy};
use engine_ui::input::UiInputState;
use engine_ui::node::{Node, WorldAnchor};
use engine_ui::style::{Corners, Pick, Position};
use engine_ui::{Color, El, el};

use crate::Subject;
use crate::panel::CreatorUi;
use crate::posing::PoseEdit;

/// Degrees a joint turns per pixel dragged.
const DEG_PER_PX: f32 = 0.45;

/// The handles, by bone.
#[derive(Resource, Debug, Default)]
pub struct JointHandles {
    dots: Vec<(Entity, String)>,
}

fn dot(bone: &str) -> El {
    El::new()
        .id(format!("joint_{bone}"))
        .on_click(format!("creator.pick:{bone}"))
        .with_style(|s| {
            s.position(Position::Absolute)
                .width(15.0)
                .height(15.0)
                .radius(Corners::all(7.5))
                .bg(Color::hexa(0xf2f2f2c0))
                .border(2.0, Color::hexa(0x15181ee0))
                .pick(Pick::Block)
        })
}

/// One handle per posable bone (hidden until a posing tab shows them).
pub fn setup(world: &mut World) {
    let mut dots = Vec::new();
    for (bone, _) in engine::world::animation::HUMANOID_BONES {
        if anatomy::movements(bone).is_none() {
            continue;
        }
        match el::spawn(world, None, &dot(bone)) {
            Ok(sp) => {
                if let Some(root) = sp.root {
                    world.entity_mut(root).insert(WorldAnchor::default());
                    dots.push((root, bone.to_string()));
                }
            }
            Err(e) => tracing::warn!("joint handles: {e}"),
        }
    }
    world.insert_resource(JointHandles { dots });
}

/// Places the handles on the posed joints, shows the ones the tab wants, and turns a joint
/// while its handle is dragged.
pub fn update(world: &mut World) {
    let Some(handles) = world.get_resource::<JointHandles>() else {
        return;
    };
    let dots = handles.dots.clone();
    let (tab, hand, selected) = {
        let ui = world.resource::<CreatorUi>();
        (ui.tab_name(), ui.hand, ui.selected.clone())
    };
    let subject = {
        let mut q = world.query_filtered::<Entity, With<Subject>>();
        q.iter(world).next()
    };
    let Some(subject) = subject else {
        return;
    };
    // Every joint's posed position.
    let points = {
        let mut q = world.query_filtered::<(
            &Transform,
            Option<&BuiltCharacter>,
            Option<&engine::world::animation::SkeletonPose>,
        ), With<Subject>>();
        match q.iter(world).next() {
            Some((t, Some(built), Some(pose))) => points_of(t, built, pose),
            _ => Vec::new(),
        }
    };
    let side = ["_l", "_r"][hand];
    for (e, bone) in &dots {
        let at = points.iter().find(|(b, _, _)| b == bone);
        let show = match (tab, at) {
            ("Pose", Some((_, _, BoneKind::Body))) => true,
            ("Hands", Some((b, _, kind))) => {
                b.ends_with(side) && (matches!(kind, BoneKind::Finger(..)) || b.starts_with("hand"))
            }
            _ => false,
        };
        let picked = selected.as_deref() == Some(bone.as_str());
        if let Some((_, p, _)) = at
            && let Some(mut a) = world.get_mut::<WorldAnchor>(*e)
            && a.position != *p
        {
            a.position = *p;
        }
        if let Some(mut n) = world.get_mut::<Node>(*e) {
            let size = if tab == "Hands" { 11.0 } else { 15.0 };
            let style = n
                .style
                .clone()
                .display(if show {
                    engine_ui::style::Display::Flex
                } else {
                    engine_ui::style::Display::None
                })
                .width(size)
                .height(size)
                .radius(Corners::all(size * 0.5))
                .bg(if picked {
                    Color::hexa(0xffb347f0)
                } else {
                    Color::hexa(0xf2f2f2c0)
                });
            if n.style != style {
                n.style = style;
            }
        }
    }
    // Dragging a handle turns its joint.
    let pressed = world.resource::<UiInputState>().pressed.map(|(e, _)| e);
    let Some(bone) =
        pressed.and_then(|p| dots.iter().find(|(e, _)| *e == p).map(|(_, b)| b.clone()))
    else {
        return;
    };
    let delta = world.resource::<Mouse>().delta;
    if delta == Vec2::ZERO {
        return;
    }
    let Some(before) = world.get::<PoseEdit>(subject).cloned() else {
        return;
    };
    let mut p = before.clone();
    let a = p.joints.entry(bone.clone()).or_insert([0.0; 3]);
    a[0] -= delta.y * DEG_PER_PX;
    a[1] += delta.x * DEG_PER_PX;
    p.mirrored_from(&before);
    p.tidy();
    if p != before {
        world.entity_mut(subject).insert(p);
    }
    let mut ui = world.resource_mut::<CreatorUi>();
    if ui.selected.as_deref() != Some(bone.as_str()) {
        ui.selected = Some(bone);
        ui.rebuild = true;
    }
}

fn points_of(
    t: &Transform,
    built: &BuiltCharacter,
    pose: &engine::world::animation::SkeletonPose,
) -> Vec<(String, DVec3, BoneKind)> {
    let Some(rig) = built.meshes.rig.as_ref() else {
        return Vec::new();
    };
    let mut palette = Vec::new();
    rig.palette(pose, &rig.map(pose), &mut palette);
    rig.bones
        .iter()
        .enumerate()
        .map(|(b, name)| {
            let rest = engine_character::rig::to_humanoid(rig.joints[b]);
            let p = palette[b].transform_point3(rest);
            (
                name.clone(),
                t.translation + (t.rotation * p).as_dvec3(),
                rig.kind[b],
            )
        })
        .collect()
}
