# devports

A CLI to manage ports held by development servers.

## Why?

See which ports your development servers still hold, open or kill one with a single key.

## Installation

### Requirements

- `macOS 11` or later on a Mac with Apple silicon.

### Quick Start

Run this command in your terminal to download and install the latest release.

```bash
curl -fsSL https://raw.githubusercontent.com/wereallcode/devports/master/scripts/install.sh | bash
```

Open a new terminal and run `devports`.

_If `devports` is not found, follow the installer's instructions to add `$HOME/.local/bin` to your shell's `PATH`._

#### Update

Run the installation command again to update to the latest release.

#### Uninstall

To uninstall the release installation, run:

```bash
curl -fsSL https://raw.githubusercontent.com/wereallcode/devports/master/scripts/install.sh | bash -s -- --uninstall
```

This removes devports' installed files and command link; running servers and shell configuration are preserved.

## Guides

- [Manual Installation](docs/guides/Manual-Installation.md)
