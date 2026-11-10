//! Rapier's quarantine (non-finite state) turns bodies off; their joints must then stay out of
//! the solver (third_party/rapier3d/PATCHES.md, patch 4). A ragdoll that went non-finite in the
//! Weaving hall's blades stop took the solver down ("entered unreachable code" in the joint
//! coloring) when every part of it was switched off at once.

use rapier3d::prelude::*;

struct World {
    pipeline: PhysicsPipeline,
    params: IntegrationParameters,
    islands: IslandManager,
    broad: BroadPhaseBvh,
    narrow: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    joints: ImpulseJointSet,
    multibodies: MultibodyJointSet,
    soft: SoftBodySet,
    ccd: CCDSolver,
}

impl World {
    fn new() -> Self {
        Self {
            pipeline: PhysicsPipeline::new(),
            params: IntegrationParameters::default(),
            islands: IslandManager::new(),
            broad: BroadPhaseBvh::new(),
            narrow: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            joints: ImpulseJointSet::new(),
            multibodies: MultibodyJointSet::new(),
            soft: SoftBodySet::new(),
            ccd: CCDSolver::new(),
        }
    }

    fn ball(&mut self, at: Vector) -> RigidBodyHandle {
        let h = self
            .bodies
            .insert(RigidBodyBuilder::dynamic().translation(at).can_sleep(false));
        self.colliders
            .insert_with_parent(ColliderBuilder::ball(0.1), h, &mut self.bodies);
        h
    }

    /// Take `parts` out with their joints and put them back, as the engine does when it moves
    /// bodies to another region (`Simulation::move_body`): new handles, same bodies.
    fn move_to_new_region(&mut self, parts: [RigidBodyHandle; 3]) -> [RigidBodyHandle; 3] {
        let made: Vec<_> = self
            .joints
            .iter()
            .map(|(_, j)| {
                let at = |b| parts.iter().position(|p| *p == b).unwrap();
                (at(j.body1()), at(j.body2()))
            })
            .collect();
        let moved = parts.map(|h| {
            let rb = self
                .bodies
                .remove(
                    h,
                    &mut self.islands,
                    &mut self.colliders,
                    &mut self.joints,
                    &mut self.multibodies,
                    &mut self.soft,
                    true,
                )
                .unwrap();
            let nh = self.bodies.insert(rb);
            self.colliders
                .insert_with_parent(ColliderBuilder::ball(0.1), nh, &mut self.bodies);
            nh
        });
        // The engine makes the joints again in a later sync, from its own description (so
        // enabled), once the bodies are in place.
        self.step();
        for (a, b) in made {
            self.joints.insert(moved[a], moved[b], link(), true);
        }
        moved
    }

    fn step(&mut self) {
        self.pipeline.step(
            Vector::new(0.0, -9.81, 0.0),
            &self.params,
            &mut self.islands,
            &mut self.broad,
            &mut self.narrow,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.joints,
            &mut self.multibodies,
            &mut self.soft,
            &mut self.ccd,
            &(),
            &(),
        );
    }
}

fn link() -> SphericalJoint {
    SphericalJointBuilder::new()
        .local_anchor1(Vector::new(0.15, 0.0, 0.0))
        .local_anchor2(Vector::new(-0.15, 0.0, 0.0))
        .build()
}

/// A chain of three balls on spherical joints, plus free balls that keep the awake island: with
/// most bodies awake, the joint selection scans every joint (as in a busy scene), so it meets
/// the chain's joints even once none of their bodies is in the active set.
fn chain(w: &mut World) -> [RigidBodyHandle; 3] {
    for i in 0..4 {
        w.ball(Vector::new(5.0 + i as f32, 1.0, 0.0));
    }
    let parts = [0.0, 0.3, 0.6].map(|x| w.ball(Vector::new(x, 1.0, 0.0)));
    for pair in parts.windows(2) {
        w.joints.insert(pair[0], pair[1], link(), true);
    }
    parts
}

#[test]
fn a_ragdoll_that_goes_non_finite_is_switched_off_without_taking_the_solver_down() {
    let mut w = World::new();
    let parts = chain(&mut w);
    for _ in 0..3 {
        w.step();
    }
    // A non-finite force: the state turns non-finite inside the step, so the quarantine
    // catches it at the end of the step and switches the parts off for the next one (a
    // non-finite velocity set between steps is caught at the start and its bodies sleep).
    for h in parts {
        w.bodies[h].add_force(Vector::new(f32::NAN, 0.0, 0.0), true);
    }
    w.step();
    // The engine moves the switched-off ragdoll to another region: bodies and joints are made
    // anew there, the bodies still disabled, the joints enabled (Rapier disables a joint only
    // when a body it already holds is disabled).
    let parts = w.move_to_new_region(parts);
    for _ in 0..5 {
        w.step();
    }
    for h in parts {
        assert!(!w.bodies[h].is_enabled(), "quarantined");
        assert!(w.bodies[h].translation().is_finite());
    }
}

#[test]
fn joints_between_switched_off_bodies_stay_out_of_the_solver() {
    let mut w = World::new();
    let parts = chain(&mut w);
    for _ in 0..3 {
        w.step();
    }
    for h in parts {
        w.bodies[h].set_enabled(false);
    }
    w.step();
    let parts = w.move_to_new_region(parts);
    for _ in 0..5 {
        w.step();
    }
    // Back on, the chain hangs together again.
    for h in parts {
        w.bodies[h].set_enabled(true);
    }
    for _ in 0..30 {
        w.step();
    }
    let gap = (w.bodies[parts[1]].translation() - w.bodies[parts[0]].translation()).length();
    assert!((gap - 0.3).abs() < 0.05, "joint holds again: {gap}");
}
