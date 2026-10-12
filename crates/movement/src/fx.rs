//! Screen and camera cues for speed and slow motion.
//!
//! * [`SlowMotionFx`] (resource): while the world is slowed, a cool grade and darker,
//!   tinted edges; while the perceiver dashes, radial speed lines. Drawn by a small overlay
//!   [`RenderFeature`] onto the final image.
//! * [`CameraFx`] (component on a camera): FOV kick when sprinting or dashing, a narrower
//!   view in focus, a roll into the wall while wall-running.

use bytemuck::{Pod, Zeroable};
use engine_core::math::{Quat, damp};
use engine_core::prelude::*;
use engine_render::{
    Camera, FrameContext, GpuShared, Projection, RenderFeature, RenderSettings, ViewData, wgpu,
};
use engine_world::LocomotionMode;

use crate::mover::Mover;
use crate::tempo::{FOCUS_KEY, Perceiver};

/// Slow-motion screen effect settings. Patch it live (`resource.set SlowMotionFx`).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlowMotionFx {
    pub enabled: bool,
    /// Edge darkening at full slow motion, 0..1.
    pub vignette: f32,
    /// Linear RGB of the slow-motion tint.
    pub tint: [f32; 3],
    /// Tint over the whole image at full slow motion, 0..1.
    pub grade: f32,
    /// Speed lines while the perceiver dashes, 0..1.
    pub speed_lines: f32,
    /// Current strength (0 = normal speed, 1 = time stopped). Written each frame.
    pub strength: f32,
}

impl Default for SlowMotionFx {
    fn default() -> Self {
        Self {
            enabled: true,
            vignette: 0.5,
            tint: [0.35, 0.6, 1.0],
            grade: 0.1,
            speed_lines: 0.5,
            strength: 0.0,
        }
    }
}

const SHADER: &str = r#"
struct MvFx {
    tint: vec4<f32>,   // rgb, grade
    params: vec4<f32>, // strength, vignette, lines, time
    extra: vec4<f32>,  // aspect
};
@group(0) @binding(0) var<uniform> mv_fx: MvFx;

@vertex
fn vs_mv_fx(@builtin(vertex_index) index: u32) -> FullscreenOut {
    return fullscreen_vertex(index, 0.0);
}

@fragment
fn fs_mv_fx(in: FullscreenOut) -> @location(0) vec4<f32> {
    let strength = mv_fx.params.x;
    let lines = mv_fx.params.z;
    let c = (in.uv - vec2<f32>(0.5)) * vec2<f32>(mv_fx.extra.x, 1.0);
    let r = length(c);
    let edge = smoothstep(0.25, 0.9, r);
    var color = mv_fx.tint.rgb * 0.12;
    var alpha = strength * (mv_fx.tint.w + edge * mv_fx.params.y);
    // Radial speed lines at the edges: 160 sectors, a few lit at a time, flickering at 12 Hz.
    // Each is a brush stroke: a wedge that tapers to a point toward the middle of the frame,
    // with its own length and strength.
    let a = atan2(c.y, c.x) / TAU * 160.0;
    let sector = floor(a);
    let seed = hash12(vec2<f32>(sector, floor(mv_fx.params.w * 12.0)));
    let lit = step(0.82, seed);
    let start = 0.4 + 0.25 * fract(seed * 7.13);
    let grow = smoothstep(start, start + 0.5, r);
    let half_width = mix(0.03, 0.32, grow);
    let d = abs(fract(a) - 0.5);
    let stroke = 1.0 - smoothstep(half_width * 0.55, half_width, d);
    let weight = 0.55 + 0.45 * fract(seed * 13.7);
    let streak = lit * stroke * sqrt(grow) * weight * lines;
    let line_color = mix(vec3<f32>(1.0), mv_fx.tint.rgb, 0.35);
    color = mix(color, line_color, saturate(streak / max(alpha + streak, 1e-4)));
    alpha = alpha + streak * 0.35;
    return vec4<f32>(color, clamp(alpha, 0.0, 0.8));
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct Uniform {
    tint: [f32; 4],
    params: [f32; 4],
    extra: [f32; 4],
}

/// The overlay feature (added by the plugin).
#[derive(Default)]
pub(crate) struct SlowMotionOverlay {
    pipeline: Option<wgpu::RenderPipeline>,
    buffer: Option<wgpu::Buffer>,
    group: Option<wgpu::BindGroup>,
    uniform: Uniform,
    visible: bool,
}

impl RenderFeature for SlowMotionOverlay {
    fn name(&self) -> &str {
        "movement_slow_motion"
    }

    /// Before debug lines (1000) and UI, after nothing else.
    fn order(&self) -> i32 {
        500
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let shader = gpu.shader("movement slow motion", SHADER, false);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("movement slow motion"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("movement slow motion"),
            size: std::mem::size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("movement slow motion"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("movement slow motion"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        self.pipeline = Some(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("movement slow motion"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_mv_fx"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_mv_fx"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.formats.ldr,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            }),
        );
        self.buffer = Some(buffer);
        self.group = Some(group);
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.visible = false;
        let Some(fx) = world.get_resource::<SlowMotionFx>().copied() else {
            return;
        };
        if !fx.enabled {
            return;
        }
        let dashing = {
            let mut q = world.query_filtered::<&Mover, With<Perceiver>>();
            q.iter(world)
                .next()
                .is_some_and(|m| m.mode == LocomotionMode::Dash)
        };
        let t = world
            .get_resource::<Time>()
            .map_or(0.0, |t| t.real_elapsed_secs_f64());
        let lines = if dashing { fx.speed_lines } else { 0.0 };
        if fx.strength < 0.01 && lines < 0.01 {
            return;
        }
        self.uniform = Uniform {
            tint: [fx.tint[0], fx.tint[1], fx.tint[2], fx.grade],
            params: [fx.strength, fx.vignette, lines, (t % 1000.0) as f32],
            extra: [view.width as f32 / view.height.max(1) as f32, 0.0, 0.0, 0.0],
        };
        if let Some(buffer) = &self.buffer {
            gpu.queue
                .write_buffer(buffer, 0, bytemuck::bytes_of(&self.uniform));
            self.visible = true;
        }
    }

    fn overlay(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        let (true, Some(pipeline), Some(group)) = (self.visible, &self.pipeline, &self.group)
        else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("movement slow motion"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.ldr,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "visible": self.visible,
            "strength": self.uniform.params[0],
            "speed_lines": self.uniform.params[2],
        })
    }
}

/// Track how slowed the world is (Update). Strength follows the dilation factor but ignores
/// a user pause, so pausing never looks like slow motion.
pub(crate) fn update_slow_motion_fx(dilation: Res<TimeDilation>, mut fx: ResMut<SlowMotionFx>) {
    let s = (1.0 - dilation.factor()).clamp(0.0, 1.0);
    if (fx.strength - s).abs() > 1e-4 {
        fx.strength = s;
    }
}

/// Camera cues for the character it follows. Put it on a camera entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraFx {
    /// The character to react to (`None` = the perceiver).
    #[serde(skip)]
    pub target: Option<Entity>,
    /// Field of view with no effect, degrees (0 = read it from the camera once).
    pub base_fov_deg: f32,
    /// Added while sprinting, sliding or wall-running, degrees.
    pub sprint_fov_deg: f32,
    /// Added while dashing, degrees.
    pub dash_fov_deg: f32,
    /// Added at full focus slow motion (negative narrows the view), degrees.
    pub focus_fov_deg: f32,
    /// Roll toward the wall while wall-running, degrees.
    pub wallrun_roll_deg: f32,
    /// How quickly effects follow, 1/s.
    pub response: f32,
    /// Current FOV offset and roll (written each frame).
    pub fov_offset_deg: f32,
    pub roll_deg: f32,
}

impl Default for CameraFx {
    fn default() -> Self {
        Self {
            target: None,
            base_fov_deg: 0.0,
            sprint_fov_deg: 6.0,
            dash_fov_deg: 14.0,
            focus_fov_deg: -8.0,
            wallrun_roll_deg: 7.0,
            response: 8.0,
            fov_offset_deg: 0.0,
            roll_deg: 0.0,
        }
    }
}

/// Apply [`CameraFx`] (Update, after cameras have moved).
pub(crate) fn camera_fx(
    time: Res<Time>,
    dilation: Res<TimeDilation>,
    movers: Query<&Mover>,
    perceiver: Query<Entity, (With<Perceiver>, With<Mover>)>,
    mut cams: Query<(&mut CameraFx, &mut Camera, &mut Transform)>,
) {
    let dt = time.real_delta_secs();
    let focus = if dilation.is_active(FOCUS_KEY) {
        (1.0 - dilation.factor()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    for (mut fx, mut cam, mut transform) in &mut cams {
        let Projection::Perspective { fov_y_deg, .. } = &mut cam.projection else {
            continue;
        };
        if fx.base_fov_deg <= 0.0 {
            fx.base_fov_deg = *fov_y_deg;
        }
        let mover = fx
            .target
            .or_else(|| perceiver.iter().next())
            .and_then(|e| movers.get(e).ok());
        let (want_fov, want_roll) = match mover {
            Some(m) => (
                match m.mode {
                    LocomotionMode::Dash => fx.dash_fov_deg,
                    LocomotionMode::Sprint | LocomotionMode::Slide | LocomotionMode::WallRun => {
                        fx.sprint_fov_deg
                    }
                    _ => 0.0,
                } + fx.focus_fov_deg * focus,
                -m.wall_side() * fx.wallrun_roll_deg,
            ),
            None => (fx.focus_fov_deg * focus, 0.0),
        };
        fx.fov_offset_deg = damp(fx.fov_offset_deg, want_fov, fx.response, dt);
        *fov_y_deg = (fx.base_fov_deg + fx.fov_offset_deg).clamp(10.0, 150.0);
        let roll = damp(fx.roll_deg, want_roll, fx.response, dt);
        let delta = (roll - fx.roll_deg).to_radians();
        if delta.abs() > 1e-6 {
            transform.rotation *= Quat::from_rotation_z(delta);
        }
        fx.roll_deg = roll;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_shader_validates() {
        let code = engine_render::compose_shader(SHADER, false);
        engine_render::validate_wgsl("movement_slow_motion", &code).unwrap();
    }

    #[test]
    fn uniform_layout() {
        assert_eq!(std::mem::size_of::<Uniform>(), 48);
    }
}
