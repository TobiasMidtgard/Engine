//! Woven elements: the magic module's authored pairs and triples (`combine::RECIPES`), drawn as
//! `spell.<weave>.<role>` like the elements. A weave is built from its ingredients' own layers,
//! thinned and turned partly toward the weave's color, plus one signature layer of its own, so
//! both ingredients stay recognisable inside a look of its own (the way a Genshin swirl is the
//! wind's shape carrying the fire's color, and Magicka's steam is its own state, not a mix of
//! colors; engine-design/particles-research.md, round 30).
//!
//! - clean: steam (fire + frost), plasma (fire + storm), tempest (frost + storm), aether (all
//!   three);
//! - dark: plague (blood + blight), nightfeed (blood + shadow), wither (blight + shadow), doom
//!   (all three).

use super::*;

/// One ingredient of a weave: an existing effect's layers, thinned to `rate` of their count and
/// turned `tint` of the way to the weave's color.
struct Part {
    from: &'static str,
    rate: f32,
    tint: f32,
}

const fn part(from: &'static str, rate: f32, tint: f32) -> Part {
    Part { from, rate, tint }
}

/// `desc`'s layers renamed with `prefix` (sub-emitters follow), counts scaled by `rate`. A
/// recolored part drops its fireballs: their soot is lit by the scene, not by their color, so
/// a tinted fireball turns into a dark lump.
fn layers_of(desc: &EffectDesc, prefix: &str, rate: f32, tinted: bool) -> Vec<EmitterDesc> {
    desc.emitters
        .iter()
        .filter(|e| !(tinted && e.sprite == Sprite::Fireball))
        .map(|e| {
            let mut e = e.clone();
            e.name = format!("{prefix}{}", e.name);
            for sub in [&mut e.on_hit, &mut e.on_death, &mut e.trailing]
                .into_iter()
                .flatten()
            {
                sub.layer = format!("{prefix}{}", sub.layer);
            }
            e.rate *= rate;
            e.rate_per_meter *= rate;
            for b in &mut e.bursts {
                b.count = ((b.count as f32 * rate).round() as u32).max(1);
            }
            e
        })
        .collect()
}

/// A weave made of `parts` plus its own `extra` layers. Light, trail, duration and prewarm come
/// from the parts (the first that has one), the mark from `mark` or the first part's.
fn weave(
    all: &[EffectDesc],
    name: &str,
    description: &str,
    color: [f32; 3],
    parts: &[Part],
    extra: Vec<EmitterDesc>,
    mark: Option<MarkDesc>,
) -> EffectDesc {
    let mut out = EffectDesc {
        name: name.into(),
        description: description.into(),
        mark,
        ..Default::default()
    };
    for (i, p) in parts.iter().enumerate() {
        let Some(base) = all.iter().find(|e| e.name == p.from) else {
            panic!("{name}: no effect {} to weave from", p.from);
        };
        let mut base = base.clone();
        base.recolor(color, p.tint);
        out.emitters.extend(layers_of(
            &base,
            &format!("{}_", i + 1),
            p.rate,
            p.tint > 0.0,
        ));
        if out.light.is_none() {
            out.light = base.light;
        }
        if out.trail.is_none() {
            out.trail = base.trail;
        }
        if out.mark.is_none() {
            out.mark = base.mark;
        }
        out.prewarm = out.prewarm.max(base.prewarm);
        out.duration = match (out.duration, base.duration) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (None, d) if i == 0 => d,
            (d, _) => d,
        };
    }
    out.emitters.extend(extra);
    out
}

pub(super) fn all(existing: &[EffectDesc]) -> Vec<EffectDesc> {
    let mut v = Vec::new();
    v.extend(steam(existing));
    v.extend(plasma(existing));
    v.extend(tempest(existing));
    v.extend(aether(existing));
    v.extend(plague(existing));
    v.extend(nightfeed(existing));
    v.extend(wither(existing));
    v.extend(doom(existing));
    v
}

// ---------------------------------------------------------------- shared signature layers

/// Billowing white steam: lit, thick, rising and thinning into wisps.
fn steam_cloud(name: &str, albedo: f32, peak: f32) -> EmitterDesc {
    EmitterDesc {
        color: g(&[
            (0.0, [albedo, albedo, albedo * 1.02, 0.0]),
            (0.08, [albedo, albedo, albedo * 1.02, peak]),
            (0.6, [albedo, albedo, albedo * 1.02, peak * 0.6]),
            (1.0, [albedo, albedo, albedo * 1.02, 0.0]),
        ]),
        emissive: 120.0,
        erode: 0.6,
        wind: 0.5,
        ..smoke(name, albedo, peak)
    }
}

/// Flakes or motes whirled round a column: the wind's shape carrying another element.
fn whirl(
    name: &str,
    rgb: [f32; 3],
    rate: f32,
    radius: f32,
    height: f32,
    emissive: f32,
) -> EmitterDesc {
    EmitterDesc {
        rate,
        shape: EmitShape::Ring {
            radius,
            thickness: radius * 0.4,
        },
        offset: [0.0, 0.1, 0.0],
        speed: [0.0, 0.2],
        orbit_speed: 3.2,
        vortex: 6.0,
        acceleration: [0.0, height, 0.0],
        drag: 0.6,
        local_space: true,
        lifetime: [1.2, 1.8],
        size: [0.03, 0.06],
        sprite: Sprite::Star,
        facing: Facing::Velocity,
        stretch: 0.05,
        color: fade(rgb, 1.0, 0.15, 0.6),
        emissive,
        additive: 1.0,
        soft: 0.0,
        wind: 0.2,
        ..em(name)
    }
}

/// A swarm of flies over rot: tiny dark specks darting about.
pub(super) fn flies(rate: f32, radius: f32) -> EmitterDesc {
    EmitterDesc {
        rate,
        shape: EmitShape::Disc { radius },
        offset: [0.0, 0.7, 0.0],
        speed: [0.3, 0.8],
        turbulence: 9.0,
        turbulence_scale: 0.35,
        vortex: 2.0,
        drag: 2.5,
        local_space: true,
        lifetime: [2.0, 3.0],
        size: [0.018, 0.026],
        sprite: Sprite::Dust,
        lit: 1.0,
        color: fade([0.03, 0.03, 0.025], 1.0, 0.1, 0.8),
        emissive: 0.0,
        soft: 0.0,
        wind: 0.0,
        ..em("flies")
    }
}

/// Grey ash flakes drifting down, tumbling.
fn ash(rate: f32, radius: f32) -> EmitterDesc {
    EmitterDesc {
        rate,
        shape: EmitShape::Disc { radius },
        offset: [0.0, 2.4, 0.0],
        speed: [0.0, 0.1],
        velocity: [0.0, -0.35, 0.0],
        turbulence: 1.2,
        drag: 1.0,
        lifetime: [3.0, 4.5],
        size: [0.03, 0.06],
        spin_deg: [-180.0, 180.0],
        sprite: Sprite::Dust,
        lit: 1.0,
        color: fade([0.2, 0.2, 0.18], 0.9, 0.1, 0.7),
        emissive: 0.0,
        soft: 0.0,
        wind: 0.6,
        ..em("ash")
    }
}

/// Sparkles of every color twinkling round a heart (aether).
fn prism(name: &str, rate: f32, radius: f32, surface: bool) -> EmitterDesc {
    EmitterDesc {
        rate,
        shape: EmitShape::Sphere { radius, surface },
        speed: [0.0, 0.2],
        orbit_speed: 1.5,
        local_space: true,
        lifetime: [0.5, 0.9],
        size: [0.04, 0.08],
        sprite: Sprite::Star,
        color: g(&[
            (0.0, [1.0, 0.6, 0.4, 0.0]),
            (0.2, [1.0, 0.85, 0.5, 1.0]),
            (0.45, [0.5, 1.0, 0.8, 1.0]),
            (0.7, [0.5, 0.65, 1.0, 1.0]),
            (1.0, [0.9, 0.5, 1.0, 0.0]),
        ]),
        emissive: 20_000.0,
        additive: 1.0,
        soft: 0.0,
        wind: 0.0,
        ..em(name)
    }
}

/// A heart glowing in the weave's color.
fn heart(name: &str, rgb: [f32; 3], size: f32, emissive: f32) -> EmitterDesc {
    EmitterDesc {
        rate: 4.0,
        steady: true,
        speed: [0.0, 0.0],
        local_space: true,
        lifetime: [0.5, 0.5],
        size: [size, size],
        rotation_deg: [0.0, 360.0],
        sprite: Sprite::Plasma,
        color: fade(rgb, 1.0, 0.25, 0.7),
        emissive,
        additive: 1.0,
        soft: 0.0,
        wind: 0.0,
        ..em(name)
    }
}

// ---------------------------------------------------------------- steam

const STEAM: [f32; 3] = [0.92, 0.95, 1.0];

fn steam(all: &[EffectDesc]) -> Vec<EffectDesc> {
    let bolt = weave(
        all,
        "spell.steam.bolt",
        "A scalding steam bolt: a white-hot ball of boiling water wrapped in billowing steam \
         that streams off behind it, droplets spitting from it.",
        STEAM,
        &[
            part("spell.fire.bolt", 0.45, 0.75),
            part("spell.frost.bolt", 0.4, 0.5),
        ],
        vec![EmitterDesc {
            rate: 26.0,
            rate_per_meter: 12.0,
            shape: EmitShape::Sphere {
                radius: 0.18,
                surface: false,
            },
            speed: [0.1, 0.4],
            acceleration: [0.0, 0.9, 0.0],
            lifetime: [1.0, 1.8],
            size: [0.35, 0.6],
            ..steam_cloud("steam", 0.9, 0.55)
        }],
        None,
    );
    let impact = weave(
        all,
        "spell.steam.impact",
        "A steam bolt bursting: a white flash and a great billow of scalding steam boiling up \
         and rolling out, hot droplets sprayed round; it leaves the ground wet.",
        STEAM,
        &[part("spell.frost.impact", 0.35, 0.5)],
        vec![
            EmitterDesc {
                bursts: once(1),
                speed: [0.0, 0.0],
                lifetime: [0.18, 0.18],
                size: [2.4, 2.4],
                size_over_life: c(&[(0.0, 0.5), (1.0, 1.4)]),
                color: g(&[(0.0, [1.0, 0.97, 0.9, 1.0]), (1.0, [1.0, 0.85, 0.7, 0.0])]),
                emissive: 30_000.0,
                additive: 1.0,
                soft: 0.0,
                ..em("flash")
            },
            EmitterDesc {
                bursts: vec![at(0.0, 18), at(0.15, 10)],
                shape: EmitShape::Hemisphere { radius: 0.5 },
                speed: [1.2, 3.5],
                drag: 2.2,
                acceleration: [0.0, 1.3, 0.0],
                lifetime: [2.5, 4.0],
                size: [1.0, 1.6],
                size_over_life: c(&[(0.0, 0.5), (0.3, 1.4), (1.0, 2.8)]),
                ..steam_cloud("billow", 0.92, 0.7)
            },
            droplets(
                "spray",
                70,
                [0.75, 0.85, 0.95],
                [2.0, 6.0],
                1.0,
                Collide::Die,
            ),
        ],
        mark(MarkKind::Wet, 1.4, 45.0, 0.0),
    );
    let field = weave(
        all,
        "spell.steam.field",
        "A steam vent: wet ground hissing with columns of steam boiling up and leaning in the \
         wind, a warm glow in the vents and a low fog spreading out.",
        STEAM,
        &[part("spell.frost.field", 0.3, 0.6)],
        vec![
            // The heat under it: a dull orange glow in the vents.
            super::elements::ground_glow([1.0, 0.5, 0.2], 3.6, 0.2, 1500.0),
            EmitterDesc {
                rate: 24.0,
                shape: EmitShape::Disc { radius: 1.5 },
                speed: [0.2, 0.6],
                acceleration: [0.0, 1.5, 0.0],
                lifetime: [2.5, 3.5],
                size: [0.7, 1.1],
                size_over_life: c(&[(0.0, 0.5), (1.0, 2.6)]),
                ..steam_cloud("columns", 0.9, 0.5)
            },
            EmitterDesc {
                rate: 10.0,
                shape: EmitShape::Disc { radius: 2.0 },
                offset: [0.0, 0.2, 0.0],
                speed: [0.3, 0.7],
                drag: 1.2,
                lifetime: [3.0, 4.0],
                size: [1.2, 1.6],
                size_over_life: c(&[(0.0, 0.7), (1.0, 1.8)]),
                ..steam_cloud("fog", 0.88, 0.3)
            },
        ],
        mark(MarkKind::Wet, 2.3, 120.0, 0.0),
    );
    vec![bolt, impact, field]
}

// ---------------------------------------------------------------- plasma

const PLASMA: [f32; 3] = [1.0, 0.55, 1.0];

fn plasma(all: &[EffectDesc]) -> Vec<EffectDesc> {
    let bolt = weave(
        all,
        "spell.plasma.bolt",
        "A plasma bolt: a violet-white star of burning lightning, flames and arcs lashing off \
         it together, a jagged trail of magenta behind.",
        PLASMA,
        &[
            part("spell.storm.bolt", 0.8, 0.55),
            part("spell.fire.bolt", 0.45, 0.45),
        ],
        vec![heart("star", [1.0, 0.85, 1.0], 0.5, 60_000.0)],
        None,
    );
    let impact = weave(
        all,
        "spell.plasma.impact",
        "A plasma bolt bursting: a blinding violet flash, a fireball shot through with arcs, \
         magenta sparks and a fern of lightning burned into the ground.",
        PLASMA,
        &[
            part("spell.storm.impact", 0.8, 0.55),
            part("spell.fire.impact", 0.5, 0.45),
        ],
        vec![EmitterDesc {
            bursts: once(3),
            speed: [0.0, 0.0],
            lifetime: [0.35, 0.5],
            size: [1.4, 1.8],
            size_over_life: c(&[(0.0, 0.4), (0.3, 1.2), (1.0, 1.6)]),
            rotation_deg: [0.0, 360.0],
            sprite: Sprite::Plasma,
            color: g(&[(0.0, [1.0, 0.9, 1.0, 1.0]), (1.0, [0.8, 0.3, 1.0, 0.0])]),
            emissive: 40_000.0,
            additive: 1.0,
            soft: 0.0,
            ..em("plasma burst")
        }],
        None,
    );
    let field = weave(
        all,
        "spell.plasma.field",
        "A plasma pool: violet fire burning over the ground with arcs crawling through the \
         flames and leaping out of them.",
        PLASMA,
        &[
            part("spell.fire.field", 0.6, 0.55),
            part("spell.storm.field", 0.6, 0.45),
        ],
        Vec::new(),
        None,
    );
    vec![bolt, impact, field]
}

// ---------------------------------------------------------------- tempest

const TEMPEST: [f32; 3] = [0.55, 0.8, 1.0];

fn tempest(all: &[EffectDesc]) -> Vec<EffectDesc> {
    let bolt = weave(
        all,
        "spell.tempest.bolt",
        "A tempest bolt: a ball lightning in a whirl of snow and ice shards, sleet streaming \
         off it.",
        TEMPEST,
        &[
            part("spell.storm.bolt", 0.7, 0.2),
            part("spell.frost.bolt", 0.6, 0.2),
        ],
        vec![EmitterDesc {
            shape: EmitShape::Sphere {
                radius: 0.5,
                surface: true,
            },
            acceleration: [0.0, 0.0, 0.0],
            lifetime: [0.4, 0.7],
            ..whirl("sleet", [0.85, 0.93, 1.0], 90.0, 0.4, 0.0, 9000.0)
        }],
        None,
    );
    let impact = weave(
        all,
        "spell.tempest.impact",
        "A tempest bursting: lightning and a blast of ice shards, then a squall of snow \
         whirling up and out; it leaves frost.",
        TEMPEST,
        &[
            part("spell.storm.impact", 0.7, 0.2),
            part("spell.frost.impact", 0.7, 0.2),
        ],
        vec![EmitterDesc {
            rate: 0.0,
            bursts: once(160),
            lifetime: [0.8, 1.4],
            ..whirl("squall", [0.85, 0.93, 1.0], 0.0, 0.8, 3.0, 9000.0)
        }],
        mark(MarkKind::Frost, 1.3, 60.0, 0.0),
    );
    let field = weave(
        all,
        "spell.tempest.field",
        "A tempest: a whirlwind of snow and sleet spinning over frozen ground, lightning \
         crawling through it and leaping out of the top of the funnel.",
        TEMPEST,
        &[
            part("spell.storm.field", 0.6, 0.2),
            part("spell.frost.field", 0.45, 0.2),
        ],
        vec![
            whirl("funnel", [0.85, 0.93, 1.0], 260.0, 1.3, 2.6, 9000.0),
            EmitterDesc {
                lifetime: [1.5, 2.2],
                size: [0.5, 0.8],
                sprite: Sprite::Dust,
                facing: Facing::Camera,
                stretch: 0.0,
                color: fade([0.85, 0.9, 1.0], 0.22, 0.2, 0.6),
                emissive: 60.0,
                additive: 0.0,
                lit: 1.0,
                soft: 0.5,
                ..whirl("spindrift", [0.85, 0.9, 1.0], 40.0, 1.0, 1.6, 60.0)
            },
        ],
        mark(MarkKind::Frost, 2.3, 120.0, 0.0),
    );
    vec![bolt, impact, field]
}

// ---------------------------------------------------------------- aether

const AETHER: [f32; 3] = [0.85, 0.75, 1.0];

fn aether(all: &[EffectDesc]) -> Vec<EffectDesc> {
    let bolt = weave(
        all,
        "spell.aether.bolt",
        "An aether bolt: fire, frost and lightning woven into one pale lilac star, sparkles of \
         every color twinkling round it.",
        AETHER,
        &[
            part("spell.storm.bolt", 0.6, 0.6),
            part("spell.fire.bolt", 0.3, 0.6),
            part("spell.frost.bolt", 0.3, 0.4),
        ],
        vec![
            heart("star", [0.95, 0.9, 1.0], 0.55, 50_000.0),
            prism("prism", 60.0, 0.45, true),
        ],
        None,
    );
    let impact = weave(
        all,
        "spell.aether.impact",
        "Aether bursting: a lilac flash, flame, ice and arcs flung out together and a shower \
         of sparkles of every color.",
        AETHER,
        &[
            part("spell.storm.impact", 0.6, 0.6),
            part("spell.fire.impact", 0.35, 0.6),
            part("spell.frost.impact", 0.35, 0.4),
        ],
        vec![EmitterDesc {
            rate: 0.0,
            bursts: once(120),
            shape: EmitShape::Hemisphere { radius: 0.3 },
            speed: [2.0, 6.0],
            gravity: 0.3,
            drag: 1.2,
            local_space: false,
            lifetime: [0.8, 1.4],
            ..prism("prism", 0.0, 0.3, false)
        }],
        mark(MarkKind::Burn, 1.3, 40.0, 3.0),
    );
    let field = weave(
        all,
        "spell.aether.field",
        "An aether well: a lilac rune ring with flames, ice and arcs standing together inside \
         it and sparkles of every color rising.",
        AETHER,
        &[
            part("spell.storm.field", 0.45, 0.6),
            part("spell.fire.field", 0.2, 0.6),
            part("spell.frost.field", 0.3, 0.4),
        ],
        vec![EmitterDesc {
            shape: EmitShape::Disc { radius: 1.8 },
            velocity: [0.0, 0.6, 0.0],
            lifetime: [1.5, 2.5],
            ..prism("prism", 70.0, 1.8, false)
        }],
        None,
    );
    vec![bolt, impact, field]
}

// ---------------------------------------------------------------- plague

const PLAGUE: [f32; 3] = [0.45, 0.42, 0.08];

fn plague(all: &[EffectDesc]) -> Vec<EffectDesc> {
    let bolt = weave(
        all,
        "spell.plague.bolt",
        "A plague bolt: a festering glob of brown-green ooze and blood, a cloud of flies and \
         fumes trailing it.",
        PLAGUE,
        &[
            part("spell.blight.bolt", 0.8, 0.35),
            part("spell.blood.bolt", 0.35, 0.0),
        ],
        Vec::new(),
        None,
    );
    let impact = weave(
        all,
        "spell.plague.impact",
        "A plague bolt bursting: ooze and blood splashed out together and a sickly brown cloud \
         rolling low; it leaves rot.",
        PLAGUE,
        &[
            part("spell.blight.impact", 0.8, 0.35),
            part("spell.blood.impact", 0.45, 0.0),
        ],
        Vec::new(),
        mark(MarkKind::Rot, 1.3, 60.0, 4.0),
    );
    let field = weave(
        all,
        "spell.plague.field",
        "A plague pit: rotting ground boiling with ooze, a brown miasma hanging over it, blood \
         seeping up and flies swarming.",
        PLAGUE,
        &[
            part("spell.blight.field", 0.75, 0.35),
            part("spell.blood.field", 0.25, 0.0),
        ],
        vec![flies(30.0, 1.6)],
        mark(MarkKind::Rot, 2.3, 120.0, 0.0),
    );
    vec![bolt, impact, field]
}

// ---------------------------------------------------------------- nightfeed

const NIGHTFEED: [f32; 3] = [0.7, 0.04, 0.3];

fn nightfeed(all: &[EffectDesc]) -> Vec<EffectDesc> {
    let bolt = weave(
        all,
        "spell.nightfeed.bolt",
        "A nightfeed bolt: a ball of darkness with a crimson corona, blood whirling round it \
         and being drunk into it.",
        NIGHTFEED,
        &[
            part("spell.shadow.bolt", 0.9, 0.55),
            part("spell.blood.bolt", 0.45, 0.0),
        ],
        Vec::new(),
        None,
    );
    let impact = weave(
        all,
        "spell.nightfeed.impact",
        "Nightfeed bursting: light and blood sucked into a point, then a burst of darkness \
         rimmed in crimson and a spray of blood.",
        NIGHTFEED,
        &[
            part("spell.shadow.impact", 0.9, 0.55),
            part("spell.blood.impact", 0.5, 0.0),
        ],
        Vec::new(),
        mark(MarkKind::Blood, 1.0, 60.0, 0.0),
    );
    let field = weave(
        all,
        "spell.nightfeed.field",
        "A feeding pit: a pool of darkness rimmed in crimson, streams of blood torn up out of \
         the ground and drawn into it.",
        NIGHTFEED,
        &[
            part("spell.shadow.field", 0.9, 0.55),
            part("spell.blood.field", 0.5, 0.0),
        ],
        Vec::new(),
        mark(MarkKind::Void, 2.2, 120.0, 0.0),
    );
    vec![bolt, impact, field]
}

// ---------------------------------------------------------------- wither

const WITHER: [f32; 3] = [0.42, 0.48, 0.2];

fn wither(all: &[EffectDesc]) -> Vec<EffectDesc> {
    let bolt = weave(
        all,
        "spell.wither.bolt",
        "A wither bolt: a ball of grey-green darkness shedding ash and fumes that eat away as \
         they drift.",
        WITHER,
        &[
            part("spell.shadow.bolt", 0.8, 0.6),
            part("spell.blight.bolt", 0.45, 0.4),
        ],
        Vec::new(),
        None,
    );
    let impact = weave(
        all,
        "spell.wither.impact",
        "Wither bursting: a burst of ashen darkness and fumes, flakes of ash falling; it leaves \
         rot.",
        WITHER,
        &[
            part("spell.shadow.impact", 0.8, 0.6),
            part("spell.blight.impact", 0.45, 0.4),
        ],
        vec![EmitterDesc {
            rate: 0.0,
            bursts: once(60),
            duration: Some(0.1),
            ..ash(0.0, 1.0)
        }],
        mark(MarkKind::Rot, 1.3, 60.0, 0.0),
    );
    let field = weave(
        all,
        "spell.wither.field",
        "Withering ground: dark flames of grey-green rising from rotted ground, fumes eaten \
         away at their edges and ash drifting down over it.",
        WITHER,
        &[
            part("spell.shadow.field", 0.75, 0.6),
            part("spell.blight.field", 0.4, 0.4),
        ],
        vec![ash(40.0, 2.0)],
        mark(MarkKind::Rot, 2.3, 120.0, 0.0),
    );
    vec![bolt, impact, field]
}

// ---------------------------------------------------------------- doom

const DOOM: [f32; 3] = [0.8, 0.05, 0.12];

fn doom(all: &[EffectDesc]) -> Vec<EffectDesc> {
    let bolt = weave(
        all,
        "spell.doom.bolt",
        "A doom bolt: a great ball of darkness with a blood-red corona, blood and rot whirling \
         into it and black wisps tearing off.",
        DOOM,
        &[
            part("spell.shadow.bolt", 1.0, 0.55),
            part("spell.blood.bolt", 0.35, 0.0),
            part("spell.blight.bolt", 0.25, 0.5),
        ],
        Vec::new(),
        None,
    );
    let impact = weave(
        all,
        "spell.doom.impact",
        "Doom bursting: everything drawn into a point, then darkness, blood and rot flung out \
         together; it burns a black heart with red cracks into the ground.",
        DOOM,
        &[
            part("spell.shadow.impact", 1.0, 0.55),
            part("spell.blood.impact", 0.45, 0.0),
            part("spell.blight.impact", 0.35, 0.5),
        ],
        Vec::new(),
        mark(MarkKind::Void, 1.6, 60.0, 4.0),
    );
    let field = weave(
        all,
        "spell.doom.field",
        "A doom pit: a pool of darkness with a blood-red rune ring, black flames rising, blood \
         drawn up into it and a miasma creeping out from under it.",
        DOOM,
        &[
            part("spell.shadow.field", 1.0, 0.55),
            part("spell.blood.field", 0.35, 0.0),
            part("spell.blight.field", 0.3, 0.5),
        ],
        Vec::new(),
        mark(MarkKind::Void, 2.4, 120.0, 0.0),
    );
    vec![bolt, impact, field]
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_authored_weave_has_a_bolt_an_impact_and_a_field() {
        let all = super::super::all();
        for weave in [
            "steam",
            "plasma",
            "tempest",
            "aether",
            "plague",
            "nightfeed",
            "wither",
            "doom",
        ] {
            for role in ["bolt", "impact", "field"] {
                let name = format!("spell.{weave}.{role}");
                let e = all
                    .iter()
                    .find(|e| e.name == name)
                    .unwrap_or_else(|| panic!("{name} is missing"));
                e.validate().unwrap_or_else(|err| panic!("{err}"));
                assert_eq!(e.is_one_shot(), role == "impact", "{name}");
            }
        }
    }
}
