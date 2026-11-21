use crate::{
    actions::{self, StopOptions},
    discovery, list,
    model::{ProcessIdentity, Server},
    selection,
    terminal::{Key, Terminal},
};
use std::{
    io::{self, IsTerminal},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const INPUT_WAIT: Duration = Duration::from_millis(150);
const KILL_GRACE: Duration = Duration::from_secs(3);
const STATUS_DURATION: Duration = Duration::from_secs(4);

pub enum Effect {
    None,
    Quit,
    Open(Server),
    Kill(Server),
}

#[derive(Default)]
pub struct App {
    servers: Vec<Server>,
    selected: Option<ProcessIdentity>,
    index: usize,
    status: Option<(String, Instant)>,
}

impl App {
    pub fn refresh(&mut self, servers: Vec<Server>) {
        self.servers = servers;
        let visible = list::sorted(&self.servers);
        let index = self
            .selected
            .and_then(|id| {
                visible
                    .iter()
                    .position(|server| server.process.identity() == id)
            })
            .unwrap_or(self.index)
            .min(visible.len().saturating_sub(1));
        self.select(index);
    }

    fn select(&mut self, index: usize) {
        let visible = list::sorted(&self.servers);
        self.index = index;
        self.selected = visible.get(index).map(|server| server.process.identity());
    }

    pub fn apply_scan(&mut self, scan: Result<crate::model::Snapshot, String>) {
        match scan {
            Ok(snapshot) => self.refresh(snapshot.servers),
            Err(error) => self.set_status(format!("Error: Could not refresh: {error}")),
        }
    }

    pub fn is_empty(&self) -> bool {
        list::sorted(&self.servers).is_empty()
    }

    pub fn set_status(&mut self, status: String) {
        self.status = Some((status, Instant::now()));
    }

    pub fn expire_status(&mut self, now: Instant) -> bool {
        let expired = self
            .status
            .as_ref()
            .is_some_and(|(_, shown)| now.duration_since(*shown) >= STATUS_DURATION);
        if expired {
            self.status = None;
        }
        expired
    }

    pub fn key(&mut self, key: Key) -> Effect {
        let count = list::sorted(&self.servers).len();
        match key {
            Key::Quit | Key::Escape | Key::Char('q' | 'Q') => return Effect::Quit,
            _ if count == 0 => {}
            Key::Up => self.select((self.index + count - 1) % count),
            Key::Down => self.select((self.index + 1) % count),
            Key::Enter => return Effect::Open(self.current()),
            Key::Char('k' | 'K') => return Effect::Kill(self.current()),
            _ => {}
        }
        Effect::None
    }

    fn current(&self) -> Server {
        list::sorted(&self.servers)[self.index].clone()
    }

    pub fn frame(&self, width: usize, height: usize, color: bool, now: u64) -> String {
        let visible = list::sorted(&self.servers);
        let list = list::render(
            &visible,
            Some(self.index),
            self.status.as_ref().map(|(status, _)| status.as_str()),
            width,
            color,
            now,
            // Blank lines, heading, count and footer take 10 rows; a status 2 more.
            height
                .saturating_sub(10 + if self.status.is_some() { 2 } else { 0 })
                .max(1),
        );
        format!("\n{list}\n")
    }
}

pub fn run() -> Result<(), String> {
    let mut app = App::default();
    app.refresh(discovery::scan()?.servers);
    let interactive = io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var_os("TERM").is_none_or(|term| term != "dumb");
    let color = interactive && std::env::var_os("NO_COLOR").is_none();
    if !interactive || app.is_empty() {
        let visible = list::sorted(&app.servers);
        let list = list::render(&visible, None, None, 1000, color, now(), usize::MAX);
        if interactive {
            println!("\n{list}\n");
        } else {
            println!("{list}");
        }
        return Ok(());
    }
    let mut terminal = Terminal::enter()?;
    let mut last_refresh = Instant::now();
    let mut dirty = true;
    let mut last_size = (0, 0);
    loop {
        let size = terminal.size();
        dirty |= app.expire_status(Instant::now());
        if dirty || size != last_size {
            terminal.draw(&app.frame(size.0, size.1, color, now()))?;
            last_size = size;
            dirty = false;
        }
        if let Some(key) = terminal.read_key(INPUT_WAIT)? {
            match app.key(key) {
                Effect::None => {
                    // Selection changes must show at once, not on the next refresh.
                    dirty = true;
                    continue;
                }
                Effect::Quit => return Ok(()),
                Effect::Open(server) => {
                    app.set_status(match actions::open(&server, None) {
                        Ok(url) => format!("Opened {url}"),
                        Err(error) => format!("Error: {error}"),
                    });
                    dirty = true;
                    continue;
                }
                Effect::Kill(server) => {
                    let label = format!("{} {}", list::ports(&server), server.project);
                    app.set_status(match kill(&server, &app.servers) {
                        Ok(()) => format!("✓ Killed {label}"),
                        Err(error) => format!("Error: {error}"),
                    });
                }
            }
        } else if last_refresh.elapsed() < REFRESH_INTERVAL {
            continue;
        }
        app.apply_scan(discovery::scan());
        last_refresh = Instant::now();
        dirty = true;
        if app.is_empty() {
            let size = terminal.size();
            terminal.draw(&app.frame(size.0, size.1, color, now()))?;
            return Ok(());
        }
    }
}

fn kill(server: &Server, all: &[Server]) -> Result<(), String> {
    let options = StopOptions {
        force: true,
        timeout: KILL_GRACE,
    };
    let targets = selection::group_targets(vec![server.clone()], all, &discovery::processes());
    targets
        .iter()
        .try_for_each(|target| actions::stop(target, &options))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Listener, ProcessInfo};

    fn server(pid: u32, started_at: u64) -> Server {
        let process = ProcessInfo {
            pid,
            parent: None,
            uid: 501,
            name: "node".into(),
            executable: None,
            command: vec!["node".into()],
            directory: Some(format!("/work/{pid}").into()),
            started_at,
        };
        Server {
            launcher: process.clone(),
            process,
            project: format!("web-{pid}"),
            listeners: vec![Listener {
                address: "127.0.0.1".into(),
                port: 3000 + pid as u16,
            }],
            development: true,
        }
    }

    fn app() -> App {
        let mut app = App::default();
        app.refresh(vec![server(1, 10), server(2, 20), server(3, 30)]);
        app
    }

    fn selected_pid(app: &mut App) -> u32 {
        match app.key(Key::Enter) {
            Effect::Open(server) => server.process.pid,
            _ => panic!("expected open"),
        }
    }

    #[test]
    fn arrows_move_and_wrap() {
        let mut app = app();
        assert_eq!(selected_pid(&mut app), 3);
        app.key(Key::Down);
        assert_eq!(selected_pid(&mut app), 2);
        app.key(Key::Up);
        app.key(Key::Up);
        assert_eq!(selected_pid(&mut app), 1);
        app.key(Key::Down);
        assert_eq!(selected_pid(&mut app), 3);
    }

    #[test]
    fn enter_opens_selected() {
        let mut app = app();
        app.key(Key::Down);
        assert!(matches!(app.key(Key::Enter), Effect::Open(s) if s.process.pid == 2));
    }

    #[test]
    fn k_kills_selected() {
        let mut app = app();
        assert!(matches!(app.key(Key::Char('k')), Effect::Kill(s) if s.process.pid == 3));
        assert!(matches!(app.key(Key::Char('K')), Effect::Kill(_)));
    }

    #[test]
    fn q_esc_quit() {
        for key in [Key::Char('q'), Key::Char('Q'), Key::Escape, Key::Quit] {
            assert!(matches!(app().key(key), Effect::Quit));
        }
    }

    #[test]
    fn selection_follows_process_across_refresh() {
        let mut app = app();
        app.key(Key::Down);
        app.refresh(vec![
            server(4, 40),
            server(1, 10),
            server(2, 20),
            server(3, 30),
        ]);
        assert_eq!(selected_pid(&mut app), 2);
    }

    #[test]
    fn selection_clamps_when_selected_server_disappears() {
        let mut app = app();
        app.key(Key::Up);
        app.refresh(vec![server(2, 20), server(3, 30)]);
        assert_eq!(selected_pid(&mut app), 2);
        app.refresh(vec![]);
        assert!(matches!(app.key(Key::Enter), Effect::None));
        assert!(
            app.frame(80, 40, false, 100)
                .contains("No development servers running.")
        );
    }

    #[test]
    fn j_is_ignored() {
        let mut app = app();
        assert!(matches!(app.key(Key::Char('j')), Effect::None));
        assert_eq!(selected_pid(&mut app), 3);
    }

    #[test]
    fn keys_do_nothing_when_empty() {
        let mut app = App::default();
        app.refresh(vec![]);
        for key in [Key::Up, Key::Down, Key::Enter, Key::Char('k')] {
            assert!(matches!(app.key(key), Effect::None));
        }
    }

    #[test]
    fn failed_refresh_keeps_the_list_and_reports_it() {
        let mut app = app();
        app.apply_scan(Err("lsof failed".into()));
        assert_eq!(selected_pid(&mut app), 3);
        assert!(
            app.frame(100, 40, false, 100)
                .contains("Error: Could not refresh: lsof failed")
        );
    }

    #[test]
    fn status_disappears_after_a_few_seconds() {
        let mut app = app();
        app.set_status("✓ Killed :3001 web-1".into());
        assert!(!app.expire_status(Instant::now()));
        assert!(app.expire_status(Instant::now() + STATUS_DURATION));
        assert!(!app.frame(100, 40, false, 100).contains("Killed"));
        assert!(!app.expire_status(Instant::now() + STATUS_DURATION));
    }

    #[test]
    fn frame_shows_status_and_selection() {
        let mut app = app();
        app.set_status("✓ Killed :3001 web-1".into());
        let frame = app.frame(100, 40, false, 100);
        assert!(frame.contains("→ :3003"), "{frame}");
        assert!(
            frame.starts_with("\ndevports") && frame.ends_with("q Quit\n"),
            "{frame:?}"
        );
        assert!(frame.contains("✓ Killed :3001 web-1"), "{frame}");
    }
}
