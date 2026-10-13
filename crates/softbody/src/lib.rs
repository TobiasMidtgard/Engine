#![doc = include_str!("../README.md")]

pub mod api;
pub mod body;
pub mod character;
pub mod cloth;
pub mod collide;
pub mod demo;
pub mod gpu_cloth;
pub mod hair;
pub mod render;
pub mod rope;
pub mod settings;
pub mod solver;
pub mod systems;
pub mod volume;

use engine_core::prelude::*;
use engine_render::RenderAppExt;

pub use body::{SoftBodySim, SoftKind};
pub use character::{BoneCapsule, BoneColliders};
pub use cloth::{Cloth, ClothPins};
pub use gpu_cloth::GpuClothProbe;
pub use hair::{FurZone, HairEmitter, HairGroom, HairStyle, MeshSkin, MeshSkins};
pub use rope::{Beads, BridgePlank, Rope, RopeBridge, RopeEnd, SoftLoad};
pub use settings::{Pattern, SimSettings, SoftBodyConfig, SoftMaterial};
pub use systems::SoftBodyFailed;
pub use volume::{SoftVolume, VolumeShape};

/// System sets of the soft-body module, in `FixedUpdate` after rigid physics.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SoftBodySet {
    /// Gather colliders and loads, react to bullets, blasts and fire.
    Prepare,
    /// Step every body.
    Step,
    /// Send impulses, smoke and landings to the world; move bridge planks.
    Output,
}

/// Cloth, ropes, rope bridges, soft volumes, hair and fur on one XPBD solver, two-way coupled
/// to rigid bodies through the physics contract. See the crate docs.
pub struct SoftBodyPlugin;

impl Plugin for SoftBodyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SoftBodyConfig>()
            .init_resource::<systems::ColliderWorld>()
            .init_resource::<systems::LoadList>()
            .init_resource::<gpu_cloth::GpuClothShift>()
            .register_resource::<SoftBodyConfig>(
                "Soft-body quality: enabled, lod_distance and freeze_distance (m from the focus), quality (substep multiplier).",
            )
            .register_component::<Cloth>(
                "Simulated cloth (flag, curtain, cape, sail): size (m), resolution, anchor (uv at the entity), panel (four corners [x, y] in local XY: top-left, top-right, bottom-right, bottom-left; m), camber (sewn-in belly, fraction of width), pins (LeftEdge, TopEdge{every}, TopCorners, Bones{rows, bones}, Points, Edges{left, top, right, bottom}), mass_per_m2 (kg/m²), stretch/shear/bend compliance (m/N), tear_ratio, pierceable, flammable, aero_drag, aero_friction, aero_lift (sails ≈ 1), sim, material. On an entity carried by a dynamic rigid body, the pins pull on that body (a sail drives its boat).",
            )
            .register_component::<Rope>(
                "Rope or cable: end_a/end_b (Free, Here{offset}, World{at}, Entity{entity, offset}), length (m), segments, radius (m), mass_per_m (kg/m), compliance, cuttable, beads (spacing, radius, elongation, facets), sim, material.",
            )
            .register_component::<RopeBridge>(
                "Walkable rope bridge along local +X: length, width, planks, sag, rail_height (m), plank size and mass (kg), sim, material.",
            )
            .register_component::<SoftVolume>(
                "Soft solid (jelly, slime, cushion): shape (Box{size, bevel} or Sphere{radius}, m), resolution, mass_kg, shape_compliance and lattice_compliance (m/N), plasticity (1/s), yield_m, detail, sim, material.",
            )
            .register_component::<HairGroom>(
                "Hair or fur grown from the parent's SkeletonPose bones: emitters (Scalp{bone, center, radii, guides, hairline}, Fur{bone, to, radius, guides, comb}, Surface{mesh (asset key of a skinned body), guides, flow_from (bone), gravity, zones [{bone, offset, radius, length}], bones, skip_bones}), style (length m, segments, strands_per_guide, spread, clumping, curl, stiffness, droop, thickness, melanin, redness, tint), sim.",
            )
            .register_component::<BoneColliders>(
                "Collision capsules on a character's bones for cloth and hair: capsules [{bone, to, radius (m), offset}], friction.",
            )
            .register_component::<SoftLoad>(
                "Weight (kg) this entity puts on rope bridges it stands on, without a physics character.",
            )
            .configure_sets(
                FixedUpdate,
                (SoftBodySet::Prepare, SoftBodySet::Step, SoftBodySet::Output)
                    .chain()
                    .in_set(SimSet::PostPhysics),
            )
            .add_systems(
                FixedUpdate,
                (
                    (
                        systems::gather_colliders,
                        systems::gather_loads,
                        systems::react_to_events,
                    )
                        .in_set(SoftBodySet::Prepare),
                    systems::step_bodies.in_set(SoftBodySet::Step),
                    (systems::emit_outputs, systems::drive_planks).in_set(SoftBodySet::Output),
                ),
            )
            .add_systems(
                PostUpdate,
                (systems::follow_world_shift, gpu_cloth::follow_world_shift)
                    .in_set(engine_world::WorldShiftSet::Apply),
            )
            .add_systems(
                PostUpdate,
                (systems::rebuild_changed, systems::build_bodies)
                    .chain()
                    .in_set(UpdateSet::AfterPropagate),
            )
            .add_render_feature(render::SoftBodyFeature::default());
        api::register(app);
        demo::register(app);
    }
}
