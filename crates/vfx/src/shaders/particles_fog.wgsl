// Particles: fog. Composed after lib_types + lib_frame + particles_common.

// The fog between the camera and a camera-relative point, as the scene gets it from `apply_fog`
// (volumetric fog with its light shafts and lamps, height fog beyond, aerial perspective far
// out): `through` is how much of the point's light reaches the camera, `inscatter` the light
// the fog adds in front of it (pre-exposed).
struct ParticleFog {
    through: vec3<f32>,
    inscatter: vec3<f32>,
};

fn particle_fog(rel: vec3<f32>) -> ParticleFog {
    let f = surface_fog(rel);
    var fog: ParticleFog;
    fog.inscatter = f.inscatter;
    fog.through = f.transmittance;
    return fog;
}
