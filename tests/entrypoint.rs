#![cfg(unix)]

use std::{
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn run_with_deadline(args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_devports"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(3) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("devports did not reject noninteractive invocation promptly");
        }
        thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn rejects_subcommands_and_flags_with_the_interactive_entrypoint_hint() {
    for args in [
        vec!["list"],
        vec!["stop", "3000"],
        vec!["--help"],
        vec!["--json"],
        vec!["--version", "list"],
    ] {
        let output = run_with_deadline(&args);
        assert!(!output.status.success(), "unexpectedly accepted {args:?}");
        let error = String::from_utf8_lossy(&output.stderr).to_lowercase();
        assert!(
            error.contains("devports takes no arguments except --version."),
            "missing actionable entrypoint hint for {args:?}: {error}"
        );
        assert!(
            output.stdout.is_empty(),
            "argument rejection should not draw a UI"
        );
    }
}

#[test]
fn prints_the_list_without_a_terminal_for_agents() {
    let output = run_with_deadline(&[]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.starts_with("devports — A CLI to manage ports held by development servers."),
        "{text}"
    );
    assert!(!text.contains("Select"), "{text}");
    assert!(
        !text.contains('\u{1b}'),
        "plain output must not contain escapes"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn prints_the_version_without_a_terminal() {
    let output = run_with_deadline(&["--version"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        env!("CARGO_PKG_VERSION")
    );
    assert!(output.stderr.is_empty());
}
