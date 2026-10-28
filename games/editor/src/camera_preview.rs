//! What a scene camera sees, and flying it from the scene view.
//!
//! Selecting a camera shows its view picture-in-picture in the scene view's corner, live as it is
//! moved or its settings change (Unreal's camera preview, Unity's Cameras overlay); a pin keeps
//! it up while something else is selected. Pilot (Unreal's "Pilot", Unity's "Control a camera in
//! first person") looks through the camera: the editor camera takes its place and lens, and
//! flying the view moves the camera, one undo step when the pilot ejects (Esc or Eject).
//!
//! The preview is an editor-only camera that copies the scene camera every frame and renders as
//! an [`ExtraView`] into a corner of the window; the scene camera itself is never changed.

use engine::prelude::*;
use engine::world::{QualityPreset, Scalability};

use crate::camera::EditorCamera;
use crate::editor::{self, Editor};
use crate::scene_ops::EditorOnly;
use crate::theme;
use crate::undo::Command;

/// Preview width as a share of the scene view's, and its bounds in points.
const SHARE: f32 = 0.3;
const MIN_WIDTH: f32 = 220.0;
const MAX_WIDTH: f32 = 440.0;
const MARGIN: f32 = 10.0;
const HEADER: f32 = 24.0;

#[derive(Resource, Default)]
pub struct CameraPreview {
    /// Shown even while it isn't selected.
    pub pinned: Option<Entity>,
    /// The editor-only camera rendering the preview.
    preview: Option<Entity>,
    /// Where the picture goes this frame (window pixels), set by the scene view.
    inset: Option<(Entity, UVec2, UVec2)>,
    pilot: Option<Pilot>,
}

/// A scene camera being flown from the scene view.
struct Pilot {
    camera: Entity,
    /// Its transform when the pilot started (for undo).
    before: Transform,
    /// The editor camera's transform as last copied to it.
    last: Transform,
    /// The editor camera's own lens, put back on eject.
    lens: Camera,
}

/// The scene camera being piloted, if any.
pub fn piloting(world: &World) -> Option<Entity> {
    world
        .get_resource::<CameraPreview>()
        .and_then(|p| p.pilot.as_ref())
        .map(|p| p.camera)
}

/// The scene camera to preview: the pinned one, else the active selection if it is a camera.
/// Not while piloting it (the scene view already shows what it sees).
fn target(world: &World, editor: &Editor) -> Option<Entity> {
    let preview = world.get_resource::<CameraPreview>()?;
    let is_camera =
        |e: Entity| world.get::<Camera>(e).is_some() && world.get::<EditorOnly>(e).is_none();
    let e = preview
        .pinned
        .filter(|&e| world.get_entity(e).is_ok() && is_camera(e))
        .or_else(|| editor.primary().filter(|&e| is_camera(e)))?;
    (piloting(world) != Some(e)).then_some(e)
}

/// The preview in the scene view's corner (header with pin and pilot, the picture under it) and
/// the pilot banner. Called by the scene view after its overlays.
pub fn ui(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, rect: egui::Rect) {
    pilot_banner(ui, world, editor, rect);
    let Some(camera) = target(world, editor) else {
        if let Some(mut preview) = world.get_resource_mut::<CameraPreview>() {
            preview.inset = None;
        }
        return;
    };
    let aspect = {
        let window = world.resource::<Window>();
        (window.width.max(1) as f32 / window.height.max(1) as f32).clamp(0.5, 3.0)
    };
    let width = (rect.width() * SHARE).clamp(MIN_WIDTH, MAX_WIDTH);
    let size = egui::vec2(width, width / aspect);
    let picture = egui::Rect::from_min_size(
        rect.right_bottom() - size - egui::vec2(MARGIN, MARGIN),
        size,
    );
    let frame = egui::Rect::from_min_max(picture.min - egui::vec2(0.0, HEADER), picture.max);
    if frame.top() < rect.top() + 60.0 {
        return;
    }
    // Clicks on the picture stay with it, not the scene behind.
    ui.interact(
        picture,
        egui::Id::new("camera preview picture"),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        egui::Rect::from_min_max(frame.min, picture.right_top()),
        egui::CornerRadius {
            nw: 5,
            ne: 5,
            sw: 0,
            se: 0,
        },
        egui::Color32::from_rgba_unmultiplied(22, 23, 26, 235),
    );
    painter.rect_stroke(
        frame.expand(1.0),
        5,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(60)),
        egui::StrokeKind::Outside,
    );
    let pinned = world
        .get_resource::<CameraPreview>()
        .is_some_and(|p| p.pinned == Some(camera));
    let name = crate::scene_ops::label(world, camera);
    let header = egui::Rect::from_min_max(frame.min, picture.right_top());
    let mut pin = None;
    let mut pilot = false;
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(header.shrink2(egui::vec2(6.0, 2.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(egui::RichText::new(format!("📷 {name}")).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("✈ Pilot").min_size(egui::vec2(0.0, 18.0)))
                    .on_hover_text(
                        "Look through this camera; flying the view moves it (Esc ejects)",
                    )
                    .clicked()
                {
                    pilot = true;
                }
                if ui
                    .add(
                        egui::Button::new("📌")
                            .selected(pinned)
                            .min_size(egui::vec2(22.0, 18.0)),
                    )
                    .on_hover_text("Keep this preview while something else is selected")
                    .clicked()
                {
                    pin = Some(!pinned);
                }
                ui.label(theme::dim(lens_label(world, camera)));
            });
        },
    );
    let ppp = ui.ctx().pixels_per_point();
    let min = (picture.min.to_vec2() * ppp).round();
    let px = (picture.size() * ppp).round();
    if let Some(mut preview) = world.get_resource_mut::<CameraPreview>() {
        preview.inset = Some((
            camera,
            UVec2::new(min.x.max(0.0) as u32, min.y.max(0.0) as u32),
            UVec2::new(px.x.max(1.0) as u32, px.y.max(1.0) as u32),
        ));
        if let Some(pin) = pin {
            preview.pinned = pin.then_some(camera);
        }
    }
    if pilot {
        start_pilot(world, editor, camera);
    }
}

/// "60° fov" or "ortho 10 m".
fn lens_label(world: &World, camera: Entity) -> String {
    match world.get::<Camera>(camera).map(|c| &c.projection) {
        Some(Projection::Perspective { fov_y_deg, .. }) => format!("{fov_y_deg:.0}°"),
        Some(Projection::Orthographic { .. }) => "ortho".into(),
        _ => String::new(),
    }
}

fn pilot_banner(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, rect: egui::Rect) {
    let Some(camera) = piloting(world) else {
        return;
    };
    let name = crate::scene_ops::label(world, camera);
    let painter = ui.painter_at(rect);
    painter.rect_stroke(
        rect.shrink(1.5),
        0,
        egui::Stroke::new(3.0, theme::ACCENT),
        egui::StrokeKind::Inside,
    );
    let mut eject = false;
    egui::Area::new(egui::Id::new("pilot banner"))
        .fixed_pos(egui::pos2(rect.center().x, rect.top() + 44.0))
        .pivot(egui::Align2::CENTER_TOP)
        .order(egui::Order::Middle)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme::ACCENT)
                .corner_radius(12)
                .inner_margin(egui::Margin::symmetric(12, 3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("✈  Piloting {name}"))
                                .color(egui::Color32::BLACK)
                                .strong(),
                        );
                        eject = ui
                            .add(egui::Button::new("Eject").min_size(egui::vec2(0.0, 18.0)))
                            .on_hover_text("Stop piloting (Esc)")
                            .clicked();
                    });
                });
        });
    if eject {
        stop_pilot(world, editor);
    }
}

/// Look through `camera`: the editor camera takes its place and lens.
pub fn start_pilot(world: &mut World, editor: &mut Editor, camera: Entity) {
    stop_pilot(world, editor);
    let (Some(&before), Some(global), Some(lens)) = (
        world.get::<Transform>(camera),
        world.get::<GlobalTransform>(camera).copied(),
        world.get::<Camera>(camera).copied(),
    ) else {
        return;
    };
    let eye = Transform {
        scale: Vec3::ONE,
        ..global.to_transform()
    };
    let editor_lens = world
        .get::<Camera>(editor.camera)
        .cloned()
        .unwrap_or_default();
    if let Some(mut t) = world.get_mut::<Transform>(editor.camera) {
        *t = eye;
    }
    if let Some(mut c) = world.get_mut::<Camera>(editor.camera) {
        c.projection = lens.projection;
    }
    if let Some(mut fly) = world.get_mut::<EditorCamera>(editor.camera) {
        let looking = EditorCamera::looking(eye.forward());
        fly.yaw = looking.yaw;
        fly.pitch = looking.pitch;
    }
    world.get_resource_or_init::<CameraPreview>().pilot = Some(Pilot {
        camera,
        before,
        last: eye,
        lens: editor_lens,
    });
    let name = crate::scene_ops::label(world, camera);
    editor.info(
        editor::now(world),
        format!("Piloting {name}: fly to move it, Esc ejects"),
    );
}

/// Stop piloting: the editor camera gets its own lens back where it is, and the camera's move
/// becomes one undo step.
pub fn stop_pilot(world: &mut World, editor: &mut Editor) -> bool {
    let Some(pilot) = world
        .get_resource_mut::<CameraPreview>()
        .and_then(|mut p| p.pilot.take())
    else {
        return false;
    };
    if let Some(mut c) = world.get_mut::<Camera>(editor.camera) {
        c.projection = pilot.lens.projection;
    }
    if let Some(&after) = world.get::<Transform>(pilot.camera)
        && after != pilot.before
        && let (Ok(before), Ok(after)) = (
            serde_json::to_value(pilot.before),
            serde_json::to_value(after),
        )
    {
        let command = Command::Component {
            entity: pilot.camera,
            name: "Transform".into(),
            before: Some(before),
            after: Some(after),
        };
        let now = editor::now(world);
        editor.history.record(world, command, now);
    }
    true
}

/// Keep the preview camera and the piloted camera in step (Update, after the editor UI and the
/// fly camera).
pub fn sync(world: &mut World) {
    world.resource_scope(|world, mut editor: Mut<Editor>| {
        fly_piloted(world, &mut editor);
    });
    let Some(preview) = world.get_resource::<CameraPreview>() else {
        return;
    };
    let (inset, existing) = (preview.inset, preview.preview);
    let wanted = inset.and_then(|(camera, min, size)| {
        let lens = *world.get::<Camera>(camera)?;
        let global = world.get::<GlobalTransform>(camera)?;
        let eye = Transform {
            scale: Vec3::ONE,
            ..global.to_transform()
        };
        Some((lens, eye, Viewport::new(min, size)))
    });
    match (wanted, existing) {
        (Some((lens, eye, viewport)), existing) => {
            // Extra views draw lowest order first: the preview goes over the scene views.
            let lens = Camera {
                active: true,
                order: 2000,
                ..lens
            };
            match existing.filter(|&e| world.get_entity(e).is_ok()) {
                Some(e) => {
                    let mut entity = world.entity_mut(e);
                    if entity.get::<Camera>() != Some(&lens) {
                        entity.insert(lens);
                    }
                    if entity.get::<Viewport>() != Some(&viewport) {
                        entity.insert(viewport);
                    }
                    if entity.get::<Transform>() != Some(&eye) {
                        entity.insert(eye);
                    }
                }
                None => {
                    let e = world
                        .spawn((
                            EditorOnly,
                            Name::new("Camera preview"),
                            lens,
                            eye,
                            viewport,
                            ExtraView::window()
                                .with_quality(Scalability::preset(QualityPreset::Medium)),
                        ))
                        .id();
                    world.resource_mut::<CameraPreview>().preview = Some(e);
                }
            }
        }
        (None, Some(e)) => {
            if world.get_entity(e).is_ok() {
                world.despawn(e);
            }
            world.resource_mut::<CameraPreview>().preview = None;
        }
        (None, None) => {}
    }
}

/// While piloting, a move of the editor camera moves the scene camera (its scale kept); a camera
/// that went away ends the pilot.
fn fly_piloted(world: &mut World, editor: &mut Editor) {
    let Some((camera, last)) = world
        .get_resource::<CameraPreview>()
        .and_then(|p| p.pilot.as_ref())
        .map(|p| (p.camera, p.last))
    else {
        return;
    };
    if world.get::<Camera>(camera).is_none() {
        stop_pilot(world, editor);
        return;
    }
    // Lens edits in Details show at once.
    if let Some(lens) = world.get::<Camera>(camera).map(|c| c.projection)
        && let Some(mut c) = world.get_mut::<Camera>(editor.camera)
        && c.projection != lens
    {
        c.projection = lens;
    }
    let Some(&eye) = world.get::<Transform>(editor.camera) else {
        return;
    };
    if eye == last {
        return;
    }
    let parent_inv = world
        .get::<ChildOf>(camera)
        .and_then(|p| world.get::<GlobalTransform>(p.parent()))
        .map(|g| g.inverse())
        .unwrap_or(glam::DAffine3::IDENTITY);
    let world_affine =
        glam::DAffine3::from_rotation_translation(eye.rotation.as_dquat(), eye.translation);
    let mut local = editor::transform_from_affine(parent_inv * world_affine);
    if let Some(t) = world.get::<Transform>(camera) {
        local.scale = t.scale;
    }
    if let Some(mut t) = world.get_mut::<Transform>(camera) {
        *t = local;
    }
    if let Some(mut preview) = world.get_resource_mut::<CameraPreview>()
        && let Some(pilot) = &mut preview.pilot
    {
        pilot.last = eye;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piloting_moves_the_camera_and_undoes_in_one_step() {
        let mut app = App::new();
        app.update();
        let world = &mut app.world;
        world.insert_resource(CameraPreview::default());
        let editor_camera = world
            .spawn((
                EditorOnly,
                Camera::perspective(60.0),
                EditorCamera::looking(-Vec3::Z),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        let start = Transform::from_xyz(1.0, 2.0, 3.0).looking_at(DVec3::ZERO, Vec3::Y);
        let camera = world
            .spawn((
                Camera::perspective(35.0),
                start,
                GlobalTransform::from(start),
            ))
            .id();
        let mut editor = Editor::new(editor_camera);
        start_pilot(world, &mut editor, camera);
        assert_eq!(piloting(world), Some(camera));
        // The editor view takes the camera's place and lens.
        let eye = *world.get::<Transform>(editor_camera).unwrap();
        assert!((eye.translation - start.translation).length() < 1e-9);
        assert!(matches!(
            world.get::<Camera>(editor_camera).unwrap().projection,
            Projection::Perspective { fov_y_deg, .. } if fov_y_deg == 35.0
        ));
        // Entering moves nothing; flying does.
        fly_piloted(world, &mut editor);
        assert_eq!(*world.get::<Transform>(camera).unwrap(), start);
        world
            .get_mut::<Transform>(editor_camera)
            .unwrap()
            .translation
            .x += 4.0;
        fly_piloted(world, &mut editor);
        let moved = world.get::<Transform>(camera).unwrap().translation;
        assert!((moved.x - 5.0).abs() < 1e-6, "{moved}");
        // Ejecting gives the editor its lens back and leaves one undo step.
        assert!(stop_pilot(world, &mut editor));
        assert!(matches!(
            world.get::<Camera>(editor_camera).unwrap().projection,
            Projection::Perspective { fov_y_deg, .. } if fov_y_deg == 60.0
        ));
        assert!(editor.history.can_undo());
        editor.history.undo(world).unwrap();
        let back = world.get::<Transform>(camera).unwrap().translation;
        assert!((back - start.translation).length() < 1e-6, "{back}");
    }
}
