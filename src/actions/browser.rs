use crate::model::Server;
use std::process::{Command, Stdio};

pub fn open(server: &Server, port: Option<u16>) -> Result<String, String> {
    let url = browser_url(server, port)?;
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    let output = Command::new(opener)
        .arg(&url)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("Cannot launch {opener}: {error}. Open {url} manually."))?;
    if !output.status.success() {
        return Err(format!(
            "{opener} failed with {}. {} Open {url} manually.",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(url)
}

pub fn browser_url(server: &Server, port: Option<u16>) -> Result<String, String> {
    let available = server.ports();
    let port = match port {
        Some(port) if available.contains(&port) => port,
        Some(port) => return Err(format!("This server is not listening on port {port}.")),
        // Dev tools often add HMR or inspector ports; the lowest is the app.
        None => *available
            .first()
            .ok_or("This server is not listening on any port.")?,
    };
    let listener = server
        .listeners
        .iter()
        .filter(|listener| listener.port == port)
        .min_by_key(|listener| match listener.address.as_str() {
            "127.0.0.1" | "::1" | "[::1]" => 0,
            "*" | "0.0.0.0" | "::" | "[::]" => 1,
            _ => 2,
        })
        .expect("selected port exists");
    Ok(listener.url())
}

#[cfg(test)]
mod tests {
    use super::browser_url;
    use crate::model::{Listener, ProcessInfo, Server};

    #[test]
    fn several_ports_open_the_lowest_one() {
        let process = ProcessInfo {
            pid: 1,
            parent: None,
            uid: 501,
            name: "node".into(),
            executable: None,
            command: vec![],
            directory: None,
            started_at: 1,
        };
        let listener = |port| Listener {
            address: "127.0.0.1".into(),
            port,
        };
        let server = Server {
            launcher: process.clone(),
            process,
            project: "web".into(),
            listeners: vec![listener(24678), listener(5173)],
            development: true,
        };
        assert_eq!(browser_url(&server, None).unwrap(), "http://127.0.0.1:5173");
    }
}
