> [!IMPORTANT]
> Acknowledgement: This program has been written with the help of AI

# Spur

A fast, native Git client for Windows that keeps an eye on all of your repositories at once,
whether they live on NTFS or inside WSL.

![Spur showing a repository's history](assets/screenshot.png)

Spur is built for people who work across many repositories. Point it at one or more folders
and it finds every checkout below them, shows what changed, what's ahead or behind, and what
can be safely fast-forwarded. From there, you can do the everyday work (staging, committing,
pushing, stashing, resolving conflicts) without switching tools.

Repositories inside a WSL distribution are handled by the Git inside that distribution. Windows
repositories use Git for Windows. The UI is a single native Windows executable.

> Spur is in early development. It is usable day to day, but expect rough edges and breaking
> changes to the settings format.

## Features

**Workspace**
- Scan any number of root folders (Windows or WSL paths) and discover every repository and
  linked worktree underneath. Common build and dependency folders (`node_modules`, `target`,
  `.venv`, …) are skipped.
- Live status for every repository with bounded background refresh. Changes made by other
  tools show up without clicking anything.
- Fetch one or all remotes, and pull only the repositories that can be fast-forwarded safely.
- Command palette (<kbd>Ctrl</kbd> <kbd>K</kbd>) for opening repositories, filtering,
  sorting, and running actions across a selection.
- Open a repository in your editor or another Git client through a configurable external command.

**Repository**
- Commit history with a branch graph, commit details, and per-file diffs.
- Local Changes: stage, unstage, or discard files, or individual hunks straight from the diff.
- Commit with hooks and signing intact. A rejected commit keeps your message.
- Push, fetch, and pull dialogs, plus branch, tag, and remote management.
- Stashes: create (discard / keep index / keep all), inspect, apply, and drop.
- A conflict resolver with Ours / Theirs / Both per conflict.
- Reset, and apply a patch straight from the clipboard.
- An undo safety net: destructive operations keep a backup, and <kbd>Ctrl</kbd> <kbd>Alt</kbd>
  <kbd>Z</kbd> reverts the last one.
- An operation log with the full output of every Git command that ran.

**App**
- Tabs for open repositories.
- Built-in dark and light themes, plus your own.
- Remappable keyboard shortcuts.
- Large repositories stay responsive: lists are virtualized and diffs are cached.

## Requirements

- Windows 10 or 11, or macOS (experimental). On macOS Spur runs the `git` on your `PATH`
  (Apple's or Homebrew's) and there is no WSL; paths are plain macOS paths.
- [Git for Windows](https://git-scm.com/download/win) on `PATH`, for repositories on Windows drives
- Optional: WSL 2 with a distribution that has `git` installed, for repositories inside WSL

Without WSL, Spur works with Windows repositories only, and Linux paths are reported as
unavailable. Spur uses your default WSL distribution; set `SPUR_WSL_DISTRO` to use
another one.

## Building

You need the stable Rust toolchain (MSVC) and the Visual Studio C++ Build Tools.

```powershell
git clone https://github.com/press-again/SpurGit.git
cd SpurGit
cargo build --release
```

The executable is `target\release\spurgit.exe`.

On macOS you need the stable Rust toolchain and the Xcode Command Line Tools
(`xcode-select --install`). `packaging/macos/bundle.sh` builds `target/release/Spur.app` (ad-hoc
signed, with its icon) and a zip of it; `cargo build --release` alone gives the bare
`target/release/spurgit`.

The released app is not notarized. After unzipping it into `/Applications`, open it once with
right-click > Open, or clear the download quarantine flag:

```sh
xattr -dr com.apple.quarantine /Applications/Spur.app
```

Started from Finder or the Dock, Spur looks for Git in Homebrew's directories as well as the
system ones, and opens the folder picker on first run instead of scanning anything.

## Usage

Start Spur and press <kbd>Ctrl</kbd> <kbd>K</kbd> to add root folders and open repositories.
Roots are saved, so the next start picks up where you left off.

You can also pass roots on the command line:

```powershell
spurgit --root=C:\dev --root=/home/me/src
```

| Option | Description |
| --- | --- |
| `--root=<path>` | Folder to scan for repositories. Repeatable; overrides the saved roots. |
| `--exclude=<path>` | Folder to skip during discovery. Repeatable. |
| `--repo=<path>` | Open a repository as a tab on launch. Repeatable. |
| `--desktop` | For shortcuts: when no roots are configured, open the root selection instead of scanning the current directory. |
| `--size=<W>x<H>` | Initial window size (default `1280x800`). |
| `--maximized` | Start with the window maximized. |

If no roots are given or saved, Spur uses `GIS_PATH` (colon-separated Linux paths) if it is
set, and otherwise the current directory.

## Safe pulls

Fetch runs first; a pull then updates only the repositories that can be fast-forwarded
safely. Every other repository is skipped with a visible reason. A repository is eligible
only when all of the following hold:

- It has a checked-out branch with a configured upstream that exists locally.
- It is behind the upstream, and neither ahead nor diverged.
- The index and worktree are clean, including untracked files.
- No merge, rebase, cherry-pick, or revert is in progress.
- The fetch in the same operation succeeded.

Integration is fast-forward only: no merge commits, no rebase, and no automatic stashing.
Repositories with populated submodules are skipped for now.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| <kbd>Ctrl</kbd> <kbd>K</kbd> | Command palette |
| <kbd>Ctrl</kbd> <kbd>1</kbd> / <kbd>2</kbd> / <kbd>3</kbd> | History / Local Changes / Stashes |
| <kbd>Ctrl</kbd> <kbd>Tab</kbd>, <kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>Tab</kbd> | Next / previous tab |
| <kbd>Ctrl</kbd> <kbd>W</kbd> | Close tab |
| <kbd>Ctrl</kbd> <kbd>Enter</kbd> | Commit |
| <kbd>Ctrl</kbd> <kbd>Alt</kbd> <kbd>Z</kbd> | Undo last operation |
| <kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>L</kbd> | Operation log |
| <kbd>Ctrl</kbd> <kbd>,</kbd> | Settings |
| <kbd>?</kbd> | All shortcuts |

Shortcuts can be remapped in Settings or in `keymap.json`.

## Configuration

Settings live in `%APPDATA%\SpurGit\settings.json` (`~/Library/Application Support/SpurGit/settings.json`
on macOS). Most of them can be changed in the app. A
few can only be set in the file for now:

```json
{
  "exclude": ["C:\\dev\\archive"],
  "external_client": {
    "program": "wsl.exe",
    "args": ["-d", "Ubuntu-26.04", "-e", "code", "--remote", "wsl+Ubuntu-26.04"]
  }
}
```

`external_client` is the command used to open a repository elsewhere. The repository path is
appended as the last argument, so for WSL repositories the program must understand Linux paths.

Environment variables:

| Variable | Effect |
| --- | --- |
| `SPUR_WSL=off` | Ignore WSL even if a distribution is installed. |
| `SPUR_WSL_DISTRO` | WSL distribution to use instead of the default one (e.g. `Debian`). |
| `SPUR_WINDOWS_GIT` | Path to the Windows `git.exe` to use instead of the one on `PATH`. |
| `GIS_PATH` | Fallback scan roots (see Usage). |
| `GIS_JOBS` | Number of concurrent Git processes (default 4). |

## How it handles WSL

Git traffic across the WSL boundary is slow, so Spur runs each repository's Git where the
repository lives:

- **Inside WSL** (`/home/…`): `git` runs in the distribution through `wsl.exe`.
- **On a Windows drive** (`C:\…`, `/mnt/c/…`): Git for Windows runs natively. If it can't read
  a checkout (for example a worktree created from WSL), Spur falls back to WSL Git.

Commands are always run with argument arrays and never through a shell. Paths are passed as
literal pathspecs, so file names like `*.txt` or `:(glob)x` are never treated as patterns.

## Development

```powershell
cargo test                   # unit tests
cargo test -- --ignored      # integration tests against real Git (requires WSL)
```

The integration tests create throwaway repositories in `/tmp` with an isolated `HOME`, and never
touch your own repositories. Diagnostics are written to stderr. Run a debug build from a
terminal to see them.

## Acknowledgements

- The visual design and icon animations are inspired by [Zeron](https://github.com/zeronsh/zeron) (MIT).
- The UI is built with [GPUI](https://www.gpui.rs/) via `gpui-kit`.
- [Geist](https://vercel.com/font) fonts, under the SIL Open Font License. See
  [THIRD_PARTY_NOTICES](assets/fonts/THIRD_PARTY_NOTICES.md).

## License

See [LICENSE](LICENSE).
