//! `DefaultPlugins` already contains `AiBridgePlugin` (off unless `ENGINE_BRIDGE` is set), so a
//! game that wants the bridge always on configures the group's copy with `.set(...)`. Adding a
//! second `AiBridgePlugin` after the group is ignored.

use engine::ai_bridge::{AiBridgePlugin, BridgeInfo};
use engine::prelude::*;

#[test]
fn default_plugins_set_turns_the_bridge_on() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(AiBridgePlugin::listen("127.0.0.1:0").without_env()));
    let info = app.world.resource::<BridgeInfo>();
    assert!(info.listening, "the bridge is not listening: {info:?}");
    let address = info.address.as_deref().unwrap_or_default();
    assert!(
        address.starts_with("127.0.0.1:") && !address.ends_with(":0"),
        "the bridge should report the free port it bound, got {address:?}"
    );
}

#[test]
fn default_plugins_set_can_require_a_token() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins.set(
            AiBridgePlugin::listen("127.0.0.1:0")
                .with_token("secret")
                .without_env(),
        ),
    );
    let info = app.world.resource::<BridgeInfo>();
    assert!(info.listening && info.token_required, "{info:?}");
}

#[test]
fn a_second_bridge_plugin_after_default_plugins_is_ignored() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(AiBridgePlugin::default().without_env()))
        .add_plugins(AiBridgePlugin::listen("127.0.0.1:0").without_env());
    let info = app.world.resource::<BridgeInfo>();
    assert!(
        !info.listening && info.address.is_none(),
        "the group's copy (off) should win: {info:?}"
    );
}
