use crate::{
    discovery,
    model::{Listener, ProcessInfo, Server},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6, TcpStream},
    thread,
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind};

pub struct StopOptions {
    pub force: bool,
    pub timeout: Duration,
}

pub fn stop(server: &Server, options: &StopOptions) -> Result<(), String> {
    let processes = discovery::processes();
    validate_target(&server.process, &processes)?;
    validate_target(&server.launcher, &processes)?;
    let protected = ancestor_pids(&processes, std::process::id());
    let targets = process_tree(&processes, &server.launcher);
    let endpoints = listener_endpoints(&server.listeners)?;
    if targets
        .iter()
        .any(|process| process.pid <= 1 || protected.contains(&process.pid))
    {
        return Err("Refusing to stop this command or one of its parent processes.".into());
    }
    for process in &targets {
        signal_if_same(process, libc::SIGTERM)?;
    }
    let remaining = wait_until_stopped(&targets, options.timeout);
    if remaining.is_empty() {
        return wait_for_socket_release(&endpoints);
    }
    if !options.force {
        return Err(format!(
            "Still running after {:.1}s: PID {}.",
            options.timeout.as_secs_f64(),
            format_pids(&remaining)
        ));
    }
    for process in &remaining {
        signal_if_same(process, libc::SIGKILL)?;
    }
    let remaining = wait_until_stopped(&remaining, Duration::from_secs(2));
    if remaining.is_empty() {
        wait_for_socket_release(&endpoints)
    } else {
        Err(format!("Could not stop PID {}.", format_pids(&remaining)))
    }
}

fn ancestor_pids(processes: &BTreeMap<u32, ProcessInfo>, pid: u32) -> BTreeSet<u32> {
    let mut ancestors = BTreeSet::new();
    let mut cursor = Some(pid);
    while let Some(pid) = cursor {
        if !ancestors.insert(pid) {
            break;
        }
        cursor = processes.get(&pid).and_then(|process| process.parent);
    }
    ancestors
}

fn format_pids(processes: &[ProcessInfo]) -> String {
    processes
        .iter()
        .map(|process| process.pid.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn validate_target(
    expected: &ProcessInfo,
    processes: &BTreeMap<u32, ProcessInfo>,
) -> Result<(), String> {
    if expected.uid != discovery::current_uid() || expected.pid <= 1 || expected.started_at == 0 {
        return Err(format!(
            "Cannot safely identify PID {} as your process.",
            expected.pid
        ));
    }
    match processes.get(&expected.pid) {
        Some(actual) if expected.identity() == actual.identity() => Ok(()),
        _ => Err(format!(
            "PID {} has exited or changed since discovery. Refresh the server list.",
            expected.pid
        )),
    }
}

pub(super) fn process_tree(
    processes: &BTreeMap<u32, ProcessInfo>,
    root: &ProcessInfo,
) -> Vec<ProcessInfo> {
    let mut targets = vec![root.clone()];
    let mut seen = BTreeSet::from([root.pid]);
    let mut position = 0;
    while position < targets.len() {
        let parent = targets[position].pid;
        for process in processes.values() {
            if process.parent == Some(parent) && process.uid == root.uid && seen.insert(process.pid)
            {
                targets.push(process.clone());
            }
        }
        position += 1;
    }
    targets
}

fn still_running(expected: &ProcessInfo) -> bool {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[Pid::from_u32(expected.pid)]),
        true,
        ProcessRefreshKind::nothing()
            .without_tasks()
            .with_user(UpdateKind::Always),
    );
    system
        .process(Pid::from_u32(expected.pid))
        .is_some_and(|actual| {
            actual.start_time() == expected.started_at
                && actual.user_id().is_some_and(|uid| **uid == expected.uid)
                && actual.status() != ProcessStatus::Zombie
        })
}

fn signal_if_same(expected: &ProcessInfo, signal: i32) -> Result<(), String> {
    if !still_running(expected) {
        return Ok(());
    }
    let pid = i32::try_from(expected.pid).map_err(|_| "Invalid process ID.".to_owned())?;
    if pid <= 1 || expected.uid != discovery::current_uid() {
        return Err("Refusing to signal an invalid or foreign process.".into());
    }
    // SAFETY: a positive PID was validated against this user's current process identity.
    if unsafe { libc::kill(pid, signal) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(format!("Could not signal PID {}: {error}", expected.pid))
}

fn wait_until_stopped(targets: &[ProcessInfo], timeout: Duration) -> Vec<ProcessInfo> {
    let start = Instant::now();
    loop {
        let remaining: Vec<_> = targets
            .iter()
            .filter(|process| still_running(process))
            .cloned()
            .collect();
        if remaining.is_empty() || start.elapsed() >= timeout {
            return remaining;
        }
        thread::sleep(Duration::from_millis(60).min(timeout.saturating_sub(start.elapsed())));
    }
}

fn listener_endpoints(listeners: &[Listener]) -> Result<BTreeSet<SocketAddr>, String> {
    let mut endpoints = BTreeSet::new();
    for listener in listeners {
        let address = listener.address.trim_matches(['[', ']']);
        if address == "*" {
            endpoints.insert((Ipv4Addr::LOCALHOST, listener.port).into());
            endpoints.insert((Ipv6Addr::LOCALHOST, listener.port).into());
            continue;
        }
        let (address, scope) = address.split_once('%').unwrap_or((address, ""));
        let ip: IpAddr = address.parse().map_err(|_| {
            format!(
                "Cannot verify listening address {}; the server has been left running.",
                listener.address
            )
        })?;
        let endpoint = match ip {
            IpAddr::V4(ip) => (
                if ip.is_unspecified() {
                    Ipv4Addr::LOCALHOST
                } else {
                    ip
                },
                listener.port,
            )
                .into(),
            IpAddr::V6(ip) => SocketAddr::V6(SocketAddrV6::new(
                if ip.is_unspecified() {
                    Ipv6Addr::LOCALHOST
                } else {
                    ip
                },
                listener.port,
                0,
                interface_index(scope)?,
            )),
        };
        endpoints.insert(endpoint);
    }
    Ok(endpoints)
}

fn interface_index(scope: &str) -> Result<u32, String> {
    if scope.is_empty() {
        return Ok(0);
    }
    if let Ok(number) = scope.parse() {
        return Ok(number);
    }
    let name =
        std::ffi::CString::new(scope).map_err(|_| "Invalid interface name in listener address.")?;
    // SAFETY: name is a valid NUL-terminated interface name.
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    if index == 0 {
        return Err(format!(
            "Cannot identify interface {scope}; the server has been left running."
        ));
    }
    Ok(index)
}

fn wait_for_socket_release(endpoints: &BTreeSet<SocketAddr>) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut remaining = endpoints.clone();
    while !remaining.is_empty() {
        let mut occupied = BTreeSet::new();
        for endpoint in &remaining {
            let timeout = deadline.saturating_duration_since(Instant::now());
            if timeout.is_zero() {
                occupied.insert(*endpoint);
                continue;
            }
            match TcpStream::connect_timeout(endpoint, timeout.min(Duration::from_millis(25))) {
                Ok(stream) => {
                    drop(stream);
                    occupied.insert(*endpoint);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionRefused
                            | io::ErrorKind::AddrNotAvailable
                            | io::ErrorKind::NetworkUnreachable
                            | io::ErrorKind::HostUnreachable
                    ) || error.raw_os_error() == Some(libc::EAFNOSUPPORT) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    occupied.insert(*endpoint);
                }
                Err(error) => {
                    return Err(format!(
                        "The process exited, but socket release could not be verified for {endpoint}: {error}. Refresh the server list."
                    ));
                }
            }
        }
        remaining = occupied;
        if !remaining.is_empty() && Instant::now() >= deadline {
            return Err(format!(
                "The process exited, but TCP endpoints {} still accept connections or could not be confirmed closed. Another process may own them; refresh the server list.",
                remaining
                    .iter()
                    .map(SocketAddr::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !remaining.is_empty() {
            thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
    Ok(())
}
