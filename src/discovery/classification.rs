use crate::model::ProcessInfo;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

fn executable_name(process: &ProcessInfo) -> String {
    process
        .executable
        .as_deref()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| process.name.to_lowercase())
}

fn is_app_process(process: &ProcessInfo) -> bool {
    if executable_name(process) == "python"
        && process.executable.as_deref().is_some_and(|path| {
            path.components()
                .any(|part| part.as_os_str() == "Python.app")
        })
    {
        return false;
    }
    process.executable.as_deref().is_some_and(|path| {
        path.components()
            .any(|part| part.as_os_str().to_string_lossy().ends_with(".app"))
    })
}

fn is_agent(process: &ProcessInfo) -> bool {
    let name = executable_name(process);
    matches!(name.as_str(), "codex" | "claude" | "gemini" | "opencode")
        || process.command.iter().take(2).any(|arg| {
            arg.contains("/codex/")
                || arg.contains("/@openai/codex/")
                || arg.contains("/@anthropic-ai/claude-code/")
                || arg.contains("/@google/gemini-cli/")
        })
}

fn is_runner(process: &ProcessInfo) -> bool {
    if is_app_process(process) || is_agent(process) {
        return false;
    }
    let name = executable_name(process);
    let title = process.command.first().map(String::as_str).unwrap_or("");
    if ["npm ", "pnpm ", "yarn "]
        .iter()
        .any(|prefix| title.starts_with(prefix) || process.name.starts_with(prefix))
    {
        return true;
    }
    if matches!(
        name.as_str(),
        "npm"
            | "pnpm"
            | "yarn"
            | "nodemon"
            | "cargo-watch"
            | "watchexec"
            | "air"
            | "reflex"
            | "rerun"
    ) {
        return true;
    }
    if matches!(name.as_str(), "cargo" | "go" | "uv" | "bun" | "deno") {
        return process
            .command
            .iter()
            .skip(1)
            .any(|arg| matches!(arg.as_str(), "run" | "watch" | "serve" | "task"));
    }
    if matches!(name.as_str(), "node" | "nodejs") {
        return process.command.iter().skip(1).any(|arg| {
            matches!(
                arg.as_str(),
                "--watch" | "--watch-preserve-output" | "dev" | "serve" | "watch"
            ) || arg.starts_with("--watch=")
                || Path::new(arg).file_name().is_some_and(|base| {
                    matches!(
                        base.to_str(),
                        Some(
                            "npm-cli.js"
                                | "pnpm.cjs"
                                | "pnpm.js"
                                | "yarn.js"
                                | "yarn.cjs"
                                | "nodemon.js"
                        )
                    )
                })
        });
    }
    false
}

fn is_shell_wrapper(process: &ProcessInfo) -> bool {
    matches!(
        executable_name(process).as_str(),
        "sh" | "dash" | "bash" | "zsh"
    ) && process.command.get(1).is_some_and(|arg| arg == "-c")
}

fn same_context(child: &ProcessInfo, parent: &ProcessInfo) -> bool {
    child.uid == parent.uid
        && child
            .directory
            .as_ref()
            .zip(parent.directory.as_ref())
            .is_some_and(|(child, parent)| {
                child == parent || (child.starts_with(parent) && project_root(parent).is_some())
            })
        && parent.pid > 1
        && parent.pid != std::process::id()
}

pub(super) fn find_launcher<'a>(
    process: &'a ProcessInfo,
    processes: &'a BTreeMap<u32, ProcessInfo>,
) -> &'a ProcessInfo {
    let mut launcher = process;
    let mut visited = BTreeSet::from([process.pid]);
    for _ in 0..16 {
        let Some(parent) = launcher.parent.and_then(|pid| processes.get(&pid)) else {
            break;
        };
        if !visited.insert(parent.pid) || !same_context(launcher, parent) {
            break;
        }
        if is_runner(parent) {
            launcher = parent;
            continue;
        }
        if is_shell_wrapper(parent)
            && let Some(grandparent) = parent.parent.and_then(|pid| processes.get(&pid))
            && same_context(parent, grandparent)
            && is_runner(grandparent)
            && visited.insert(grandparent.pid)
        {
            launcher = grandparent;
            continue;
        }
        break;
    }
    launcher
}

pub(super) fn project_root(directory: &Path) -> Option<PathBuf> {
    const MARKERS: &[&str] = &[
        "package.json",
        "Cargo.toml",
        "go.mod",
        "pyproject.toml",
        "requirements.txt",
        "Pipfile",
        "Gemfile",
        "composer.json",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "mix.exs",
        "deno.json",
        "deno.jsonc",
        ".git",
    ];
    directory
        .ancestors()
        .take(12)
        .find(|path| MARKERS.iter().any(|marker| path.join(marker).exists()))
        .map(Path::to_path_buf)
}

pub(super) fn classify(process: &ProcessInfo, launcher: &ProcessInfo) -> bool {
    if is_app_process(process) || is_agent(process) {
        return false;
    }
    if launcher.pid != process.pid || is_runner(process) {
        return true;
    }
    let name = executable_name(process);
    if matches!(
        name.as_str(),
        "node"
            | "nodejs"
            | "bun"
            | "deno"
            | "ruby"
            | "php"
            | "java"
            | "dotnet"
            | "beam.smp"
            | "elixir"
            | "erl"
            | "julia"
    ) || name == "python"
        || name.strip_prefix("python").is_some_and(|version| {
            !version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
    {
        return true;
    }
    if let (Some(directory), Some(executable)) = (&process.directory, &process.executable)
        && let Some(root) = project_root(directory)
        && executable.starts_with(&root)
    {
        return true;
    }
    false
}
