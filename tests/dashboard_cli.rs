//! Dashboard argument and pipe-rejection executable tests.
use std::{
    process::Command,
    time::{Duration, Instant},
};

const BIN: &str = env!("CARGO_BIN_EXE_starlink");

fn run(args: &[&str]) -> std::process::Output {
    Command::new(BIN).args(args).output().expect("run starlink")
}

#[test]
fn dashboard_rejects_json_and_pipes_before_contacting_endpoint() {
    // The unroutable endpoint makes an accidental connection readily apparent, while
    // the short elapsed-time check proves these are local command-contract errors.
    for args in [
        &["--endpoint", "http://192.0.2.1:9201", "--json", "dashboard"][..],
        &["--endpoint", "http://192.0.2.1:9201", "dashboard"][..],
    ] {
        let started = Instant::now();
        let output = run(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{args:?} contacted network"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(if args.contains(&"--json") {
                "does not support --json"
            } else {
                "requires an interactive terminal"
            }),
            "{stderr}"
        );
    }
}

#[test]
fn dashboard_invalid_values_are_clap_errors() {
    for args in [
        &["dashboard", "--window", "0"][..],
        &["dashboard", "--window", "86401"][..],
        &["dashboard", "--interval", ".5"][..],
    ] {
        let output = run(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty());
    }
}
