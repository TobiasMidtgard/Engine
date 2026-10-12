use engine_physics::PhysicsPlugin;

#[test]
fn conforms_to_the_physics_contract() {
    engine_world::physics::conformance::assert_all(|app| {
        app.add_plugins(PhysicsPlugin);
    });
}
