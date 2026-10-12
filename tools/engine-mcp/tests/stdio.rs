//! Runs the real `engine-mcp` binary over stdio against a stand-in game bridge, the way an MCP
//! client does: every stdout line must be one JSON-RPC message, a bad line must not end the
//! session, and closing stdin must stop the server.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// A stand-in for a game's AI bridge that answers `bridge.ping`, `bridge.methods` and
/// `entity.spawn`. Returns its address.
fn fake_bridge() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || serve(stream));
        }
    });
    address
}

fn serve(stream: TcpStream) {
    let mut writer = stream.try_clone().unwrap();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        let request: Value = serde_json::from_str(&line).unwrap();
        let result = match request["method"].as_str().unwrap_or("") {
            "bridge.ping" => json!({"pong": true, "frame": 1}),
            "bridge.methods" => json!({"count": 1, "methods": [
                {"name": "entity.spawn", "summary": "Spawn an entity.", "args_example": "{}"}
            ]}),
            "entity.spawn" => json!({"id": 42}),
            other => json!({"unexpected": other}),
        };
        let response = json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
        if writer
            .write_all(format!("{response}\n").as_bytes())
            .is_err()
        {
            return;
        }
    }
}

/// Kills the server if the test fails before it exits on its own.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn stdio_session_against_a_bridge() {
    let address = fake_bridge();
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_engine-mcp"))
            .args(["--connect", &address, "--list-wait", "10"])
            .env_remove("ENGINE_BRIDGE_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start engine-mcp"),
    );

    // Every stdout line, parsed, from a reader thread (so a hang fails the test, not CI).
    let (tx, rx) = mpsc::channel::<String>();
    let stdout = server.0.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if tx.send(line).is_err() {
                return;
            }
        }
    });

    let mut stdin = server.0.stdin.take().unwrap();
    let mut send = |bytes: &[u8]| {
        stdin.write_all(bytes).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    };
    send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}}})
        .to_string()
        .as_bytes(),
    );
    send(b"\xff\xfe not utf-8");
    send(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    send(br#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    send(
        br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"entity_spawn","arguments":{"name":"Crate"}}}"#,
    );
    send(br#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#);

    let mut by_id = std::collections::HashMap::new();
    let mut parse_errors = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    while by_id.len() < 4 || parse_errors < 1 {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = rx
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("missing answers; got {by_id:?}"));
        let message: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line}"));
        assert_eq!(message["jsonrpc"], json!("2.0"), "{message}");
        match message["id"].as_u64() {
            Some(id) => {
                by_id.insert(id, message);
            }
            None => {
                assert_eq!(message["error"]["code"], json!(-32700), "{message}");
                parse_errors += 1;
            }
        }
    }

    assert_eq!(
        by_id[&1]["result"]["serverInfo"]["name"],
        json!("engine-mcp")
    );
    let tools: Vec<&str> = by_id[&2]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(tools.contains(&"entity_spawn"), "{tools:?}");
    assert_eq!(
        by_id[&3]["result"]["isError"],
        json!(false),
        "{}",
        by_id[&3]
    );
    assert!(
        by_id[&3]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("42")
    );
    assert_eq!(by_id[&4]["result"], json!({}));

    // The client goes away: the server exits by itself.
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "engine-mcp kept running after stdin closed"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
}
