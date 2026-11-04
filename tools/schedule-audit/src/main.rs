//! Prints the schedule report: `cargo run -p schedule-audit`; `-- --json` prints every
//! system's reads and writes as JSON instead, for `scripts/trace-blockers.py`.

fn main() {
    let report = schedule_audit::report();
    if std::env::args().any(|a| a == "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("the report is plain data")
        );
    } else {
        print!("{}", schedule_audit::markdown(&report));
    }
}
