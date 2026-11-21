use crate::{
    model::Server,
    text::{clipped, escape_controls as clean},
};
use std::path::Path;
use unicode_width::UnicodeWidthStr;

const HEADING: &str = "devports — A CLI to manage ports held by development servers.";
const KEYS: [(&str, &str); 4] = [
    ("↑↓", "Select"),
    ("Enter", "Open"),
    ("k", "Kill"),
    ("q", "Quit"),
];
const GAP: &str = "  ";

pub fn sorted(servers: &[Server]) -> Vec<&Server> {
    let mut servers = servers
        .iter()
        .filter(|server| server.development)
        .collect::<Vec<_>>();
    servers.sort_by_key(|server| std::cmp::Reverse(server.process.started_at));
    servers
}

pub fn age(start: u64, now: u64) -> String {
    if start == 0 {
        return "?".into();
    }
    let seconds = now.saturating_sub(start);
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}

/// `selected: None` is the plain output for pipes and agents.
pub fn render(
    servers: &[&Server],
    selected: Option<usize>,
    status: Option<&str>,
    width: usize,
    color: bool,
    now: u64,
    rows: usize,
) -> String {
    let budget = width.saturating_sub(1).max(1);
    let line = |text: &str| clipped(text, budget);
    let mut lines = vec![styled(&line(HEADING), "1", color), String::new()];
    if servers.is_empty() {
        lines.push(styled(
            &line("→ No development servers running."),
            "2",
            color,
        ));
    } else {
        lines.push(styled(
            &line(&format!("{} Running (sorted newest first)", servers.len())),
            "2",
            color,
        ));
        lines.push(String::new());
        let cells = servers
            .iter()
            .map(|server| {
                [
                    ports(server),
                    clean(&server.project),
                    clean(&directory(server.process.directory.as_deref())),
                    age(server.process.started_at, now),
                ]
            })
            .collect::<Vec<_>>();
        let mut widths = [0; 4];
        for row in &cells {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.width());
            }
        }
        // The directory gives way first, then the name, so ports and ages stay readable.
        let room = budget.saturating_sub(2 + widths[0] + widths[3] + GAP.len() * 2);
        widths[2] = if room >= widths[1] + GAP.len() + 8 {
            widths[2].min(room - widths[1] - GAP.len())
        } else {
            0
        };
        let used = if widths[2] > 0 {
            widths[2] + GAP.len()
        } else {
            0
        };
        widths[1] = widths[1].min(room.saturating_sub(used).max(1));
        let shown = if cells.len() > rows {
            rows.saturating_sub(1).max(1)
        } else {
            cells.len()
        };
        let first = selected
            .unwrap_or(0)
            .saturating_sub(shown / 2)
            .min(cells.len() - shown);
        for (index, row) in cells.iter().enumerate().skip(first).take(shown) {
            let marker = match selected {
                None => "→ ",
                Some(selected) if selected == index => "→ ",
                Some(_) => "  ",
            };
            let directory = if widths[2] > 0 {
                format!("{}{GAP}", pad(&clipped(&row[2], widths[2]), widths[2]))
            } else {
                String::new()
            };
            let text = format!(
                "{marker}{}{GAP}{}{GAP}{directory}{}",
                pad(&row[0], widths[0]),
                pad(&clipped(&row[1], widths[1]), widths[1]),
                row[3]
            );
            let text = line(&text);
            lines.push(if selected == Some(index) {
                styled(&text, "1", color)
            } else {
                text
            });
        }
        let (above, below) = (first, cells.len() - first - shown);
        if above + below > 0 {
            let hint = match (above, below) {
                (0, below) => format!("  ↓ {below} more"),
                (above, 0) => format!("  ↑ {above} more"),
                (above, below) => format!("  ↑ {above} more · ↓ {below} more"),
            };
            lines.push(styled(&line(&hint), "2", color));
        }
    }
    if let Some(status) = status.filter(|status| !status.is_empty()) {
        lines.push(String::new());
        let code = if status.starts_with("Error:") {
            "31"
        } else {
            "32"
        };
        lines.push(styled(&line(&clean(status)), code, color));
    }
    if selected.is_some() && !servers.is_empty() {
        lines.push(String::new());
        lines.push(footer(budget, color));
    }
    lines.join("\n")
}

fn footer(budget: usize, color: bool) -> String {
    let plain = KEYS
        .iter()
        .map(|(key, label)| format!("{key} {label}"))
        .collect::<Vec<_>>()
        .join("   ");
    let buttons = KEYS
        .iter()
        .map(|(key, label)| format!(" {key}  {label}"))
        .collect::<Vec<_>>()
        .join("   ");
    if !color || buttons.width() > budget {
        return styled(&clipped(&plain, budget), "2", color);
    }
    KEYS.iter()
        .map(|(key, label)| format!("{} {label}", styled(&format!(" {key} "), "7", true)))
        .collect::<Vec<_>>()
        .join("   ")
}

pub fn ports(server: &Server) -> String {
    server
        .ports()
        .iter()
        .map(|port| format!(":{port}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn directory(path: Option<&Path>) -> String {
    let Some(path) = path else {
        return "unknown".into();
    };
    if let Some(home) = std::env::var_os("HOME")
        && let Ok(relative) = path.strip_prefix(home)
    {
        return format!("~/{}", relative.display());
    }
    path.display().to_string()
}

fn pad(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(text.width())))
}

fn styled(text: &str, code: &str, color: bool) -> String {
    if color && !text.is_empty() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Listener, ProcessInfo};
    use unicode_width::UnicodeWidthStr;

    const NOW: u64 = 1_000_000;

    fn server(pid: u32, port: u16, project: &str, started_at: u64, development: bool) -> Server {
        let process = ProcessInfo {
            pid,
            parent: None,
            uid: 501,
            name: "node".into(),
            executable: None,
            command: vec!["node".into()],
            directory: Some(format!("/work/{project}").into()),
            started_at,
        };
        Server {
            launcher: process.clone(),
            process,
            project: project.into(),
            listeners: vec![Listener {
                address: "127.0.0.1".into(),
                port,
            }],
            development,
        }
    }

    fn servers() -> Vec<Server> {
        vec![
            server(1, 3000, "old-web", NOW - 3 * 3600, true),
            server(2, 5173, "shop-web", NOW - 120, true),
            server(3, 9000, "docker", NOW - 60, false),
        ]
    }

    #[test]
    fn newest_first_and_dev_only() {
        let all = servers();
        let ports = sorted(&all)
            .iter()
            .map(|server| server.listeners[0].port)
            .collect::<Vec<_>>();
        assert_eq!(ports, [5173, 3000]);
    }

    #[test]
    fn selected_row_has_arrow_and_footer() {
        let all = servers();
        let output = render(&sorted(&all), Some(1), None, 100, false, NOW, 100);
        let lines = output.lines().collect::<Vec<_>>();
        assert_eq!(
            lines[0],
            "devports — A CLI to manage ports held by development servers."
        );
        assert!(
            output.contains("2 Running (sorted newest first)"),
            "{output}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("  :5173") && l.contains("2m"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("→ :3000") && l.contains("3h"))
        );
        assert_eq!(
            *lines.last().unwrap(),
            "↑↓ Select   Enter Open   k Kill   q Quit"
        );
    }

    #[test]
    fn plain_output_has_no_selector_or_footer() {
        let all = servers();
        let output = render(&sorted(&all), None, None, 100, false, NOW, 100);
        assert!(!output.contains("Select"), "{output}");
        assert_eq!(output.matches("→ :").count(), 2, "{output}");
    }

    #[test]
    fn columns_align_and_clip_to_width() {
        let mut all = servers();
        all[0].process.directory = Some(format!("/work/{}", "deep/".repeat(20)).into());
        let output = render(&sorted(&all), Some(0), None, 100, false, NOW, 100);
        let rows = output
            .lines()
            .filter(|l| l.contains(" :"))
            .collect::<Vec<_>>();
        let column = |row: &str| row[..row.find("/work").unwrap()].chars().count();
        assert_eq!(column(rows[0]), column(rows[1]), "{output}");
        let narrow = render(&sorted(&all), Some(0), None, 40, false, NOW, 100);
        assert!(narrow.lines().all(|line| line.width() <= 39), "{narrow}");
        assert!(narrow.contains('…'), "{narrow}");
    }

    #[test]
    fn long_lists_keep_the_selection_and_footer_visible() {
        let all = (0..8)
            .map(|index| {
                server(
                    index + 10,
                    4000 + index as u16,
                    "web",
                    NOW - u64::from(index),
                    true,
                )
            })
            .collect::<Vec<_>>();
        let output = render(
            &sorted(&all),
            Some(6),
            Some("✓ Killed :1 x"),
            100,
            false,
            NOW,
            3,
        );
        let rows = output.lines().filter(|line| line.contains(":40")).count();
        assert!(rows <= 3, "{output}");
        assert!(output.contains("→ :4006"), "{output}");
        assert!(output.contains("more"), "{output}");
        assert!(
            output.contains("✓ Killed") && output.contains("q Quit"),
            "{output}"
        );
    }

    #[test]
    fn narrow_rows_keep_the_age() {
        let all = servers();
        let output = render(&sorted(&all), Some(0), None, 30, false, NOW, 10);
        assert!(output.contains("shop-web"), "{output}");
        assert!(
            output
                .lines()
                .any(|l| l.contains(":5173") && l.contains("2m")),
            "{output}"
        );
        assert!(output.lines().all(|line| line.width() <= 29), "{output}");
    }

    #[test]
    fn empty_list_message() {
        let output = render(&[], None, None, 80, false, NOW, 100);
        assert!(
            output.contains("→ No development servers running."),
            "{output}"
        );
        assert!(!output.contains("Running ("), "{output}");
    }

    #[test]
    fn ages() {
        assert_eq!(age(100, 145), "45s");
        assert_eq!(age(100, 220), "2m");
        assert_eq!(age(1, 1 + 3 * 3600), "3h");
        assert_eq!(age(1, 1 + 2 * 86400), "2d");
        assert_eq!(age(0, 9), "?");
    }

    #[test]
    fn status_line_follows_list() {
        let all = servers();
        let output = render(
            &sorted(&all),
            Some(0),
            Some("✓ Killed :3000 old-web"),
            100,
            false,
            NOW,
            100,
        );
        let status = output.find("✓ Killed").unwrap();
        assert!(status > output.find(":3000").unwrap());
        assert!(status < output.find("↑↓ Select").unwrap());
    }

    #[test]
    fn no_color_has_no_escapes() {
        let all = servers();
        let plain = render(
            &sorted(&all),
            Some(0),
            Some("Error: nope"),
            100,
            false,
            NOW,
            100,
        );
        let colored = render(
            &sorted(&all),
            Some(0),
            Some("Error: nope"),
            100,
            true,
            NOW,
            100,
        );
        assert!(!plain.contains('\x1b'));
        assert!(colored.contains("\x1b[1m"));
        assert!(
            colored.contains("\x1b[7m Enter \x1b[0m Open"),
            "{colored:?}"
        );
        let narrow = render(&sorted(&all), Some(0), None, 30, true, NOW, 10);
        assert!(!narrow.contains("\x1b[7m"), "{narrow:?}");
    }
}
