//! `engine-mcp`: an MCP (Model Context Protocol) server over stdio that lets AI assistants drive
//! a game made with this engine, through the game's AI bridge (`engine_ai_bridge`).
//!
//! stdin/stdout carry JSON-RPC 2.0, one message per line; everything else (logs, the launched
//! game's output) goes to stderr. See README.md for client setup.

mod base64;
mod bridge;
mod mcp;

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use crate::bridge::{BridgeLink, DEFAULT_ADDRESS, ENV_ADDRESS, ENV_TOKEN};
use crate::mcp::{Server, Settings};

const USAGE: &str = "\
engine-mcp: MCP server (stdio) for driving an engine game through its AI bridge.

USAGE:
    engine-mcp [--connect ADDR | --launch \"COMMAND\"] [OPTIONS]

    --connect ADDR       Talk to a running game's bridge (default: $ENGINE_BRIDGE or 127.0.0.1:7878).
    --launch \"COMMAND\"   Start the game with a shell command, e.g. \"cargo run -q -p hello -- --headless\".
                         It gets ENGINE_BRIDGE=127.0.0.1:<free port> and is stopped when this server exits.
    --token TOKEN        Bridge token (default: $ENGINE_BRIDGE_TOKEN). Passed to a launched game too.
    --startup-timeout S  Seconds to wait for a launched game's bridge (default 600; first builds are slow).
    --call-timeout S     Seconds one tool call may take (default 600).
    --list-wait S        Seconds tools/list waits for the game before listing only built-in tools
                         (default 15 with --launch, 2 otherwise).
    -h, --help           Show this help.
    -V, --version        Show the version.
";

#[derive(Debug, Clone, PartialEq)]
struct Options {
    connect: Option<String>,
    launch: Option<String>,
    token: Option<String>,
    startup_timeout: Duration,
    call_timeout: Duration,
    list_wait: Option<Duration>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            connect: None,
            launch: None,
            token: None,
            startup_timeout: Duration::from_secs(600),
            call_timeout: Duration::from_secs(600),
            list_wait: None,
        }
    }
}

enum Parsed {
    Run(Options),
    Exit(String),
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Parsed, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        let seconds = |name: &str, v: String| -> Result<Duration, String> {
            v.parse::<f64>()
                .ok()
                .filter(|s| s.is_finite() && *s >= 0.0)
                .map(Duration::from_secs_f64)
                .ok_or_else(|| format!("{name} needs a number of seconds, got '{v}'"))
        };
        match flag.as_str() {
            "--connect" => options.connect = Some(value("--connect")?),
            "--launch" => options.launch = Some(value("--launch")?),
            "--token" => options.token = Some(value("--token")?),
            "--startup-timeout" => {
                options.startup_timeout = seconds(&flag, value(&flag)?)?;
            }
            "--call-timeout" => options.call_timeout = seconds(&flag, value(&flag)?)?,
            "--list-wait" => options.list_wait = Some(seconds(&flag, value(&flag)?)?),
            "-h" | "--help" => return Ok(Parsed::Exit(USAGE.to_string())),
            "-V" | "--version" => {
                return Ok(Parsed::Exit(format!(
                    "engine-mcp {} (MCP {})\n",
                    env!("CARGO_PKG_VERSION"),
                    mcp::PROTOCOL_VERSION
                )));
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if options.connect.is_some() && options.launch.is_some() {
        return Err("use either --connect or --launch, not both".into());
    }
    Ok(Parsed::Run(options))
}

/// The bridge address from `ENGINE_BRIDGE`, read the way the game reads it.
fn env_address(value: &str) -> String {
    let v = value.trim();
    match v.to_ascii_lowercase().as_str() {
        "" | "1" | "true" | "on" | "yes" | "off" | "false" | "no" => DEFAULT_ADDRESS.to_string(),
        _ if v.chars().all(|c| c.is_ascii_digit()) => format!("127.0.0.1:{v}"),
        _ => v.to_string(),
    }
}

fn write_message(out: &Mutex<std::io::Stdout>, message: &Value) {
    // serde_json escapes newlines inside strings, so this is exactly one line.
    let mut line = message.to_string();
    line.push('\n');
    let mut out = out.lock().unwrap_or_else(|p| p.into_inner());
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
}

fn main() {
    let options = match parse_args(std::env::args().skip(1)) {
        Ok(Parsed::Run(options)) => options,
        Ok(Parsed::Exit(text)) => {
            print!("{text}");
            return;
        }
        Err(e) => {
            eprintln!("engine-mcp: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let token = options
        .token
        .clone()
        .or_else(|| std::env::var(ENV_TOKEN).ok())
        .filter(|t| !t.is_empty());

    let bridge = match &options.launch {
        Some(command) => match BridgeLink::launch(command, token, options.startup_timeout) {
            Ok(link) => link,
            Err(e) => {
                eprintln!("engine-mcp: {e}");
                std::process::exit(1);
            }
        },
        None => {
            let address = options
                .connect
                .clone()
                .or_else(|| std::env::var(ENV_ADDRESS).ok().map(|v| env_address(&v)))
                .unwrap_or_else(|| DEFAULT_ADDRESS.to_string());
            eprintln!("engine-mcp: using the game's AI bridge at {address}");
            BridgeLink::connect_to(address, token)
        }
    };
    let launched = bridge.is_launched();
    let settings = Settings {
        list_wait: options
            .list_wait
            .unwrap_or(Duration::from_secs(if launched { 15 } else { 2 })),
        call_timeout: options.call_timeout,
        ..Settings::default()
    };

    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    let notify_out = stdout.clone();
    let server = Arc::new(Server::new(
        bridge,
        settings,
        Box::new(move |message| write_message(&notify_out, &message)),
    ));
    {
        let server = server.clone();
        let startup = options.startup_timeout;
        std::thread::Builder::new()
            .name("bridge-watch".into())
            .spawn(move || server.watch(startup))
            .expect("start the watcher thread");
    }

    // Raw bytes, not `lines()`: a line that is not UTF-8 gets a parse error like any other bad
    // line instead of ending the session (and stopping the game).
    let mut stdin = std::io::stdin().lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        match stdin.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = line.trim_ascii();
        if text.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_slice(text) {
            Ok(m) => m,
            Err(e) => {
                let error = mcp::error_response(
                    &Value::Null,
                    mcp::codes::PARSE_ERROR,
                    &format!("parse error: {e}"),
                );
                write_message(&stdout, &error);
                continue;
            }
        };
        if mcp::is_slow(&message) {
            let (server, stdout) = (server.clone(), stdout.clone());
            std::thread::spawn(move || {
                if let Some(response) = server.handle(&message) {
                    write_message(&stdout, &response);
                }
            });
        } else if let Some(response) = server.handle(&message) {
            write_message(&stdout, &response);
        }
    }
    // stdin closed: the client is gone. Stop the game we launched.
    server.shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Parsed, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    fn options(args: &[&str]) -> Options {
        match parse(args) {
            Ok(Parsed::Run(o)) => o,
            Ok(Parsed::Exit(_)) => panic!("unexpected exit"),
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn parses_flags() {
        let o = options(&[
            "--launch",
            "cargo run -q -p hello -- --headless",
            "--list-wait=5",
        ]);
        assert_eq!(
            o.launch.as_deref(),
            Some("cargo run -q -p hello -- --headless")
        );
        assert_eq!(o.list_wait, Some(Duration::from_secs(5)));
        let o = options(&[
            "--connect=127.0.0.1:9000",
            "--token",
            "t",
            "--call-timeout",
            "1.5",
        ]);
        assert_eq!(o.connect.as_deref(), Some("127.0.0.1:9000"));
        assert_eq!(o.token.as_deref(), Some("t"));
        assert_eq!(o.call_timeout, Duration::from_millis(1500));
        assert_eq!(options(&[]), Options::default());
        assert!(parse(&["--connect", "a", "--launch", "b"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--launch"]).is_err());
        assert!(matches!(parse(&["--help"]), Ok(Parsed::Exit(_))));
    }

    #[test]
    fn env_addresses() {
        assert_eq!(env_address("1"), DEFAULT_ADDRESS);
        assert_eq!(env_address("9100"), "127.0.0.1:9100");
        assert_eq!(env_address("10.0.0.2:7000"), "10.0.0.2:7000");
    }
}
