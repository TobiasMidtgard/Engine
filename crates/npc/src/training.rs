//! Training equipment for showcases and tests.
//!
//! A [`SparringPost`] is a wooden post with a swinging arm that attacks one NPC on an uneven
//! beat. Each swing is telegraphed like a real attack (the arm draws back and its tip glows)
//! and sent as an `AttackEvent`, so the NPC defends exactly as it would against a player. Line
//! up NPCs of different skill in front of posts and watch reaction time and defense decide who
//! parries (white flash, the arm bounces off), who blocks, and who gets hit (red flash).

use engine_core::prelude::*;
use engine_render::{MaterialRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::{AttackEvent, AttackKind, DamageEvent, DamageKind};

use crate::agent::{Defense, NpcClock, NpcCombat, NpcVitals};
use crate::art::ArtAssets;
use crate::art::arena::PostKit;
use crate::combat::roll;

/// A post that swings at `target` (see the module docs). The post turns to face its target.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct SparringPost {
    /// The NPC it swings at. Without one it stands still.
    #[serde(skip)]
    pub target: Option<Entity>,
    /// Average seconds between swings (world time).
    pub every_secs: f32,
    /// The beat varies by up to this many seconds either way, so it can't simply be learned.
    pub jitter_secs: f32,
    /// Telegraph: seconds from the start of a swing until it lands.
    pub windup_secs: f32,
    /// Reach of the arm, meters. Targets farther away are not swung at.
    pub reach: f32,
    pub damage: f32,
    /// Swings so far, and how they ended.
    pub swings: u32,
    pub blocked: u32,
    pub parried: u32,
    pub dodged: u32,
    pub hits: u32,
    #[serde(skip)]
    state: PostState,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct PostState {
    clock: f64,
    next_at: f64,
    started: f64,
    lands_at: Option<f64>,
    landed_at: f64,
    parried: bool,
}

impl Default for SparringPost {
    fn default() -> Self {
        Self {
            target: None,
            every_secs: 2.4,
            jitter_secs: 0.7,
            windup_secs: 0.45,
            reach: 2.4,
            damage: 5.0,
            swings: 0,
            blocked: 0,
            parried: 0,
            dodged: 0,
            hits: 0,
            state: PostState {
                next_at: 1.5,
                ..Default::default()
            },
        }
    }
}

impl SparringPost {
    pub fn new(target: Entity) -> Self {
        Self {
            target: Some(target),
            ..Default::default()
        }
    }

    /// Share of landed swings the target blocked, parried or dodged.
    pub fn defended_share(&self) -> f32 {
        let landed = self.blocked + self.parried + self.dodged + self.hits;
        if landed == 0 {
            0.0
        } else {
            (landed - self.hits) as f32 / landed as f32
        }
    }
}

/// Pieces of a post's body (children of the post).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PostPart {
    Arm,
    Tip,
}

const ARM_HEIGHT: f32 = 1.25;
const ARM_LENGTH: f32 = 1.1;

fn wood() -> StandardMaterial {
    StandardMaterial::hex(0x6e4e2e).with_roughness(0.85)
}

fn tip(hot: bool) -> StandardMaterial {
    if hot {
        StandardMaterial::hex(0xff7a1a)
            .with_emissive([1.0, 0.35, 0.05], 6.0)
            .with_roughness(0.4)
    } else {
        StandardMaterial::hex(0x3a2a1c).with_roughness(0.7)
    }
}

/// The two looks of a post's striking head: at rest, and glowing while a swing is telegraphed.
#[derive(Component, Debug, Clone, PartialEq)]
pub(crate) struct TipLooks {
    cold: MaterialRef,
    hot: MaterialRef,
}

/// Dress new posts: a stone base, a turned wooden post with iron bands and a leather grip,
/// and a beam arm with a padded leather head (plain shapes without the renderer).
pub(crate) fn build_posts(
    mut commands: Commands,
    q: Query<Entity, Added<SparringPost>>,
    mut art: ArtAssets,
) {
    if q.is_empty() {
        return;
    }
    let kit = PostKit::load(&mut art);
    for e in &q {
        let Some(kit) = &kit else {
            plain_post(&mut commands, e);
            continue;
        };
        for (mesh, material) in &kit.body {
            commands.spawn((
                Transform::default(),
                MeshRenderer::new(mesh.clone(), material.clone()),
                ChildOf(e),
            ));
        }
        let arm = commands
            .spawn((
                Transform::from_translation(DVec3::new(0.0, ARM_HEIGHT as f64, 0.0)),
                PostPart::Arm,
                ChildOf(e),
            ))
            .id();
        for (mesh, material) in &kit.arm {
            commands.spawn((
                Transform::default(),
                MeshRenderer::new(mesh.clone(), material.clone()),
                ChildOf(arm),
            ));
        }
        commands.spawn((
            Transform::from_translation(DVec3::new(0.0, 0.0, -(ARM_LENGTH as f64))),
            MeshRenderer::new(kit.tip.clone(), kit.tip_cold.clone()),
            PostPart::Tip,
            TipLooks {
                cold: kit.tip_cold.clone(),
                hot: kit.tip_hot.clone(),
            },
            ChildOf(arm),
        ));
    }
}

fn plain_post(commands: &mut Commands, e: Entity) {
    {
        let base = [
            (
                Shape::Cylinder {
                    radius: 0.16,
                    height: 1.7,
                    segments: 12,
                },
                Vec3::new(0.0, 0.85, 0.0),
            ),
            (
                Shape::Cylinder {
                    radius: 0.45,
                    height: 0.12,
                    segments: 16,
                },
                Vec3::new(0.0, 0.06, 0.0),
            ),
        ];
        for (shape, at) in base {
            commands.spawn((
                Transform::from_translation(at.as_dvec3()),
                MeshRenderer::new(shape, wood()),
                ChildOf(e),
            ));
        }
        let arm = commands
            .spawn((
                Transform::from_translation(DVec3::new(0.0, ARM_HEIGHT as f64, 0.0)),
                PostPart::Arm,
                ChildOf(e),
            ))
            .id();
        commands.spawn((
            Transform::from_translation(DVec3::new(0.0, 0.0, -(ARM_LENGTH as f64) * 0.5)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.09, 0.09, ARM_LENGTH],
                },
                wood(),
            ),
            ChildOf(arm),
        ));
        commands.spawn((
            Transform::from_translation(DVec3::new(0.0, 0.0, -(ARM_LENGTH as f64))),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 0.11,
                    segments: 12,
                },
                tip(false),
            ),
            PostPart::Tip,
            TipLooks {
                cold: MaterialRef::Inline(tip(false)),
                hot: MaterialRef::Inline(tip(true)),
            },
            ChildOf(arm),
        ));
    }
}

/// Swing on the beat, and settle each swing when it lands.
pub(crate) fn swing_posts(
    time: Res<Time>,
    mut posts: Query<(
        Entity,
        &mut SparringPost,
        &mut Transform,
        Option<&LocalTimeScale>,
    )>,
    targets: Query<(&Transform, &NpcClock, &NpcCombat, &NpcVitals), Without<SparringPost>>,
    mut attacks: MessageWriter<AttackEvent>,
    mut damage: MessageWriter<DamageEvent>,
) {
    for (me, mut post, mut t, local) in &mut posts {
        let st = &mut post.state;
        st.clock += time.delta_for(local) as f64;
        let now = st.clock;
        let Some(target) = post.target else {
            continue;
        };
        let Ok((tt, clock, combat, vitals)) = targets.get(target) else {
            continue;
        };
        // Face the target.
        let to = tt.translation - t.translation;
        if to.x.abs() + to.z.abs() > 1e-3 {
            t.rotation = Quat::from_rotation_y((-to.x as f32).atan2(-to.z as f32));
        }
        let in_reach = (to.x * to.x + to.z * to.z).sqrt() as f32 <= post.reach + 0.5;
        if let Some(lands) = post.state.lands_at {
            if now < lands {
                continue;
            }
            post.state.lands_at = None;
            post.state.landed_at = now;
            post.state.parried = false;
            if vitals.downed || !in_reach {
                continue;
            }
            let theirs = clock.now;
            match combat.defense {
                Defense::Parry { until } if theirs < until => {
                    post.parried += 1;
                    post.state.parried = true;
                }
                Defense::Block { until } if theirs < until => post.blocked += 1,
                Defense::Dodge { until } if theirs < until => post.dodged += 1,
                _ => {
                    post.hits += 1;
                    let fwd = t.forward();
                    damage.write(DamageEvent {
                        target,
                        amount: post.damage,
                        kind: DamageKind::Blunt,
                        point: tt.translation + DVec3::Y * 1.2,
                        impulse: Vec3::new(fwd.x, 0.0, fwd.z) * 20.0,
                        source: Some(me),
                    });
                }
            }
            continue;
        }
        if now < post.state.next_at || vitals.downed || !in_reach {
            continue;
        }
        let jitter = (roll(me, now, 31) * 2.0 - 1.0) * post.jitter_secs;
        post.state.next_at = now + (post.every_secs + jitter).max(post.windup_secs + 0.4) as f64;
        post.state.started = now;
        post.state.lands_at = Some(now + post.windup_secs as f64);
        post.swings += 1;
        attacks.write(AttackEvent {
            attacker: me,
            target: Some(target),
            windup_secs: post.windup_secs,
            reach: post.reach,
            kind: AttackKind::Melee,
        });
    }
}

type PostPieces = (
    &'static PostPart,
    &'static mut Transform,
    Option<&'static mut MeshRenderer>,
    Option<&'static Children>,
    Option<&'static TipLooks>,
);

/// Draw the arm back during the telegraph, sweep it through on the hit, bounce it off a parry.
pub(crate) fn pose_posts(posts: Query<(&SparringPost, &Children)>, mut parts: Query<PostPieces>) {
    for (post, children) in &posts {
        let st = post.state;
        let now = st.clock;
        // Yaw of the arm around the post (0 = pointing at the target) and its pitch.
        let (yaw, pitch, hot) = if let Some(lands) = st.lands_at {
            let k =
                (1.0 - (lands - now) / post.windup_secs.max(0.01) as f64).clamp(0.0, 1.0) as f32;
            if k < 0.75 {
                // Draw back and up.
                let e = k / 0.75;
                (1.4 + 0.5 * e, 0.5 * e, true)
            } else {
                // Sweep through.
                let e = (k - 0.75) / 0.25;
                (1.9 * (1.0 - e), 0.5 * (1.0 - e), true)
            }
        } else {
            let since = (now - st.landed_at) as f32;
            if since < 0.25 {
                let e = since / 0.25;
                if st.parried {
                    // Knocked back the way it came.
                    (0.9 * (1.0 - (1.0 - e) * (1.0 - e)), 0.3 * (1.0 - e), false)
                } else {
                    // Follow through past the target.
                    (-0.5 * (1.0 - e), 0.0, false)
                }
            } else {
                // Rest, pointing off to the side, drifting back toward it.
                let e = ((since - 0.25) / 0.6).clamp(0.0, 1.0);
                let from = if st.parried { 0.9 } else { 0.0 };
                (from + (1.4 - from) * e, 0.0, false)
            }
        };
        for child in children.iter() {
            let Ok((part, mut t, _, grandchildren, _)) = parts.get_mut(child) else {
                continue;
            };
            if *part != PostPart::Arm {
                continue;
            }
            let rot = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
            if t.rotation != rot {
                t.rotation = rot;
            }
            let tips: Vec<Entity> = grandchildren
                .map(|g| g.iter().collect())
                .unwrap_or_default();
            for g in tips {
                if let Ok((PostPart::Tip, _, Some(mut mesh), _, Some(looks))) = parts.get_mut(g) {
                    let want = if hot { &looks.hot } else { &looks.cold };
                    if mesh.material != *want {
                        mesh.material = want.clone();
                    }
                }
            }
        }
    }
}
