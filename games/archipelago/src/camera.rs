//! The strategy camera: pan over the islands, turn, and zoom from a street corner to the whole
//! archipelago, the view tilting down as it rises. And picking: which ground point, cell or
//! building is under the cursor.

use engine::input::Mouse;
use engine::prelude::*;
use engine::render::Projection;

use crate::globe::{self, Frame};
use crate::island;

/// The player's view: a map point on the ground it looks at, a bearing and a distance.
#[derive(Component, Clone, Debug)]
pub struct StrategyCam {
    pub focus: DVec3,
    pub yaw: f32,
    pub dist: f32,
    /// Where it is gliding to (set these; the view follows smoothly).
    pub goal_focus: DVec3,
    pub goal_yaw: f32,
    pub goal_dist: f32,
    /// Lock the pitch to this instead of the zoom's (tour shots), radians.
    pub pitch_override: Option<f32>,
    /// Keep the focus's height as set (looking at something in the air) instead of the ground's.
    pub free_height: bool,
}

pub const MIN_DIST: f32 = 22.0;
/// Far enough to see the whole planet.
pub const MAX_DIST: f32 = 150_000.0;
/// The view that takes in a group of islands: the pitch curve's knee.
const ISLES_DIST: f32 = 2600.0;
/// From here up the view looks almost straight down at the globe.
const GLOBE_DIST: f32 = 60_000.0;

impl StrategyCam {
    pub fn new(focus: DVec3, yaw: f32, dist: f32) -> Self {
        Self {
            focus,
            yaw,
            dist,
            goal_focus: focus,
            goal_yaw: yaw,
            goal_dist: dist,
            pitch_override: None,
            free_height: false,
        }
    }

    /// Look down this far for a distance: 28° at street level, 58° over a group of islands,
    /// and on up to straight down at the globe.
    pub fn pitch_for(dist: f32) -> f32 {
        let t = ((dist / MIN_DIST).ln() / (ISLES_DIST / MIN_DIST).ln()).clamp(0.0, 1.0);
        let u = ((dist / ISLES_DIST).ln() / (GLOBE_DIST / ISLES_DIST).ln()).clamp(0.0, 1.0);
        (28.0 + 30.0 * t.powf(0.8) + 30.0 * u * u * (3.0 - 2.0 * u)).to_radians()
    }

    pub fn pitch(&self) -> f32 {
        self.pitch_override
            .unwrap_or_else(|| Self::pitch_for(self.dist))
    }

    /// The camera transform for the current state, in the world as `frame` has it.
    pub fn transform(&self, frame: &Frame) -> Transform {
        let stand = frame.stand(self.focus, self.yaw);
        let rot = (stand.rotation * Quat::from_rotation_x(-self.pitch())).normalize();
        let fwd = (rot * -Vec3::Z).as_dvec3();
        let mut eye = stand.translation - fwd * self.dist as f64;
        // Never under the ground or the sea.
        let p = frame.planet(eye);
        let len = p.length();
        let floor = island::ground(p / len).max(0.0) + 4.0;
        if len - globe::RADIUS_M < floor {
            eye = frame.world(p / len * (globe::RADIUS_M + floor));
        }
        Transform::from_translation(eye).with_rotation(rot)
    }

    /// Jump straight to the goal (no glide).
    pub fn snap(&mut self) {
        self.focus = self.goal_focus;
        self.yaw = self.goal_yaw;
        self.dist = self.goal_dist;
    }

    /// Keep the view on the chart of the region it is nearest: crossing an ocean, the focus
    /// and the bearing move to the next region's sheet (the same spot and direction on the
    /// planet). A view sent to another region's sheet glides there over the planet.
    fn rechart(&mut self) {
        let goal = self.goal_focus;
        let k = globe::nearest_region(globe::dir(goal.x, goal.z));
        if k != globe::region_of(goal.x, goal.z) {
            let mut m = globe::map_on(k, globe::dir(goal.x, goal.z));
            m.y = goal.y;
            self.goal_yaw = globe::carry_yaw(goal, self.goal_yaw, m);
            self.goal_focus = m;
        }
        let at = self.focus;
        if globe::region_of(at.x, at.z) != k {
            let mut m = globe::map_on(k, globe::dir(at.x, at.z));
            m.y = at.y;
            self.yaw = globe::carry_yaw(at, self.yaw, m);
            self.focus = m;
        }
    }
}

/// Input and glide.
#[allow(clippy::too_many_arguments)]
/// Keep everything on screen simulated: smoke, wakes and other effects rest beyond the
/// engine's simulation range of the camera, which a strategy view is always far from.
pub fn simulation_reach(mut range: ResMut<engine::world::SimulationRange>, q: Query<&StrategyCam>) {
    if let Some(cam) = q.iter().next() {
        let want = (cam.dist as f64 * 2.2).max(500.0);
        if (range.active_m - want).abs() > 1.0 {
            range.active_m = want;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn control(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<Mouse>,
    buttons: Res<ButtonInput<MouseButton>>,
    capture: Option<Res<engine_ui::UiInputCapture>>,
    config: Res<EngineConfig>,
    frame: Res<Frame>,
    mut q: Query<(&mut StrategyCam, &mut Transform)>,
) {
    let dt = time.real_delta_secs().min(0.1);
    let ui = capture.as_ref().is_some_and(|c| c.pointer || c.keyboard);
    for (mut cam, mut t) in &mut q {
        if !config.headless {
            let mut pan = Vec2::ZERO;
            if !capture.as_ref().is_some_and(|c| c.keyboard) {
                if keys.pressed(KeyCode::W) || keys.pressed(KeyCode::ArrowUp) {
                    pan.y += 1.0;
                }
                if keys.pressed(KeyCode::S) || keys.pressed(KeyCode::ArrowDown) {
                    pan.y -= 1.0;
                }
                if keys.pressed(KeyCode::A) || keys.pressed(KeyCode::ArrowLeft) {
                    pan.x -= 1.0;
                }
                if keys.pressed(KeyCode::D) || keys.pressed(KeyCode::ArrowRight) {
                    pan.x += 1.0;
                }
                if keys.pressed(KeyCode::Q) {
                    cam.goal_yaw += 1.6 * dt;
                }
                if keys.pressed(KeyCode::E) {
                    cam.goal_yaw -= 1.6 * dt;
                }
            }
            if buttons.pressed(MouseButton::Middle) {
                cam.goal_yaw -= mouse.delta.x * 0.006;
            }
            if !ui && mouse.scroll.y != 0.0 {
                cam.goal_dist =
                    (cam.goal_dist * 0.86f32.powf(mouse.scroll.y)).clamp(MIN_DIST, MAX_DIST);
            }
            if pan != Vec2::ZERO {
                let fast = if keys.pressed(KeyCode::ShiftLeft) {
                    2.5
                } else {
                    1.0
                };
                let speed = (cam.goal_dist * 0.9 + 20.0) * fast * dt;
                let (s, c) = cam.goal_yaw.sin_cos();
                let fwd = DVec2::new(-s as f64, -c as f64);
                let right = DVec2::new(c as f64, -s as f64);
                let d = (fwd * pan.y as f64 + right * pan.x as f64) * speed as f64;
                cam.goal_focus.x += d.x;
                cam.goal_focus.z += d.y;
            }
            // Right-drag pans with the ground under the cursor.
            if buttons.pressed(MouseButton::Right) && mouse.delta != Vec2::ZERO {
                let k = cam.goal_dist as f64 * 0.0016;
                let (s, c) = cam.goal_yaw.sin_cos();
                let fwd = DVec2::new(-s as f64, -c as f64);
                let right = DVec2::new(c as f64, -s as f64);
                let d = (-right * mouse.delta.x as f64 + fwd * mouse.delta.y as f64) * k;
                cam.goal_focus.x += d.x;
                cam.goal_focus.z += d.y;
            }
        }
        cam.rechart();
        let ground = island::true_height(cam.goal_focus.x, cam.goal_focus.z).max(0.0);
        if !cam.free_height {
            cam.goal_focus.y = ground;
        }
        let k = 1.0 - (-dt * 7.0).exp();
        let goal = cam.goal_focus;
        let focus = cam.focus;
        cam.focus = focus + (goal - focus) * k as f64;
        let (yaw, dist) = (cam.yaw, cam.dist);
        cam.yaw = yaw + (cam.goal_yaw - yaw) * k;
        cam.dist = dist + (cam.goal_dist - dist) * k;
        *t = cam.transform(&frame);
    }
}

/// The world ray under a cursor position (physical pixels).
pub fn cursor_ray(
    t: &Transform,
    cam: &Camera,
    window: &engine::core::window::Window,
    px: Vec2,
) -> (DVec3, Vec3) {
    let fov = match cam.projection {
        Projection::Perspective { fov_y_deg, .. } => fov_y_deg.to_radians(),
        _ => 65f32.to_radians(),
    };
    let (w, h) = (window.width.max(1) as f32, window.height.max(1) as f32);
    let x = px.x / w * 2.0 - 1.0;
    let y = 1.0 - px.y / h * 2.0;
    let th = (fov * 0.5).tan();
    let d = Vec3::new(x * th * w / h, y * th, -1.0).normalize();
    (t.translation, t.rotation * d)
}

/// Where a world ray meets the ground or the sea, as a map point.
pub fn ground_hit(frame: &Frame, origin: DVec3, dir: Vec3) -> Option<DVec3> {
    // March in the planet's own space.
    let o = frame.planet(origin);
    let d = (frame.rot.inverse() * dir.as_dvec3()).normalize();
    let r = globe::RADIUS_M;
    let height = |p: DVec3| p.length() - r;
    let below = |p: DVec3| height(p) <= island::ground(p.normalize()).max(0.0);
    // Skip the empty air down to just above the highest ground.
    let top = r + island::HEIGHT_RANGE_M.1 + 50.0;
    let mut t = 0.0f64;
    if o.length() > top {
        let b = o.dot(d);
        let disc = b * b - (o.length_squared() - top * top);
        if disc <= 0.0 || -b - disc.sqrt() < 0.0 {
            return None;
        }
        t = -b - disc.sqrt();
    }
    let mut prev = t;
    let end = t + 4.0 * r;
    while t < end {
        let p = o + d * t;
        if p.length() > top + 1.0 && p.dot(d) > 0.0 {
            // Past the planet and climbing away.
            return None;
        }
        if below(p) {
            let (mut a, mut b) = (prev, t);
            for _ in 0..24 {
                let m = (a + b) * 0.5;
                if below(o + d * m) {
                    b = m;
                } else {
                    a = m;
                }
            }
            return Some(globe::map_of(o + d * b));
        }
        prev = t;
        // Steps grow with the height above the ground (a slope steeper than 2:1 is rare).
        let clear = height(p) - island::ground(p.normalize()).max(0.0);
        t += (clear * 0.4).max(t * 0.01).max(1.0);
    }
    None
}

/// The ground point under the mouse, if any (a map point).
pub fn pick(world: &mut World) -> Option<DVec3> {
    let px = world.resource::<Mouse>().position?;
    let window = world.resource::<engine::core::window::Window>().clone();
    let frame = *world.resource::<Frame>();
    let mut q = world.query::<(&Transform, &Camera, &StrategyCam)>();
    let (t, cam, _) = q.iter(world).next()?;
    let (o, d) = cursor_ray(t, cam, &window, px);
    ground_hit(&frame, o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_rises_with_zoom_and_the_centre_ray_hits_the_focus() {
        assert!(StrategyCam::pitch_for(MIN_DIST) < StrategyCam::pitch_for(MAX_DIST));
        let cam = StrategyCam::new(
            DVec3::new(120.0, island::height(120.0, -40.0), -40.0),
            0.7,
            300.0,
        );
        let frame = Frame::default();
        let t = cam.transform(&frame);
        let hit = ground_hit(&frame, t.translation, t.rotation * -Vec3::Z).unwrap();
        assert!(
            (hit - cam.focus).length() < 30.0,
            "{hit:?} vs {:?}",
            cam.focus
        );
    }
}
