//! The grapple reticle: a small mark at the middle of the view in play mode that says whether
//! the grapple (F) would hook what it points at (playtest 2026-10-29 #19, 2026-11-01 #15).
//!
//! As in Titanfall 2 and Apex Legends, the mark itself carries the state: in range its ticks
//! close in, brighten and a ring lights around them; out of range they spread and dim; while
//! the grapple cools down the ring fills back in, and while the hook is out it stays lit.
//! [`GrappleAim`] (written every frame in `Update`) holds the same answer for a game's own
//! HUD (`$GrappleAim.ready`): turn [`GrappleReticle`] off when that HUD draws its own.
//!
//! The mark is sized from the window's height (24 px tall at 1080p by default) and drawn over
//! the main view's middle, after the scene and before the UI, as one small quad.

use bytemuck::{Pod, Zeroable};
use engine_core::math::{UVec2, Vec2, damp};
use engine_core::prelude::*;
use engine_render::{
    Camera, FrameContext, GpuShared, RenderFeature, RenderSettings, ViewData, wgpu,
};
use engine_world::interact::Using;
use engine_world::physics::{CharacterController, PhysicsQueries, QueryFilter};

use crate::grapple::shoulders;
use crate::mover::Mover;
use crate::player::PlayerControlled;
use crate::profile::MovementProfile;

/// What the player's grapple would do now (`Update`, after the camera moves).
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GrappleAim {
    /// A player character is on foot with a camera to aim from: the reticle shows.
    pub active: bool,
    /// Fired now, the hook would catch on something.
    pub ready: bool,
    /// How far away that is, m (0 when not ready).
    pub distance_m: f32,
    /// The grapple's reach from the shoulders, m.
    pub range_m: f32,
    /// 0..1: 1 once the grapple can fire again.
    pub charge: f32,
    /// The hook is out (flying or holding).
    pub hooked: bool,
}

/// How the grapple reticle looks. Patch it live (`resource.set GrappleReticle`).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GrappleReticle {
    pub enabled: bool,
    /// Height of the whole mark at a 1080-pixel-tall window, pixels (it scales with the
    /// window).
    pub size_px: f32,
    /// Linear RGB of the ring lit in range.
    pub color: [f32; 3],
}

impl Default for GrappleReticle {
    fn default() -> Self {
        Self {
            enabled: true,
            size_px: 24.0,
            color: [0.25, 0.95, 0.8],
        }
    }
}

/// Fill [`GrappleAim`] for the player character on foot: the same two rays a shot takes, from
/// the camera to what it looks at, then from the shoulders to that point.
#[allow(clippy::type_complexity)]
pub(crate) fn aim_grapple(
    mut aim: ResMut<GrappleAim>,
    queries: Res<PhysicsQueries>,
    cameras: Query<(&Camera, &GlobalTransform, &Transform, Has<ChildOf>)>,
    players: Query<
        (
            Entity,
            &Transform,
            &Mover,
            &MovementProfile,
            &CharacterController,
        ),
        (With<PlayerControlled>, Without<Using>),
    >,
) {
    let view = cameras
        .iter()
        .filter(|(c, ..)| c.active)
        .max_by_key(|(c, ..)| c.order)
        // A root camera moved this frame is where its Transform says (not propagated yet).
        .map(|(_, g, t, child)| if child { *g } else { GlobalTransform::from(*t) });
    let next = match (view, players.iter().next()) {
        (Some(cam), Some((me, t, m, p, cc))) if p.grapple => {
            let (eye, fwd) = (cam.translation(), cam.forward());
            let from = shoulders(t.translation, m.up, cc.height);
            let filter = QueryFilter::excluding(me);
            // As `player::grapple_aim` and `grapple::fire` do it.
            let reach = p.grapple_range + (t.translation - eye).length() as f32;
            let hit = queries
                .0
                .raycast(eye, fwd, reach, &filter)
                .map(|h| h.point)
                .and_then(|at| {
                    let dir = (at - from).as_vec3().try_normalize()?;
                    queries.0.raycast(from, dir, p.grapple_range, &filter)
                });
            let cooldown = p.grapple_cooldown_secs.max(0.01);
            let busy = m.mantle.is_some() || m.vault.is_some() || m.hang.is_some();
            let charge = (1.0 - m.grapple_cooldown / cooldown).clamp(0.0, 1.0);
            GrappleAim {
                active: true,
                ready: hit.is_some() && charge >= 1.0 && !busy,
                distance_m: hit.map_or(0.0, |h| h.distance),
                range_m: p.grapple_range,
                charge,
                hooked: m.grapple.is_some(),
            }
        }
        _ => GrappleAim::default(),
    };
    if *aim != next {
        *aim = next;
    }
}

const SHADER: &str = r#"
struct MvReticle {
    rect: vec4<f32>,  // center (ndc), half size (ndc)
    size: vec4<f32>,  // half size (px), px per 1080p pixel, alpha
    state: vec4<f32>, // lit 0..1, ring 0..1, hooked
    color: vec4<f32>, // ring rgb
};
@group(0) @binding(0) var<uniform> ret: MvReticle;

struct RetOut {
    @builtin(position) position: vec4<f32>,
    @location(0) p: vec2<f32>,
};

@vertex
fn vs_mv_reticle(@builtin(vertex_index) index: u32) -> RetOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = corners[index];
    var out: RetOut;
    out.position = vec4<f32>(ret.rect.xy + c * ret.rect.zw, 0.0, 1.0);
    out.p = c * ret.size.x;
    return out;
}

fn seg(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

fn over(src: vec4<f32>, dst: vec4<f32>) -> vec4<f32> {
    let a = src.a + dst.a * (1.0 - src.a);
    let c = (src.rgb * src.a + dst.rgb * dst.a * (1.0 - src.a)) / max(a, 1e-4);
    return vec4<f32>(c, a);
}

@fragment
fn fs_mv_reticle(in: RetOut) -> @location(0) vec4<f32> {
    let px = ret.size.y;
    let lit = ret.state.x;
    let p = in.p;
    // Four ticks round the middle: they close in when the hook would catch.
    let gap = mix(8.5, 4.5, lit) * px;
    let len = 4.5 * px;
    var d = seg(p, vec2<f32>(gap, 0.0), vec2<f32>(gap + len, 0.0));
    d = min(d, seg(p, vec2<f32>(-gap, 0.0), vec2<f32>(-gap - len, 0.0)));
    d = min(d, seg(p, vec2<f32>(0.0, gap), vec2<f32>(0.0, gap + len)));
    d = min(d, seg(p, vec2<f32>(0.0, -gap), vec2<f32>(0.0, -gap - len)));
    let r = length(p);
    let half_line = 0.75 * px;
    let marks = min(d - half_line, r - 1.1 * px);
    // The ring: lit in range, filling clockwise from the top while the grapple recharges.
    let ring_r = 10.0 * px;
    let turn = fract(atan2(p.x, p.y) / TAU + 1.0);
    let on_arc = step(turn, ret.state.y);
    let ring = abs(r - ring_r) - 0.7 * px;
    let aa = max(px, 0.75);
    let marks_a = (1.0 - smoothstep(0.0, aa, marks)) * mix(0.45, 0.95, lit);
    let ring_a = (1.0 - smoothstep(0.0, aa, ring)) * on_arc * mix(0.35, 0.9, max(lit, ret.state.z));
    // A soft dark edge keeps it readable on snow and sky.
    let ring_shade = select(1e4, ring, on_arc > 0.5 && ret.state.y > 0.01);
    let shade = 1.0 - smoothstep(0.0, 1.6 * aa, min(marks, ring_shade) - 0.9 * px);
    var out = vec4<f32>(vec3<f32>(0.0), shade * 0.35);
    let ring_color = mix(vec3<f32>(0.8), ret.color.rgb, max(lit, ret.state.z));
    out = over(vec4<f32>(ring_color, ring_a), out);
    out = over(vec4<f32>(vec3<f32>(1.0), marks_a), out);
    return vec4<f32>(out.rgb, out.a * ret.size.z);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct Uniform {
    rect: [f32; 4],
    size: [f32; 4],
    state: [f32; 4],
    color: [f32; 4],
}

/// The reticle's render feature (added by the plugin).
#[derive(Default)]
pub(crate) struct ReticleFeature {
    pipeline: Option<wgpu::RenderPipeline>,
    buffer: Option<wgpu::Buffer>,
    group: Option<wgpu::BindGroup>,
    uniform: Uniform,
    visible: bool,
    /// Eased state: lit, ring, fade in.
    lit: f32,
    ring: f32,
    shown: f32,
    last_secs: Option<f64>,
}

impl RenderFeature for ReticleFeature {
    fn name(&self) -> &str {
        "movement_grapple_reticle"
    }

    /// Over the slow-motion grade (500), under debug lines (1000) and the UI.
    fn order(&self) -> i32 {
        510
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let shader = gpu.shader("movement grapple reticle", SHADER, false);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("movement grapple reticle"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("movement grapple reticle"),
            size: std::mem::size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("movement grapple reticle"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("movement grapple reticle"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        self.pipeline = Some(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("movement grapple reticle"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_mv_reticle"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_mv_reticle"),
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
        if view.extra_view {
            return;
        }
        self.visible = false;
        let style = world
            .get_resource::<GrappleReticle>()
            .copied()
            .unwrap_or_default();
        let aim = world
            .get_resource::<GrappleAim>()
            .copied()
            .unwrap_or_default();
        let now = world
            .get_resource::<Time>()
            .map_or(0.0, |t| t.real_elapsed_secs_f64());
        let dt = self
            .last_secs
            .map_or(1.0, |t| (now - t).clamp(0.0, 0.25) as f32);
        self.last_secs = Some(now);
        let on = style.enabled && aim.active;
        self.shown = damp(self.shown, on as u8 as f32, 12.0, dt);
        self.lit = damp(self.lit, (aim.ready || aim.hooked) as u8 as f32, 20.0, dt);
        let ring = if aim.hooked || aim.ready {
            1.0
        } else if aim.charge < 1.0 {
            aim.charge
        } else {
            0.0
        };
        // Fill as the cooldown runs, snap empty when the shot leaves.
        self.ring = if ring < self.ring - 0.5 {
            ring
        } else {
            damp(self.ring, ring, 20.0, dt)
        };
        if self.shown < 0.01 {
            return;
        }
        // In the middle of the scene's rectangle, sized from the window.
        let window = view.window.max(UVec2::ONE).as_vec2();
        let px = (window.y / 1080.0).max(0.5) * style.size_px / 24.0;
        let half = 14.0 * px;
        let center = view.viewport_origin.as_vec2() + view.viewport_size.as_vec2() * 0.5;
        let ndc = Vec2::new(
            center.x / window.x * 2.0 - 1.0,
            1.0 - center.y / window.y * 2.0,
        );
        self.uniform = Uniform {
            rect: [ndc.x, ndc.y, half * 2.0 / window.x, half * 2.0 / window.y],
            size: [half, px, self.shown, 0.0],
            state: [self.lit, self.ring, aim.hooked as u8 as f32, 0.0],
            color: [style.color[0], style.color[1], style.color[2], 1.0],
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
            label: Some("movement grapple reticle"),
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
        pass.draw(0..6, 0..1);
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "visible": self.visible,
            "lit": self.lit,
            "ring": self.ring,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reticle_shader_validates() {
        let code = engine_render::compose_shader(SHADER, false);
        engine_render::validate_wgsl("movement_grapple_reticle", &code).unwrap();
        assert_eq!(std::mem::size_of::<Uniform>(), 64);
    }
}
