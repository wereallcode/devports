# Manual Installation

This guide covers manual installation, updates and removal.

## 1. Requirements

- `macOS 11` or later on a Mac with Apple silicon.
- `Rust stable` through [rustup](https://rustup.rs/).
- `Xcode Command Line Tools`, `Git` and `make`.

Install missing command line tools with `xcode-select --install`.

### 1.1. Compatibility

- The build and automated checks passed with `Rust 1.98.1` on `macOS 27.2.1` (`Apple Silicon`).

## 2. Install

Open a new terminal and follow these steps.

```bash
git clone https://github.com/wereallcode/devports.git "$HOME/.devports"
```

```bash
cd "$HOME/.devports" && make build
```

Run `devports` with this command.

```bash
./dist/devports
```

### 2.1. Add to `PATH`

Follow these steps to add `devports` to your `PATH` and run it from any directory.

```zsh
vi ~/.zshrc
```

Add this line at the end of the file and save it.

```zsh
export PATH="$HOME/.devports/dist:$PATH"
```

Apply the change.

```zsh
source ~/.zshrc
```

Run `devports`. 🥳

```zsh
devports
```

## 3. Update

Follow these steps to update your local copy and rebuild.

```bash
cd "$HOME/.devports" && git pull --ff-only
```

```bash
make build
```

## 4. Uninstall

1. Remove the `PATH` line from your shell startup file.
2. Open a new terminal.
3. Delete `$HOME/.devports` if you no longer need the source.
