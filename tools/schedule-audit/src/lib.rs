//! The frame's schedules with every module in them, as the showcase adds them, built but never
//! run: which systems run alone (exclusive, `&mut World`) and which systems write each
//! component. Each exclusive system holds every other system off the cores while it runs, and
//! two writers of one component never run side by side, so both lists are where a frame turns
//! serial (engine-design/next-session-plan.md, perf week P2).
//!
//! `cargo run -p schedule-audit` prints the report. The tests hold the lists: a new exclusive
//! system, or a new writer of `Transform` in the fixed step or of `GlobalTransform` after the
//! update, fails until it is either made parallel or added to the list on purpose.

use engine::prelude::*;
pub use engine_core::profile::{ScheduleAccess, schedule_access};

/// An app with the showcase's plugins (no window, nothing run).
pub fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<engine_core::config::EngineConfig>()
        .headless = true;
    app.add_plugins(engine::DefaultPlugins)
        .add_plugins((
            engine_physics::PhysicsPlugin,
            engine_vehicles::VehiclesPlugin,
            engine_space::SpacePlugin,
            engine_terrain::TerrainPlugin,
            engine_water::WaterPlugin,
            engine_weather::WeatherPlugin,
            engine_fire::FirePlugin,
            engine_smoke::SmokePlugin,
        ))
        .add_plugins((
            engine_vfx::VfxPlugin,
            engine_foliage::FoliagePlugin,
            engine_anim::AnimationPlugin,
            engine_npc::NpcPlugin,
            engine_ui::UiPlugin,
            engine_destruction::DestructionPlugin,
            engine_movement::MovementPlugin,
            engine_audio::AudioPlugin,
        ))
        .add_plugins(engine_softbody::SoftBodyPlugin)
        .add_plugins(engine_character::CharacterPlugin)
        .add_plugins(engine_script::ScriptPlugin);
    app
}

/// The report for [`app`].
pub fn report() -> Vec<ScheduleAccess> {
    schedule_access(&mut app().world)
}

/// The report as Markdown.
pub fn markdown(report: &[ScheduleAccess]) -> String {
    let mut out = String::new();
    for s in report {
        out += &format!("## {}\n\n", s.schedule);
        out += &format!("Exclusive ({}):\n", s.exclusive.len());
        for e in &s.exclusive {
            out += &format!("- {e}\n");
        }
        let mut shared: Vec<_> = s.writers.iter().filter(|(_, w)| w.len() > 1).collect();
        shared.sort_by_key(|(_, w)| std::cmp::Reverse(w.len()));
        out += &format!(
            "\nComponents with more than one writer ({}):\n",
            shared.len()
        );
        for (c, w) in shared {
            out += &format!("- {c} ({}): {}\n", w.len(), w.join(", "));
        }
        out += "\n";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The systems that run alone today, per schedule (`apply_deferred` points counted apart).
    /// The perf week shrinks these lists; a system joins one only on purpose: an exclusive
    /// system that has work most frames holds every core but one idle while it runs (AGENTS.md
    /// "Let systems run side by side": a `Query`/`Res` system, or a `run_if` so it runs only
    /// when it has work).
    const EXCLUSIVE: &[(&str, usize, &[&str])] = &[
        ("Extract", 0, &[]),
        (
            "First",
            0,
            &[
                "bevy_ecs::message_update_system",
                "engine_ai_bridge::bridge_frame_start_system",
                // Only while off-frame jobs are queued (`off_frame_due`).
                "engine_core::apply_off_frame_system",
            ],
        ),
        ("FixedPostUpdate", 0, &[]),
        ("FixedPreUpdate", 0, &[]),
        (
            "FixedUpdate",
            19,
            &[
                "engine_foliage::apply_damage",
                "engine_foliage::ecology_step",
                "engine_foliage::run_demo_felling",
                "engine_physics::physics_sync",
                "engine_physics::physics_write_back",
            ],
        ),
        (
            "Last",
            0,
            &[
                "engine::stream_system",
                "engine_ai_bridge::bridge_frame_end_system",
                "engine_core::parallelism_report_system",
            ],
        ),
        (
            "PostUpdate",
            6,
            &[
                "engine_script::shift_scripts",
                "engine_space::fly",
                "engine_space::reanchor",
                "engine_space::update_space",
                "engine_ui::bars_system",
                "engine_ui::cooldowns_system",
                "engine_ui::key_hints_system",
                "engine_ui::sliders_system",
                "engine_ui::toasts_system",
                "engine_ui::toggles_system",
                "engine_ui::ui_layout_system",
            ],
        ),
        (
            "PreUpdate",
            1,
            &[
                "engine_assets::apply_loaded_assets",
                "engine_destruction::ensure_assets",
                "engine_foliage::init_regions",
                "engine_ui::ui_input_system",
            ],
        ),
        ("Render", 0, &["engine_render::render_system"]),
        ("Startup", 0, &["engine_character::load_pack"]),
        (
            "Update",
            16,
            &[
                "engine_fire::run_demos",
                "engine_foliage::build_scenery",
                "engine_foliage::chop_on_interact",
                "engine_foliage::offer_chops",
                "engine_movement::toggle",
                "engine_script::draw_state_boards",
                "engine_script::preview_scripts",
                "engine_script::run_cutscenes",
                "engine_script::run_scripts",
                "engine_script::run_spots",
                "engine_script::run_story",
                "engine_smoke::land_range_art",
                "engine_smoke::run_demos",
                "engine_ui::crafting_system",
                "engine_ui::dialogue_system",
                "engine_ui::hud_system",
                "engine_ui::inventory_screen_system",
                "engine_ui::loading_system",
                "engine_ui::map_system",
                "engine_ui::menus_system",
                "engine_ui::quests_system",
                "engine_ui::saves_system",
                "engine_ui::shop_screen_system",
                "engine_ui::skills_system",
                "engine_ui::title_system",
                "engine_ui::wheel_system",
                "engine_vehicles::board",
            ],
        ),
    ];

    /// Writers of `GlobalTransform` after the update besides transform propagation. Each one
    /// waits for propagation and holds the animation module's pose work behind it
    /// (engine-design/anim-parallel.md). Small copy systems (place_lights) are fine.
    /// (Movement's step_effects left in 47390901.)
    const GLOBAL_TRANSFORM_WRITERS: &[&str] = &[
        "engine_anim::attachment_system",
        "engine_anim::bow_string_system",
        "engine_anim::knockdown_system",
        "engine_core::propagate_transforms_system",
        "engine_vfx::place_lights",
    ];

    /// Most systems writing `Transform` in the fixed step. Two writers of one component only
    /// run side by side when their queries can't meet (`With`/`Without`), which this count can't
    /// see, so it is a ceiling to bring down, not a list.
    const FIXED_TRANSFORM_WRITERS_MAX: usize = 44;

    fn fix(what: &str, extra: &BTreeSet<&str>, gone: &BTreeSet<&str>) -> String {
        format!(
            "{what} changed.\n  new: {extra:?}\n  gone: {gone:?}\nA new one: make it run beside \
             the rest (a Query/Res system, or a plain work check piped into engine_core::schedule::queue_when; a run_if doesn't help), or add it here \
             on purpose. One gone: take it off the list here. `cargo run -p schedule-audit` \
             prints the whole report."
        )
    }

    fn compare(what: &str, now: &[String], frozen: &[&str]) -> Option<String> {
        let now: BTreeSet<&str> = now.iter().map(String::as_str).collect();
        let frozen: BTreeSet<&str> = frozen.iter().copied().collect();
        let extra: BTreeSet<_> = now.difference(&frozen).copied().collect();
        let gone: BTreeSet<_> = frozen.difference(&now).copied().collect();
        (!extra.is_empty() || !gone.is_empty()).then(|| fix(what, &extra, &gone))
    }

    #[test]
    fn the_frame_turns_serial_only_where_it_did() {
        let report = report();
        let mut problems = Vec::new();
        for (schedule, deferred, frozen) in EXCLUSIVE {
            let Some(s) = report.iter().find(|s| s.schedule == *schedule) else {
                problems.push(format!("schedule {schedule} is gone: update EXCLUSIVE"));
                continue;
            };
            let (syncs, systems): (Vec<_>, Vec<_>) = s
                .exclusive
                .iter()
                .cloned()
                .partition(|e| e == "bevy_ecs::apply_deferred");
            if let Some(p) = compare(&format!("{schedule}'s exclusive systems"), &systems, frozen) {
                problems.push(p);
            }
            if syncs.len() > *deferred {
                problems.push(format!(
                    "{schedule} has {} command sync points (apply_deferred), more than {deferred}: \
                     a new .chain() or .after() between a system with Commands and one that reads \
                     what they spawn adds one.",
                    syncs.len()
                ));
            } else if syncs.len() < *deferred {
                problems.push(format!(
                    "{schedule} has {} command sync points, fewer than {deferred}: lower the \
                     number here.",
                    syncs.len()
                ));
            }
        }
        let names: BTreeSet<_> = report.iter().map(|s| s.schedule.as_str()).collect();
        for s in names {
            if !EXCLUSIVE.iter().any(|(n, ..)| *n == s) {
                problems.push(format!("new schedule {s}: add it to EXCLUSIVE"));
            }
        }
        let writers = |schedule: &str, component: &str| -> Vec<String> {
            report
                .iter()
                .find(|s| s.schedule == schedule)
                .and_then(|s| s.writers.get(component).cloned())
                .unwrap_or_default()
        };
        if let Some(p) = compare(
            "PostUpdate's GlobalTransform writers",
            &writers("PostUpdate", "GlobalTransform"),
            GLOBAL_TRANSFORM_WRITERS,
        ) {
            problems.push(p);
        }
        let fixed = writers("FixedUpdate", "Transform");
        if fixed.len() > FIXED_TRANSFORM_WRITERS_MAX {
            problems.push(format!(
                "{} systems write Transform in FixedUpdate, more than {FIXED_TRANSFORM_WRITERS_MAX}: \
                 {fixed:?}",
                fixed.len()
            ));
        } else if fixed.len() < FIXED_TRANSFORM_WRITERS_MAX {
            problems.push(format!(
                "{} systems write Transform in FixedUpdate: lower FIXED_TRANSFORM_WRITERS_MAX \
                 to it.",
                fixed.len()
            ));
        }
        assert!(problems.is_empty(), "\n{}", problems.join("\n\n"));
    }
}
