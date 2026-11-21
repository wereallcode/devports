#![cfg(unix)]

use devports::{
    actions::{self, StopOptions},
    discovery,
    model::Server,
};
use serde_json::Value;
use std::{
    fs,
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const SERVER: &str = r#"import json, os, signal, socket, sys
port = int(sys.argv[1])
if len(sys.argv) > 2 and sys.argv[2] == 'stubborn':
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1', port))
s.listen(8)
with open('ready', 'w') as ready:
    json.dump({'pid': os.getpid()}, ready)
while True:
    conn, addr = s.accept()
    conn.close()
"#;

struct Fixture {
    directory: TempDir,
    child: Child,
    port: u16,
}

impl Fixture {
    fn new(stubborn: bool) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("devports fixture ")
            .tempdir()
            .unwrap();
        let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);
        let script = directory.path().join("server.py");
        fs::write(&script, SERVER).unwrap();
        let child = Command::new("python3")
            .arg("-u")
            .arg(&script)
            .arg(port.to_string())
            .args(if stubborn { vec!["stubborn"] } else { vec![] })
            .current_dir(directory.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("python3 is required for integration tests");
        let fixture = Self {
            directory,
            child,
            port,
        };
        fixture.ready();
        fixture
    }

    fn ready(&self) {
        let start = Instant::now();
        while serde_json::from_slice::<Value>(
            &fs::read(self.directory.path().join("ready")).unwrap_or_default(),
        )
        .is_err()
        {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "test server did not start"
            );
            thread::sleep(Duration::from_millis(30));
        }
    }

    fn server(&self) -> Server {
        let snapshot = discovery::scan().unwrap();
        let directory = self.directory.path().canonicalize().unwrap();
        let mut matches: Vec<_> = snapshot
            .servers
            .into_iter()
            .filter(|server| {
                server.development
                    && server.ports().contains(&self.port)
                    && server.process.directory.as_ref() == Some(&directory)
            })
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly this fixture's development server"
        );
        matches.remove(0)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Clean up even after a failed assertion, only inside this private cwd.
        let directory = self.directory.path().canonicalize().unwrap();
        for expected in discovery::processes().into_values().filter(|process| {
            process.uid == discovery::current_uid()
                && process.directory.as_ref() == Some(&directory)
        }) {
            if let Some(actual) = discovery::process(expected.pid)
                && actual.started_at == expected.started_at
                && actual.uid == expected.uid
            {
                // SAFETY: this still-identical process belongs to the private fixture cwd.
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
fn discovers_and_selects_external_server_without_touching_it() {
    let fixture = Fixture::new(false);
    let server = fixture.server();
    assert_eq!(server.process.pid, fixture.child.id());
    assert_eq!(server.listeners[0].port, fixture.port);
    assert_eq!(
        actions::browser_url(&server, Some(fixture.port)).unwrap(),
        format!("http://127.0.0.1:{}", fixture.port)
    );
    assert!(TcpStream::connect(("127.0.0.1", fixture.port)).is_ok());
}

#[test]
fn force_is_explicit_and_timeout_is_an_error() {
    let fixture = Fixture::new(true);
    let server = fixture.server();
    let graceful = StopOptions {
        force: false,
        timeout: Duration::from_millis(150),
    };
    assert!(actions::stop(&server, &graceful).is_err());
    assert!(TcpStream::connect(("127.0.0.1", fixture.port)).is_ok());
    let forced = StopOptions {
        force: true,
        timeout: Duration::from_millis(150),
    };
    actions::stop(&server, &forced).unwrap();
    assert!(TcpStream::connect(("127.0.0.1", fixture.port)).is_err());
}

#[test]
fn stale_process_identity_cannot_stop_a_live_server() {
    let fixture = Fixture::new(false);
    let mut stale = fixture.server();
    stale.process.started_at = stale.process.started_at.saturating_add(1);
    assert!(actions::stop(&stale, &graceful()).is_err());
    assert!(TcpStream::connect(("127.0.0.1", fixture.port)).is_ok());
}

#[test]
fn ipv6_wildcard_produces_an_ipv6_loopback_browser_url() {
    let listener = match TcpListener::bind("[::]:0") {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("skipping IPv6 test: {error}");
            return;
        }
    };
    let port = listener.local_addr().unwrap().port();
    let snapshot = discovery::scan().unwrap();
    let server = snapshot
        .servers
        .into_iter()
        .find(|server| server.ports().contains(&port))
        .expect("the IPv6 listener is discovered");
    assert_eq!(
        actions::browser_url(&server, Some(port)).unwrap(),
        format!("http://[::1]:{port}")
    );
}
