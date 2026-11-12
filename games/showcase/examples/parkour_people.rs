//! The movement module's parkour films ([`engine_movement`]'s `parkour_film` example) with the
//! character module drawing the pilot as a person: the same clips, scripts and cameras.
//!
//! ```text
//! cargo run -p showcase --release --example parkour_people -- [out_dir] [clip ...]
//! ```
//!
//! Run it from the repository's root, so the character module finds its fetched human model
//! (`assets/fetched/character/`); the environment variables are the film's (`FILM_SIZE`,
//! `FILM_STILLS`, `FILM_BOOM`, `FILM_YAW`, `FILM_DRY`).

#[allow(dead_code)]
#[path = "../../../crates/movement/examples/parkour_film.rs"]
mod film;

fn main() {
    film::run_with(
        |app| {
            app.add_plugins(engine_character::CharacterPlugin);
        },
        |world| {
            world
                .get_resource::<engine_character::CharacterPipelinesReady>()
                .is_some()
        },
    );
}
