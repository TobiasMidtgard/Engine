//! Drives a real `App` over TCP from client threads while the test's main thread runs frames,
//! exactly like a game with an agent attached.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use engine_ai_bridge::{AiBridgePlugin, BridgeInfo, codes};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use serde_json::{Value, json};

/// A minimal line-based JSON-RPC client, as an agent would write it.
struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

impl Client {
    fn connect(address: &str) -> Self {
        let stream = TcpStream::connect(address).expect("connect to the bridge");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        Self {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
            next_id: 1,
        }
    }

    fn send_line(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).unwrap();
        self.writer.write_all(b"\n").unwrap();
    }

    fn send(&mut self, message: Value) {
        self.send_line(&message.to_string());
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).expect("read a response");
        assert!(n > 0, "the bridge closed the connection");
        let value: Value = serde_json::from_str(&line).expect("response is JSON");
        assert_eq!(value["jsonrpc"], json!("2.0"), "{value}");
        value
    }

    /// Send a request and return the whole response object.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let response = self.read();
        assert_eq!(response["id"], json!(id), "{response}");
        response
    }

    /// Send a request and return its result, failing the test on an error.
    fn call(&mut self, method: &str, params: Value) -> Value {
        let response = self.request(method, params);
        assert!(
            response.get("error").is_none(),
            "{method} failed: {response}"
        );
        response["result"].clone()
    }
}

fn bridge_app(plugin: AiBridgePlugin) -> (App, String) {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((engine_input::InputPlugin, plugin));
    let info = app.world.resource::<BridgeInfo>().clone();
    assert!(info.listening, "bridge did not start: {:?}", info.error);
    (app, info.address.unwrap())
}

/// Run frames on this thread until the client thread is done (like a game loop would).
fn run_until_done<T: Send + 'static>(
    app: &mut App,
    client: impl FnOnce() -> T + Send + 'static,
) -> T {
    let handle = std::thread::spawn(client);
    let deadline = Instant::now() + Duration::from_secs(120);
    while !handle.is_finished() {
        assert!(
            Instant::now() < deadline,
            "the client did not finish in time"
        );
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    handle
        .join()
        .expect("client thread failed (see its panic above)")
}

#[test]
fn tcp_client_spawns_lists_and_steps() {
    let (mut app, address) = bridge_app(AiBridgePlugin::listen("127.0.0.1:0").without_env());
    assert!(address.starts_with("127.0.0.1:") && !address.ends_with(":0"));

    let addr = address.clone();
    let spawned_id = run_until_done(&mut app, move || {
        let mut c = Client::connect(&addr);

        let ping = c.call("bridge.ping", json!({}));
        assert_eq!(ping["pong"], json!(true));

        let spawned = c.call(
            "entity.spawn",
            json!({"name": "Crate", "components": {"Transform": {"translation": [1.0, 2.0, 3.0]}}}),
        );
        let id = spawned["id"].as_u64().expect("spawn returns an id");

        let list = c.call("entity.list", json!({"name_contains": "Crate"}));
        assert_eq!(list["total"], json!(1), "{list}");
        let entity = &list["entities"][0];
        assert_eq!(entity["id"], json!(id));
        assert_eq!(entity["name"], json!("Crate"));
        let components: Vec<&str> = entity["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert!(components.contains(&"Transform"), "{components:?}");

        let got = c.call("entity.get", json!({"name": "Crate"}));
        assert_eq!(
            got["components"]["Transform"]["translation"],
            json!([1.0, 2.0, 3.0])
        );

        // Pause, then step exactly 3 frames: time advances 3 × 1/60 s and stays paused.
        assert_eq!(c.call("engine.pause", json!({}))["paused"], json!(true));
        let before = c.call("engine.status", json!({}));
        assert_eq!(before["paused"], json!(true));
        let step = c.call("engine.step", json!({"frames": 3}));
        assert_eq!(step["frames"], json!(3));
        assert_eq!(
            step["frame"].as_u64().unwrap() - step["start_frame"].as_u64().unwrap(),
            3,
            "{step}"
        );
        assert_eq!(step["paused"], json!(true));
        let after = c.call("engine.status", json!({}));
        assert_eq!(after["paused"], json!(true));
        let advanced = after["elapsed_world_secs"].as_f64().unwrap()
            - before["elapsed_world_secs"].as_f64().unwrap();
        assert!(
            (advanced - 3.0 / 60.0).abs() < 1e-9,
            "world advanced {advanced} s"
        );
        assert!(after["entities"].as_u64().unwrap() >= 1);

        // Input APIs from InputPlugin: press, run a frame, read the action back.
        c.call("input.press", json!({"action": "jump"}));
        c.call("engine.step", json!({"frames": 1}));
        let state = c.call("input.state", json!({}));
        assert_eq!(state["jump"]["pressed"], json!(true), "{state}");

        // Errors are JSON-RPC error objects with readable messages.
        let missing = c.request("entity.spwan", json!({}));
        assert_eq!(missing["error"]["code"], json!(codes::METHOD_NOT_FOUND));
        assert!(
            missing["error"]["message"]
                .as_str()
                .unwrap()
                .contains("entity.spawn"),
            "{missing}"
        );
        let failed = c.request("entity.get", json!({"name": "Nobody"}));
        assert_eq!(failed["error"]["code"], json!(codes::API_ERROR));

        c.send_line("this is not json");
        let parse = c.read();
        assert_eq!(parse["id"], Value::Null);
        assert_eq!(parse["error"]["code"], json!(codes::PARSE_ERROR));

        c.send_line(r#"[{"jsonrpc":"2.0","id":1,"method":"bridge.ping"}]"#);
        assert_eq!(c.read()["error"]["code"], json!(codes::INVALID_REQUEST));

        // A notification (no id) runs but gets no answer; the next answer is for the request.
        c.send(json!({"jsonrpc": "2.0", "method": "engine.resume"}));
        let status = c.call("engine.status", json!({}));
        assert_eq!(status["paused"], json!(false));

        // Pipelining: answers carry their ids and may arrive out of order (the step waits for
        // frames, the ping does not).
        c.send(json!({"jsonrpc": "2.0", "id": "slow", "method": "engine.step", "params": {"frames": 20}}));
        c.send(json!({"jsonrpc": "2.0", "id": "fast", "method": "bridge.ping"}));
        let first = c.read();
        let second = c.read();
        assert_eq!(first["id"], json!("fast"), "{first}");
        assert_eq!(second["id"], json!("slow"), "{second}");

        // A second client is served while the first one waits on a long step.
        c.send(
            json!({"jsonrpc": "2.0", "id": 99, "method": "engine.step", "params": {"frames": 30}}),
        );
        let mut other = Client::connect(&addr);
        let other_ping = other.call("bridge.ping", json!({}));
        let long_step = c.read();
        assert_eq!(long_step["id"], json!(99));
        assert!(
            other_ping["frame"].as_u64().unwrap() < long_step["result"]["frame"].as_u64().unwrap(),
            "the second client waited for the first client's step"
        );
        id
    });

    // The spawn really happened in the world the main thread owns.
    let mut q = app.world.query::<(Entity, &Name)>();
    let (entity, _) = q
        .iter(&app.world)
        .find(|(_, n)| n.as_str() == "Crate")
        .expect("Crate exists");
    assert_eq!(entity.to_bits(), spawned_id);
    let info = app.world.resource::<BridgeInfo>();
    // Every request that reached the main thread (parse errors and batches never do).
    assert!(info.requests >= 15, "requests served: {}", info.requests);
}

#[test]
fn token_is_required_when_configured() {
    let (mut app, address) = bridge_app(
        AiBridgePlugin::listen("127.0.0.1:0")
            .without_env()
            .with_token("s3cret"),
    );
    assert!(app.world.resource::<BridgeInfo>().token_required);

    run_until_done(&mut app, move || {
        let mut c = Client::connect(&address);
        let rejected = c.request("bridge.ping", json!({}));
        assert_eq!(
            rejected["error"]["code"],
            json!(codes::UNAUTHORIZED),
            "{rejected}"
        );
        assert!(
            rejected["error"]["message"]
                .as_str()
                .unwrap()
                .contains("token")
        );

        c.send(json!({"jsonrpc": "2.0", "id": 2, "method": "bridge.ping", "token": "wrong"}));
        assert_eq!(c.read()["error"]["code"], json!(codes::UNAUTHORIZED));

        c.send(json!({"jsonrpc": "2.0", "id": 3, "method": "bridge.ping", "token": "s3cret"}));
        let ok = c.read();
        assert_eq!(ok["id"], json!(3));
        assert_eq!(ok["result"]["pong"], json!(true), "{ok}");

        // The token on one request does not unlock the connection.
        let again = c.request("engine.status", json!({}));
        assert_eq!(again["error"]["code"], json!(codes::UNAUTHORIZED));

        // `auth` unlocks the whole connection.
        let mut d = Client::connect(&address);
        let bad = d.request("auth", json!({"token": "guess"}));
        assert_eq!(bad["error"]["code"], json!(codes::UNAUTHORIZED));
        assert!(d.request("engine.status", json!({})).get("error").is_some());
        let good = d.call("auth", json!({"token": "s3cret"}));
        assert_eq!(good["ok"], json!(true));
        let status = d.call("engine.status", json!({}));
        assert!(status["frame"].is_u64());
    });
}

#[test]
fn dropping_the_app_closes_the_bridge() {
    let (mut app, address) = bridge_app(AiBridgePlugin::listen("127.0.0.1:0").without_env());
    let addr = address.clone();
    let mut client = run_until_done(&mut app, move || {
        let mut client = Client::connect(&addr);
        assert_eq!(client.call("bridge.ping", json!({}))["pong"], json!(true));
        client
    });

    drop(app);
    // The open connection is closed (end of stream or reset, not a read timeout)...
    client
        .writer
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut line = String::new();
    match client.reader.read_line(&mut line) {
        Ok(0) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        other => panic!("the connection is still open after the app was dropped: {other:?} {line}"),
    }
    // ...and the bridge stops serving new connections once the accept thread has exited.
    let deadline = Instant::now() + Duration::from_secs(5);
    while still_our_bridge(&address) {
        assert!(
            Instant::now() < deadline,
            "the bridge still accepts connections after the app was dropped"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// True if our (app-less) bridge still answers at `address`: it would reply "shutting down".
/// A refused connection, silence or a foreign answer (another process may reuse the freed port
/// on a busy machine) all mean it is gone.
fn still_our_bridge(address: &str) -> bool {
    let Ok(stream) = TcpStream::connect(address) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
    let mut writer = stream.try_clone().unwrap();
    let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "bridge.ping"}).to_string() + "\n";
    if writer.write_all(ping.as_bytes()).is_err() {
        return false;
    }
    let mut line = String::new();
    if BufReader::new(stream).read_line(&mut line).unwrap_or(0) == 0 {
        return false;
    }
    serde_json::from_str::<Value>(&line)
        .is_ok_and(|v| v["error"]["code"] == json!(codes::SHUTTING_DOWN))
}

#[test]
fn screenshot_over_tcp() {
    use engine_render::RenderDisabled;
    use engine_render::prelude::*;

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 320;
        w.height = 180;
    }
    app.add_plugins((
        RenderPlugin,
        AiBridgePlugin::listen("127.0.0.1:0").without_env(),
    ));
    app.world.spawn((
        Camera::default(),
        Transform::from_xyz(0.0, 1.5, 6.0).looking_at(DVec3::new(0.0, 1.0, 0.0), Vec3::Y),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.4, -0.8, -0.5), Vec3::Y),
    ));
    app.update_n(2);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let address = app.world.resource::<BridgeInfo>().address.clone().unwrap();
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let (empty_path, ball_path) = (dir.join("bridge_empty.png"), dir.join("bridge_ball.png"));
    for p in [&empty_path, &ball_path] {
        let _ = std::fs::remove_file(p);
    }
    let (empty_arg, ball_arg) = (
        empty_path.to_string_lossy().to_string(),
        ball_path.to_string_lossy().to_string(),
    );

    let result = run_until_done(&mut app, move || {
        let mut c = Client::connect(&address);
        c.call("render.screenshot", json!({"path": empty_arg}));
        // A red ball right where the camera looks, spawned and shown over the bridge.
        c.call(
            "entity.spawn",
            json!({"name": "Ball", "components": {
                "Transform": {"translation": [0.0, 1.0, 0.0]},
                "MeshRenderer": {
                    "mesh": {"shape": {"Sphere": {"radius": 1.0, "segments": 32}}},
                    "material": {"inline": {"base_color": [0.8, 0.03, 0.02, 1.0]}}
                }
            }}),
        );
        c.call("engine.step", json!({"frames": 1}));
        c.call("render.screenshot", json!({"path": ball_arg}))
    });
    assert_eq!(result["width"], json!(320), "{result}");
    assert_eq!(result["height"], json!(180), "{result}");

    let center = |path: &std::path::Path| {
        let img = image::open(path).expect("screenshot written").to_rgba8();
        let [r, g, b, _] = img.get_pixel(img.width() / 2, img.height() / 2).0;
        [r as i32, g as i32, b as i32]
    };
    let [r, g, b] = center(&ball_path);
    assert!(
        r > g + 40 && r > b + 40,
        "the ball spawned over the bridge is not in the picture: center pixel {:?}",
        [r, g, b]
    );
    let before = center(&empty_path);
    assert!(
        !(before[0] > before[1] + 40 && before[0] > before[2] + 40),
        "the picture taken before the spawn is already red: {before:?}"
    );
}
