//! The planet: one small round world that every region of the game lies on.
//!
//! The game thinks on flat charts. Each [`Region`] (a continent and its seas) has a chart
//! centered on its anchor on the globe; chart coordinates are *map space*: `x` east, `z` south,
//! meters along the ground, `y` meters above sea level. Grid cells, roads, island shapes and
//! prices all live there, each region's sheet set far apart from the next along map `x` so
//! cells never collide.
//!
//! A chart is an azimuthal equidistant projection: distances and bearings from the anchor are
//! true, and within a region's reach lengths are off by well under one percent. This module
//! turns map points into points on the planet (and back), and gives the frame a building,
//! ship or camera stands in there: +Y along the planet's radius.
//!
//! The world the engine draws is the planet seen from a frame the game moves: the spot the
//! camera looks at is kept near the world's origin with +Y up ([`reanchor`]), since the sky,
//! the clouds, smoke and wind all take +Y for up. [`Frame`] is that frame; everything the game
//! places goes through it.

use engine::prelude::*;
use engine::world::{WorldShift, WorldShiftSet};

/// Radius of the planet at sea level, meters: small enough that continents are a day's sail
/// apart and the whole globe is one long zoom away.
pub const RADIUS_M: f64 = 40_000.0;

/// Map-space distance from the home sheet's middle to each other region's, meters: far
/// enough apart that any sheet can chart the whole planet (half way round is 126 km) without
/// running into the next, and near enough that single-precision map numbers stay good to
/// two centimeters.
pub const SHEET_M: f64 = 250_000.0;

/// What grows and what the ground looks like in a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Biome {
    Temperate,
    Tropical,
    Arid,
    Arctic,
}

/// A continent and its seas: one sheet of map space.
#[derive(Clone, Debug)]
pub struct Region {
    pub name: &'static str,
    /// One line for the globe's label and the tooltip.
    pub blurb: &'static str,
    pub biome: Biome,
    /// Where its chart's middle sits on the planet, degrees.
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// How far its lands and coastal waters reach from the middle, meters.
    pub reach_m: f64,
    anchor: DVec3,
    east: DVec3,
    south: DVec3,
}

/// Planet-local unit direction of a latitude and longitude, degrees (the terrain's
/// convention: +Y north, longitude 0 toward +Z, 90° toward +X).
pub fn dir_from_lat_lon(lat_deg: f64, lon_deg: f64) -> DVec3 {
    let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
    DVec3::new(lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos())
}

impl Region {
    fn new(
        name: &'static str,
        blurb: &'static str,
        biome: Biome,
        lat_deg: f64,
        lon_deg: f64,
        reach_m: f64,
    ) -> Self {
        let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
        let anchor = dir_from_lat_lon(lat_deg, lon_deg);
        let east = DVec3::new(lon.cos(), 0.0, -lon.sin());
        let north = DVec3::new(-lat.sin() * lon.sin(), lat.cos(), -lat.sin() * lon.cos());
        Self {
            name,
            blurb,
            biome,
            lat_deg,
            lon_deg,
            reach_m,
            anchor,
            east,
            south: -north,
        }
    }

    /// Planet-local unit direction of the chart's middle.
    pub fn anchor(&self) -> DVec3 {
        self.anchor
    }
}

/// The regions. The home region is first; its sheet's middle is map `(0, 0)`.
pub fn regions() -> &'static [Region] {
    static REGIONS: std::sync::OnceLock<Vec<Region>> = std::sync::OnceLock::new();
    REGIONS.get_or_init(|| {
        vec![
            Region::new(
                "The Home Isles",
                "Green islands in a cool sea: wool, grain and brick.",
                Biome::Temperate,
                46.0,
                0.0,
                9_000.0,
            ),
            Region::new(
                "Sarrowmere",
                "A jungle coast under the sun: palms, cane and hardwood.",
                Biome::Tropical,
                8.0,
                24.0,
                11_000.0,
            ),
            Region::new(
                "The Amber Reach",
                "Dry hills and thorn savanna, red earth and long beaches.",
                Biome::Arid,
                22.0,
                -34.0,
                11_000.0,
            ),
            Region::new(
                "Frostholm",
                "Ice, dark spruce and whaling grounds at the top of the world.",
                Biome::Arctic,
                80.0,
                -150.0,
                10_000.0,
            ),
        ]
    })
}

/// Map-space middle of region `k`'s sheet: the home sheet at the origin, the others east,
/// west and north of it.
pub fn sheet_origin(k: usize) -> DVec2 {
    match k {
        0 => DVec2::ZERO,
        1 => DVec2::new(SHEET_M, 0.0),
        2 => DVec2::new(-SHEET_M, 0.0),
        _ => DVec2::new(0.0, -SHEET_M),
    }
}

/// The region whose sheet map point `(x, z)` lies on.
pub fn region_of(x: f64, z: f64) -> usize {
    let half = SHEET_M * 0.5;
    if x > half {
        1
    } else if x < -half {
        2
    } else if z < -half {
        3
    } else {
        0
    }
}

/// The region of a map point.
pub fn region_at(x: f64, z: f64) -> &'static Region {
    &regions()[region_of(x, z)]
}

/// Planet-local unit direction of map point `(x, z)`.
pub fn dir(x: f64, z: f64) -> DVec3 {
    let k = region_of(x, z);
    let r = &regions()[k];
    let l = DVec2::new(x, z) - sheet_origin(k);
    let rho = l.length();
    if rho < 1e-6 {
        return r.anchor;
    }
    let (s, c) = (rho / RADIUS_M).sin_cos();
    let t = (r.east * l.x + r.south * l.y) / rho;
    (r.anchor * c + t * s).normalize()
}

/// Planet-local position of map point `p` (`y` above sea level).
pub fn point(p: DVec3) -> DVec3 {
    dir(p.x, p.z) * (RADIUS_M + p.y)
}

/// The frame of map point `(x, z)` on the planet: its unit axes there, map +X (east on the
/// chart), up, and map +Z, as a rotation from map directions to planet-local ones.
pub fn frame(x: f64, z: f64) -> DQuat {
    let k = region_of(x, z);
    let r = &regions()[k];
    let l = DVec2::new(x, z) - sheet_origin(k);
    let rho = l.length();
    if rho < 1e-6 {
        return DQuat::from_mat3(&DMat3::from_cols(r.east, r.anchor, r.south));
    }
    let (s, c) = (rho / RADIUS_M).sin_cos();
    let (ux, uz) = (l.x / rho, l.y / rho);
    // Along the radius of the chart, and across it.
    let t = r.east * ux + r.south * uz;
    let radial = t * c - r.anchor * s;
    let across = r.south * ux - r.east * uz;
    let up = r.anchor * c + t * s;
    // Across the chart's radius the ground is a little shorter than the chart shows it.
    let k = s / (rho / RADIUS_M);
    let x_axis = (radial * ux - across * (uz * k)).normalize();
    let z_axis = x_axis.cross(up).normalize();
    DQuat::from_mat3(&DMat3::from_cols(x_axis, up, z_axis)).normalize()
}

/// The map point under planet-local direction `d` on region `k`'s chart (`y` = 0).
pub fn map_on(k: usize, d: DVec3) -> DVec3 {
    let r = &regions()[k];
    let c = d.dot(r.anchor).clamp(-1.0, 1.0);
    let t = d - r.anchor * c;
    let len = t.length();
    let o = sheet_origin(k);
    if len < 1e-12 {
        return DVec3::new(o.x, 0.0, o.y);
    }
    let rho = c.acos() * RADIUS_M;
    DVec3::new(
        o.x + t.dot(r.east) / len * rho,
        0.0,
        o.y + t.dot(r.south) / len * rho,
    )
}

/// The region nearest planet-local direction `d` (by how far into its reach it lies).
pub fn nearest_region(d: DVec3) -> usize {
    let mut best = (0, f64::MAX);
    for (k, r) in regions().iter().enumerate() {
        let a = d.dot(r.anchor).clamp(-1.0, 1.0).acos() * RADIUS_M / r.reach_m;
        if a < best.1 {
            best = (k, a);
        }
    }
    best.0
}

/// The map point of a planet-local position: on the nearest region's chart, `y` its height
/// above sea level.
pub fn map_of(p: DVec3) -> DVec3 {
    let len = p.length();
    let d = p / len;
    let mut m = map_on(nearest_region(d), d);
    m.y = len - RADIUS_M;
    m
}

/// How far map point `(x, z)` is from the middle of its sheet, as a share of the region's
/// reach (1 at the rim).
pub fn reach_share(x: f64, z: f64) -> f64 {
    let k = region_of(x, z);
    (DVec2::new(x, z) - sheet_origin(k)).length() / regions()[k].reach_m
}

/// A map bearing (yaw about +Y, 0 looking along −Z) at `from` carried to map point `to`,
/// which may be on another sheet: the same direction over the ground.
pub fn carry_yaw(from: DVec3, yaw: f32, to: DVec3) -> f32 {
    let look = frame(from.x, from.z) * (DQuat::from_rotation_y(yaw as f64) * DVec3::NEG_Z);
    let local = frame(to.x, to.z).inverse() * look;
    (-local.x).atan2(-local.z) as f32
}

/// Great-circle distance between two map points (on any sheets), meters at sea level.
pub fn distance(a: DVec3, b: DVec3) -> f64 {
    dir(a.x, a.z).dot(dir(b.x, b.z)).clamp(-1.0, 1.0).acos() * RADIUS_M
}

// ------------------------------------------------------------------------------ ground tiles

/// Side of a ground tile, map meters: things drawn many at a time (houses, hedges, people)
/// are grouped by tile, each group standing flat on the planet at its tile's middle. Across
/// a tile the planet's up turns by a third of a degree.
pub const TILE_M: f64 = 384.0;

/// A patch of the planet with its own flat space: origin at a map point at sea level, axes
/// the map's there. What is placed in it lies exactly where the round planet has it.
#[derive(Clone, Copy, Debug)]
pub struct Local {
    /// Planet-local rotation of its axes and position of its origin.
    pub rot: DQuat,
    pub origin: DVec3,
    inv: DQuat,
}

impl Local {
    /// The local space at map point `(x, z)`.
    pub fn at(x: f64, z: f64) -> Self {
        let rot = frame(x, z);
        Self {
            rot,
            origin: dir(x, z) * RADIUS_M,
            inv: rot.inverse(),
        }
    }

    /// The local space of the tile that holds map point `(x, z)`.
    pub fn tile(tile: IVec2) -> Self {
        Self::at(
            (tile.x as f64 + 0.5) * TILE_M,
            (tile.y as f64 + 0.5) * TILE_M,
        )
    }

    /// Map point `p` in this space.
    pub fn put(&self, p: DVec3) -> Vec3 {
        (self.inv * (point(p) - self.origin)).as_vec3()
    }
}

/// The tile that holds map point `(x, z)`.
pub fn tile_of(x: f64, z: f64) -> IVec2 {
    IVec2::new((x / TILE_M).floor() as i32, (z / TILE_M).floor() as i32)
}

// ------------------------------------------------------------------------------ the frame

/// The world frame the planet is drawn in: `world = rot · planet + pos`.
#[derive(Resource, Clone, Copy, Debug)]
pub struct Frame {
    pub rot: DQuat,
    pub pos: DVec3,
}

impl Default for Frame {
    fn default() -> Self {
        let home = regions()[0].anchor;
        // The home region's middle on top, its chart's axes along the world's.
        let rot = frame(0.0, 0.0).inverse();
        Self {
            rot,
            pos: -(rot * (home * RADIUS_M)),
        }
    }
}

impl Frame {
    /// World position of a planet-local one.
    pub fn world(&self, planet: DVec3) -> DVec3 {
        self.rot * planet + self.pos
    }

    /// Planet-local position of a world one.
    pub fn planet(&self, world: DVec3) -> DVec3 {
        self.rot.inverse() * (world - self.pos)
    }

    /// World position of a map point.
    pub fn at(&self, p: DVec3) -> DVec3 {
        self.world(point(p))
    }

    /// World rotation of something standing at map point `(x, z)` turned `yaw` about its up.
    pub fn facing(&self, x: f64, z: f64, yaw: f32) -> Quat {
        (self.rot * frame(x, z) * DQuat::from_rotation_y(yaw as f64))
            .normalize()
            .as_quat()
    }

    /// The transform of something standing at map point `p` turned `yaw` about its up.
    pub fn stand(&self, p: DVec3, yaw: f32) -> Transform {
        Transform::from_translation(self.at(p)).with_rotation(self.facing(p.x, p.z, yaw))
    }

    /// The transform of a [`Local`] space.
    pub fn local(&self, l: &Local) -> Transform {
        Transform::from_translation(self.world(l.origin))
            .with_rotation((self.rot * l.rot).normalize().as_quat())
    }

    /// World position of the planet's middle.
    pub fn center(&self) -> DVec3 {
        self.pos
    }
}

/// The planet's ground entity (the terrain): its transform is the frame.
#[derive(Component)]
pub struct Planet;

/// Read the frame from the planet's transform (the engine moves it when the world shifts).
pub fn track_frame(mut frame: ResMut<Frame>, q: Query<&Transform, With<Planet>>) {
    if let Some(t) = q.iter().next() {
        let rot = t.rotation.as_dquat().normalize();
        if frame.rot != rot || frame.pos != t.translation {
            *frame = Frame {
                rot,
                pos: t.translation,
            };
        }
    }
}

/// How far the camera's focus may stray from the world's origin before the world is moved
/// under it, meters (the sky's up leans one degree every 700 m).
const STRAY_M: f64 = 600.0;

/// Keep the spot the camera looks at near the world's origin with +Y up: when it has strayed,
/// shift the world (the engine and every module move with it).
pub fn reanchor(world: &mut World) {
    let mut q = world.query::<&crate::camera::StrategyCam>();
    let Some(focus) = q.iter(world).next().map(|c| c.focus) else {
        return;
    };
    let frame = *world.resource::<Frame>();
    // The sea-level point under the focus, and up there.
    let spot = frame.at(DVec3::new(focus.x, 0.0, focus.z));
    let up = (spot - frame.center()).normalize();
    if spot.length() < STRAY_M && up.y > 0.999_9 {
        return;
    }
    let rotation = DQuat::from_rotation_arc(up, DVec3::Y);
    let shift = WorldShift {
        rotation,
        translation: -(rotation * spot),
        ..WorldShift::IDENTITY
    };
    world.write_message(shift);
    // The frame follows at once so this frame's placements agree with the moved world.
    let mut f = world.resource_mut::<Frame>();
    f.rot = (rotation * f.rot).normalize();
    f.pos = shift.point(f.pos);
}

/// The systems that keep the frame.
pub fn add(app: &mut App) {
    app.init_resource::<Frame>()
        .add_systems(First, track_frame)
        .add_systems(PostUpdate, reanchor.in_set(WorldShiftSet::Write));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charts_round_trip_and_keep_lengths() {
        for (k, r) in regions().iter().enumerate() {
            let o = sheet_origin(k);
            for (dx, dz) in [
                (0.0, 0.0),
                (1200.0, -300.0),
                (-4000.0, 5200.0),
                (7000.0, 100.0),
            ] {
                let p = DVec3::new(o.x + dx, 12.0, o.y + dz);
                let back = map_of(point(p));
                assert!((back - p).length() < 1e-3, "{}: {p} -> {back}", r.name);
                // A step of a meter on the chart is a meter on the ground, within 1.5 %.
                let step = (point(p + DVec3::X) - point(p)).length();
                assert!((step - 1.0).abs() < 0.015, "{}: step {step}", r.name);
                // The frame's axes follow the chart's.
                let f = frame(p.x, p.z);
                let east = (point(p + DVec3::X) - point(p)).normalize();
                assert!((f * DVec3::X - east).length() < 1e-3);
                assert!((f * DVec3::Y - dir(p.x, p.z)).length() < 1e-6);
            }
        }
    }

    #[test]
    fn regions_keep_apart() {
        let rs = regions();
        for (i, a) in rs.iter().enumerate() {
            for b in &rs[i + 1..] {
                let d = a.anchor.dot(b.anchor).acos() * RADIUS_M;
                assert!(
                    d > a.reach_m + b.reach_m + 2_000.0,
                    "{} and {} are {d:.0} m apart",
                    a.name,
                    b.name
                );
            }
            // Its own middle is nearest to itself.
            assert_eq!(nearest_region(a.anchor), i);
        }
    }

    #[test]
    fn the_home_frame_is_the_map() {
        let f = Frame::default();
        let p = DVec3::new(300.0, 5.0, -200.0);
        let w = f.at(p);
        assert!(
            (w.x - 300.0).abs() < 0.5 && (w.z + 200.0).abs() < 0.5,
            "{w}"
        );
        // A hundred-odd meters out the ground has dropped a little under the flat map.
        assert!(w.y < 5.0 && w.y > 3.0, "{w}");
        let back = map_of(f.planet(w));
        assert!((back - p).length() < 1e-3);
        let yaw = carry_yaw(p, 0.7, p);
        assert!((yaw - 0.7).abs() < 1e-5);
    }
}
