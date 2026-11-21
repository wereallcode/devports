use std::{collections::BTreeSet, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub started_at: u64,
    pub uid: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent: Option<u32>,
    pub uid: u32,
    pub name: String,
    pub executable: Option<PathBuf>,
    pub command: Vec<String>,
    pub directory: Option<PathBuf>,
    pub started_at: u64,
}

impl ProcessInfo {
    pub fn identity(&self) -> ProcessIdentity {
        ProcessIdentity {
            pid: self.pid,
            started_at: self.started_at,
            uid: self.uid,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Listener {
    pub address: String,
    pub port: u16,
}

impl Listener {
    pub fn url(&self) -> String {
        let host = match self.address.as_str() {
            "*" | "0.0.0.0" => "127.0.0.1".to_owned(),
            "::" | "[::]" => "[::1]".to_owned(),
            value if value.contains(':') && !value.starts_with('[') => format!("[{value}]"),
            value => value.to_owned(),
        };
        format!("http://{host}:{}", self.port)
    }
}

#[derive(Clone, Debug)]
pub struct Server {
    pub process: ProcessInfo,
    pub launcher: ProcessInfo,
    pub project: String,
    pub listeners: Vec<Listener>,
    pub development: bool,
}

impl Server {
    pub fn ports(&self) -> BTreeSet<u16> {
        self.listeners
            .iter()
            .map(|listener| listener.port)
            .collect()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub servers: Vec<Server>,
}
