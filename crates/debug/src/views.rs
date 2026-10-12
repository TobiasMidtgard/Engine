//! Built-in debug views of the `engine_world` contracts: wind, ground surface state, focus
//! points, gravity and world events. All are off by default and cost nothing until enabled.
//!
//! Toggle them from code (`ResMut<DebugViews>`), from data / the AI bridge
//! (`resource.set DebugViews {...}`) or with `debug.views {"wind": {"enabled": true}}`.

use bevy_ecs::prelude::*;
use engine_core::math::{DVec3, Vec3};
use engine_core::transform::GlobalTransform;
use engine_render::Camera;
use engine_world::{
    ContactKind, FocusPoints, Gravity, Ground, GroundContact, NoiseEvent, ProjectileTrace,
    SurfaceCell, SurfaceLayers, WindField, WorldImpulse,
};
use serde::{Deserialize, Serialize};

use crate::color::{self, Color};
use crate::draw::DebugDraw;

/// Most grid points per side a view samples (keeps a huge radius from stalling a frame).
const MAX_SAMPLES_PER_SIDE: i32 = 64;

/// Toggles and tuning for the built-in debug views. Registered resource: patch it by name.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugViews {
    /// Master switch for ALL debug drawing (your shapes and the views). Off = nothing is
    /// recorded or drawn.
    pub enabled: bool,
    /// Wind arrows sampled from `WindField` around the focus (or camera).
    pub wind: WindView,
    /// Ground wetness / puddles / snow / scorch from `SurfaceLayers` as colored points.
    pub surface: SurfaceView,
    /// Markers at every `FocusPoints` position (primary magenta, others pink).
    pub focus: bool,
    /// Gravity direction and strength at the primary focus (orange arrow, 0.15 m per m/s²).
    pub gravity: bool,
    /// World events as they happen.
    pub events: EventViews,
}

impl Default for DebugViews {
    fn default() -> Self {
        Self {
            enabled: true,
            wind: WindView::default(),
            surface: SurfaceView::default(),
            focus: false,
            gravity: false,
            events: EventViews::default(),
        }
    }
}

impl DebugViews {
    /// Turn every built-in view on or off (the master switch is left alone).
    pub fn set_all(&mut self, on: bool) {
        self.wind.enabled = on;
        self.surface.enabled = on;
        self.focus = on;
        self.gravity = on;
        self.events.projectiles = on;
        self.events.impulses = on;
        self.events.ground_contacts = on;
        self.events.noise = on;
    }

    /// True if any built-in view is on.
    pub fn any(&self) -> bool {
        self.wind.enabled || self.surface.enabled || self.focus || self.gravity || self.events.any()
    }
}

/// Wind arrows on a horizontal grid around the focus. Arrow length is speed × `scale`
/// (capped at two grid spacings); color goes blue (calm) → red (`max_speed` and above).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindView {
    pub enabled: bool,
    /// Radius of the sampled disc, meters.
    pub radius: f32,
    /// Distance between arrows, meters.
    pub spacing: f32,
    /// Height of the arrows above the ground, meters.
    pub height: f32,
    /// Arrow length per wind speed, meters per (m/s).
    pub scale: f32,
    /// Wind speed shown fully red, m/s.
    pub max_speed: f32,
}

impl Default for WindView {
    fn default() -> Self {
        Self {
            enabled: false,
            radius: 16.0,
            spacing: 4.0,
            height: 2.0,
            scale: 0.25,
            max_speed: 20.0,
        }
    }
}

/// Ground surface state as small crosses just above the ground: brown dry, blue wet, cyan
/// puddles, white snow, black scorch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceView {
    pub enabled: bool,
    /// Radius of the shown disc, meters.
    pub radius: f32,
    /// Distance between points, meters (at least one surface grid cell).
    pub spacing: f32,
}

impl Default for SurfaceView {
    fn default() -> Self {
        Self {
            enabled: false,
            radius: 24.0,
            spacing: 2.0,
        }
    }
}

/// Which world events to show, and for how long (real seconds).
///
/// * `projectiles`: `ProjectileTrace` paths (orange) with a cross at the end.
/// * `impulses`: `WorldImpulse` spheres of their radius (red).
/// * `ground_contacts`: `GroundContact` footprints (oriented rectangle + normal; green feet,
///   lime paws/hooves, yellow wheels/tracks, white bodies).
/// * `noise`: `NoiseEvent` circles of their audible radius (purple).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventViews {
    pub projectiles: bool,
    pub impulses: bool,
    pub ground_contacts: bool,
    pub noise: bool,
    /// How long each event stays visible, real seconds.
    pub duration: f32,
}

impl Default for EventViews {
    fn default() -> Self {
        Self {
            projectiles: false,
            impulses: false,
            ground_contacts: false,
            noise: false,
            duration: 1.5,
        }
    }
}

impl EventViews {
    pub fn any(&self) -> bool {
        self.projectiles || self.impulses || self.ground_contacts || self.noise
    }
}

/// Where the views are centered: the primary focus, else the active camera, else the origin.
fn view_center(focus: Option<&FocusPoints>, cameras: &Query<(&Camera, &GlobalTransform)>) -> DVec3 {
    if let Some(f) = focus
        && let Some(p) = f.points.first()
    {
        return *p;
    }
    cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| t.translation())
        .unwrap_or(DVec3::ZERO)
}

/// Visit horizontal grid points (world-aligned, so they don't swim as the center moves)
/// inside a disc of `radius` around `center`.
fn for_each_grid_point(center: DVec3, radius: f32, spacing: f32, mut f: impl FnMut(f64, f64)) {
    let s = spacing.max(0.1) as f64;
    let n = ((radius.max(0.0) as f64 / s).ceil() as i32).min(MAX_SAMPLES_PER_SIDE / 2);
    let (cx, cz) = ((center.x / s).round(), (center.z / s).round());
    let r2 = radius as f64 * radius as f64;
    for iz in -n..=n {
        for ix in -n..=n {
            let (x, z) = ((cx + ix as f64) * s, (cz + iz as f64) * s);
            let (dx, dz) = (x - center.x, z - center.z);
            if dx * dx + dz * dz <= r2 {
                f(x, z);
            }
        }
    }
}

fn ground_height(ground: Option<&Ground>, p: DVec3) -> f64 {
    ground.and_then(|g| g.height(p)).unwrap_or(0.0)
}

/// Color of one surface cell.
pub fn surface_color(cell: &SurfaceCell) -> Color {
    let mut c = color::rgba(0.45, 0.32, 0.18, 0.7); // dry soil
    c = color::mix(c, color::BLUE, cell.wetness.clamp(0.0, 1.0));
    if cell.puddle_m > 0.002 {
        c = color::mix(c, color::CYAN, (cell.puddle_m / 0.05).clamp(0.3, 1.0));
    }
    c = color::mix(c, color::WHITE, (cell.snow_m / 0.05).clamp(0.0, 1.0));
    c = color::mix(c, color::BLACK, cell.scorch.clamp(0.0, 1.0) * 0.8);
    color::with_alpha(c, 0.9)
}

/// Extract system: wind, surface, focus and gravity views.
#[allow(clippy::too_many_arguments)]
pub fn environment_views_system(
    views: Res<DebugViews>,
    mut draw: ResMut<DebugDraw>,
    wind: Option<Res<WindField>>,
    surface: Option<Res<SurfaceLayers>>,
    focus: Option<Res<FocusPoints>>,
    gravity: Option<Res<Gravity>>,
    ground: Option<Res<Ground>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
) {
    if !views.enabled
        || !(views.wind.enabled || views.surface.enabled || views.focus || views.gravity)
    {
        return;
    }
    let center = view_center(focus.as_deref(), &cameras);
    let ground = ground.as_deref();

    if views.wind.enabled
        && let Some(wind) = wind.as_deref()
    {
        let v = &views.wind;
        let max_len = v.spacing.max(0.1) * 2.0;
        for_each_grid_point(center, v.radius, v.spacing, |x, z| {
            let base = DVec3::new(x, center.y, z);
            let p = DVec3::new(x, ground_height(ground, base) + v.height as f64, z);
            let w = wind.sample(p);
            let speed = w.length();
            let c = color::ramp(speed / v.max_speed.max(1e-3));
            if speed < 0.05 {
                draw.point(p, c);
                return;
            }
            let len = (speed * v.scale).min(max_len);
            draw.arrow(p, p + (w / speed * len).as_dvec3(), c);
        });
    }

    if views.surface.enabled
        && let Some(surface) = surface.as_deref()
    {
        let v = &views.surface;
        let spacing = v.spacing.max(surface.grid.cell);
        let size = spacing as f64 * 0.35;
        for_each_grid_point(center, v.radius, spacing, |x, z| {
            let base = DVec3::new(x, center.y, z);
            if !surface.grid.contains(base) {
                return;
            }
            let cell = surface.grid.sample_nearest(base);
            let p = DVec3::new(x, ground_height(ground, base) + 0.05, z);
            let c = surface_color(&cell);
            draw.line(p - DVec3::X * size, p + DVec3::X * size, c);
            draw.line(p - DVec3::Z * size, p + DVec3::Z * size, c);
        });
    }

    let up_at = |p: DVec3| gravity.as_deref().map_or(Vec3::Y, |g| g.up_at(p));

    if views.focus
        && let Some(focus) = focus.as_deref()
    {
        for (i, p) in focus.points.iter().enumerate() {
            let c = if i == 0 { color::MAGENTA } else { color::PINK };
            let up = up_at(*p);
            draw.circle(*p, up, 0.5, c).on_top();
            draw.line(*p, *p + up.as_dvec3() * 2.0, c).on_top();
            draw.point(*p, c).on_top();
        }
    }

    if views.gravity
        && let Some(gravity) = gravity.as_deref()
    {
        let g = gravity.at(center);
        let up = up_at(center).as_dvec3();
        let from = center + up * 2.5;
        if g.length() < 1e-4 {
            // Zero g: a small ring instead of an arrow.
            draw.circle(from, up.as_vec3(), 0.25, color::ORANGE)
                .on_top();
        } else {
            draw.arrow(from, from + (g * 0.15).as_dvec3(), color::ORANGE)
                .on_top();
        }
    }
}

/// Extract system: world events as short-lived shapes.
pub fn event_views_system(
    views: Res<DebugViews>,
    mut draw: ResMut<DebugDraw>,
    gravity: Option<Res<Gravity>>,
    mut projectiles: MessageReader<ProjectileTrace>,
    mut impulses: MessageReader<WorldImpulse>,
    mut contacts: MessageReader<GroundContact>,
    mut noises: MessageReader<NoiseEvent>,
) {
    let e = &views.events;
    let on = views.enabled;
    let secs = e.duration;

    if on && e.projectiles {
        for p in projectiles.read() {
            draw.line(p.from, p.to, color::ORANGE).duration(secs);
            draw.cross(p.to, (p.radius * 2.0).max(0.1), color::ORANGE)
                .duration(secs);
        }
    } else {
        projectiles.clear();
    }

    if on && e.impulses {
        for i in impulses.read() {
            draw.sphere(
                i.center,
                i.radius.max(0.05),
                color::with_alpha(color::RED, 0.85),
            )
            .duration(secs);
            draw.cross(i.center, 0.3, color::RED).duration(secs);
        }
    } else {
        impulses.clear();
    }

    if on && e.ground_contacts {
        for c in contacts.read() {
            let col = match c.kind {
                ContactKind::Foot => color::GREEN,
                ContactKind::Paw | ContactKind::Hoof => color::LIME,
                ContactKind::Wheel | ContactKind::Track => color::YELLOW,
                ContactKind::Body => color::WHITE,
            };
            footprint(&mut draw, c, col, secs);
        }
    } else {
        contacts.clear();
    }

    if on && e.noise {
        for n in noises.read() {
            let up = gravity.as_deref().map_or(Vec3::Y, |g| g.up_at(n.position));
            draw.circle(n.position, up, n.radius.max(0.05), color::PURPLE)
                .duration(secs);
            draw.point(n.position, color::PURPLE).duration(secs);
        }
    } else {
        noises.clear();
    }
}

/// Oriented rectangle of the contact's size in its ground plane, plus a short normal.
fn footprint(draw: &mut DebugDraw, c: &GroundContact, color: Color, secs: f32) {
    let n = c.normal.try_normalize().unwrap_or(Vec3::Y);
    // Yaw is measured around the normal from the world X/Z frame (−Z forward at yaw 0).
    let rot = engine_core::math::Quat::from_rotation_arc(Vec3::Y, n)
        * engine_core::math::Quat::from_rotation_y(c.yaw);
    let width = (rot * Vec3::X * (c.size[0].max(0.02) * 0.5)).as_dvec3();
    let length = (rot * Vec3::NEG_Z * (c.size[1].max(0.02) * 0.5)).as_dvec3();
    let p = c.position;
    let corners = [
        p - width - length,
        p + width - length,
        p + width + length,
        p - width + length,
    ];
    draw.polygon(corners, color).duration(secs);
    draw.line(p, p + (n * 0.3).as_dvec3(), color).duration(secs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_points_stay_inside_the_disc_and_are_world_aligned() {
        let center = DVec3::new(1.0e8 + 0.3, 0.0, -7.9);
        let mut pts = Vec::new();
        for_each_grid_point(center, 4.0, 2.0, |x, z| pts.push((x, z)));
        assert!(!pts.is_empty());
        for (x, z) in &pts {
            assert!((x - center.x).hypot(z - center.z) <= 4.0);
            assert_eq!(x % 2.0, 0.0);
            assert_eq!(z % 2.0, 0.0);
        }
        // Huge radii are capped.
        let mut n = 0;
        for_each_grid_point(DVec3::ZERO, 1.0e6, 1.0, |_, _| n += 1);
        assert!(n <= (MAX_SAMPLES_PER_SIDE as usize + 1).pow(2));
    }

    #[test]
    fn surface_colors_reflect_the_state() {
        let dry = surface_color(&SurfaceCell::default());
        let wet = surface_color(&SurfaceCell {
            wetness: 1.0,
            ..Default::default()
        });
        let snow = surface_color(&SurfaceCell {
            snow_m: 0.2,
            ..Default::default()
        });
        assert!(wet[2] > dry[2] && wet[0] < dry[0]);
        assert!(snow.iter().take(3).all(|c| *c > 0.95));
    }

    #[test]
    fn set_all_toggles_every_view() {
        let mut v = DebugViews::default();
        assert!(!v.any());
        v.set_all(true);
        assert!(v.any() && v.events.noise && v.wind.enabled);
        v.set_all(false);
        assert!(!v.any());
        assert!(v.enabled, "the master switch is separate");
    }
}
