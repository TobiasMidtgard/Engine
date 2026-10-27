//! The installed logger keeps the global max level at what its filters want, so `trace!` and
//! `debug!` calls (wgpu logs every render pass command) stop at a level compare.

use tracing::level_filters::LevelFilter;

#[test]
fn trace_records_stop_at_the_level_check() {
    // SAFETY: this test binary runs this one test, before any other thread reads the variable.
    unsafe { std::env::remove_var("RUST_LOG") };
    engine_core::log::init("info,wgpu=warn");
    assert_eq!(LevelFilter::current(), LevelFilter::INFO);
}
