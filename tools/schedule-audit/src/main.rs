//! Prints the schedule report: `cargo run -p schedule-audit`.

fn main() {
    print!("{}", schedule_audit::markdown(&schedule_audit::report()));
}
