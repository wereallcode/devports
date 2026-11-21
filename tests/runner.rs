#![cfg(unix)]

use devports::{
    actions::{self, StopOptions},
    discovery,
    model::Server,
};
use serde_json::Value;
use std::{
    fs,
    net::TcpStream,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

// A real npm -> sh -c -> node --watch -> node chain; `&` keeps sh from exec-ing node.
const PACKAGE: &str = r#"{
  "name": "devports-isolated-runner-fixture",
  "private": true,
  "scripts": { "dev": "node --watch server.cjs & wait" }
}"#;

const SERVER: &str = r#"
const fs = require('node:fs');
const http = require('node:http');
const server = http.createServer((request, response) => response.end('fixture'));
server.listen(0, '127.0.0.1', () => {
  fs.writeFileSync('ready.json', JSON.stringify({ pid: process.pid, port: server.address().port }));
});
"#;

struct Fixture {
    directory: TempDir,
    canonical_directory: PathBuf,
    child: Child,
}

impl Fixture {
    fn start() -> Option<Self> {
        for program in ["node", "npm", "lsof"] {
            let mut check = Command::new(program);
            check.arg(if program == "lsof" { "-v" } else { "--version" });
            if !check.output().is_ok_and(|output| output.status.success()) {
                eprintln!("skipping runner integration test: {program} is unavailable");
                return None;
            }
        }
        let directory = tempfile::Builder::new()
            .prefix("devports runner fixture ")
            .tempdir()
            .unwrap();
        let canonical_directory = directory.path().canonicalize().unwrap();
        fs::write(directory.path().join("package.json"), PACKAGE).unwrap();
        fs::write(directory.path().join("server.cjs"), SERVER).unwrap();
        let output = fs::File::create(directory.path().join("runner.log")).unwrap();
        let mut command = Command::new("npm");
        command
            .args(["run", "dev", "--silent"])
            .current_dir(&canonical_directory)
            .env_remove("NODE_OPTIONS")
            .env("npm_config_update_notifier", "false")
            .stdin(Stdio::null())
            .stdout(output.try_clone().unwrap())
            .stderr(output);
        // SAFETY: setsid is async-signal-safe and isolates only this fixture.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let fixture = Self {
            child: command.spawn().expect("start isolated npm runner"),
            directory,
            canonical_directory,
        };
        fixture.ready();
        Some(fixture)
    }

    fn ready(&self) -> (u32, u16) {
        let start = Instant::now();
        loop {
            if let Ok(bytes) = fs::read(self.directory.path().join("ready.json"))
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                && let (Some(pid), Some(port)) = (value["pid"].as_u64(), value["port"].as_u64())
            {
                return (pid as u32, port as u16);
            }
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "runner did not become ready: {}",
                fs::read_to_string(self.directory.path().join("runner.log")).unwrap_or_default()
            );
            thread::sleep(Duration::from_millis(40));
        }
    }

    fn list(&self) -> Vec<Server> {
        discovery::scan()
            .unwrap()
            .servers
            .into_iter()
            .filter(|server| {
                server.development
                    && server
                        .process
                        .directory
                        .as_ref()
                        .is_some_and(|directory| directory.starts_with(&self.canonical_directory))
            })
            .collect()
    }

    fn assert_no_listener_or_respawn(&self) {
        // Touch the watched file so a surviving watcher would respawn the server.
        fs::write(
            self.directory.path().join("server.cjs"),
            format!("{SERVER}\n// respawn trigger\n"),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(700));
        assert!(self.list().is_empty());
        let survivors: Vec<_> = discovery::processes()
            .into_values()
            .filter(|process| {
                process.uid == discovery::current_uid()
                    && process.directory.as_ref() == Some(&self.canonical_directory)
            })
            .map(|process| (process.pid, process.name))
            .collect();
        assert!(
            survivors.is_empty(),
            "fixture processes survived: {survivors:?}"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Only signal processes inside this fixture's private cwd.
        let processes = discovery::processes();
        for expected in processes.into_values().filter(|process| {
            process.uid == discovery::current_uid()
                && process.directory.as_ref() == Some(&self.canonical_directory)
        }) {
            if let Some(actual) = discovery::process(expected.pid)
                && actual.started_at == expected.started_at
                && actual.uid == expected.uid
            {
                // SAFETY: this still-identical process belongs to this fixture's unique cwd.
                unsafe { libc::kill(expected.pid as i32, libc::SIGKILL) };
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn graceful() -> StopOptions {
    StopOptions {
        force: false,
        timeout: Duration::from_secs(5),
    }
}

#[test]
fn detects_and_stops_package_runner_with_watcher_and_shell_children() {
    let Some(fixture) = Fixture::start() else {
        return;
    };
    let (listener_pid, port) = fixture.ready();
    let servers = fixture.list();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].process.pid, listener_pid);
    assert_eq!(servers[0].launcher.pid, fixture.child.id());
    assert_ne!(listener_pid, fixture.child.id());
    assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());

    actions::stop(&servers[0], &graceful()).unwrap();
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    fixture.assert_no_listener_or_respawn();
}
