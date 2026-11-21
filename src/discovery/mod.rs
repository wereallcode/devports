mod classification;

use classification::{classify, find_launcher, project_root};

use crate::model::{Listener, ProcessInfo, Server, Snapshot};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Command,
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind};

pub fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not modify memory.
    unsafe { libc::geteuid() }
}

fn refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .without_tasks()
        .with_user(UpdateKind::Always)
        .with_cmd(UpdateKind::Always)
        .with_cwd(UpdateKind::Always)
        .with_exe(UpdateKind::Always)
}

fn process_info(process: &sysinfo::Process) -> Option<ProcessInfo> {
    if matches!(
        process.status(),
        ProcessStatus::Zombie | ProcessStatus::Dead
    ) {
        return None;
    }
    Some(ProcessInfo {
        pid: process.pid().as_u32(),
        parent: process.parent().map(|pid| pid.as_u32()),
        uid: **process.user_id()?,
        name: process.name().to_string_lossy().into_owned(),
        executable: process.exe().map(Path::to_path_buf),
        command: process
            .cmd()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
        directory: process
            .cwd()
            .map(|path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf())),
        started_at: process.start_time(),
    })
}

pub fn processes() -> BTreeMap<u32, ProcessInfo> {
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());
    system
        .processes()
        .values()
        .filter_map(process_info)
        .map(|process| (process.pid, process))
        .collect()
}

pub fn process(pid: u32) -> Option<ProcessInfo> {
    let mut system = System::new();
    let pid = Pid::from_u32(pid);
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, refresh_kind());
    system.process(pid).and_then(process_info)
}

#[derive(Debug, Default)]
struct SocketOwner {
    uid: Option<u32>,
    listeners: BTreeSet<Listener>,
}

#[derive(Default)]
struct SocketFields {
    protocol: String,
    family: String,
    name: String,
    listening: bool,
}

impl SocketFields {
    fn finish(&mut self, owner: Option<&mut SocketOwner>) {
        if self.protocol == "TCP"
            && self.listening
            && let (Some(owner), Some(mut listener)) = (owner, parse_address(&self.name))
        {
            if listener.address == "*" {
                listener.address = match self.family.as_str() {
                    "IPv6" => "::".to_owned(),
                    "IPv4" => "0.0.0.0".to_owned(),
                    _ => listener.address,
                };
            }
            owner.listeners.insert(listener);
        }
        *self = Self::default();
    }
}

fn parse_address(value: &str) -> Option<Listener> {
    let (address, port) = value.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    if address.is_empty() || port == 0 || address.contains("->") {
        return None;
    }
    Some(Listener {
        address: address.trim_matches(['[', ']']).to_owned(),
        port,
    })
}

fn parse_lsof(text: &str) -> BTreeMap<u32, SocketOwner> {
    let mut owners = BTreeMap::<u32, SocketOwner>::new();
    let mut pid = None;
    let mut socket = SocketFields::default();
    for line in text.lines() {
        let Some((field, value)) = line
            .as_bytes()
            .first()
            .and_then(|field| line.get(1..).map(|value| (char::from(*field), value)))
        else {
            continue;
        };
        match field {
            'p' => {
                socket.finish(pid.and_then(|pid| owners.get_mut(&pid)));
                pid = value.parse::<u32>().ok();
                if let Some(pid) = pid {
                    owners.entry(pid).or_default();
                }
            }
            'u' => {
                if let Some(owner) = pid.and_then(|pid| owners.get_mut(&pid)) {
                    owner.uid = value.parse().ok();
                }
            }
            'f' => socket.finish(pid.and_then(|pid| owners.get_mut(&pid))),
            'P' => socket.protocol = value.to_owned(),
            't' => socket.family = value.to_owned(),
            'n' => socket.name = value.to_owned(),
            'T' if value == "ST=LISTEN" => socket.listening = true,
            _ => {}
        }
    }
    socket.finish(pid.and_then(|pid| owners.get_mut(&pid)));
    owners.retain(|_, owner| !owner.listeners.is_empty());
    owners
}

fn cwd_fallback(pid: u32) -> Option<PathBuf> {
    let output = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix('n').map(PathBuf::from))
        .filter(|path| path.is_absolute())
        .map(|path| path.canonicalize().unwrap_or(path))
}

pub fn scan() -> Result<Snapshot, String> {
    let output = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-FpcuPntT"])
        .output()
        .map_err(|error| {
            format!("Cannot run lsof: {error}. Install lsof and make it available on PATH.")
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success()
        && !(output.status.code() == Some(1)
            && output.stdout.is_empty()
            && stderr.trim().is_empty())
    {
        return Err(format!(
            "Cannot discover listening ports: {}",
            if stderr.trim().is_empty() {
                format!("lsof exited with {}", output.status)
            } else {
                stderr.trim().to_owned()
            }
        ));
    }
    let uid = current_uid();
    let owners = parse_lsof(&stdout);
    let mut snapshot = Snapshot::default();
    if owners.is_empty() {
        return Ok(snapshot);
    }
    let mut processes = processes();
    for (pid, owner) in owners {
        if owner.uid != Some(uid) {
            continue;
        }
        // The process may have exited since lsof listed it.
        let Some(info) = processes.get_mut(&pid) else {
            continue;
        };
        if info.uid != uid {
            continue;
        }
        if info.directory.is_none() {
            info.directory = cwd_fallback(pid);
        }
        let info = info.clone();
        let launcher = find_launcher(&info, &processes).clone();
        let development = classify(&info, &launcher);
        let project = info
            .directory
            .as_deref()
            .and_then(project_root)
            .or_else(|| info.directory.clone())
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| info.name.clone());
        snapshot.servers.push(Server {
            process: info,
            launcher,
            project,
            listeners: owner.listeners.into_iter().collect(),
            development,
        });
    }
    snapshot.servers.sort_by_key(|server| {
        (
            server.listeners.iter().map(|listener| listener.port).min(),
            server.process.pid,
        )
    });
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(pid: u32, parent: Option<u32>, exe: &str, command: &[&str]) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent,
            uid: 501,
            name: Path::new(exe)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            executable: Some(exe.into()),
            command: command.iter().map(|arg| (*arg).to_owned()).collect(),
            directory: Some("/work/project".into()),
            started_at: 100,
        }
    }

    #[test]
    fn parses_ipv4_ipv6_wildcard_and_deduplicates_descriptors() {
        let output = "p42\ncnode\nu501\nf3\nPTCP\nn127.0.0.1:3000\nTST=LISTEN\nf4\nPTCP\nn[::1]:3001\nTST=LISTEN\nf5\nPTCP\nn*:3002\nTST=LISTEN\nf6\nPTCP\nn127.0.0.1:3000\nTST=LISTEN\np43\ncpython\nu502\nf7\nPTCP\nn[::]:8000\nTST=LISTEN\n";
        let owners = parse_lsof(output);
        assert_eq!(owners.len(), 2);
        assert_eq!(owners[&42].uid, Some(501));
        assert_eq!(owners[&42].listeners.len(), 3);
        assert!(owners[&42].listeners.contains(&Listener {
            address: "::1".into(),
            port: 3001
        }));
        assert_eq!(
            owners[&43].listeners.iter().next().unwrap().url(),
            "http://[::1]:8000"
        );
    }

    #[test]
    fn skips_non_listening_and_malformed_sockets() {
        let output = "p42\nu501\nf3\nPTCP\nn127.0.0.1:3000->127.0.0.1:4000\nTST=ESTABLISHED\nf4\nPUDP\nn*:3000\nf5\nPTCP\nn*:99999\nTST=LISTEN\n";
        assert!(parse_lsof(output).is_empty());
        assert!(parse_address("*:0").is_none());
        assert!(parse_address(":4000").is_none());
    }

    #[test]
    fn runtimes_do_not_depend_on_common_ports_or_frameworks() {
        for name in [
            "node",
            "python3.13",
            "ruby",
            "php",
            "java",
            "bun",
            "deno",
            "dotnet",
        ] {
            let process = fixture(100, None, &format!("/usr/bin/{name}"), &[name]);
            assert!(classify(&process, &process), "{name}");
        }
        let service = fixture(100, None, "/usr/sbin/sshd", &["sshd"]);
        assert!(!classify(&service, &service));
    }

    #[test]
    fn excludes_application_helpers_and_coding_agents() {
        let python = fixture(
            99,
            None,
            "/Library/Frameworks/Python.framework/Versions/3.13/Resources/Python.app/Contents/MacOS/Python",
            &["python3", "-m", "http.server"],
        );
        assert!(classify(&python, &python));
        let app = fixture(
            100,
            None,
            "/Applications/Editor.app/Contents/node",
            &["node"],
        );
        assert!(!classify(&app, &app));
        let agent = fixture(
            100,
            None,
            "/usr/bin/node",
            &["node", "/tools/@anthropic-ai/claude-code/cli.js"],
        );
        assert!(!classify(&agent, &agent));
    }

    #[test]
    fn selects_package_runner_through_its_shell_but_not_terminal() {
        let listener = fixture(104, Some(103), "/usr/bin/node", &["node", "server.js"]);
        let shell = fixture(103, Some(102), "/bin/sh", &["sh", "-c", "node server.js"]);
        let runner = fixture(
            102,
            Some(101),
            "/usr/bin/node",
            &["node", "/usr/lib/npm/bin/npm-cli.js", "run", "dev"],
        );
        let terminal = fixture(101, None, "/bin/zsh", &["zsh"]);
        let map = [listener.clone(), shell, runner, terminal]
            .into_iter()
            .map(|p| (p.pid, p))
            .collect();
        assert_eq!(find_launcher(&listener, &map).pid, 102);
    }

    #[test]
    fn recognizes_dedicated_watchers_in_the_runner_chain() {
        let listener = fixture(104, Some(103), "/work/project/target/debug/api", &["api"]);
        let cargo = fixture(103, Some(102), "/usr/bin/cargo", &["cargo", "run"]);
        let watcher = fixture(
            102,
            None,
            "/usr/bin/cargo-watch",
            &["cargo-watch", "-x", "run"],
        );
        let map = [listener.clone(), cargo, watcher]
            .into_iter()
            .map(|process| (process.pid, process))
            .collect();
        assert_eq!(find_launcher(&listener, &map).pid, 102);
    }

    #[test]
    fn workspace_runner_can_launch_from_a_child_directory() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("package.json"), "{}").unwrap();
        let mut listener = fixture(104, Some(102), "/usr/bin/node", &["node", "server.js"]);
        listener.directory = Some(directory.path().join("packages/web"));
        let mut runner = fixture(
            102,
            None,
            "/usr/bin/npm",
            &["npm", "run", "dev", "--workspace", "web"],
        );
        runner.directory = Some(directory.path().to_path_buf());
        let map = [(102, runner)].into_iter().collect();
        assert_eq!(find_launcher(&listener, &map).pid, 102);
    }

    #[test]
    fn wildcard_urls_retain_the_socket_family() {
        let owners = parse_lsof(
            "p42\nu501\nf3\ntIPv6\nPTCP\nn*:3000\nTST=LISTEN\nf4\ntIPv4\nPTCP\nn*:3000\nTST=LISTEN\n",
        );
        let urls: BTreeSet<_> = owners[&42]
            .listeners
            .iter()
            .map(|listener| listener.url())
            .collect();
        assert_eq!(
            urls,
            BTreeSet::from([
                "http://[::1]:3000".to_owned(),
                "http://127.0.0.1:3000".to_owned()
            ])
        );
    }

    #[test]
    fn does_not_cross_user_directory_or_unknown_parent() {
        let listener = fixture(104, Some(103), "/usr/bin/node", &["node", "server.js"]);
        for (uid, cwd, exe, args) in [
            (
                502,
                "/work/project",
                "/usr/bin/npm",
                vec!["npm", "run", "dev"],
            ),
            (501, "/other", "/usr/bin/npm", vec!["npm", "run", "dev"]),
            (
                501,
                "/work/project",
                "/usr/bin/node",
                vec!["node", "agent.js"],
            ),
            (
                501,
                "/work/project",
                "/bin/sh",
                vec!["sh", "-c", "node server.js"],
            ),
        ] {
            let mut parent = fixture(103, None, exe, &args);
            parent.uid = uid;
            parent.directory = Some(cwd.into());
            let map = [(103, parent)].into_iter().collect();
            assert_eq!(find_launcher(&listener, &map).pid, 104);
        }
    }

    #[test]
    fn detects_a_project_local_binary_without_framework_names() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("Cargo.toml"), "[package]").unwrap();
        let mut process = fixture(100, None, "/unused", &["custom-web"]);
        process.directory = Some(directory.path().into());
        process.executable = Some(directory.path().join("target/debug/custom-web"));
        assert!(classify(&process, &process));
        process.executable = Some("/usr/bin/custom-service".into());
        assert!(!classify(&process, &process));
    }
}
