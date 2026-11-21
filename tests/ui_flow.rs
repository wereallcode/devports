#![cfg(unix)]

#[test]
fn inline_list_opens_kills_and_restores_the_terminal() {
    let output = std::process::Command::new("python3")
        .arg("-c")
        .arg(include_str!("support/ui_flow.py"))
        .arg(env!("CARGO_BIN_EXE_devports"))
        .output()
        .expect("python3 is required for PTY integration tests");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
