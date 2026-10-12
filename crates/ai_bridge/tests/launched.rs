//! A game started by a launcher (`ENGINE_BRIDGE_EXIT_ON_STDIN_EOF`, as `engine-mcp --launch`
//! does) must exit when its bridge cannot listen, instead of running where the launcher can
//! never reach it. The environment is process-wide, so the game runs in a child process: this
//! test binary re-runs itself with the variables set.

use std::io::Read;
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use engine_ai_bridge::{AiBridgePlugin, BridgeInfo};
use engine_core::prelude::*;

const CHILD: &str = "ENGINE_AI_BRIDGE_TEST_CHILD";

#[test]
fn launched_game_exits_when_the_bridge_cannot_listen() {
    if std::env::var_os(CHILD).is_some() {
        // The child: a game whose bridge port is taken.
        let mut app = App::new();
        app.add_plugins(AiBridgePlugin::default());
        let info = app.world.resource::<BridgeInfo>().clone();
        assert!(!info.listening, "the port was supposed to be taken");
        assert!(info.error.is_some());
        app.update();
        assert_eq!(app.should_exit(), Some(AppExit::Error(1)));
        return;
    }

    // Hold a port so the child cannot listen on it.
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap().to_string();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "launched_game_exits_when_the_bridge_cannot_listen",
            "--exact",
            "--test-threads=1",
        ])
        .env(CHILD, "1")
        .env("ENGINE_BRIDGE", &address)
        .env("ENGINE_BRIDGE_EXIT_ON_STDIN_EOF", "1")
        .env_remove("ENGINE_BRIDGE_TOKEN")
        // Kept open until the child is done, like a launcher's pipe.
        .stdin(Stdio::piped())
        // Small enough to fit the pipe buffers; read once the child has exited.
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the child test did not finish");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(taken);
    let mut output = String::new();
    for pipe in [
        child.stdout.take().map(|p| Box::new(p) as Box<dyn Read>),
        child.stderr.take().map(|p| Box::new(p) as Box<dyn Read>),
    ]
    .into_iter()
    .flatten()
    {
        let mut pipe = pipe;
        let _ = pipe.read_to_string(&mut output);
    }
    assert!(
        status.success(),
        "the child test failed ({status}):\n{output}"
    );
}
