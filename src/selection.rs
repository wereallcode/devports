use crate::model::{ProcessInfo, Server};
use std::collections::{BTreeMap, BTreeSet};

/// Collapse overlapping process trees and report all listeners an action affects.
pub fn group_targets(
    selected: Vec<Server>,
    all: &[Server],
    processes: &BTreeMap<u32, ProcessInfo>,
) -> Vec<Server> {
    let descendant = |mut pid: u32, root: &ProcessInfo| {
        let mut visited = BTreeSet::new();
        while visited.insert(pid) {
            if pid == root.pid {
                return true;
            }
            let Some(process) = processes
                .get(&pid)
                .filter(|process| process.uid == root.uid)
            else {
                return false;
            };
            let Some(parent) = process.parent else {
                return false;
            };
            pid = parent;
        }
        false
    };
    let mut groups = BTreeMap::new();
    for server in selected {
        groups.entry(server.launcher.pid).or_insert(server);
    }
    let roots: Vec<_> = groups
        .values()
        .map(|server| server.launcher.clone())
        .collect();
    groups.retain(|pid, _| {
        !roots
            .iter()
            .any(|root| root.pid != *pid && descendant(*pid, root))
    });
    for server in groups.values_mut() {
        server.listeners = all
            .iter()
            .filter(|other| {
                other.launcher.pid == server.launcher.pid
                    || descendant(other.process.pid, &server.launcher)
            })
            .flat_map(|other| other.listeners.clone())
            .collect();
        server.listeners.sort();
        server.listeners.dedup();
    }
    groups.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Listener, ProcessInfo};

    fn server(pid: u32, port: u16) -> Server {
        let process = ProcessInfo {
            pid,
            parent: None,
            uid: 501,
            name: "node".into(),
            executable: None,
            command: vec![],
            directory: Some("/work/app".into()),
            started_at: 1,
        };
        Server {
            launcher: process.clone(),
            process,
            project: "app".into(),
            listeners: vec![Listener {
                address: "*".into(),
                port,
            }],
            development: true,
        }
    }

    #[test]
    fn bulk_selection_collapses_descendants_and_preview_includes_child_ports() {
        let parent = server(40, 3000);
        let mut child = server(42, 8000);
        child.process.parent = Some(41);
        child.launcher = child.process.clone();
        let mut bridge = server(41, 0).process;
        bridge.parent = Some(40);
        let processes = [parent.process.clone(), bridge, child.process.clone()]
            .into_iter()
            .map(|process| (process.pid, process))
            .collect();
        let all = vec![parent.clone(), child];
        for selected in [vec![parent], all.clone()] {
            let grouped = group_targets(selected, &all, &processes);
            assert_eq!(grouped.len(), 1);
            assert_eq!(grouped[0].launcher.pid, 40);
            assert_eq!(
                grouped[0]
                    .listeners
                    .iter()
                    .map(|listener| listener.port)
                    .collect::<Vec<_>>(),
                [3000, 8000]
            );
        }
    }
}
