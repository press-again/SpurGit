//! Git access through the target WSL distribution.
//!
//! The application runs natively on Windows; every Git command executes
//! inside the WSL distro via `wsl.exe -e` with an argument array — no shell
//! strings, no path translation, Linux Git semantics preserved.
//!
//! Runner integration: every command runs through [`crate::process`] with
//! an argument array, concurrent drains, and a bounded log tail.
//!
//! Repository routing environment variables (`GIT_DIR`, `GIT_WORK_TREE`,
//! `GIT_INDEX_FILE`, ...) are removed for every invocation with `env -u`, so
//! an inherited variable cannot redirect an operation away from the requested
//! worktree. Per-command injected configuration (`GIT_CONFIG*`) is
//! removed for the same reason, while the user's HOME, global/system config,
//! hooks, SSH setup, and credential helpers are preserved untouched.
//! Windows-side variables normally do not cross into WSL except through
//! `WSLENV`; the removal happens on the Linux side (`env -u`), where Git
//! actually reads them. `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` stay
//! untouched: they are part of the user's environment.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;

use crate::model::{HistoryCommit, RefKind};
use crate::process::{self, Output};

/// Commits fetched per history page.
pub const HISTORY_PAGE: usize = 300;

/// Field separator (ASCII unit separator) inside one `git log` record; records
/// are NUL-separated by `-z`, so only `%s` could theoretically clash.
const FS: char = '\u{1f}';

/// Target WSL distribution, resolved once: `SPUR_WSL_DISTRO` when set,
/// otherwise the default distribution, which reports its own name. Empty when
/// WSL is disabled or has no distribution.
pub fn distro() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        if cfg!(not(windows)) || wsl_disabled_by_env() {
            return String::new();
        }
        if let Ok(name) = std::env::var("SPUR_WSL_DISTRO")
            && !name.trim().is_empty()
        {
            return name.trim().to_string();
        }
        process::Command::new("wsl.exe")
            .args(["-e", "printenv", "WSL_DISTRO_NAME"])
            .output()
            .ok()
            .filter(Output::success)
            .map(|out| out.stdout_text().trim().to_string())
            .unwrap_or_default()
    })
}

/// Repository-routing and per-command injected-config variables removed from
/// every Git invocation (narrowly scoped: user configuration stays available).
/// `GIT_CONFIG_COUNT` gates all `GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n` pairs,
/// so clearing it neutralizes the whole injected config family.
const DENY_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
];

/// `wsl.exe -d <distro> -e env -u …` — the prefix every Git command shares.
/// `wsl.exe` hands the child a terminal nobody can see, so Git's username
/// prompt is disabled: a missing credential fails instead of waiting forever.
fn command() -> process::Command {
    // Off Windows the "Linux side" is this machine: no tunnel, same `env -u`.
    #[cfg(windows)]
    let mut c = process::Command::new("wsl.exe").args(["-d", distro(), "-e", "env"]);
    #[cfg(not(windows))]
    let mut c = process::Command::new("env");
    for name in DENY_ENV {
        c = c.arg("-u").arg(name);
    }
    c.arg("GIT_TERMINAL_PROMPT=0")
}

/// Native Windows Git when it is installed; used for Windows-mounted
/// repositories, where WSL Git over 9p takes tens of seconds for a large
/// status. `SPUR_WINDOWS_GIT` overrides the executable; when the probe fails,
/// those repositories fall back to WSL Git.
pub(crate) fn windows_git() -> Option<&'static str> {
    static PROBE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PROBE
        .get_or_init(|| {
            let program =
                std::env::var("SPUR_WINDOWS_GIT").unwrap_or_else(|_| "git".to_string());
            // Through the app runner: it creates the child with
            // CREATE_NO_WINDOW, so probing for Git at launch cannot flash a
            // console window.
            let available = process::Command::new(&program)
                .arg("--version")
                .output()
                .map(|out| out.success())
                .unwrap_or(false);
            available.then_some(program)
        })
        .as_deref()
}

/// Run a Git command for a worktree, choosing its host: Windows-mounted
/// repositories (`/mnt/<drive>/…`) use native Windows Git when available
/// (fast on NTFS), every other path runs inside the WSL distro. Path
/// arguments are translated for the chosen host.
pub fn run_for(worktree: &str, args: &[&str]) -> io::Result<Output> {
    run_for_with_stdin(worktree, args, None)
}

/// [`run_for`] with data fed to the child's stdin. Used by `git log --stdin`:
/// a ref-heavy repository overruns the Windows command-line limit when every
/// revision tip is passed as an argument (CreateProcess os error 206).
pub fn run_for_with_stdin(
    worktree: &str,
    args: &[&str],
    stdin: Option<Vec<u8>>,
) -> io::Result<Output> {
    run_for_full(worktree, args, stdin, None)
}

/// Whether the target WSL distribution is reachable (cached). `SPUR_WSL=off`
/// (also `0`/`false`/`no`) forces native-only mode, and a machine without WSL
/// simply probes false — Windows-mounted repositories then use native Git and
/// Linux paths are rejected with a clear diagnostic instead of failing on
/// `wsl.exe`.
static WSL_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

pub fn wsl_available() -> bool {
    if cfg!(not(windows)) {
        // The local machine is the Linux side.
        return true;
    }
    if wsl_disabled_by_env() || distro().is_empty() {
        return false;
    }
    *WSL_AVAILABLE.get_or_init(|| {
        run_linux("true", &[])
            .map(|out| out.success())
            .unwrap_or(false)
    })
}

fn wsl_disabled_by_env() -> bool {
    std::env::var("SPUR_WSL").ok().is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no"
        )
    })
}

/// [`run_for`] for preview commands (diff/blob reads): capture is bounded to
/// one byte past [`DIFF_BYTE_CAP`] so the parser can mark truncation, instead
/// of first buffering a potentially huge output whole. The child is still
/// drained to completion, so it cannot block on a full pipe.
pub fn run_for_preview(worktree: &str, args: &[&str]) -> io::Result<Output> {
    run_for_full(worktree, args, None, Some(DIFF_BYTE_CAP + 1))
}

fn run_for_full(
    worktree: &str,
    args: &[&str],
    stdin: Option<Vec<u8>>,
    capture: Option<usize>,
) -> io::Result<Output> {
    // The native attempt's failure is kept: on a machine without WSL it is the
    // truthful error for a Windows-mounted path, where the WSL fallback cannot
    // even spawn.
    let mut native_out: Option<Output> = None;
    // The repository's account profile: author identity and the GitHub login
    // go in as one-off `-c` options, right after `-C <dir>`.
    let config = crate::accounts::config_args(worktree, args);
    let with_config: Vec<&str>;
    let args: &[&str] = if config.is_empty() {
        args
    } else {
        let at = if args.first() == Some(&"-C") { 2.min(args.len()) } else { 0 };
        with_config = args[..at]
            .iter()
            .copied()
            .chain(config.iter().map(String::as_str))
            .chain(args[at..].iter().copied())
            .collect();
        &with_config
    };
    let progress = args.contains(&"--progress");
    if crate::model::to_windows_path(worktree).is_some()
        && let Some(program) = windows_git()
    {
        let mut c = process::Command::new(program);
        for arg in args {
            match crate::model::to_windows_path(arg) {
                Some(windows) => c = c.arg(windows),
                None => c = c.arg(arg),
            }
        }
        for name in DENY_ENV {
            c = c.env_remove(name);
        }
        if let Some(data) = &stdin {
            c = c.stdin(data.clone());
        }
        if let Some(limit) = capture {
            c = c.capture_limit(limit);
        }
        if progress {
            c = c.live_progress();
        }
        match c.output() {
            Ok(out) if out.success() => return Ok(out),
            // Any other native failure is the command's real result: running
            // it again in WSL would repeat a rejected push or a failed hook.
            Ok(out) if !has_linux_gitdir(worktree) => return Ok(out),
            Ok(out) => native_out = Some(out),
            Err(_) => {}
        }
        // Fall through to WSL Git: worktrees created by WSL tooling on a
        // `/mnt` mount store Linux `gitdir` pointers native Git cannot read.
    }
    if !wsl_available() {
        // Native-only machine: do not spawn a doomed `wsl.exe`; report what
        // the native host produced (or why nothing could run).
        return match native_out {
            Some(out) => Ok(out),
            None => Err(io::Error::other(format!(
                "cannot run Git for {worktree}: WSL is unavailable and native Windows Git is not available"
            ))),
        };
    }
    let mut c = command().arg("git");
    for arg in args {
        c = c.arg(arg);
    }
    if let Some(data) = stdin {
        c = c.stdin(data);
    }
    if let Some(limit) = capture {
        c = c.capture_limit(limit);
    }
    if progress {
        c = c.live_progress();
    }
    match c.output() {
        Ok(out) => Ok(out),
        // The `wsl.exe` spawn itself failed (e.g. it was removed mid-run):
        // fall back to the native attempt's output when there is one.
        Err(err) => native_out.map(Ok).unwrap_or(Err(err)),
    }
}

/// A worktree created by WSL Git on a Windows drive: its `.git` file points at
/// a Linux path, which native Git cannot follow.
fn has_linux_gitdir(worktree: &str) -> bool {
    crate::model::to_windows_path(worktree)
        .and_then(|path| std::fs::read_to_string(PathBuf::from(path).join(".git")).ok())
        .is_some_and(|text| text.starts_with("gitdir: /"))
}

/// Run `git` inside the distro with literal arguments.
pub fn run(args: &[&str]) -> io::Result<Output> {
    run_for("", args)
}

/// Run any Linux executable through the same `wsl.exe` tunnel (discovery runs
/// `find` and `test`); arguments are literal, never shell strings.
pub(crate) fn run_linux(program: &str, args: &[&str]) -> io::Result<Output> {
    let mut c = command().arg(program);
    for arg in args {
        c = c.arg(arg);
    }
    c.output()
}

/// [`run_linux`] with data fed to the child's stdin (conflict writes use
/// `dd of=<path>` so a resolution never rides the command line).
pub(crate) fn run_linux_with_stdin(
    program: &str,
    args: &[&str],
    stdin: Vec<u8>,
) -> io::Result<Output> {
    let mut c = command().arg(program);
    for arg in args {
        c = c.arg(arg);
    }
    c.stdin(stdin).output()
}

/// Run `git` with extra environment variables for the integration tests.
///
/// `windows_env` variables are handed to `wsl.exe` and shared into the Linux
/// environment through `WSLENV` — the realistic inherited-variable path.
/// `linux_env` variables are assigned on the Linux side after the `env -u`
/// clearing, for values WSL does not let WSLENV override (HOME). Routing and
/// injected-config variables are only neutralized by the `env -u` prefix in
/// [`command`], which is exactly what the integration tests exercise.
#[cfg(test)]
fn run_with_env(
    args: &[&str],
    windows_env: &[(&str, String)],
    linux_env: &[(&str, String)],
) -> io::Result<Output> {
    let mut c = command();
    for (key, value) in linux_env {
        c = c.arg(format!("{key}={value}"));
    }
    for (key, value) in windows_env {
        c = c.env(key, value);
    }
    if !windows_env.is_empty() {
        let share: Vec<&str> = windows_env.iter().map(|(key, _)| *key).collect();
        c = c.env("WSLENV", share.join(":"));
    }
    let mut c = c.arg("git");
    for arg in args {
        c = c.arg(arg);
    }
    c.output()
}

/// `git --version` for startup diagnostics: the WSL distro's Git when it is
/// reachable, otherwise native Windows Git — a machine without WSL reports
/// what it will actually use instead of a `wsl.exe` error.
pub fn git_version() -> Result<String, String> {
    if cfg!(not(windows)) {
        let out = run(&["--version"]).map_err(|e| e.to_string())?;
        return if out.success() {
            Ok(out.stdout_text().trim().to_string())
        } else {
            Err(out.error_context())
        };
    }
    if !wsl_disabled_by_env() {
        match run(&["--version"]) {
            Ok(out) if out.success() => {
                let _ = WSL_AVAILABLE.set(true);
                return Ok(format!(
                    "{} (via wsl.exe -d {})",
                    out.stdout_text().trim(),
                    distro()
                ));
            }
            Ok(out) => {
                // The distro answered without a working Git; only fall back
                // when the reachability probe agrees WSL is unusable.
                if wsl_available() {
                    return Err(out.error_context());
                }
            }
            Err(_) => {}
        }
    }
    if let Some(program) = windows_git() {
        let out = process::Command::new(program)
            .arg("--version")
            .output()
            .map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(out.error_context());
        }
        return Ok(format!(
            "{} (native Windows Git; WSL unavailable)",
            out.stdout_text().trim()
        ));
    }
    Err("no Git available: WSL is unavailable and native Windows Git was not found".to_string())
}

/// Canonical identities for one worktree, resolved through Git itself.
///
/// `worktree` is the row identity. `git_dir` is the per-worktree Git
/// directory. `common_dir` is the shared storage identity used to serialize
/// conflicting operations across linked worktrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoIdentity {
    pub worktree: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
}

pub fn identity(path: &str) -> Result<RepoIdentity, String> {
    let out = run_for(
        path,
        &[
            "-C",
            path,
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
        ],
    )
    .map_err(|e| format!("git rev-parse: {e}"))?;
    if !out.success() {
        return Err(out.error_context());
    }
    let text = out.stdout_text();
    let mut lines = text.lines();
    let worktree = lines.next().unwrap_or_default().trim();
    let git_dir = lines.next().unwrap_or_default().trim();
    let common_dir = lines.next().unwrap_or_default().trim();
    if worktree.is_empty() || git_dir.is_empty() || common_dir.is_empty() {
        return Err("git rev-parse did not report a worktree identity".into());
    }
    // Native Windows Git reports Windows paths; row identity stays Linux.
    let norm = |path: &str| crate::model::to_linux_path(path).unwrap_or_else(|| path.to_string());
    Ok(RepoIdentity {
        worktree: PathBuf::from(norm(worktree)),
        git_dir: PathBuf::from(norm(git_dir)),
        common_dir: PathBuf::from(norm(common_dir)),
    })
}

/// Porcelain v2 status and ref metadata for one worktree.
/// Consumed by the overview and bulk pull.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedStatus {
    pub snapshot: crate::status::StatusSnapshot,
    pub refs: crate::status::RefInfo,
}

impl CollectedStatus {
    /// Whether the current branch's configured upstream ref is missing
    /// locally (`✗`).
    pub fn upstream_gone(&self) -> bool {
        let Some(branch) = self.snapshot.branch.as_deref() else {
            return false;
        };
        self.refs
            .branches
            .iter()
            .find(|info| info.name == branch)
            .is_some_and(|info| info.upstream_gone)
    }

    /// Cheap "did history change?" signature: current HEAD object id plus the
    /// hash over every ref. Any commit, fetch, tag, or checkout changes it, so
    /// status rounds can refresh the history view without re-reading it.
    pub fn history_signature(&self) -> (Option<String>, u64) {
        (self.snapshot.head_oid.clone(), self.refs.revision)
    }
}

pub fn collect_status(worktree: &str) -> Result<CollectedStatus, String> {
    Ok(CollectedStatus {
        snapshot: status_snapshot(worktree)?,
        refs: ref_info(worktree)?,
    })
}

/// Raw porcelain v2 bytes parsed into a snapshot. `--untracked-files=all`,
/// `--ahead-behind`, and `--ignore-submodules=none` are explicit so a user
/// configuration cannot hide unsafe state.
pub fn status_snapshot(worktree: &str) -> Result<crate::status::StatusSnapshot, String> {
    let out = run_for(
        worktree,
        &[
            "-C",
            worktree,
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
            "--show-stash",
            "-z",
            "--untracked-files=all",
            "--ahead-behind",
            "--ignore-submodules=none",
        ],
    )
    .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    crate::status::parse_porcelain_v2(&out.stdout)
}

/// Local branches with upstream state plus the local `origin/HEAD` default.
pub fn ref_info(worktree: &str) -> Result<crate::status::RefInfo, String> {
    let format = format!(
        "--format=%(refname){FS}%(refname:short){FS}%(HEAD){FS}%(upstream:short){FS}%(upstream:track){FS}%(symref){FS}%(objectname){FS}%(committerdate:unix)"
    );
    let out = run_for(
        worktree,
        &[
            "-C",
            worktree,
            "for-each-ref",
            &format,
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )
    .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    let text = out.stdout_text();
    let mut branches = Vec::new();
    let mut remote_branches = Vec::new();
    let mut default_branch = None;
    // The revision folds every ref name and objectname (plus symref targets)
    // into one number: any branch, remote, or tag move changes it.
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for line in text.lines() {
        let mut fields = line.split(FS);
        let full = fields.next().unwrap_or_default();
        let _short = fields.next().unwrap_or_default();
        let head = fields.next().unwrap_or_default();
        let upstream = fields.next().unwrap_or_default();
        let track = fields.next().unwrap_or_default();
        let symref = fields.next().unwrap_or_default();
        let oid = fields.next().unwrap_or_default();
        let updated = fields.next().unwrap_or_default();
        full.hash(&mut hasher);
        oid.hash(&mut hasher);
        symref.hash(&mut hasher);
        if full == "refs/remotes/origin/HEAD"
            && let Some(target) = symref.strip_prefix("refs/remotes/origin/")
        {
            default_branch = Some(target.to_string());
        }
        if let Some(name) = full.strip_prefix("refs/heads/") {
            let (ahead, behind) = crate::status::parse_track_counts(track);
            branches.push(crate::status::BranchInfo {
                name: name.to_string(),
                current: head.trim() == "*",
                upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
                upstream_gone: track.trim() == "[gone]",
                ahead,
                behind,
                updated: updated.trim().parse().unwrap_or(0),
            });
        }
        if let Some(rest) = full.strip_prefix("refs/remotes/")
            && let Some((remote, branch)) = rest.split_once('/')
            && branch != "HEAD"
        {
            remote_branches.push(crate::status::RemoteBranch {
                remote: remote.to_string(),
                name: branch.to_string(),
            });
        }
    }
    remote_branches.sort_by(|a, b| {
        a.remote
            .cmp(&b.remote)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(crate::status::RefInfo {
        branches,
        remote_branches,
        default_branch,
        revision: hasher.finish(),
    })
}

/// Active operation markers for a worktree, resolved through Git so linked
/// worktrees and linked git directories work. One `rev-parse` plus one
/// `find`, regardless of how many markers exist.
#[allow(dead_code)]
pub fn operation_markers(worktree: &str) -> Result<Vec<String>, String> {
    const NAMES: [&str; 6] = [
        "MERGE_HEAD",
        "REBASE_HEAD",
        "rebase-merge",
        "rebase-apply",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ];
    let mut args: Vec<&str> = vec!["-C", worktree, "rev-parse", "--path-format=absolute"];
    for name in NAMES {
        args.push("--git-path");
        args.push(name);
    }
    let out = run_for(worktree, &args).map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    let paths: Vec<String> = out
        .stdout_text()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if paths.len() != NAMES.len() {
        return Err("git rev-parse did not report every marker path".to_string());
    }
    if crate::model::host_path(worktree).is_some() {
        // Native Windows Git reports Windows paths; test them directly.
        return Ok(NAMES
            .iter()
            .zip(paths.iter())
            .filter(|(_, path)| std::path::Path::new(path).exists())
            .map(|(name, _)| (*name).to_string())
            .collect());
    }
    let mut find: Vec<&str> = paths.iter().map(String::as_str).collect();
    find.extend(["-maxdepth", "0", "-printf", "%p\\0"]);
    let found = run_linux("find", &find).map_err(|e| e.to_string())?;
    let existing: HashSet<&[u8]> = found.stdout.split(|&byte| byte == 0).collect();
    Ok(NAMES
        .iter()
        .zip(paths.iter())
        .filter(|(_, path)| existing.contains(path.as_bytes()))
        .map(|(name, _)| (*name).to_string())
        .collect())
}

/// Number of initialized submodules (`git submodule status` entries whose
/// prefix is not `-`). Bulk pull skips these repositories.
#[allow(dead_code)]
pub fn populated_submodules(worktree: &str) -> Result<u32, String> {
    let out =
        run_for(worktree, &["-C", worktree, "submodule", "status"]).map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    let count = out
        .stdout_text()
        .lines()
        .filter(|line| {
            !line.trim().is_empty() && line.as_bytes().first().is_some_and(|byte| *byte != b'-')
        })
        .count();
    Ok(count as u32)
}

/// Inspector details for one repository: remotes, tags, and stashes. Loaded
/// for the open tab only, not part of the status refresh round. Stash entries
/// carry their `stash@{n}` ref, the stash commit's object id (stable identity
/// for caches), and the message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoDetails {
    pub remotes: Vec<(String, String)>,
    pub tags: Vec<(String, String)>,
    /// `(stash@{n}, object id, message)`; oldest entry last, like `git stash
    /// list`. Shared (`Arc`, so it can cross the executor boundary) so
    /// rendering the list copies a reference, not every string.
    pub stashes: std::sync::Arc<Vec<(String, String, String)>>,
}

pub fn repo_details(worktree: &str) -> Result<RepoDetails, String> {
    // The three reads are independent; running them on parallel threads costs
    // one `wsl.exe` round-trip instead of three (the inspector load gates the
    // Stashes/Remotes/Tags pages).
    let tag_format = format!("--format=%(refname:short){FS}%(objectname:short)");
    let stash_format = format!("--format=%gd{FS}%H{FS}%s");
    let (remotes_out, tags_out, stash_out) = std::thread::scope(|scope| {
        let remotes = scope.spawn(|| run_for(worktree, &["-C", worktree, "remote", "-v"]));
        let tags = scope.spawn(|| {
            run_for(worktree, &["-C", worktree, "for-each-ref", &tag_format, "refs/tags"])
        });
        let stashes = scope.spawn(|| {
            run_for(worktree, &["-C", worktree, "stash", "list", &stash_format])
        });
        (remotes.join(), tags.join(), stashes.join())
    });
    let join = |name: &str, joined: Result<io::Result<Output>, Box<dyn std::any::Any + Send>>| {
        joined
            .map_err(|_| format!("{name} panicked"))?
            .map_err(|e| e.to_string())
    };
    let remotes_out = join("git remote -v", remotes_out)?;
    if !remotes_out.success() {
        return Err(remotes_out.error_context());
    }
    let mut remotes = Vec::new();
    for line in remotes_out.stdout_text().lines() {
        let mut fields = line.split_whitespace();
        let (Some(name), Some(url), Some(kind)) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        if kind == "(fetch)" {
            remotes.push((name.to_string(), url.to_string()));
        }
    }

    let tags_out = join("git for-each-ref", tags_out)?;
    let mut tags = Vec::new();
    if tags_out.success() {
        for line in tags_out.stdout_text().lines() {
            if let Some((name, hash)) = line.split_once(FS) {
                tags.push((name.to_string(), hash.to_string()));
            }
        }
    }

    let stash_out = join("git stash list", stash_out)?;
    let mut stashes = Vec::new();
    if stash_out.success() {
        for line in stash_out.stdout_text().lines() {
            let mut fields = line.splitn(3, FS);
            let (Some(id), Some(object), Some(message)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            stashes.push((id.to_string(), object.to_string(), message.to_string()));
        }
    }

    Ok(RepoDetails {
        remotes,
        tags,
        stashes: std::sync::Arc::new(stashes),
    })
}

/// Revision tips for a history session: every ref plus HEAD, i.e. exactly the
/// set `git log --all` would start from. Resolved once so later pages page a
/// fixed revision set even when refs move (fetch, commit, checkout).
pub fn revision_tips(worktree: &str) -> Vec<String> {
    let mut tips: Vec<String> = Vec::new();
    // Branch, remote, and tag refs only: `--all` would also pull in
    // `refs/stash`, whose commits (and its `index on …` parents) must not
    // appear in the history graph.
    if let Ok(o) = run_for(
        worktree,
        &[
            "-C",
            worktree,
            "for-each-ref",
            "--format=%(objectname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )
    && o.success()
    {
        tips.extend(
            o.stdout_text()
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string),
        );
    }
    if let Ok(o) = run_for(
        worktree,
        &["-C", worktree, "rev-parse", "--verify", "--quiet", "HEAD"],
    ) && o.success()
    {
        let stdout = o.stdout_text();
        let head = stdout.trim();
        if !head.is_empty() && !tips.iter().any(|t| t == head) {
            tips.push(head.to_string());
        }
    }
    tips
}

/// One page of history from a fixed revision set, newest first in date order
/// (`--date-order`: children before parents, otherwise commit time) so the
/// list reads like SourceGit's graph. A repository without commits is an
/// empty page, not an error. `remotes` classify `%D` decorations and are
/// resolved once per history session (not re-read per page).
pub fn log_history(
    worktree: &str,
    tips: &[String],
    remotes: &HashSet<String>,
    skip: usize,
    limit: usize,
) -> Result<Vec<HistoryCommit>, String> {
    let format = format!("--format=%H{FS}%P{FS}%an{FS}%ar{FS}%D{FS}%s");
    let n = format!("-n{limit}");
    let skip = format!("--skip={skip}");
    let args: Vec<&str> = vec![
        "-C",
        worktree,
        "--no-optional-locks",
        "log",
        "--date-order",
        &n,
        &skip,
        &format,
        "-z",
        // The fixed tips go through stdin: a repository with thousands of
        // refs overruns the Windows command-line limit as arguments. Empty
        // stdin behaves exactly like no revision arguments (HEAD).
        "--stdin",
    ];
    let mut stdin: Vec<u8> = Vec::new();
    for tip in tips {
        stdin.extend_from_slice(tip.as_bytes());
        stdin.push(b'\n');
    }
    let out = run_for_with_stdin(worktree, &args, Some(stdin)).map_err(|e| e.to_string())?;
    if !out.success() {
        let stderr = out.stderr_text();
        let err = stderr.trim();
        if err.contains("does not have any commits yet")
            || err.contains("unknown revision")
            || err.contains("bad revision")
        {
            return Ok(Vec::new());
        }
        return Err(if err.is_empty() {
            format!("git log exited {:?}", out.status)
        } else {
            err.to_string()
        });
    }
    Ok(parse_log(&out.stdout_text(), remotes))
}

/// Remote names, so `%D` decorations can be classified as branch vs remote.
pub fn remote_names(worktree: &str) -> HashSet<String> {
    run_for(worktree, &["-C", worktree, "remote"])
        .map(|o| {
            o.stdout_text()
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_log(raw: &str, remotes: &HashSet<String>) -> Vec<HistoryCommit> {
    raw.split('\0')
        .filter(|rec| !rec.is_empty())
        .map(|rec| {
            let mut f = rec.split(FS);
            let hash = f.next().unwrap_or_default().to_string();
            let parents = f
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let author = f.next().unwrap_or_default().to_string();
            let date = f.next().unwrap_or_default().to_string();
            let decoration = f.next().unwrap_or_default();
            let subject = f.next().unwrap_or_default().to_string();
            HistoryCommit {
                hash,
                parents,
                subject,
                author,
                date,
                refs: parse_decoration(decoration, remotes),
                graph: Default::default(),
            }
        })
        .collect()
}

// ---- Local Changes: staging and the internal diff viewer (Release 2) ----

/// Maximum diff bytes parsed and rendered for one file.
pub const DIFF_BYTE_CAP: usize = 2 * 1024 * 1024;

/// Parsed-row ceiling. Per-row overhead (two heap allocations, the struct)
/// dominates memory for pathological short-line diffs — a 2 MiB input of
/// 2-byte lines expands to ~1M rows (~88 MiB); parsing stops here with the
/// truncation marker set. Large enough for any human-reviewable diff.
pub const MAX_DIFF_ROWS: usize = 50_000;

/// One rendered unified-diff line. `text` has its diff prefix removed and is
/// display-sanitized; `raw` keeps the original bytes (also without the prefix)
/// so a single hunk can be rebuilt as a patch for `git apply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
    /// Unsanitized bytes of the line, without its diff prefix — stored only
    /// when they differ from [`DiffLine::text`] (control characters, invalid
    /// UTF-8). `None` means the text bytes ARE the raw bytes, so ordinary
    /// ASCII lines keep one allocation instead of two.
    pub raw: Option<Vec<u8>>,
    /// Hunk this line belongs to (`None` for file headers and binary meta).
    pub hunk: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    Hunk,
    Meta,
}

impl DiffLine {
    fn new(
        kind: DiffLineKind,
        old: Option<u32>,
        new: Option<u32>,
        text: String,
        raw: Vec<u8>,
    ) -> Self {
        // Keep the raw copy only when sanitization changed the bytes; the
        // patch builder reconstructs identical lines from `text`.
        let raw = (text.as_bytes() != raw.as_slice()).then_some(raw);
        Self {
            kind,
            old,
            new,
            text,
            raw,
            hunk: None,
        }
    }

    /// Unsanitized line bytes for patch reconstruction.
    fn raw_bytes(&self) -> &[u8] {
        self.raw.as_deref().unwrap_or(self.text.as_bytes())
    }
}

/// Line range of one hunk inside [`FileDiff::lines`] (`start` is the `@@`
/// header, `end` is one past the last line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffHunk {
    pub start: usize,
    pub end: usize,
}

/// Parsed diff for one file side (worktree or index).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub binary: bool,
    pub truncated: bool,
    pub additions: usize,
    pub deletions: usize,
    pub lines: Vec<DiffLine>,
    /// Number of leading meta lines (`diff --git`, `index`, `---`, `+++`, …):
    /// the file header a single-hunk patch must repeat.
    pub header_len: usize,
    /// Hunks in order; empty for binary and synthesized untracked previews.
    pub hunks: Vec<DiffHunk>,
    /// Longest rendered line in characters; computed once at parse time so
    /// rendering never rescans every line for the horizontal width.
    pub max_chars: usize,
}

impl FileDiff {
    /// Rebuild a `git apply`-able patch for one hunk: the file header plus the
    /// hunk's lines with their original prefixes and bytes.
    pub fn patch_for_hunk(&self, index: usize) -> Option<Vec<u8>> {
        let hunk = *self.hunks.get(index)?;
        let mut patch = Vec::new();
        for line in &self.lines[..self.header_len] {
            patch.extend_from_slice(line.raw_bytes());
            patch.push(b'\n');
        }
        for line in &self.lines[hunk.start..hunk.end] {
            match line.kind {
                DiffLineKind::Added => patch.push(b'+'),
                DiffLineKind::Removed => patch.push(b'-'),
                DiffLineKind::Context => patch.push(b' '),
                DiffLineKind::Hunk | DiffLineKind::Meta => {}
            }
            patch.extend_from_slice(line.raw_bytes());
            patch.push(b'\n');
        }
        Some(patch)
    }

    /// Retained size of the parsed model for the diff cache's byte budget:
    /// the struct itself, the row/hunk vector capacities, and each row's
    /// text capacity plus any separately stored raw backup. Counting
    /// capacities matters — a 50 000-row short-line diff retains ~5.5 MiB
    /// even though its payload is ~2 MiB. Ordinary lines whose sanitized text
    /// equals the raw bytes are counted once. Panes
    /// currently displaying a diff hold their own reference, which eviction
    /// cannot reclaim.
    pub fn approx_bytes(&self) -> usize {
        let mut bytes = std::mem::size_of::<Self>();
        bytes += self.lines.capacity() * std::mem::size_of::<DiffLine>();
        bytes += self.hunks.capacity() * std::mem::size_of::<DiffHunk>();
        for line in &self.lines {
            bytes += line.text.capacity();
            bytes += line.raw.as_ref().map_or(0, |raw| raw.capacity());
        }
        bytes
    }
}

/// Parse unified diff bytes (no color, no external helpers) into rows. The
/// parser is deliberately tolerant: unknown lines become meta rows so a future
/// Git format cannot break the pane.
pub fn parse_unified_diff(bytes: &[u8]) -> FileDiff {
    let mut diff = FileDiff::default();
    let mut slice = bytes;
    if bytes.len() > DIFF_BYTE_CAP {
        diff.truncated = true;
        slice = &bytes[..DIFF_BYTE_CAP];
        if let Some(end) = slice.iter().rposition(|&byte| byte == b'\n') {
            slice = &slice[..=end];
        }
    }
    let mut old_line = 0u32;
    let mut new_line = 0u32;
    let mut current_hunk: Option<usize> = None;
    let mut header_done = false;
    for raw in slice.split(|&byte| byte == b'\n') {
        if raw.is_empty() {
            continue;
        }
        if diff.lines.len() >= MAX_DIFF_ROWS {
            diff.truncated = true;
            break;
        }
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw).to_vec();
        let text = crate::process::sanitize(&raw);
        let line_index = diff.lines.len();
        if text.starts_with("@@") {
            if !header_done {
                diff.header_len = line_index;
                header_done = true;
            }
            if let Some(previous) = current_hunk {
                diff.hunks[previous].end = line_index;
            }
            if let Some((old, new)) = parse_hunk_header(&text) {
                old_line = old;
                new_line = new;
            }
            diff.hunks.push(DiffHunk {
                start: line_index,
                end: line_index + 1,
            });
            current_hunk = Some(diff.hunks.len() - 1);
            diff.lines
                .push(DiffLine::new(DiffLineKind::Hunk, None, None, text, raw));
            continue;
        }
        let mut line = if text.starts_with("Binary files ") || text.starts_with("GIT binary patch") {
            diff.binary = true;
            DiffLine::new(DiffLineKind::Meta, None, None, text, raw)
        } else if let Some(body) = text
            .strip_prefix('+')
            .filter(|_| !text.starts_with("+++"))
        {
            new_line += 1;
            diff.additions += 1;
            DiffLine::new(
                DiffLineKind::Added,
                None,
                Some(new_line - 1),
                body.to_string(),
                raw[1..].to_vec(),
            )
        } else if let Some(body) = text
            .strip_prefix('-')
            .filter(|_| !text.starts_with("---"))
        {
            old_line += 1;
            diff.deletions += 1;
            DiffLine::new(
                DiffLineKind::Removed,
                Some(old_line - 1),
                None,
                body.to_string(),
                raw[1..].to_vec(),
            )
        } else if let Some(body) = text.strip_prefix(' ') {
            let line = DiffLine::new(
                DiffLineKind::Context,
                Some(old_line),
                Some(new_line),
                body.to_string(),
                raw[1..].to_vec(),
            );
            old_line += 1;
            new_line += 1;
            line
        } else {
            // diff --git/index/---/+++/\ No newline/rename headers. A meta line
            // inside a hunk (`\ No newline…`) belongs to that hunk's patch.
            DiffLine::new(DiffLineKind::Meta, None, None, text, raw)
        };
        line.hunk = current_hunk;
        diff.lines.push(line);
        if let Some(ix) = current_hunk {
            diff.hunks[ix].end = diff.lines.len();
        }
    }
    if !header_done {
        diff.header_len = diff.lines.len();
    }
    // One pass at parse time; renderers read the stored width.
    diff.max_chars = diff
        .lines
        .iter()
        .map(|line| line.text.chars().count())
        .max()
        .unwrap_or(0);
    diff
}

/// `@@ -12,5 +12,7 @@ …` -> `(12, 12)` (start lines).
fn parse_hunk_header(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.split_whitespace();
    if parts.next()? != "@@" {
        return None;
    }
    let minus = parts.next()?;
    let plus = parts.next()?;
    let old = minus.strip_prefix('-')?.split(',').next()?.parse().ok()?;
    let new = plus.strip_prefix('+')?.split(',').next()?.parse().ok()?;
    Some((old, new))
}

/// NUL-separated pathspec bytes for `--pathspec-file-nul` (no trailing NUL).
fn pathspec_stdin(paths: &[Vec<u8>]) -> Vec<u8> {
    let mut data = Vec::new();
    for (ix, path) in paths.iter().enumerate() {
        if ix > 0 {
            data.push(0);
        }
        data.extend_from_slice(path);
    }
    data
}

/// Run a worktree/index command with byte-exact literal pathspecs.
///
/// `--literal-pathspecs` disables pathspec magic for the whole command —
/// `--` alone does not. Paths travel on stdin so
/// non-UTF-8 bytes survive the Windows command line.
fn index_command(
    worktree: &str,
    verb: &str,
    extra: &[&str],
    paths: &[Vec<u8>],
) -> Result<(), String> {
    if paths.is_empty() {
        return Err("no paths given".to_string());
    }
    let args = index_args(worktree, verb, extra);
    let out = run_for_with_stdin(worktree, &args, Some(pathspec_stdin(paths)))
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

fn index_args<'a>(worktree: &'a str, verb: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args: Vec<&str> = vec!["--literal-pathspecs", "-C", worktree, verb];
    args.extend_from_slice(extra);
    args.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
    args
}

/// Stage exactly these paths (tracked modifications/deletions and untracked
/// additions). Literal pathspecs: a file named `*.txt` matches only itself.
pub fn stage_paths(worktree: &str, paths: &[Vec<u8>]) -> Result<(), String> {
    index_command(worktree, "add", &[], paths)
}

/// Unstage exactly these paths. `reset` is used instead of `restore --staged`
/// because `restore` cannot resolve HEAD in an unborn repository.
pub fn unstage_paths(worktree: &str, paths: &[Vec<u8>]) -> Result<(), String> {
    index_command(worktree, "reset", &["-q"], paths)
}

/// Commit exactly the staged index snapshot. The message travels on stdin
/// (`--file=-`), so multiline text and any bytes are safe; hooks, signing, and
/// every other configured behavior stay enabled, and nothing is staged
/// automatically (`commit -a` would be the wrong thing).
/// Returns the new short hash for the operation log.
pub fn commit_staged(worktree: &str, message: &[u8]) -> Result<String, String> {
    if String::from_utf8_lossy(message).trim().is_empty() {
        return Err("commit message is empty".to_string());
    }
    let out = run_for_with_stdin(
        worktree,
        &["-C", worktree, "commit", "--file=-"],
        Some(message.to_vec()),
    )
    .map_err(|e| e.to_string())?;
    if !out.success() {
        let stderr = out.stderr_text();
        let detail = stderr.trim();
        return Err(if detail.is_empty() {
            format!("git commit exited {:?}", out.status)
        } else {
            detail.to_string()
        });
    }
    let hash = run_for(worktree, &["-C", worktree, "rev-parse", "--short", "HEAD"])
        .ok()
        .filter(Output::success)
        .map(|out| out.stdout_text().trim().to_string())
        .unwrap_or_default();
    Ok(hash)
}

/// Apply a single-hunk patch (`FileDiff::patch_for_hunk`) to the index
/// (`cached`) or the worktree, optionally reversed. Used by the diff pane's
/// per-hunk Stage/Unstage/Discard buttons and the clipboard "Apply patch"
/// entry, which applies to the worktree only.
///
/// Deliberately no `--3way`: it implies `--index` (applying the result to
/// the index as well as refusing dirty worktrees with "does not match
/// index"), while pasted patches belong in the worktree unstaged for review.
/// Probed on git 2.53.
pub fn apply_patch(
    worktree: &str,
    patch: &[u8],
    cached: bool,
    reverse: bool,
) -> Result<(), String> {
    if patch.is_empty() {
        return Err("empty patch".to_string());
    }
    let mut args: Vec<&str> = vec!["-C", worktree, "apply", "--whitespace=nowarn"];
    if cached {
        args.push("--cached");
    }
    if reverse {
        args.push("--reverse");
    }
    args.push("-");
    let out =
        run_for_with_stdin(worktree, &args, Some(patch.to_vec())).map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// File paths touched by a unified patch for the "Apply patch" preview:
/// the `diff --git` b-side, or the a-side when the b-side is
/// `/dev/null` (deletions). C-quoted paths are unwrapped; text without a
/// `diff --git` line yields no entry, which the UI reports as "not a patch".
pub fn patch_file_list(patch: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in patch.lines() {
        let Some(rest) = line.strip_prefix("diff --git ") else {
            continue;
        };
        let Some((ours, theirs)) = split_diff_paths(rest) else {
            continue;
        };
        let path = if theirs == "/dev/null" { ours } else { theirs };
        let path = path
            .strip_prefix("a/")
            .or_else(|| path.strip_prefix("b/"))
            .unwrap_or(&path)
            .to_string();
        if !path.is_empty() && !files.contains(&path) {
            files.push(path);
        }
    }
    files
}

/// Split the two paths of a `diff --git` line, unwrapping Git's C-quoting
/// (`"a/sp ace" "b/sp ace"`, octal bytes for non-ASCII). Plain `a/f b/f`
/// needs no unquoting.
fn split_diff_paths(rest: &str) -> Option<(String, String)> {
    fn octal(value: u8) -> Option<u8> {
        (b'0'..=b'7').contains(&value).then_some(value - b'0')
    }
    // Git leaves names with spaces unquoted (`a/my file b/my file`); when
    // both sides name the same path (every non-rename) the split is the
    // middle space.
    if !rest.starts_with('"') && rest.len() % 2 == 1 {
        let mid = rest.len() / 2;
        if rest.as_bytes()[mid] == b' ' {
            let (ours, theirs) = (&rest[..mid], &rest[mid + 1..]);
            if ours.strip_prefix("a/").is_some_and(|a| Some(a) == theirs.strip_prefix("b/")) {
                return Some((ours.to_string(), theirs.to_string()));
            }
        }
    }
    let bytes = rest.as_bytes();
    let mut paths = Vec::new();
    let mut ix = 0;
    while ix < bytes.len() && paths.len() < 2 {
        while ix < bytes.len() && bytes[ix] == b' ' {
            ix += 1;
        }
        if ix >= bytes.len() {
            break;
        }
        let mut path = Vec::new();
        if bytes[ix] == b'"' {
            ix += 1;
            let mut closed = false;
            while ix < bytes.len() {
                match bytes[ix] {
                    b'"' => {
                        closed = true;
                        ix += 1;
                        break;
                    }
                    b'\\' => {
                        ix += 1;
                        match bytes.get(ix) {
                            Some(b'n') => path.push(b'\n'),
                            Some(b't') => path.push(b'\t'),
                            Some(b'"') => path.push(b'"'),
                            Some(b'\\') => path.push(b'\\'),
                            Some(first) => {
                                // Octal byte (`\303\244`); anything else is
                                // not a path git would quote.
                                let (b1, b2, b3) = (
                                    octal(*first)?,
                                    octal(*bytes.get(ix + 1)?)?,
                                    octal(*bytes.get(ix + 2)?)?,
                                );
                                path.push(b1 * 64 + b2 * 8 + b3);
                                ix += 2;
                            }
                            None => return None,
                        }
                        ix += 1;
                    }
                    byte => {
                        path.push(byte);
                        ix += 1;
                    }
                }
            }
            if !closed {
                return None;
            }
        } else {
            let start = ix;
            while ix < bytes.len() && bytes[ix] != b' ' {
                ix += 1;
            }
            path.extend_from_slice(&bytes[start..ix]);
        }
        paths.push(String::from_utf8_lossy(&path).into_owned());
    }
    if paths.len() == 2 {
        Some((paths.remove(0), paths.remove(0)))
    } else {
        None
    }
}

/// Restore paths from the index or HEAD. `to_head` removes index **and**
/// worktree changes; otherwise only worktree changes are discarded. Used by
/// the Local Changes context menu's Discard action.
pub fn restore_paths(worktree: &str, paths: &[Vec<u8>], to_head: bool) -> Result<(), String> {
    if to_head {
        index_command(
            worktree,
            "restore",
            &["--source=HEAD", "--staged", "--worktree"],
            paths,
        )
    } else {
        index_command(worktree, "restore", &["--worktree"], paths)
    }
}

/// Delete untracked files from the worktree (Discard on a `?` entry).
/// `git status --untracked-files=all` lists files, never directories.
pub fn delete_untracked_paths(worktree: &str, paths: &[Vec<u8>]) -> Result<(), String> {
    for path in paths {
        let relative = String::from_utf8_lossy(path);
        let joined = format!("{}/{}", worktree.trim_end_matches('/'), relative);
        if crate::model::host_path(worktree).is_some() {
            let windows = crate::model::host_path(&joined)
                .ok_or_else(|| format!("path is not representable on this host: {relative}"))?;
            match std::fs::remove_file(&windows) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.to_string()),
            }
        } else if std::str::from_utf8(path).is_ok() {
            let out = run_linux("rm", &["-f", &joined]).map_err(|e| e.to_string())?;
            if !out.success() {
                return Err(out.error_context());
            }
        } else {
            return Err("cannot delete a non-UTF-8 path on this host".to_string());
        }
    }
    Ok(())
}

/// What happens to the paths after a stash (SourceGit's
/// `DealWithChangesAfterStashing`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StashMode {
    /// All changes are moved into the stash (gone from the worktree).
    #[default]
    Discard,
    /// Staged changes stay intact; only unstaged changes are stashed.
    KeepIndex,
    /// The stash entry is created and everything is applied back immediately,
    /// so all changes stay in the worktree.
    KeepAll,
}

/// Stash exactly these paths (including untracked content when asked). The
/// worktree paths are restored to their committed state by Git's stash, except
/// in [`StashMode::KeepIndex`] / [`StashMode::KeepAll`].
pub fn stash_paths(
    worktree: &str,
    paths: &[Vec<u8>],
    untracked: bool,
    message: &str,
    mode: StashMode,
) -> Result<(), String> {
    let mut extra: Vec<&str> = vec!["push"];
    if !message.is_empty() {
        extra.push("-m");
        extra.push(message);
    }
    if untracked {
        extra.push("--include-untracked");
    }
    if mode == StashMode::KeepIndex {
        extra.push("--keep-index");
    }
    index_command(worktree, "stash", &extra, paths)?;
    if mode == StashMode::KeepAll {
        // Same sequence SourceGit uses: the stash entry exists, then the
        // changes are applied back (index state included).
        let out = run_for(
            worktree,
            &["-C", worktree, "stash", "apply", "--index", "stash@{0}"],
        )
        .map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(out.error_context());
        }
    }
    Ok(())
}

/// What happens to local changes when a new branch is checked out (the
/// "Create Branch" dialog's deal-with-changes choice).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BranchChanges {
    /// Changes stay in the worktree; Git refuses when they conflict with the
    /// switch.
    #[default]
    Keep,
    /// Changes (untracked included) are stashed before the switch and
    /// reapplied afterwards.
    Stash,
    /// The checkout discards local changes (`--force`).
    Discard,
}

/// Create a branch from `base`. `checkout` switches to it; `overwrite` resets
/// an existing branch of the same name (`-B` / `branch -f`). Local changes
/// only matter when the worktree moves, so `changes` applies to a checked-out
/// creation: keep them, stash and reapply, or discard them.
pub fn create_branch(
    worktree: &str,
    name: &str,
    base: &str,
    checkout: bool,
    overwrite: bool,
    changes: BranchChanges,
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("branch name is empty".to_string());
    }
    let out = run_for(
        worktree,
        &["-C", worktree, "check-ref-format", "--branch", name],
    )
    .map_err(|e| e.to_string())?;
    if !out.success() {
        let detail = out.stderr_text();
        return Err(if detail.trim().is_empty() {
            format!("invalid branch name '{name}'")
        } else {
            detail.trim().to_string()
        });
    }

    // Stash only when there is something to stash: popping an unrelated entry
    // would be a data bug. A clean status costs one query and is exact.
    let mut stashed = false;
    if checkout && changes == BranchChanges::Stash && !status_snapshot(worktree)?.is_clean() {
        let message = format!("Spur: create branch {name}");
        let out = run_for(
            worktree,
            &[
                "-C",
                worktree,
                "stash",
                "push",
                "--include-untracked",
                "-m",
                &message,
            ],
        )
        .map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(out.error_context());
        }
        stashed = true;
    }

    let mut args: Vec<&str> = vec!["-C", worktree];
    if checkout {
        args.push("checkout");
        if changes == BranchChanges::Discard {
            args.push("--force");
        }
        args.push(if overwrite { "-B" } else { "-b" });
        args.push(name);
        args.push(base);
    } else {
        args.push("branch");
        if overwrite {
            args.push("-f");
        }
        args.push(name);
        args.push(base);
    }
    let out = run_for(worktree, &args).map_err(|e| e.to_string())?;
    if !out.success() {
        let detail = out.error_context();
        return Err(if stashed {
            format!("{detail} (local changes remain in the stash)")
        } else {
            detail
        });
    }

    if stashed {
        let out = run_for(worktree, &["-C", worktree, "stash", "pop"])
            .map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(format!(
                "branch {name} was created, but reapplying the stashed changes failed: {}",
                out.error_context()
            ));
        }
    }
    Ok(())
}

/// Switch the worktree to an existing local branch (branch context menu).
/// Local changes must not conflict; Git reports otherwise.
pub fn checkout_branch(worktree: &str, name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.starts_with('-') {
        return Err(format!("invalid branch name '{name}'"));
    }
    let out =
        run_for(worktree, &["-C", worktree, "checkout", name]).map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Check out a remote-tracking branch (Remotes context menu): switch to the
/// local branch of the same name when it exists, otherwise create it with
/// `--track` so the upstream is configured.
pub fn checkout_remote_branch(worktree: &str, remote: &str, branch: &str) -> Result<(), String> {
    if remote.trim().is_empty()
        || remote.starts_with('-')
        || branch.trim().is_empty()
        || branch.starts_with('-')
    {
        return Err(format!("invalid remote branch '{remote}/{branch}'"));
    }
    let local_ref = format!("refs/heads/{branch}");
    let local_exists = run_for(
        worktree,
        &["-C", worktree, "show-ref", "--verify", "--quiet", &local_ref],
    )
    .map(|out| out.success())
    .unwrap_or(false);
    let out = if local_exists {
        run_for(worktree, &["-C", worktree, "checkout", branch])
    } else {
        let tracking = format!("{remote}/{branch}");
        run_for(
            worktree,
            &["-C", worktree, "checkout", "-b", branch, "--track", &tracking],
        )
    }
    .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Rename a local branch. Refusing to overwrite an existing name is git's
/// job; the failure is reported. Renaming the checked-out branch is allowed
/// and keeps it checked out.
pub fn rename_branch(worktree: &str, old: &str, new: &str) -> Result<(), String> {
    if old.trim().is_empty()
        || old.starts_with('-')
        || new.trim().is_empty()
        || new.starts_with('-')
    {
        return Err(format!("invalid branch rename '{old}' → '{new}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "branch", "-m", old, new])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Delete a local branch. Without `force` git refuses unmerged branches
/// ("not fully merged") and the checked-out branch; callers surface that
/// refusal instead of forcing.
pub fn delete_branch(worktree: &str, name: &str, force: bool) -> Result<(), String> {
    if name.trim().is_empty() || name.starts_with('-') {
        return Err(format!("invalid branch name '{name}'"));
    }
    let flag = if force { "-D" } else { "-d" };
    let out = run_for(worktree, &["-C", worktree, "branch", flag, name])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

// ---- Undo safety net: backup refs and the restores that consume them ----

/// Namespace for undo backup refs. Local refs, never pushed; the UI keeps the
/// refname in its per-repository undo stack.
pub const UNDO_REF_PREFIX: &str = "refs/spur/undo";
/// Backup refs older than this are pruned when a new one is written.
pub const UNDO_PRUNE_DAYS: u64 = 14;
/// Largest deleted untracked file kept for discard-undo.
pub const UNDO_FILE_CAP: usize = 8 * 1024 * 1024;

/// Pure refname builder (unit-tested): `refs/spur/undo/<secs>_<label>`.
pub fn undo_refname_at(secs: u64, label: &str) -> String {
    let mut clean = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            clean.push(c);
        } else if !clean.ends_with('_') && !clean.is_empty() {
            clean.push('_');
        }
    }
    let clean = clean.trim_matches('_');
    let clean = if clean.is_empty() { "op" } else { clean };
    format!("{UNDO_REF_PREFIX}/{secs}_{clean}")
}

/// Timestamped backup refname for a human label (`"discard src/foo.rs"`).
pub fn undo_refname(label: &str) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    undo_refname_at(secs, label)
}

/// Resolve any revision to its object id. Callers pass exact names (`HEAD`, a
/// branch, `stash@{0}`); a leading dash is never a revision.
pub fn rev_parse(worktree: &str, revision: &str) -> Result<String, String> {
    if revision.trim().is_empty() || revision.starts_with('-') {
        return Err(format!("invalid revision '{revision}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "rev-parse", "--verify", revision])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    let oid = out.stdout_text().trim().to_string();
    if !valid_hash(&oid) {
        return Err(format!("could not resolve '{revision}'"));
    }
    Ok(oid)
}

/// Point a new backup ref at `oid` and prune refs older than
/// [`UNDO_PRUNE_DAYS`]. Pruning is best-effort: it never fails the backup, so
/// backup plumbing can never block a user operation.
pub fn write_backup_ref(worktree: &str, oid: &str, label: &str) -> Result<String, String> {
    if !valid_hash(oid) {
        return Err(format!("invalid object id '{oid}'"));
    }
    // The object id keeps names unique: several backups written in the same
    // second (one per untracked file, HEAD + stash) must never overwrite
    // each other.
    let name = undo_refname(&format!("{label} {}", &oid[..oid.len().min(12)]));
    let out = run_for(worktree, &["-C", worktree, "update-ref", &name, oid])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    prune_undo_refs(worktree);
    Ok(name)
}

/// Delete a backup ref. Names outside the undo namespaces are refused.
pub fn delete_backup_ref(worktree: &str, name: &str) -> Result<(), String> {
    if !name.starts_with(UNDO_REF_PREFIX) || name.starts_with('-')
    {
        return Err(format!("not an undo backup '{name}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "update-ref", "-d", name])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Pure cutoff helper: `now - days*86400`, saturating.
pub fn prune_cutoff_secs(now_secs: u64, days: u64) -> u64 {
    now_secs.saturating_sub(days.saturating_mul(24 * 3600))
}

/// One backup refname with the write time [`undo_refname_at`] embedded;
/// `None` when the shape is wrong. The object's own date (`creatordate`) is
/// useless here: old commits would prune instantly, blobs never. Pure so the
/// pruning rule is unit-testable.
pub fn parse_undo_ref_line(line: &str) -> Option<(String, u64)> {
    let name = line.trim();
    let rest = name.strip_prefix(UNDO_REF_PREFIX)?.strip_prefix('/')?;
    let (secs, _) = rest.split_once('_')?;
    Some((name.to_string(), secs.parse().ok()?))
}

/// Delete backup refs older than [`UNDO_PRUNE_DAYS`]. Returns the pruned
/// count; a failed listing prunes nothing.
pub fn prune_undo_refs(worktree: &str) -> usize {
    let out = match run_for(
        worktree,
        &[
            "-C",
            worktree,
            "for-each-ref",
            "--format=%(refname)",
            UNDO_REF_PREFIX,
        ],
    ) {
        Ok(out) if out.success() => out,
        _ => return 0,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cutoff = prune_cutoff_secs(now, UNDO_PRUNE_DAYS);
    let mut pruned = 0;
    for line in out.stdout_text().lines() {
        let Some((name, when)) = parse_undo_ref_line(line) else {
            continue;
        };
        if when < cutoff && delete_backup_ref(worktree, &name).is_ok() {
            pruned += 1;
        }
    }
    pruned
}

/// A `git stash create` commit holding the current tracked changes, without
/// touching the stash list. `None` means a clean tree (nothing to back up).
pub fn stash_create_id(worktree: &str) -> Result<Option<String>, String> {
    let out = run_for(worktree, &["-C", worktree, "stash", "create"])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    let oid = out.stdout_text().trim().to_string();
    if oid.is_empty() {
        return Ok(None);
    }
    if !valid_hash(&oid) {
        return Err("could not back up the changes".to_string());
    }
    Ok(Some(oid))
}

/// Turn a stash-like commit (from `stash create`, or a dropped entry's object)
/// into a real stash entry. The UI applies it and drops the entry on undo.
pub fn stash_store_id(worktree: &str, oid: &str, message: &str) -> Result<(), String> {
    if !valid_hash(oid) {
        return Err(format!("invalid object id '{oid}'"));
    }
    let message = message.trim();
    let message = if message.is_empty() {
        "Spur undo"
    } else {
        message
    };
    let out = run_for(worktree, &["-C", worktree, "stash", "store", "-m", message, oid])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Paths as argv pathspecs (for commands without `--pathspec-from-file`).
fn utf8_pathspecs(paths: &[Vec<u8>]) -> Result<Vec<String>, String> {
    paths
        .iter()
        .map(|path| {
            String::from_utf8(path.clone())
                .map_err(|_| "cannot pass a non-UTF-8 path on this host".to_string())
        })
        .collect()
}

/// True when `paths` carry no changes: worktree vs index, or (with
/// `against_head`) worktree and index vs HEAD. Guards path restores so they
/// never overwrite edits made after the discard.
pub fn paths_unmodified(
    worktree: &str,
    paths: &[Vec<u8>],
    against_head: bool,
) -> Result<bool, String> {
    let specs = utf8_pathspecs(paths)?;
    let checks: &[&[&str]] = if against_head {
        &[&["HEAD"], &["--cached", "HEAD"]]
    } else {
        &[&[]]
    };
    for extra in checks {
        let mut args = vec!["--literal-pathspecs", "-C", worktree, "diff", "--quiet"];
        args.extend_from_slice(extra);
        args.push("--");
        args.extend(specs.iter().map(String::as_str));
        let out = run_for(worktree, &args).map_err(|e| e.to_string())?;
        match out.status {
            Some(0) if !out.cancelled => {}
            Some(1) if !out.cancelled => return Ok(false),
            _ => return Err(out.error_context()),
        }
    }
    Ok(true)
}

/// Restore `paths` from a stash-like commit: the worktree side from the
/// commit itself, the index side (with `staged`) from its index parent
/// (`^2`). Only these paths change, never the rest of the tree.
pub fn restore_paths_from_stash(
    worktree: &str,
    oid: &str,
    paths: &[Vec<u8>],
    staged: bool,
) -> Result<(), String> {
    if !valid_hash(oid) {
        return Err(format!("invalid object id '{oid}'"));
    }
    if staged {
        let source = format!("--source={oid}^2");
        index_command(worktree, "restore", &[&source, "--staged"], paths)?;
    }
    let source = format!("--source={oid}");
    index_command(worktree, "restore", &[&source, "--worktree"], paths)
}

/// Recreate a deleted branch at its backed-up tip.
pub fn recreate_branch_at(worktree: &str, name: &str, oid: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('-') {
        return Err(format!("invalid branch name '{name}'"));
    }
    if !valid_hash(oid) {
        return Err(format!("invalid object id '{oid}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "branch", name, oid])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Move HEAD (and the checked-out branch) back. The UI refuses this while the
/// worktree is dirty, so no tracked change is lost here.
pub fn reset_hard_to(worktree: &str, oid: &str) -> Result<(), String> {
    if !valid_hash(oid) {
        return Err(format!("invalid object id '{oid}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "reset", "--hard", oid])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Point a deleted tag back at its object. `update-ref` restores annotated
/// and lightweight tags exactly (the object itself never changed).
pub fn restore_tag_to(worktree: &str, name: &str, oid: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('-') {
        return Err(format!("invalid tag name '{name}'"));
    }
    if !valid_hash(oid) {
        return Err(format!("invalid object id '{oid}'"));
    }
    let reference = format!("refs/tags/{name}");
    // Empty old value: refuse when a tag of that name was created since,
    // instead of silently repointing it.
    let out = run_for(worktree, &["-C", worktree, "update-ref", &reference, oid, ""])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Store arbitrary bytes (a deleted untracked file, a hunk patch) as a blob
/// so the backup ref points at durable content, not just an id.
pub fn hash_blob(worktree: &str, bytes: &[u8]) -> Result<String, String> {
    let out = run_for_with_stdin(
        worktree,
        &["-C", worktree, "hash-object", "-w", "--stdin"],
        Some(bytes.to_vec()),
    )
    .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    let oid = out.stdout_text().trim().to_string();
    if !valid_hash(&oid) {
        return Err("could not store the backup".to_string());
    }
    Ok(oid)
}

/// Read back blob bytes stored by [`hash_blob`].
pub fn cat_blob(worktree: &str, oid: &str) -> Result<Vec<u8>, String> {
    if !valid_hash(oid) {
        return Err(format!("invalid object id '{oid}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "cat-file", "-p", oid])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(out.stdout.clone())
}

/// Read a to-be-discarded untracked file for the backup: `None` means it is
/// already gone (nothing to back up); over-cap files are refused so the queue
/// discards without undo rather than pretending.
pub fn read_untracked_for_undo(worktree: &str, path: &[u8]) -> Result<Option<Vec<u8>>, String> {
    match read_worktree_file(worktree, path, UNDO_FILE_CAP + 1) {
        Ok(bytes) if bytes.len() > UNDO_FILE_CAP => {
            Err("file is too large to back up for undo".to_string())
        }
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) => {
            if worktree_file_missing(worktree, path) {
                Ok(None)
            } else {
                Err(err)
            }
        }
    }
}

/// Rewrite a backed-up untracked file. Refuses when the path exists: the user
/// recreated it after the discard, and overwriting would destroy their work.
pub fn restore_untracked_file(
    worktree: &str,
    path: &[u8],
    bytes: &[u8],
) -> Result<(), String> {
    if path.is_empty() {
        return Err("no path given".to_string());
    }
    if !worktree_file_missing(worktree, path) {
        return Err("file was recreated since; undo refused".to_string());
    }
    write_worktree_file(worktree, path, bytes.to_vec())
}

/// Best-effort existence probe for backup capture: tells "already gone" apart
/// from real read failures.
fn worktree_file_missing(worktree: &str, path: &[u8]) -> bool {
    let relative = String::from_utf8_lossy(path);
    let joined = format!("{}/{}", worktree.trim_end_matches('/'), relative);
    if let Some(windows) = crate::model::host_path(&joined) {
        return !std::path::Path::new(&windows).exists();
    }
    if std::str::from_utf8(path).is_err() {
        return false;
    }
    run_linux("test", &["-e", &joined]).is_ok_and(|out| !out.success())
}

/// Drop stale remote-tracking branches that no longer exist on the remote.
pub fn remote_prune(worktree: &str, remote: &str) -> Result<(), String> {
    if remote.trim().is_empty() || remote.starts_with('-') {
        return Err(format!("invalid remote '{remote}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "remote", "prune", remote])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Point a remote at a new URL (fetch and push).
pub fn set_remote_url(worktree: &str, remote: &str, url: &str) -> Result<(), String> {
    if remote.trim().is_empty() || remote.starts_with('-') {
        return Err(format!("invalid remote '{remote}'"));
    }
    if url.trim().is_empty() || url.starts_with('-') {
        return Err(format!("invalid URL '{url}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "remote", "set-url", remote, url])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Rename a remote (tracking configuration follows the rename).
pub fn rename_remote(worktree: &str, old: &str, new: &str) -> Result<(), String> {
    if old.trim().is_empty()
        || old.starts_with('-')
        || new.trim().is_empty()
        || new.starts_with('-')
    {
        return Err(format!("invalid remote rename '{old}' → '{new}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "remote", "rename", old, new])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Remove a remote. Local branches stay where they are.
pub fn remove_remote(worktree: &str, remote: &str) -> Result<(), String> {
    if remote.trim().is_empty() || remote.starts_with('-') {
        return Err(format!("invalid remote '{remote}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "remote", "remove", remote])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Which ignore rule to derive for one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreKind {
    /// The file itself, anchored at the repo root (`/sub/dir/file.txt`).
    File,
    /// Its extension (`*.log`); none when the basename has no usable suffix.
    Extension,
    /// Its parent folder (`/sub/dir/`); none for repo-root files.
    Folder,
}

/// Escape gitignore pattern syntax so a name matches only itself:
/// `*?[\` and trailing spaces get a backslash (`pages/[id].tsx` must not
/// become a bracket expression).
fn escape_ignore(bytes: &[u8]) -> Vec<u8> {
    let trailing = bytes.len() - bytes.iter().rposition(|&b| b != b' ').map_or(0, |ix| ix + 1);
    let mut out = Vec::with_capacity(bytes.len() + 4);
    for (ix, &byte) in bytes.iter().enumerate() {
        if matches!(byte, b'*' | b'?' | b'[' | b'\\') || (byte == b' ' && ix >= bytes.len() - trailing) {
            out.push(b'\\');
        }
        out.push(byte);
    }
    out
}

/// Derive an ignore rule for `path` (repo-relative bytes, `/`-separated).
/// Pure byte logic: non-UTF-8 names work, and nothing touches the disk.
/// Names with line breaks cannot be written as a rule.
pub fn ignore_rule(path: &[u8], kind: IgnoreKind) -> Option<Vec<u8>> {
    if path.is_empty() || path.iter().any(|&b| b == b'\n' || b == b'\r') {
        return None;
    }
    match kind {
        IgnoreKind::File => {
            let mut rule = Vec::with_capacity(path.len() + 1);
            rule.push(b'/');
            rule.extend_from_slice(&escape_ignore(path));
            Some(rule)
        }
        IgnoreKind::Extension => {
            let basename = path.rsplit(|&byte| byte == b'/').next().unwrap_or(path);
            // No dot, a leading dot (dotfiles), or an empty suffix: no rule.
            let dot = basename.iter().rposition(|&byte| byte == b'.')?;
            let ext = basename.get(dot + 1 ..)?;
            if dot == 0 || ext.is_empty() {
                return None;
            }
            let mut rule = Vec::with_capacity(ext.len() + 2);
            rule.extend_from_slice(b"*.");
            rule.extend_from_slice(&escape_ignore(ext));
            Some(rule)
        }
        IgnoreKind::Folder => {
            let mut parts = path.rsplitn(2, |&byte| byte == b'/');
            let _file = parts.next();
            match parts.next() {
                Some(parent) if !parent.is_empty() => {
                    let mut rule = Vec::with_capacity(parent.len() + 2);
                    rule.push(b'/');
                    rule.extend_from_slice(&escape_ignore(parent));
                    rule.push(b'/');
                    Some(rule)
                }
                _ => None,
            }
        }
    }
}

/// Merge one rule into existing ignore-file bytes: `None` when the exact
/// line is already there (no write needed), otherwise the new content with
/// the line ending preserved. Pure for tests.
fn apply_ignore_rule(existing: &[u8], rule: &[u8]) -> Option<Vec<u8>> {
    let present = existing
        .split(|&byte| byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .any(|line| line == rule);
    if present {
        return None;
    }
    let mut merged = existing.to_vec();
    if !merged.is_empty() && !merged.ends_with(b"\n") {
        merged.push(b'\n');
    }
    merged.extend_from_slice(rule);
    merged.push(b'\n');
    Some(merged)
}

/// Cap for reading an ignore file before appending.
pub const IGNORE_FILE_CAP: usize = 1024 * 1024;

/// Append an ignore rule to `.gitignore` (or `.git/info/exclude`), then
/// optionally untrack the path itself (`git rm --cached`) for files that are
/// already tracked. Runs on the sequential change queue like every mutation.
pub fn ignore_path(
    worktree: &str,
    rule: &[u8],
    exclude: bool,
    untrack: Option<&[u8]>,
) -> Result<(), String> {
    if rule.is_empty() {
        return Err("no ignore rule given".to_string());
    }
    let target: &[u8] = if exclude {
        b".git/info/exclude"
    } else {
        b".gitignore"
    };
    // A missing file starts empty; any other read failure aborts, or the
    // write below would replace the whole file with the single rule.
    let existing = match read_worktree_file(worktree, target, IGNORE_FILE_CAP + 1) {
        Ok(bytes) => bytes,
        Err(_) if worktree_file_missing(worktree, target) => Vec::new(),
        Err(err) => return Err(err),
    };
    if existing.len() > IGNORE_FILE_CAP {
        return Err("ignore file is too large to edit here".to_string());
    }
    if let Some(merged) = apply_ignore_rule(&existing, rule) {
        write_worktree_file(worktree, target, merged)?;
    }
    if let Some(path) = untrack {
        index_command(worktree, "rm", &["--cached", "-q"], &[path.to_vec()])?;
    }
    Ok(())
}

/// A commit object id from our own history view is hex; anything else is
/// rejected before it can become a revision argument.
fn valid_hash(hash: &str) -> bool {
    !hash.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit())
}

/// Check out a commit detached (commit context menu). Git itself refuses when
/// local changes would be overwritten; the failure is reported, never forced.
pub fn checkout_detached(worktree: &str, hash: &str) -> Result<(), String> {
    if !valid_hash(hash) {
        return Err(format!("invalid commit '{hash}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "checkout", "--detach", hash])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Cherry-pick a commit onto the checked-out branch. Conflicts keep the
/// cherry-pick active so the existing resolver can finish it.
pub fn cherry_pick(worktree: &str, hash: &str) -> Result<(), String> {
    if !valid_hash(hash) {
        return Err(format!("invalid commit '{hash}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "cherry-pick", hash])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Revert a commit without opening an editor. Conflicts keep the revert
/// active so the existing resolver can finish it.
pub fn revert_commit(worktree: &str, hash: &str) -> Result<(), String> {
    if !valid_hash(hash) {
        return Err(format!("invalid commit '{hash}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "revert", "--no-edit", hash])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Rebase the checked-out branch onto `onto` (Rebase dialog). The branch is
/// rechecked first, so an external checkout cannot redirect the rebase. A
/// rebase that stops on conflicts is aborted, leaving the branch (and any
/// autostashed changes) exactly as before.
pub fn rebase_onto(
    worktree: &str,
    expected_branch: &str,
    onto: &str,
    autostash: bool,
) -> Result<(), String> {
    if onto.trim().is_empty() || onto.starts_with('-') {
        return Err(format!("invalid rebase target '{onto}'"));
    }
    let snapshot = status_snapshot(worktree)?;
    if snapshot.branch.as_deref() != Some(expected_branch) {
        return Err(format!(
            "the checked-out branch changed (expected '{expected_branch}')"
        ));
    }
    let stash_flag = if autostash { "--autostash" } else { "--no-autostash" };
    let out = run_for(worktree, &["-C", worktree, "rebase", stash_flag, onto])
        .map_err(|e| e.to_string())?;
    if out.success() {
        return Ok(());
    }
    let context = out.error_context();
    let markers = operation_markers(worktree).unwrap_or_default();
    if markers
        .iter()
        .any(|marker| marker == "rebase-merge" || marker == "rebase-apply")
    {
        let abort = run_for(worktree, &["-C", worktree, "rebase", "--abort"])
            .map_err(|e| e.to_string())?;
        if !abort.success() {
            return Err(format!(
                "{context}; aborting the rebase failed too: {}",
                abort.error_context()
            ));
        }
        return Err(format!(
            "{context} (the rebase stopped on conflicts and was aborted; '{expected_branch}' is unchanged)"
        ));
    }
    Err(context)
}

// ---- Reset dialog: move the checked-out branch to one commit ----

/// Reset mode of the reset dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

impl ResetMode {
    pub fn flag(self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        }
    }
}

/// Current branch name, or `None` when detached. A missing HEAD (unborn repo)
/// is an error, not a detach.
pub fn current_branch_name(worktree: &str) -> Result<Option<String>, String> {
    let out = run_for(
        worktree,
        &["-C", worktree, "symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .map_err(|e| e.to_string())?;
    if out.success() {
        return Ok(Some(out.stdout_text().trim().to_string()));
    }
    // Detached HEAD exits 1; anything without HEAD at all errors here.
    rev_parse(worktree, "HEAD")?;
    Ok(None)
}

/// Verify the branch/HEAD identity captured with a reset request. Refuses
/// when another tool moved the branch (or HEAD) meanwhile, so the reset can
/// never land on the wrong branch.
pub fn verify_reset_identity(
    worktree: &str,
    branch: Option<&str>,
    head: &str,
) -> Result<(), String> {
    if !valid_hash(head) {
        return Err(format!("invalid commit '{head}'"));
    }
    if current_branch_name(worktree)? != branch.map(str::to_string) {
        return Err("the checked-out branch changed since; reset refused".to_string());
    }
    if rev_parse(worktree, "HEAD")? != head {
        return Err("HEAD moved since; reset refused".to_string());
    }
    Ok(())
}

/// Reset the current branch (or detached HEAD) to `target`. Identity is
/// rechecked immediately before the reset runs; the backup is captured by
/// the queue before this executes.
pub fn reset_branch(
    worktree: &str,
    target: &str,
    mode: ResetMode,
    branch: Option<&str>,
    head: &str,
) -> Result<(), String> {
    if !valid_hash(target) {
        return Err(format!("invalid commit '{target}'"));
    }
    if let Some(name) = branch
        && (name.trim().is_empty() || name.starts_with('-'))
    {
        return Err(format!("invalid branch '{name}'"));
    }
    verify_reset_identity(worktree, branch, head)?;
    let out = run_for(worktree, &["-C", worktree, "reset", mode.flag(), target])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Commits `upstream` holds that `target` lacks: the force-push warning
/// count for the reset dialog. An unresolvable upstream is an error (the
/// dialog shows "unknown", never a confident zero).
pub fn pushed_ahead_count(
    worktree: &str,
    target: &str,
    upstream: &str,
) -> Result<u64, String> {
    if !valid_hash(target) {
        return Err(format!("invalid commit '{target}'"));
    }
    if upstream.trim().is_empty() || upstream.starts_with('-') {
        return Err(format!("invalid upstream '{upstream}'"));
    }
    let range = format!("{target}..{upstream}");
    let out = run_for(worktree, &["-C", worktree, "rev-list", "--count", &range])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    out.stdout_text()
        .trim()
        .parse()
        .map_err(|_| "could not count pushed commits".to_string())
}

/// Create a tag at a commit: annotated when `message` is non-empty, otherwise
/// lightweight. An existing name is refused by git and reported.
pub fn create_tag(
    worktree: &str,
    name: &str,
    hash: &str,
    message: &str,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('-') {
        return Err(format!("invalid tag name '{name}'"));
    }
    if !valid_hash(hash) {
        return Err(format!("invalid commit '{hash}'"));
    }
    let mut args = vec!["-C", worktree, "tag"];
    let message = message.trim();
    if !message.is_empty() {
        args.extend(["-a", "-m", message]);
    }
    args.extend([name, hash]);
    let out = run_for(worktree, &args).map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// The commit a tag points to (dereferenced: annotated tags resolve through
/// their tag object). Used to check a tag out detached.
pub fn tag_commit(worktree: &str, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('-') {
        return Err(format!("invalid tag name '{name}'"));
    }
    let revision = format!("{name}^{{commit}}");
    let out = run_for(worktree, &["-C", worktree, "rev-parse", "--verify", &revision])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(out.stdout_text().trim().to_string())
}

/// Push one tag to a remote.
pub fn push_tag(worktree: &str, remote: &str, name: &str) -> Result<(), String> {
    if remote.trim().is_empty() || remote.starts_with('-') {
        return Err(format!("invalid remote '{remote}'"));
    }
    let name = name.trim();
    if name.is_empty() || name.starts_with('-') {
        return Err(format!("invalid tag name '{name}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "push", remote, "tag", name])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// [`create_tag`], then push it to `remote` when one is given. A failed
/// creation never pushes: the name may already belong to a different tag.
pub fn create_tag_and_push(
    worktree: &str,
    name: &str,
    hash: &str,
    message: &str,
    remote: Option<&str>,
) -> Result<(), String> {
    create_tag(worktree, name, hash, message)?;
    if let Some(remote) = remote {
        push_tag(worktree, remote, name.trim())
            .map_err(|err| format!("the tag was created, but pushing it failed: {err}"))?;
    }
    Ok(())
}

/// Delete a tag locally and/or on the given remotes (`:refs/tags/<name>`
/// refspec). Local goes first so a network failure still leaves a truthful
/// error while the local delete stands; the result names what happened.
pub fn delete_tag(
    worktree: &str,
    name: &str,
    local: bool,
    remotes: &[String],
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('-') {
        return Err(format!("invalid tag name '{name}'"));
    }
    if local {
        let out = run_for(worktree, &["-C", worktree, "tag", "-d", name])
            .map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(out.error_context());
        }
    }
    for remote in remotes {
        if remote.trim().is_empty() || remote.starts_with('-') {
            return Err(format!("invalid remote '{remote}'"));
        }
        let refspec = format!(":refs/tags/{name}");
        let out = run_for(worktree, &["-C", worktree, "push", remote, &refspec])
            .map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(out.error_context());
        }
    }
    Ok(())
}

/// An apply-able patch of one commit for "Copy patch".
pub fn commit_patch(worktree: &str, hash: &str) -> Result<String, String> {
    if !valid_hash(hash) {
        return Err(format!("invalid commit '{hash}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "format-patch", "--stdout", "-1", hash])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(out.stdout_text().to_string())
}

/// Apply-able patch of one worktree file side for "Copy patch".
/// Tracked sides come from the same `git diff` the pane renders, so the
/// clipboard matches the screen; untracked files have no Git diff and are
/// synthesized as a new-file patch. Reads are unbounded: a copy must be
/// complete, not preview-capped.
pub fn worktree_file_patch(
    worktree: &str,
    path: &[u8],
    staged: bool,
    untracked: bool,
) -> Result<String, String> {
    if path.is_empty() {
        return Err("no path given".to_string());
    }
    if untracked && !staged {
        let bytes = read_worktree_file(worktree, path, DIFF_BYTE_CAP + 1)?;
        return new_file_patch(path, bytes);
    }
    // Literal pathspecs like the other path actions: `--` alone does
    // not stop Git from expanding `*`/`[`/`?` into extra files.
    let path_arg = String::from_utf8_lossy(path).into_owned();
    let mut args: Vec<&str> = vec![
        "--literal-pathspecs",
        "-C",
        worktree,
        "--no-pager",
        "--no-optional-locks",
        "diff",
    ];
    if staged {
        args.push("--cached");
    }
    args.extend([
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--unified=3",
        "--",
        &path_arg,
    ]);
    let out = run_for(worktree, &args).map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    if out.stdout.is_empty() {
        return Err("no patch available for this file".to_string());
    }
    Ok(out.stdout_text().to_string())
}

/// Apply-able patch of one stashed file for "Copy patch". Tracked
/// files come from the same `stash^..stash` diff the pane renders; files
/// that were untracked in the stash are synthesized from `stash^3`.
pub fn stash_file_patch(
    worktree: &str,
    stash: &str,
    path: &[u8],
    untracked: bool,
) -> Result<String, String> {
    if stash.trim().is_empty() || stash.starts_with('-') {
        return Err(format!("invalid stash reference '{stash}'"));
    }
    if path.is_empty() {
        return Err("no path given".to_string());
    }
    if untracked {
        let spec = format!("{stash}^3:{}", String::from_utf8_lossy(path));
        let out =
            run_for(worktree, &["-C", worktree, "show", &spec]).map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(out.error_context());
        }
        return new_file_patch(path, out.stdout);
    }
    let path_arg = String::from_utf8_lossy(path).into_owned();
    let parent = format!("{stash}^");
    let out = run_for(
        worktree,
        &[
            "--literal-pathspecs",
            "-C",
            worktree,
            "--no-pager",
            "--no-optional-locks",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--unified=3",
            "-M",
            &parent,
            stash,
            "--",
            &path_arg,
        ],
    )
    .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    if out.stdout.is_empty() {
        return Err("no patch available for this file".to_string());
    }
    Ok(out.stdout_text().to_string())
}

/// Synthesized new-file patch for bytes with no Git diff (untracked
/// worktree files, stashed untracked blobs): the shape
/// `git diff --no-index /dev/null <file>` produces, minus the index line,
/// so the result applies with `git apply`. Lines mirror the preview parser
/// (CR stripped, joined on LF). Binary and over-cap inputs are refused — a
/// truncated patch would silently drop content.
fn new_file_patch(path: &[u8], bytes: Vec<u8>) -> Result<String, String> {
    if bytes.len() > DIFF_BYTE_CAP {
        return Err("file is too large to copy as a patch".to_string());
    }
    if bytes.iter().take(8192).any(|&byte| byte == 0) {
        return Err("cannot copy a patch of a binary file".to_string());
    }
    let display = String::from_utf8_lossy(path).into_owned();
    let ours = quote_diff_path(&format!("a/{display}"));
    let theirs = quote_diff_path(&format!("b/{display}"));
    let mut lines: Vec<&[u8]> = bytes.split(|&byte| byte == b'\n').collect();
    let trailing_newline = matches!(lines.last(), Some(last) if last.is_empty());
    if trailing_newline {
        lines.pop();
    }
    let start = if lines.is_empty() { 0 } else { 1 };
    let mut patch = format!(
        "diff --git {ours} {theirs}\nnew file mode 100644\n--- /dev/null\n+++ {theirs}\n@@ -0,0 +{start},{} @@\n",
        lines.len()
    );
    for line in lines {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        patch.push('+');
        patch.push_str(&String::from_utf8_lossy(line));
        patch.push('\n');
    }
    if !trailing_newline {
        patch.push_str("\\ No newline at end of file\n");
    }
    Ok(patch)
}

/// Quote a diff-header path the way Git does when it must: C-style double
/// quotes around `a/…`/`b/…` for spaces, quotes, backslashes, and control
/// bytes, so the copied patch still applies. Plain paths stay bare.
fn quote_diff_path(path: &str) -> String {
    if path
        .bytes()
        .any(|byte| byte == b' ' || byte == b'"' || byte == b'\\' || byte < 0x20 || byte == 0x7f)
    {
        let mut quoted = String::with_capacity(path.len() + 2);
        quoted.push('"');
        for ch in path.chars() {
            match ch {
                '"' => quoted.push_str("\\\""),
                '\\' => quoted.push_str("\\\\"),
                '\n' => quoted.push_str("\\n"),
                '\t' => quoted.push_str("\\t"),
                ch if ch < '\u{20}' || ch == '\u{7f}' => {
                    quoted.push_str(&format!("\\{:03o}", ch as u32))
                }
                ch => quoted.push(ch),
            }
        }
        quoted.push('"');
        quoted
    } else {
        path.to_string()
    }
}

/// Add a remote to the repository (`git remote add <name> <url>`).
pub fn add_remote(worktree: &str, name: &str, url: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.starts_with('-') {
        return Err(format!("invalid remote name '{name}'"));
    }
    if url.trim().is_empty() || url.starts_with('-') {
        return Err(format!("invalid remote URL '{url}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "remote", "add", name, url])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Push one local branch to an explicit remote branch. The refspec is
/// spelled out (`refs/heads/local:refs/heads/target`) so the push destination
/// never depends on `push.default` or other user configuration, and no force
/// option is ever passed. `set_upstream` adds `--set-upstream` for a first
/// push or a changed destination.
pub fn push_branch(
    worktree: &str,
    remote: &str,
    local: &str,
    target: &str,
    set_upstream: bool,
) -> Result<(), String> {
    if remote.trim().is_empty() || remote.starts_with('-') {
        return Err(format!("invalid remote name '{remote}'"));
    }
    if local.trim().is_empty() || local.starts_with('-') {
        return Err(format!("invalid branch name '{local}'"));
    }
    if target.trim().is_empty() || target.starts_with('-') {
        return Err(format!("invalid target branch '{target}'"));
    }
    let refspec = format!("refs/heads/{local}:refs/heads/{target}");
    let mut args: Vec<&str> = vec!["-C", worktree, "push", "--progress"];
    if set_upstream {
        args.push("--set-upstream");
    }
    args.push(remote);
    args.push(&refspec);
    let out = run_for(worktree, &args).map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// One changed file of a commit (`--name-status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    /// Status letter (`A`/`M`/`D`/`R`/`C`/`T`/`U`); rename scores are dropped.
    pub status: char,
    pub path: Vec<u8>,
    /// Source path of a rename/copy.
    pub orig_path: Option<Vec<u8>>,
}

impl CommitFile {
    /// Display path: `old → new` for renames, sanitized.
    pub fn display_path(&self) -> String {
        match &self.orig_path {
            Some(orig) => format!(
                "{} → {}",
                crate::process::sanitize(orig),
                crate::process::sanitize(&self.path)
            ),
            None => crate::process::sanitize(&self.path),
        }
    }
}

/// Commit metadata and changed files for the detail panel, loaded on demand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetail {
    pub hash: String,
    pub author: String,
    pub email: String,
    /// ISO-8601 author date (`%aI`).
    pub date: String,
    pub parents: Vec<String>,
    /// Full raw message (subject + body).
    pub message: String,
    pub files: Vec<CommitFile>,
    /// Total added lines across the changed files (binary files contribute 0).
    pub additions: u32,
    /// Total deleted lines across the changed files.
    pub deletions: u32,
}

/// Metadata and changed files of one commit. The file list is the diff against
/// the first parent (merges included); root commits diff against the empty
/// tree (`--root`). The metadata, file list, and line totals are independent
/// reads and run in parallel — one round-trip instead of three; only a root
/// commit (whose `hash^` is unresolvable) falls back to the `diff-tree` pair.
pub fn commit_detail(worktree: &str, hash: &str) -> Result<CommitDetail, String> {
    let format = format!("%an{FS}%ae{FS}%aI{FS}%P{FS}%B");
    let parent_rev = format!("{hash}^");
    let (meta_out, files_out, numstat_out) = std::thread::scope(|scope| {
        let meta = scope.spawn(|| {
            run_for(
                worktree,
                &[
                    "-C",
                    worktree,
                    "--no-optional-locks",
                    "show",
                    "--no-patch",
                    &format!("--format={format}"),
                    hash,
                ],
            )
        });
        let files = scope.spawn(|| {
            run_for(
                worktree,
                &[
                    "-C",
                    worktree,
                    "--no-optional-locks",
                    "diff",
                    "--name-status",
                    "-z",
                    "-M",
                    &parent_rev,
                    hash,
                ],
            )
        });
        let numstat = scope.spawn(|| {
            run_for(
                worktree,
                &[
                    "-C",
                    worktree,
                    "--no-optional-locks",
                    "diff",
                    "--numstat",
                    "-M",
                    &parent_rev,
                    hash,
                ],
            )
        });
        (meta.join(), files.join(), numstat.join())
    });
    let join = |name: &str,
                joined: Result<io::Result<Output>, Box<dyn std::any::Any + Send>>|
     -> Result<Output, String> {
        joined
            .map_err(|_| format!("{name} panicked"))?
            .map_err(|e| e.to_string())
    };
    let meta_out = join("git show", meta_out)?;
    if !meta_out.success() {
        return Err(meta_out.error_context());
    }
    let text = meta_out.stdout_text();
    let mut fields = text.splitn(5, FS);
    let author = fields.next().unwrap_or_default().to_string();
    let email = fields.next().unwrap_or_default().to_string();
    let date = fields.next().unwrap_or_default().to_string();
    let parents: Vec<String> = fields
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let message = fields.next().unwrap_or_default().to_string();
    if author.is_empty() {
        return Err(format!("git show did not report an author for {hash}"));
    }

    // Root commit: `hash^` does not resolve, so re-read the pair through
    // `diff-tree --root` (rare; the normal path never needs it).
    let root = parents.is_empty();
    let files_out = join("git diff --name-status", files_out)?;
    let files = if files_out.success() {
        parse_name_status(&files_out.stdout)?
    } else if root {
        let out = run_for(
            worktree,
            &[
                "-C",
                worktree,
                "--no-optional-locks",
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-status",
                "-r",
                "-z",
                "-M",
                hash,
            ],
        )
        .map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(out.error_context());
        }
        parse_name_status(&out.stdout)?
    } else {
        return Err(files_out.error_context());
    };

    let numstat_out = join("git diff --numstat", numstat_out)?;
    let (additions, deletions) = if numstat_out.success() {
        parse_numstat_totals(&numstat_out.stdout)
    } else if root {
        let out = run_for(
            worktree,
            &[
                "-C",
                worktree,
                "--no-optional-locks",
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--numstat",
                "-r",
                "-M",
                hash,
            ],
        )
        .map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(out.error_context());
        }
        parse_numstat_totals(&out.stdout)
    } else {
        return Err(numstat_out.error_context());
    };

    Ok(CommitDetail {
        hash: hash.to_string(),
        author,
        email,
        date,
        parents,
        message,
        files,
        additions,
        deletions,
    })
}

/// Sum `--numstat` output into `(added, deleted)`; binary entries (`-`) and
/// malformed lines contribute nothing instead of failing the whole detail.
fn parse_numstat_totals(bytes: &[u8]) -> (u32, u32) {
    let mut additions = 0u32;
    let mut deletions = 0u32;
    for line in bytes.split(|&byte| byte == b'\n') {
        let line = String::from_utf8_lossy(line);
        let mut fields = line.split('\t');
        let (Some(added), Some(deleted)) = (fields.next(), fields.next()) else {
            continue;
        };
        additions = additions.saturating_add(added.trim().parse::<u32>().unwrap_or(0));
        deletions = deletions.saturating_add(deleted.trim().parse::<u32>().unwrap_or(0));
    }
    (additions, deletions)
}

/// Parse `--name-status -z` bytes: `S\0path\0`, renames/copies add the source
/// first (`R100\0old\0new\0`).
fn parse_name_status(bytes: &[u8]) -> Result<Vec<CommitFile>, String> {
    let mut fields = bytes.split(|&byte| byte == 0);
    let mut files = Vec::new();
    while let Some(status) = fields.next() {
        if status.is_empty() {
            continue;
        }
        let letter = status[0] as char;
        let Some(first) = fields.next().filter(|path| !path.is_empty()) else {
            return Err(format!("name-status record {letter:?} is missing its path"));
        };
        if matches!(letter, 'R' | 'C') {
            let Some(second) = fields.next().filter(|path| !path.is_empty()) else {
                return Err(format!(
                    "name-status rename record for {:?} is missing its destination",
                    crate::process::sanitize(first)
                ));
            };
            files.push(CommitFile {
                status: letter,
                path: second.to_vec(),
                orig_path: Some(first.to_vec()),
            });
        } else {
            files.push(CommitFile {
                status: letter,
                path: first.to_vec(),
                orig_path: None,
            });
        }
    }
    Ok(files)
}

/// One file of a commit as a unified diff (read-only detail pane). `parent`
/// is the first parent; `None` means a root commit.
pub fn commit_file_diff(
    worktree: &str,
    parent: Option<&str>,
    hash: &str,
    path: &[u8],
) -> Result<FileDiff, String> {
    let path = String::from_utf8_lossy(path).into_owned();
    let out = if let Some(parent) = parent {
        run_for_preview(
            worktree,
            &[
                "--literal-pathspecs",
                "-C",
                worktree,
                "--no-pager",
                "--no-optional-locks",
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--unified=3",
                "-M",
                parent,
                hash,
                "--",
                &path,
            ],
        )
    } else {
        run_for_preview(
            worktree,
            &[
                "--literal-pathspecs",
                "-C",
                worktree,
                "--no-pager",
                "--no-optional-locks",
                "diff-tree",
                "--root",
                "-p",
                "--no-commit-id",
                "-r",
                "-M",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--unified=3",
                hash,
                "--",
                &path,
            ],
        )
    }
    .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(parse_unified_diff(&out.stdout))
}

/// Fetch remote changes: one remote or all remotes,
/// with optional `--force` (override local refs) and `--no-tags`.
pub fn fetch_remote(
    worktree: &str,
    remote: Option<&str>,
    force: bool,
    no_tags: bool,
) -> Result<(), String> {
    let mut args: Vec<&str> = vec!["-C", worktree, "fetch", "--progress"];
    if force {
        args.push("--force");
    }
    if no_tags {
        args.push("--no-tags");
    }
    match remote {
        Some(remote) => {
            if remote.trim().is_empty() || remote.starts_with('-') {
                return Err(format!("invalid remote name '{remote}'"));
            }
            args.push(remote);
        }
        None => args.push("--all"),
    }
    let out = run_for(worktree, &args).map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Local-changes handling for an explicit pull (Pull dialog).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PullChanges {
    /// Pull over the local changes; Git refuses when they would be
    /// overwritten.
    #[default]
    Nothing,
    /// Stash everything (untracked included), pull, then reapply the stash.
    StashReapply,
    /// Move everything into a stash and pull without reapplying; the changes
    /// stay recoverable in the stash list (the stash dialog's "Discard").
    Discard,
}

/// One explicit pull of `remote/branch` into the checked-out branch. The
/// integration mode is explicit (`--rebase` or `--ff --no-rebase`), so user
/// configuration (`pull.rebase`, `pull.ff`, `branch.<name>.rebase`) cannot
/// decide it; autostash and submodule recursion are disabled because the
/// dialog manages local changes itself and the release policy skips
/// submodules. The checked-out branch is rechecked immediately before the
/// pull, so an external checkout cannot redirect the operation.
pub fn pull_remote(
    worktree: &str,
    remote: &str,
    branch: &str,
    expected_local: &str,
    rebase: bool,
    changes: PullChanges,
) -> Result<(), String> {
    if remote.trim().is_empty() || remote.starts_with('-') {
        return Err(format!("invalid remote name '{remote}'"));
    }
    if branch.trim().is_empty() || branch.starts_with('-') {
        return Err(format!("invalid branch name '{branch}'"));
    }
    let snapshot = status_snapshot(worktree)?;
    if snapshot.branch.as_deref() != Some(expected_local) {
        return Err(format!(
            "the checked-out branch changed (expected '{expected_local}')"
        ));
    }
    let mut stashed = false;
    if changes != PullChanges::Nothing && !snapshot.is_clean() {
        let out = run_for(
            worktree,
            &[
                "-C",
                worktree,
                "stash",
                "push",
                "--include-untracked",
                "-m",
                "Spur: pull",
            ],
        )
        .map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(out.error_context());
        }
        stashed = true;
    }
    let mut args: Vec<&str> = vec!["-C", worktree, "pull", "--progress"];
    if rebase {
        args.push("--rebase");
    } else {
        args.push("--ff");
        args.push("--no-rebase");
    }
    args.extend([
        "--no-autostash",
        "--no-recurse-submodules",
        remote,
        branch,
    ]);
    let out = run_for(worktree, &args).map_err(|e| e.to_string())?;
    if !out.success() {
        let context = out.error_context();
        // A failed pull never auto-restores the stash: the error tells the
        // user the changes are safe in the stash list instead.
        return Err(if stashed {
            format!("{context} (local changes are in the latest stash)")
        } else {
            context
        });
    }
    if stashed && changes == PullChanges::StashReapply {
        let out = run_for(worktree, &["-C", worktree, "stash", "pop"])
            .map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(format!(
                "the pull succeeded, but restoring the stashed changes failed: {}",
                out.error_context()
            ));
        }
    }
    Ok(())
}

/// Changed files of one stash entry, with the paths that live in the stash's
/// untracked-files commit (`stash^3`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashChanges {
    pub files: Vec<CommitFile>,
    pub untracked: Vec<Vec<u8>>,
}

/// List the files a stash entry changes (tracked and, when the stash was
/// created with untracked content, those too).
///
/// Speed: the file list and the untracked-files list are
/// independent reads, so they run on parallel threads (one `wsl.exe`
/// round-trip). The untracked list is read straight from `stash^3`; a stash
/// without untracked content fails that read, which is "no untracked files"
/// — the previous explicit `rev-parse` probe cost a whole extra round-trip.
pub fn stash_changes(worktree: &str, stash: &str) -> Result<StashChanges, String> {
    if stash.trim().is_empty() || stash.starts_with('-') {
        return Err(format!("invalid stash reference '{stash}'"));
    }
    let untracked_spec = format!("{stash}^3");
    let (show_out, untracked_out) = std::thread::scope(|scope| {
        let show = scope.spawn(|| {
            run_for(
                worktree,
                &[
                    "-C",
                    worktree,
                    "--no-optional-locks",
                    "stash",
                    "show",
                    "--name-status",
                    "-z",
                    "-M",
                    "--include-untracked",
                    stash,
                ],
            )
        });
        let untracked = scope.spawn(|| {
            run_for(
                worktree,
                &["-C", worktree, "ls-tree", "-r", "--name-only", "-z", &untracked_spec],
            )
        });
        (show.join(), untracked.join())
    });
    let show_out = show_out
        .map_err(|_| "git stash show panicked".to_string())?
        .map_err(|e| e.to_string())?;
    if !show_out.success() {
        return Err(show_out.error_context());
    }
    let files = parse_name_status(&show_out.stdout)?;

    let untracked_out = untracked_out
        .map_err(|_| "git ls-tree panicked".to_string())?
        .map_err(|e| e.to_string())?;
    let untracked = if untracked_out.success() {
        untracked_out
            .stdout
            .split(|&byte| byte == 0)
            .filter(|path| !path.is_empty())
            .map(<[u8]>::to_vec)
            .collect()
    } else {
        Vec::new()
    };
    Ok(StashChanges { files, untracked })
}

/// Apply a stash entry to the worktree, keeping the entry (`git stash apply`).
pub fn stash_apply(worktree: &str, stash: &str) -> Result<(), String> {
    stash_apply_index(worktree, stash, false)
}

/// Apply a stash entry, restoring the index too when asked. Undoing a staged
/// discard uses `--index`; the plain path is the stash menu behavior.
pub fn stash_apply_index(worktree: &str, stash: &str, with_index: bool) -> Result<(), String> {
    if stash.trim().is_empty() || stash.starts_with('-') {
        return Err(format!("invalid stash reference '{stash}'"));
    }
    let mut args = vec!["-C", worktree, "stash", "apply"];
    if with_index {
        args.push("--index");
    }
    args.push(stash);
    let out = run_for(worktree, &args).map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Pop a stash entry: apply, then drop on success (`git stash pop`). A
/// conflicting pop keeps the entry and reports, like apply.
pub fn stash_pop(worktree: &str, stash: &str) -> Result<(), String> {
    if stash.trim().is_empty() || stash.starts_with('-') {
        return Err(format!("invalid stash reference '{stash}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "stash", "pop", stash])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Create and check out a branch at a stash entry (`git stash branch`),
/// dropping the entry on success.
pub fn stash_branch(worktree: &str, stash: &str, name: &str) -> Result<(), String> {
    if stash.trim().is_empty() || stash.starts_with('-') {
        return Err(format!("invalid stash reference '{stash}'"));
    }
    let name = name.trim();
    if name.is_empty() || name.starts_with('-') {
        return Err(format!("invalid branch name '{name}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "stash", "branch", name, stash])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Restore one stashed file to the worktree without touching the index, so
/// the result shows up in Local Changes for review. Untracked-in-stash files
/// come from the stash's third parent. Byte-exact literal pathspecs.
pub fn stash_apply_file(
    worktree: &str,
    stash: &str,
    path: &[u8],
    untracked: bool,
) -> Result<(), String> {
    if stash.trim().is_empty() || stash.starts_with('-') {
        return Err(format!("invalid stash reference '{stash}'"));
    }
    if path.is_empty() {
        return Err("no path given".to_string());
    }
    let source = if untracked {
        format!("{stash}^3")
    } else {
        stash.to_string()
    };
    let source_ref = source.as_str();
    index_command(worktree, "restore", &["--source", source_ref, "--worktree"], &[path.to_vec()])
}

/// Current `stash@{n}` of the entry whose commit is `oid`. Queued stash
/// ops carry the id they were clicked with; positions shift on every
/// push/pop/drop, so they re-resolve by identity before running.
pub fn resolve_stash_ref(worktree: &str, oid: &str) -> Result<String, String> {
    if !valid_hash(oid) {
        return Err(format!("invalid object id '{oid}'"));
    }
    let out = run_for(
        worktree,
        &["-C", worktree, "stash", "list", "--format=%gd %H"],
    )
    .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    out.stdout_text()
        .lines()
        .find_map(|line| {
            let (id, hash) = line.split_once(' ')?;
            (hash.trim() == oid).then(|| id.to_string())
        })
        .ok_or_else(|| "the stash entry is gone (the stash list changed)".to_string())
}

/// Drop a stash entry (`git stash drop`). Destructive; the UI confirms first.
pub fn stash_drop(worktree: &str, stash: &str) -> Result<(), String> {
    if stash.trim().is_empty() || stash.starts_with('-') {
        return Err(format!("invalid stash reference '{stash}'"));
    }
    let out = run_for(worktree, &["-C", worktree, "stash", "drop", stash])
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Read-only unified diff of one file inside a stash. Untracked-in-stash files
/// are previewed from the `stash^3` blob as an all-added diff.
pub fn stash_file_diff(
    worktree: &str,
    stash: &str,
    path: &[u8],
    untracked: bool,
) -> Result<FileDiff, String> {
    if untracked {
        let spec = format!("{stash}^3:{}", String::from_utf8_lossy(path));
        let out =
            run_for_preview(worktree, &["-C", worktree, "show", &spec]).map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(out.error_context());
        }
        // Bounded display: the capture stops one byte past the shared diff
        // cap, so `all_added_diff` still sees an over-cap input.
        return Ok(all_added_diff(out.stdout));
    }
    let path = String::from_utf8_lossy(path).into_owned();
    let parent = format!("{stash}^");
    let out = run_for_preview(
        worktree,
        &[
            "--literal-pathspecs",
            "-C",
            worktree,
            "--no-pager",
            "--no-optional-locks",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--unified=3",
            "-M",
            &parent,
            stash,
            "--",
            &path,
        ],
    )
    .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(parse_unified_diff(&out.stdout))
}

/// Added/deleted line totals of a stash-like commit (raw object ids work,
/// like everywhere else), narrowed to `paths` when given: the
/// Recently-discarded size line. Binary entries count nothing, like the
/// commit totals.
pub fn stash_numstat(worktree: &str, stash: &str, paths: &[Vec<u8>]) -> Result<(u32, u32), String> {
    if stash.trim().is_empty() || stash.starts_with('-') {
        return Err(format!("invalid stash reference '{stash}'"));
    }
    let parent = format!("{stash}^");
    let specs = utf8_pathspecs(paths)?;
    let mut args = vec![
        "--literal-pathspecs",
        "-C",
        worktree,
        "diff",
        "--numstat",
        &parent,
        stash,
        "--",
    ];
    args.extend(specs.iter().map(String::as_str));
    let out = run_for(worktree, &args).map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(parse_numstat_totals(&out.stdout))
}

/// Diff one file for the given side. Untracked files have no Git diff, so
/// their content is synthesized as an all-added preview.
pub fn diff_file(
    worktree: &str,
    path: &[u8],
    staged: bool,
    untracked: bool,
) -> Result<FileDiff, String> {
    if untracked && !staged {
        return untracked_diff(worktree, path);
    }
    let path = String::from_utf8_lossy(path).into_owned();
    let mut args: Vec<&str> = vec![
        "--literal-pathspecs",
        "-C",
        worktree,
        "--no-pager",
        "--no-optional-locks",
        "diff",
    ];
    if staged {
        args.push("--cached");
    }
    // No external diff, textconv, pager, or color: the pane is the only viewer.
    // Capture is bounded to the diff cap.
    args.extend(["--no-ext-diff", "--no-textconv", "--no-color", "--unified=3", "--", &path]);
    let out = run_for_preview(worktree, &args).map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(parse_unified_diff(&out.stdout))
}

fn untracked_diff(worktree: &str, path: &[u8]) -> Result<FileDiff, String> {
    let bytes = read_worktree_file(worktree, path, DIFF_BYTE_CAP + 1)?;
    Ok(all_added_diff(bytes))
}

/// All-added preview for bytes that have no Git diff (untracked worktree
/// files, stashed untracked blobs): binary detection and the shared byte cap.
pub(crate) fn all_added_diff(mut bytes: Vec<u8>) -> FileDiff {
    let mut diff = FileDiff::default();
    if bytes.len() > DIFF_BYTE_CAP {
        diff.truncated = true;
        bytes.truncate(DIFF_BYTE_CAP);
        if let Some(end) = bytes.iter().rposition(|&byte| byte == b'\n') {
            bytes.truncate(end + 1);
        }
    }
    if bytes.iter().take(8192).any(|&byte| byte == 0) {
        diff.binary = true;
        return diff;
    }
    let mut lines: Vec<(String, Vec<u8>)> = Vec::new();
    for raw in bytes.split(|&byte| byte == b'\n') {
        if raw.is_empty() {
            continue;
        }
        if lines.len() >= MAX_DIFF_ROWS {
            diff.truncated = true;
            break;
        }
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        lines.push((crate::process::sanitize(raw), raw.to_vec()));
    }
    diff.additions = lines.len();
    diff.lines.push(DiffLine::new(
        DiffLineKind::Hunk,
        None,
        None,
        format!("@@ -0,0 +1,{} @@ (untracked)", lines.len()),
        Vec::new(),
    ));
    for (ix, (text, raw)) in lines.into_iter().enumerate() {
        diff.lines.push(DiffLine::new(
            DiffLineKind::Added,
            None,
            Some(ix as u32 + 1),
            text,
            raw,
        ));
    }
    diff.max_chars = diff
        .lines
        .iter()
        .map(|line| line.text.chars().count())
        .max()
        .unwrap_or(0);
    diff
}

/// Read a worktree-relative file for the untracked preview, bounded to `max`
/// bytes, through the repository's host (native read for `/mnt` mounts, the
/// distro's `head` otherwise).
fn read_worktree_file(worktree: &str, path: &[u8], max: usize) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let relative = String::from_utf8_lossy(path);
    let joined = format!("{}/{}", worktree.trim_end_matches('/'), relative);
    if crate::model::host_path(worktree).is_some() {
        let windows = crate::model::host_path(&joined)
            .ok_or_else(|| format!("path is not representable on this host: {relative}"))?;
        let file = std::fs::File::open(&windows).map_err(|e| e.to_string())?;
        let mut data = Vec::new();
        file.take(max as u64)
            .read_to_end(&mut data)
            .map_err(|e| e.to_string())?;
        Ok(data)
    } else if relative.contains('\u{fffd}') || std::str::from_utf8(path).is_err() {
        // The path cannot cross the wsl.exe command line; staging still works
        // byte-exactly through stdin.
        Err("diff for a non-UTF-8 path is not supported on this host".to_string())
    } else {
        let max = max.to_string();
        let out = run_linux("head", &["-c", &max, &joined]).map_err(|e| e.to_string())?;
        if !out.success() {
            return Err(out.error_context());
        }
        Ok(out.stdout)
    }
}

// ---- merge conflicts: detect, compare, resolve per section ----

/// Ceiling for a conflicted worktree file read into the resolver.
pub const CONFLICT_FILE_CAP: usize = 8 * 1024 * 1024;

/// The user's choice for one conflict section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    Ours,
    Theirs,
    Both,
}

/// One region of a conflicted worktree file, kept as exact bytes (line endings
/// included) so a resolution is byte-faithful for the untouched parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictSection {
    Common(Vec<u8>),
    Conflict {
        /// Marker labels (`HEAD`, the incoming branch, …); empty when the
        /// marker carried none.
        ours_label: String,
        theirs_label: String,
        ours: Vec<u8>,
        theirs: Vec<u8>,
    },
}

impl ConflictSection {
    pub fn is_conflict(&self) -> bool {
        matches!(self, ConflictSection::Conflict { .. })
    }

    /// Display lines of a section side (no line endings, sanitized).
    pub fn lines(bytes: &[u8]) -> Vec<String> {
        String::from_utf8_lossy(bytes)
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(|line| crate::process::sanitize(line.as_bytes()))
            .collect()
    }
}

/// Detect a marker line and its label: exactly seven marker characters at the
/// line start (Git's format), with the rest as the label.
fn marker_line(body: &[u8], marker: u8) -> Option<&[u8]> {
    if body.len() >= 7 && body[..7].iter().all(|&byte| byte == marker) {
        Some(&body[7..])
    } else {
        None
    }
}

fn marker_label(rest: &[u8], fallback: &str) -> String {
    let label = String::from_utf8_lossy(rest).trim().to_string();
    if label.is_empty() {
        fallback.to_string()
    } else {
        label
    }
}

/// Parse conflict markers out of a worktree file's bytes. Common text and both
/// sides are kept byte-exact; a `|||||||` base block (diff3 style) is skipped.
/// Unterminated or nested markers are errors rather than silent guesses.
pub fn parse_conflict_markers(bytes: &[u8]) -> Result<Vec<ConflictSection>, String> {
    #[derive(PartialEq)]
    enum Phase {
        Ours,
        Base,
        Theirs,
    }
    struct Open {
        ours_label: String,
        theirs_label: String,
        ours: Vec<u8>,
        theirs: Vec<u8>,
        phase: Phase,
    }

    let mut sections = Vec::new();
    let mut common: Vec<u8> = Vec::new();
    let mut open: Option<Open> = None;
    for line in bytes.split_inclusive(|&byte| byte == b'\n') {
        let body = line.strip_suffix(b"\n").unwrap_or(line);
        let body = body.strip_suffix(b"\r").unwrap_or(body);
        if let Some(rest) = marker_line(body, b'<') {
            if open.is_some() {
                return Err("nested conflict markers".to_string());
            }
            if !common.is_empty() {
                sections.push(ConflictSection::Common(std::mem::take(&mut common)));
            }
            open = Some(Open {
                ours_label: marker_label(rest, "ours"),
                theirs_label: "theirs".to_string(),
                ours: Vec::new(),
                theirs: Vec::new(),
                phase: Phase::Ours,
            });
            continue;
        }
        if let Some(current) = open.as_mut() {
            if marker_line(body, b'|').is_some() {
                if current.phase != Phase::Ours {
                    return Err("misplaced diff3 base marker".to_string());
                }
                current.phase = Phase::Base;
                continue;
            }
            if marker_line(body, b'=').is_some() {
                if current.phase == Phase::Theirs {
                    return Err("misplaced conflict separator".to_string());
                }
                current.phase = Phase::Theirs;
                continue;
            }
            if let Some(rest) = marker_line(body, b'>') {
                current.theirs_label = marker_label(rest, "theirs");
                sections.push(ConflictSection::Conflict {
                    ours_label: current.ours_label.clone(),
                    theirs_label: current.theirs_label.clone(),
                    ours: std::mem::take(&mut current.ours),
                    theirs: std::mem::take(&mut current.theirs),
                });
                open = None;
                continue;
            }
            match current.phase {
                Phase::Ours => current.ours.extend_from_slice(line),
                Phase::Theirs => current.theirs.extend_from_slice(line),
                Phase::Base => {}
            }
            continue;
        }
        common.extend_from_slice(line);
    }
    if open.is_some() {
        return Err("unterminated conflict marker".to_string());
    }
    if !common.is_empty() {
        sections.push(ConflictSection::Common(common));
    }
    Ok(sections)
}

/// Number of conflict blocks in a parsed section list.
pub fn conflict_count(sections: &[ConflictSection]) -> usize {
    sections.iter().filter(|section| section.is_conflict()).count()
}

/// Rebuild a file from the parsed sections and one choice per conflict.
pub fn resolve_conflict_sections(
    sections: &[ConflictSection],
    choices: &[ConflictChoice],
) -> Result<Vec<u8>, String> {
    let expected = conflict_count(sections);
    if choices.len() != expected {
        return Err(format!(
            "expected {expected} conflict choices, got {}",
            choices.len()
        ));
    }
    let mut out = Vec::new();
    let mut choice_ix = 0;
    for section in sections {
        match section {
            ConflictSection::Common(bytes) => out.extend_from_slice(bytes),
            ConflictSection::Conflict { ours, theirs, .. } => {
                match choices[choice_ix] {
                    ConflictChoice::Ours => out.extend_from_slice(ours),
                    ConflictChoice::Theirs => out.extend_from_slice(theirs),
                    ConflictChoice::Both => {
                        out.extend_from_slice(ours);
                        out.extend_from_slice(theirs);
                    }
                }
                choice_ix += 1;
            }
        }
    }
    Ok(out)
}

/// Read and parse the selected worktree file's conflict markers.
pub fn conflict_sections(worktree: &str, path: &[u8]) -> Result<Vec<ConflictSection>, String> {
    let bytes = read_worktree_file(worktree, path, CONFLICT_FILE_CAP + 1)?;
    if bytes.len() > CONFLICT_FILE_CAP {
        return Err("conflicted file is too large to resolve here".to_string());
    }
    parse_conflict_markers(&bytes)
}

/// Write a worktree file through the repository's host: a native write for
/// `/mnt` mounts, `dd of=<path>` over the WSL tunnel otherwise.
pub(crate) fn write_worktree_file(worktree: &str, path: &[u8], bytes: Vec<u8>) -> Result<(), String> {
    let relative = String::from_utf8_lossy(path);
    let joined = format!("{}/{}", worktree.trim_end_matches('/'), relative);
    if let Some(windows) = crate::model::host_path(&joined) {
        return std::fs::write(&windows, bytes).map_err(|e| e.to_string());
    }
    if std::str::from_utf8(path).is_err() {
        return Err("cannot write a non-UTF-8 path on this host".to_string());
    }
    let operand = format!("of={joined}");
    let out = run_linux_with_stdin("dd", &[&operand, "status=none"], bytes)
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.error_context());
    }
    Ok(())
}

/// Apply the per-conflict choices: rewrite the worktree file without markers
/// and stage it, which is what marks the path resolved in Git.
pub fn resolve_conflict_file(
    worktree: &str,
    path: &[u8],
    choices: &[ConflictChoice],
) -> Result<(), String> {
    let bytes = read_worktree_file(worktree, path, CONFLICT_FILE_CAP + 1)?;
    if bytes.len() > CONFLICT_FILE_CAP {
        return Err("conflicted file is too large to resolve here".to_string());
    }
    let sections = parse_conflict_markers(&bytes)?;
    let resolved = resolve_conflict_sections(&sections, choices)?;
    write_worktree_file(worktree, path, resolved)?;
    stage_paths(worktree, &[path.to_vec()])
}

/// `%D` decoration text, e.g. `HEAD -> main, origin/main, tag: v1.0`.
/// Refs are ordered HEAD, branches, remotes, then tags so chip rows read
/// consistently regardless of git's decoration order.
fn parse_decoration(decoration: &str, remotes: &HashSet<String>) -> Vec<(String, RefKind)> {    let mut refs = Vec::new();
    for item in decoration.split(", ").map(str::trim).filter(|i| !i.is_empty()) {
        if let Some(name) = item.strip_prefix("HEAD -> ") {
            refs.push(("HEAD".to_string(), RefKind::Head));
            refs.push((name.to_string(), RefKind::Branch));
        } else if item == "HEAD" {
            refs.push(("HEAD".to_string(), RefKind::Head));
        } else if let Some(name) = item.strip_prefix("tag: ") {
            refs.push((name.to_string(), RefKind::Tag));
        } else {
            let remote = item
                .split_once('/')
                .is_some_and(|(head, _)| remotes.contains(head));
            refs.push((
                item.to_string(),
                if remote {
                    RefKind::Remote
                } else {
                    RefKind::Branch
                },
            ));
        }
    }
    refs.sort_by_key(|(_, kind)| match kind {
        RefKind::Head => 0,
        RefKind::Branch => 1,
        RefKind::Remote => 2,
        RefKind::Tag => 3,
    });
    refs
}

/// Short ref names with the commit they point to (annotated tags dereferenced),
/// from local branches, remote-tracking branches, and tags.
pub fn ref_heads(worktree: &str) -> Vec<(String, String)> {
    let format = format!("--format=%(refname:short){FS}%(objectname){FS}%(*objectname)");
    let out = match run_for(
        worktree,
        &[
            "-C",
            worktree,
            "for-each-ref",
            &format,
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    ) {
        Ok(o) if o.success() => o,
        _ => return Vec::new(),
    };
    out.stdout_text()
        .lines()
        .filter_map(|line| {
            let mut f = line.split(FS);
            let name = f.next().unwrap_or_default();
            let object = f.next().unwrap_or_default();
            let deref = f.next().unwrap_or_default();
            let hash = if deref.is_empty() { object } else { deref };
            (!name.is_empty() && !hash.is_empty())
                .then(|| (name.to_string(), hash.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_rules_derive_file_ext_and_folder_shapes() {
        let rule = |path: &str, kind: IgnoreKind| ignore_rule(path.as_bytes(), kind);
        assert_eq!(
            rule("sub/dir/file.txt", IgnoreKind::File),
            Some(b"/sub/dir/file.txt".to_vec())
        );
        assert_eq!(
            rule("sub/dir/file.txt", IgnoreKind::Extension),
            Some(b"*.txt".to_vec())
        );
        assert_eq!(
            rule("sub/dir/file.txt", IgnoreKind::Folder),
            Some(b"/sub/dir/".to_vec())
        );
        // Root files have no folder; extension-less and dotfiles have no ext.
        assert_eq!(rule("file.txt", IgnoreKind::Folder), None);
        assert_eq!(rule("Makefile", IgnoreKind::Extension), None);
        assert_eq!(rule(".env", IgnoreKind::Extension), None);
        assert_eq!(rule("a.tar.gz", IgnoreKind::Extension), Some(b"*.gz".to_vec()));
        assert_eq!(rule("", IgnoreKind::File), None);
        // Pattern syntax in names is escaped, so a rule matches only itself.
        assert_eq!(
            rule("pages/[id].tsx", IgnoreKind::File),
            Some(b"/pages/\\[id].tsx".to_vec())
        );
        assert_eq!(rule("a*b/c.t?t", IgnoreKind::Extension), Some(b"*.t\\?t".to_vec()));
        assert_eq!(rule("x/trail ", IgnoreKind::File), Some(b"/x/trail\\ ".to_vec()));
        assert_eq!(rule("bad\nname", IgnoreKind::File), None);
        // Non-UTF-8 names stay byte-exact.
        assert_eq!(
            ignore_rule(b"sub/\xff.txt", IgnoreKind::Extension),
            Some(b"*.txt".to_vec())
        );
    }

    #[test]
    fn ignore_merge_dedupes_and_keeps_endings() {
        assert_eq!(apply_ignore_rule(b"", b"/a.txt"), Some(b"/a.txt\n".to_vec()));
        assert_eq!(
            apply_ignore_rule(b"/a.txt\n", b"/a.txt"),
            None,
            "an exact line must not duplicate"
        );
        assert_eq!(
            apply_ignore_rule(b"/a.txt", b"/b.txt"),
            Some(b"/a.txt\n/b.txt\n".to_vec()),
            "a missing trailing newline is repaired"
        );
        assert_eq!(
            apply_ignore_rule(b"/a.txt\r\n", b"/a.txt"),
            None,
            "CRLF endings compare cleanly"
        );
    }

    #[test]
    fn undo_refnames_sanitize_labels_and_keep_time_order() {
        assert_eq!(
            undo_refname_at(1700000000, "discard src/foo.rs"),
            "refs/spur/undo/1700000000_discard_src_foo.rs"
        );
        assert_eq!(
            undo_refname_at(1, "branch f/x"),
            "refs/spur/undo/1_branch_f_x"
        );
        assert_eq!(undo_refname_at(1, ""), "refs/spur/undo/1_op");
        assert_eq!(undo_refname_at(1, "///"), "refs/spur/undo/1_op");
        assert!(
            undo_refname_at(2, "x") > undo_refname_at(1, "x"),
            "later backups must sort after earlier ones"
        );
    }

    #[test]
    fn undo_prune_cutoff_subtracts_whole_days() {
        assert_eq!(prune_cutoff_secs(14 * 24 * 3600 + 1, 14), 1);
        assert_eq!(
            prune_cutoff_secs(100, 14),
            0,
            "saturates instead of wrapping"
        );
        assert_eq!(
            prune_cutoff_secs(u64::MAX, 14),
            u64::MAX - 14 * 24 * 3600
        );
    }

    #[test]
    fn undo_ref_lines_parse_or_reject() {
        assert_eq!(
            parse_undo_ref_line("refs/spur/undo/1700000000_x_abc123"),
            Some(("refs/spur/undo/1700000000_x_abc123".to_string(), 1700000000))
        );
        assert_eq!(parse_undo_ref_line("refs/heads/main_123"), None);
        assert_eq!(parse_undo_ref_line("refs/spur/undo/x_notanumber"), None);
        assert_eq!(parse_undo_ref_line("refs/spur/undo/123"), None);
        assert_eq!(parse_undo_ref_line("garbage"), None);
    }

    #[test]
    fn undo_guards_reject_before_touching_git() {
        // Every path below fails on validation, so no test may spawn a
        // process here (unit suite runs without the WSL distro).
        assert!(rev_parse("/nope", "-evil").is_err());
        assert!(rev_parse("/nope", "").is_err());
        assert!(write_backup_ref("/nope", "not-an-oid!", "x").is_err());
        assert!(delete_backup_ref("/nope", "refs/heads/main").is_err());
        assert!(delete_backup_ref("/nope", "-x").is_err());
        assert!(stash_store_id("/nope", "nope", "x").is_err());
        assert!(recreate_branch_at("/nope", "-evil", "abc123").is_err());
        assert!(recreate_branch_at("/nope", "ok", "xyz").is_err());
        assert!(reset_hard_to("/nope", "xyz").is_err());
        assert!(restore_tag_to("/nope", "-evil", "abc123").is_err());
        assert!(cat_blob("/nope", "xyz").is_err());
        assert!(restore_untracked_file("/nope", b"", b"x").is_err());
    }

    #[test]
    fn new_file_patch_shapes_match_git_no_index() {
        let patch = new_file_patch(b"new.txt", b"one\ntwo\n".to_vec()).expect("patch");
        assert_eq!(
            patch,
            "diff --git a/new.txt b/new.txt\n\
             new file mode 100644\n\
             --- /dev/null\n\
             +++ b/new.txt\n\
             @@ -0,0 +1,2 @@\n\
             +one\n\
             +two\n"
        );

        // A missing trailing newline gets the marker, not a phantom line.
        let patch = new_file_patch(b"new.txt", b"one\ntwo".to_vec()).expect("patch");
        assert!(patch.contains("+two\n\\ No newline at end of file\n"));
        assert!(!patch.contains("+two\n+\n"));

        // An empty file is a zero-line hunk starting at 0.
        let patch = new_file_patch(b"empty.txt", Vec::new()).expect("patch");
        assert!(patch.contains("@@ -0,0 +0,0 @@\n"));
        assert!(!patch.contains("\\ No newline"));

        // CRLF is stripped like the preview parser, so the patch matches
        // the displayed diff.
        let patch = new_file_patch(b"win.txt", b"one\r\ntwo\r\n".to_vec()).expect("patch");
        assert!(patch.contains("+one\n+two\n"));
        assert!(!patch.contains('\r'));

        // Spaced paths are C-quoted with the `a/`/`b/` prefixes inside.
        let patch = new_file_patch(b"my file.txt", b"x\n".to_vec()).expect("patch");
        assert!(patch.contains("diff --git \"a/my file.txt\" \"b/my file.txt\"\n"));
        assert!(patch.contains("+++ \"b/my file.txt\"\n"));

        // Binary and over-cap inputs are refused, never truncated.
        assert!(new_file_patch(b"bin", vec![b'a', 0, b'b']).is_err());
        assert!(new_file_patch(b"big", vec![b'x'; DIFF_BYTE_CAP + 1]).is_err());
    }

    #[test]
    fn patch_file_list_reads_diff_git_headers() {
        let patch = "From abc123 Mon Sep 17 00:00:00 2001\n\
             Subject: example\n\
             \n\
             diff --git a/one.txt b/one.txt\n\
             index 111..222 100644\n\
             --- a/one.txt\n\
             +++ b/one.txt\n\
             @@ -1 +1 @@\n\
             -one\n\
             +two\n\
             diff --git a/new.txt b/new.txt\n\
             new file mode 100644\n\
             --- /dev/null\n\
             +++ b/new.txt\n\
             @@ -0,0 +1 @@\n\
             +new\n";
        assert_eq!(patch_file_list(patch), vec!["one.txt", "new.txt"]);

        // Deletions list the a-side; anything else lists the b-side.
        let patch = "diff --git a/gone.txt b/gone.txt\n\
             deleted file mode 100644\n\
             --- a/gone.txt\n\
             +++ /dev/null\n";
        assert_eq!(patch_file_list(patch), vec!["gone.txt"]);

        // C-quoted paths (spaces, quotes, octal UTF-8) round-trip with the
        // copy-patch synthesizer.
        let bytes = new_file_patch("my f\"x\"/ä.txt".as_bytes(), b"x\n".to_vec()).expect("patch");
        let files = patch_file_list(&bytes);
        assert_eq!(files, vec!["my f\"x\"/ä.txt"]);

        // Prose without a header is not a patch; duplicates list once.
        assert!(patch_file_list("just some text\n").is_empty());
        // Git leaves spaces unquoted: the split is the middle space.
        assert_eq!(
            patch_file_list("diff --git a/my file.txt b/my file.txt\n"),
            vec!["my file.txt"]
        );
        assert!(patch_file_list("").is_empty());
        let patch = "diff --git a/f b/f\ndiff --git a/f b/f\n";
        assert_eq!(patch_file_list(patch), vec!["f"]);
    }

    #[test]
    fn parses_records_fields_and_refs() {
        let remotes: HashSet<String> = ["origin".to_string()].into();
        let raw = format!(
            "abc{FS}parent1 parent2{FS}Ada{FS}2 days ago{FS}HEAD -> main, origin/main, tag: v1.0{FS}Subject here\0\
             def{FS}{FS}Bob{FS}3 days ago{FS}feature/x{FS}No upstream\0\
             ghi{FS}{FS}Ada{FS}now{FS}tag: v2.0, docs-orphan{FS}Tag first in git\0"
        );
        let commits = parse_log(&raw, &remotes);
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].hash, "abc");
        assert_eq!(commits[0].parents, vec!["parent1", "parent2"]);
        assert_eq!(commits[0].subject, "Subject here");
        assert_eq!(
            commits[0].refs,
            vec![
                ("HEAD".to_string(), RefKind::Head),
                ("main".to_string(), RefKind::Branch),
                ("origin/main".to_string(), RefKind::Remote),
                ("v1.0".to_string(), RefKind::Tag),
            ]
        );
        assert!(commits[1].parents.is_empty());
        assert_eq!(
            commits[1].refs,
            vec![("feature/x".to_string(), RefKind::Branch)]
        );
        // Branch chips always lead, even when git lists the tag first.
        assert_eq!(
            commits[2].refs,
            vec![
                ("docs-orphan".to_string(), RefKind::Branch),
                ("v2.0".to_string(), RefKind::Tag),
            ]
        );
    }

    #[test]
    fn detached_head_is_a_lone_head_ref() {
        let commits = parse_log(&format!("abc{FS}{FS}A{FS}now{FS}HEAD{FS}s"), &HashSet::new());
        assert_eq!(commits[0].refs, vec![("HEAD".to_string(), RefKind::Head)]);
    }

    #[test]
    fn name_status_records_keep_rename_sources_and_path_bytes() {
        let mut bytes = Vec::new();
        for record in [
            b"M\0src/main.rs\0".as_slice(),
            b"A\0new file.txt\0",
            b"D\0gone.txt\0",
            b"R100\0old name.txt\0renamed \xff.txt\0",
            b"T\0linked\0",
        ] {
            bytes.extend_from_slice(record);
        }
        let files = parse_name_status(&bytes).unwrap();
        assert_eq!(files.len(), 5);
        assert_eq!(files[0].status, 'M');
        assert_eq!(files[0].path, b"src/main.rs");
        assert_eq!(files[0].orig_path, None);
        assert_eq!(files[1].status, 'A');
        assert_eq!(files[1].path, b"new file.txt");
        assert_eq!(files[3].status, 'R');
        assert_eq!(files[3].orig_path.as_deref(), Some(b"old name.txt".as_ref()));
        assert_eq!(files[3].path, b"renamed \xff.txt");
        assert_eq!(files[3].display_path(), "old name.txt → renamed \u{fffd}.txt");
        assert_eq!(files[4].status, 'T');

        // A truncated rename is an error, never a silently lost file.
        assert!(parse_name_status(b"R100\0only-old\0").is_err());
        assert!(parse_name_status(b"M\0src/main.rs\0").is_ok());
    }

    #[test]
    fn numstat_totals_sum_lines_and_skip_binaries() {
        let bytes = b"10\t2\tsrc/main.rs\n3\t0\tdocs/readme.md\n-\t-\timages/logo.png\n";
        assert_eq!(parse_numstat_totals(bytes), (13, 2));
        assert_eq!(parse_numstat_totals(b""), (0, 0));
        // A malformed row (no tab) contributes nothing instead of failing.
        assert_eq!(parse_numstat_totals(b"garbage\n4\t5\tx\n"), (4, 5));
    }

    #[cfg(windows)]
    #[test]
    fn commits_carry_the_profile_identity_of_their_worktree() {
        let Some(git) = windows_git() else {
            return;
        };
        let base = std::env::temp_dir().join(format!("spur-profile-commit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let native = |args: &[&str]| {
            let out = std::process::Command::new(git)
                .current_dir(&base)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        native(&["init", "-q", "-b", "main"]);
        std::fs::write(base.join("a.txt"), "one
").unwrap();
        native(&["add", "a.txt"]);

        let worktree = crate::model::to_linux_path(&base.to_string_lossy()).unwrap();
        let profile = crate::accounts::Profile {
            label: "test".into(),
            name: "Profile Author".into(),
            email: "profile@test.invalid".into(),
            github: None,
        };
        crate::accounts::set_for(&worktree, Some(&profile));
        let committed = commit_staged(&worktree, b"with a profile");
        crate::accounts::set_for(&worktree, None);
        committed.expect("commit");

        // The machine's own Git identity must not win over the profile.
        assert_eq!(
            native(&["log", "-1", "--format=%an <%ae> | %cn <%ce>"]),
            "Profile Author <profile@test.invalid> | Profile Author <profile@test.invalid>"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(windows)]
    #[test]
    fn rejected_native_push_is_not_retried_in_wsl() {
        let Some(git) = windows_git() else {
            return;
        };
        let base = std::env::temp_dir().join(format!("spur-push-native-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let native = |dir: &std::path::Path, args: &[&str]| {
            let out = std::process::Command::new(git)
                .current_dir(dir)
                .args(["-c", "user.name=Spur Test", "-c", "user.email=spur@test.invalid"])
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        native(&base, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        for name in ["a", "b"] {
            native(&base, &["init", "-q", "-b", "main", name]);
            let repo = base.join(name);
            native(&repo, &["commit", "-q", "--allow-empty", "-m", name]);
            native(&repo, &["remote", "add", "origin", "../remote.git"]);
        }
        native(&base.join("a"), &["push", "-q", "origin", "main"]);

        // b's unrelated history is rejected, and that rejection is the result.
        let b = crate::model::to_linux_path(&base.join("b").to_string_lossy()).unwrap();
        let start = crate::process::journal_len();
        assert!(push_branch(&b, "origin", "main", "main", false).is_err());
        let wsl_runs = crate::process::journal_since(start)
            .into_iter()
            .filter(|record| record.argv[0] == "wsl.exe" && record.argv.contains(&b))
            .count();
        assert_eq!(wsl_runs, 0, "a rejected push must not run again in WSL");
        let _ = std::fs::remove_dir_all(&base);
    }
}

#[cfg(test)]
mod diff_tests {
    use super::*;

    #[test]
    fn unified_diff_parses_hunks_line_numbers_and_counts() {
        let raw = b"diff --git a/f.txt b/f.txt\n\
                    index 000..111 100644\n\
                    --- a/f.txt\n\
                    +++ b/f.txt\n\
                    @@ -1,3 +1,4 @@\n\
                    \x20context\n\
                    -old\n\
                    +new\n\
                    +extra\n\
                    \x20tail\n";
        let diff = parse_unified_diff(raw);
        assert!(!diff.truncated && !diff.binary);
        assert_eq!((diff.additions, diff.deletions), (2, 1));
        assert_eq!(diff.lines[0].kind, DiffLineKind::Meta);

        let hunk = diff
            .lines
            .iter()
            .find(|line| line.kind == DiffLineKind::Hunk)
            .unwrap();
        assert!(hunk.text.starts_with("@@ -1,3 +1,4 @@"), "{}", hunk.text);

        let removed = diff
            .lines
            .iter()
            .find(|line| line.kind == DiffLineKind::Removed)
            .unwrap();
        assert_eq!((removed.old, removed.new, removed.text.as_str()), (Some(2), None, "old"));

        let added: Vec<&DiffLine> = diff
            .lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Added)
            .collect();
        assert_eq!(added[0].new, Some(2));
        assert_eq!(added[1].new, Some(3));
        assert_eq!(added[1].text, "extra");

        let tail = diff.lines.last().unwrap();
        assert_eq!(tail.kind, DiffLineKind::Context);
        assert_eq!((tail.old, tail.new), (Some(3), Some(4)));
    }

    #[test]
    fn diff_row_cap_bounds_pathological_short_line_input() {
        // > MAX_DIFF_ROWS one-character added lines: parsing stops at the cap
        // with the truncation marker set (the row overhead would otherwise be
        // tens of MiB).
        let mut raw = Vec::with_capacity(MAX_DIFF_ROWS * 3);
        raw.extend_from_slice(b"@@ -0,0 +1,10 @@\n");
        for _ in 0..(MAX_DIFF_ROWS + 1_000) {
            raw.extend_from_slice(b"+x\n");
        }
        let diff = parse_unified_diff(&raw);
        assert!(diff.truncated);
        assert_eq!(diff.lines.len(), MAX_DIFF_ROWS);
        assert_eq!(diff.max_chars, "@@ -0,0 +1,10 @@".len());
    }

    #[test]
    fn diff_max_chars_is_stored_at_parse_time() {
        let raw = b"@@ -1 +1 @@\n-short\n+a much longer replacement line\n";
        let diff = parse_unified_diff(raw);
        assert_eq!(diff.max_chars, "a much longer replacement line".len());
    }

    #[test]
    fn diff_handles_binary_crlf_and_no_newline_markers() {
        let binary = parse_unified_diff(b"Binary files a/x.png and b/x.png differ\n");
        assert!(binary.binary);
        assert!(binary.additions == 0 && binary.deletions == 0);

        let crlf = parse_unified_diff(b"@@ -1 +1 @@\r\n-a\r\n+b\r\n\\ No newline at end of file\r\n");
        let added = crlf.lines.iter().find(|l| l.kind == DiffLineKind::Added).unwrap();
        assert_eq!(added.text, "b");
        assert!(crlf.lines.iter().any(|l| l.kind == DiffLineKind::Meta && l.text.starts_with('\\')));
    }

    #[test]
    fn oversized_diffs_are_truncated_at_a_line_boundary() {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"@@ -1,0 +1,200000 @@\n");
        for i in 0..200_000 {
            raw.extend_from_slice(format!("+line-{i}\n").as_bytes());
        }
        assert!(raw.len() > DIFF_BYTE_CAP);
        let diff = parse_unified_diff(&raw);
        assert!(diff.truncated);
        assert!(diff.lines.last().unwrap().text.starts_with("line-"));
        assert!(diff.additions > 0);
    }

    #[test]
    fn literal_pathspecs_and_stdin_pathspec_files_are_always_used() {
        let args = index_args("/w/repo", "add", &[]);
        assert_eq!(args[0], "--literal-pathspecs", "-- must precede the subcommand");
        assert!(args.contains(&"--pathspec-from-file=-"));
        assert!(args.contains(&"--pathspec-file-nul"));

        let unstaged = index_args("/w/repo", "reset", &["-q"]);
        assert!(unstaged.contains(&"-q"), "unstage is a quiet reset");
        assert!(unstaged.contains(&"--literal-pathspecs"));

        assert_eq!(
            pathspec_stdin(&[b"a.txt".to_vec(), b"b c.txt".to_vec()]),
            b"a.txt\0b c.txt"
        );
        assert!(pathspec_stdin(&[]).is_empty());
    }

    #[test]
    fn hunk_patches_are_byte_exact_with_shared_text_storage() {
        // Ordinary lines store one backing allocation (text == raw); lines
        // whose sanitized text differs (tabs, invalid UTF-8) keep a raw
        // backup. The reconstructed patch is byte-exact either way.
        let raw = b"diff --git a/f.txt b/f.txt\n\
                    --- a/f.txt\n\
                    +++ b/f.txt\n\
                    @@ -1,3 +1,3 @@\n\
                    \x20plain\n\
                    -old\ttab\n\
                    +new\xffbyte\n\
                    \x20tail\n";
        let diff = parse_unified_diff(raw);
        let plain = diff.lines.iter().find(|line| line.text == "plain").unwrap();
        assert!(plain.raw.is_none(), "plain ASCII text needs no raw copy");
        assert!(diff.lines.iter().find(|line| line.text.contains("old")).unwrap().raw.is_some());
        assert!(diff.lines.iter().find(|line| line.text.contains("new")).unwrap().raw.is_some());

        let rebuilt = diff.patch_for_hunk(0).unwrap();
        assert_eq!(
            rebuilt,
            b"diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@\n plain\n-old\ttab\n+new\xffbyte\n tail\n"
        );
        // Accounting counts the one allocation ordinary lines keep.
        let expected = std::mem::size_of::<FileDiff>()
            + diff.lines.capacity() * std::mem::size_of::<DiffLine>()
            + diff.hunks.capacity() * std::mem::size_of::<DiffHunk>()
            + diff
                .lines
                .iter()
                .map(|line| {
                    line.text.capacity() + line.raw.as_ref().map_or(0, |raw| raw.capacity())
                })
                .sum::<usize>();
        assert_eq!(diff.approx_bytes(), expected);
    }

    #[test]
    fn hunk_patches_repeat_the_header_and_keep_raw_bytes() {
        let raw = b"diff --git a/f.txt b/f.txt\n\
                    index 000..111 100644\n\
                    --- a/f.txt\n\
                    +++ b/f.txt\n\
                    @@ -1,2 +1,2 @@\n\
                    \x20keep\n\
                    -old\n\
                    +new\twith-tab\n\
                    @@ -10,1 +10,1 @@\n\
                    \x20ctx\n\
                    -a\n\
                    +b\n";
        let diff = parse_unified_diff(raw);
        assert_eq!(diff.hunks.len(), 2);
        assert_eq!(diff.header_len, 4);

        let second = diff.patch_for_hunk(1).unwrap();
        let second = String::from_utf8_lossy(&second);
        assert!(
            second.starts_with("diff --git a/f.txt b/f.txt\n"),
            "{second}"
        );
        assert!(
            second.contains("--- a/f.txt\n+++ b/f.txt\n@@ -10,1 +10,1 @@\n"),
            "{second}"
        );
        assert!(second.contains("-a\n+b\n"), "{second}");
        assert!(!second.contains("old"), "only the requested hunk");

        // Sanitization (tabs become spaces) must not leak into the patch.
        let first = diff.patch_for_hunk(0).unwrap();
        let first = String::from_utf8_lossy(&first);
        assert!(first.contains("+new\twith-tab"), "{first}");
        assert!(first.contains(" keep\n-old\n"), "{first}");
    }

    #[test]
    fn empty_commit_messages_never_reach_git() {
        let err = commit_staged("/definitely/not/a/repo", b" \n\t ").unwrap_err();
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn history_signature_tracks_head_and_refs() {
        let mut records = Vec::new();
        for line in ["# branch.oid abc", "# branch.head main"] {
            records.extend_from_slice(line.as_bytes());
            records.push(0);
        }
        let snapshot = crate::status::parse_porcelain_v2(&records).unwrap();
        let collected = CollectedStatus {
            snapshot: snapshot.clone(),
            refs: crate::status::RefInfo {
                revision: 7,
                ..Default::default()
            },
        };
        assert_eq!(collected.history_signature(), (Some("abc".into()), 7));

        let mut moved = Vec::new();
        for line in ["# branch.oid def", "# branch.head main"] {
            moved.extend_from_slice(line.as_bytes());
            moved.push(0);
        }
        let new_head = CollectedStatus {
            snapshot: crate::status::parse_porcelain_v2(&moved).unwrap(),
            refs: crate::status::RefInfo {
                revision: 7,
                ..Default::default()
            },
        };
        assert_ne!(
            collected.history_signature(),
            new_head.history_signature(),
            "a new HEAD must change the signature"
        );

        let refs_moved = CollectedStatus {
            snapshot,
            refs: crate::status::RefInfo {
                revision: 8,
                ..Default::default()
            },
        };
        assert_ne!(
            collected.history_signature(),
            refs_moved.history_signature(),
            "a moved ref must change the signature"
        );
    }
}

/// Real-Git integration tests (identity, failure context). They need the
/// WSL distro, so they are ignored by default: run them with
/// `cargo test -- --ignored`.
#[cfg(test)]
mod wsl_tests {
    use super::*;
    use crate::process::wsl_support as wsl;

    fn linux_env(home: &str) -> Vec<(&'static str, String)> {
        vec![
            ("HOME", home.to_string()),
            ("GIT_CONFIG_NOSYSTEM", "1".to_string()),
        ]
    }

    fn init_repo(home: &str, path: &str) {
        wsl::must(&["mkdir", "-p", path]);
        wsl::must_env(home, &["git", "-C", path, "init", "-q", "-b", "main"]);
        wsl::write_file(&format!("{path}/file.txt"), b"one\n");
        wsl::must_env(home, &["git", "-C", path, "add", "file.txt"]);
        wsl::must_env(home, &["git", "-C", path, "commit", "-q", "-m", "init"]);
    }

    fn rev(repo: &str, reference: &str) -> String {
        String::from_utf8_lossy(&wsl::must(&["git", "-C", repo, "rev-parse", reference]))
            .trim()
            .to_string()
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn branch_rename_delete_track_counts_and_dates() {
        let dir = wsl::temp_dir("w16");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let repo = format!("{dir}/repo");
        init_repo(&home, &repo);
        let main_tip = rev(&repo, "HEAD");

        // A merged side branch deletes with `-d`; rename round-trips.
        wsl::must_env(&home, &["git", "-C", &repo, "branch", "old-name"]);
        rename_branch(&repo, "old-name", "new-name").expect("rename must work");
        assert_eq!(rev(&repo, "new-name"), main_tip);
        assert!(
            !wsl::wsl(&[
                "git",
                "-C",
                &repo,
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/old-name"
            ])
            .status
            .success(),
            "the old name must be gone"
        );
        delete_branch(&repo, "new-name", false).expect("a merged branch deletes with -d");

        // An unmerged branch is refused without force, then goes with `-D`.
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "-b", "side"]);
        wsl::write_file(&format!("{repo}/side.txt"), b"side\n");
        wsl::must_env(&home, &["git", "-C", &repo, "add", "-A"]);
        wsl::must_env(&home, &["git", "-C", &repo, "commit", "-q", "-m", "side"]);
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "main"]);
        let refused = delete_branch(&repo, "side", false).unwrap_err();
        assert!(
            refused.contains("not fully merged"),
            "git must refuse the safe delete: {refused}"
        );
        delete_branch(&repo, "side", true).expect("force deletes the unmerged branch");

        // The checked-out branch and invalid names never reach `-D`.
        assert!(delete_branch(&repo, "main", true).is_err());
        assert!(rename_branch(&repo, "", "x").is_err());
        assert!(rename_branch(&repo, "main", "-x").is_err());
        assert!(delete_branch(&repo, "-x", true).is_err());
        assert!(delete_branch(&repo, "nope", false).is_err());

        // Branch metadata: tip time is known, and counts start at zero
        // without an upstream.
        let info = ref_info(&repo).expect("ref info must read");
        let main = info
            .branches
            .iter()
            .find(|branch| branch.name == "main")
            .expect("main must be listed");
        assert!(main.current);
        assert_eq!((main.ahead, main.behind), (0, 0));
        assert!(main.updated > 0, "tip time must be known");
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn ignore_rules_land_in_gitignore_or_exclude() {
        let dir = wsl::temp_dir("w20");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let repo = format!("{dir}/repo");
        init_repo(&home, &repo);
        let ignored = |path: &str| {
            String::from_utf8_lossy(&wsl::must(&[
                "git", "-C", &repo, "status", "--porcelain=v2", "--untracked-files=all", "--", path,
            ]))
            .trim()
            .is_empty()
        };
        let gitignore = || wsl::must(&["cat", &format!("{repo}/.gitignore")]);

        // Untracked file, extension, and folder rules hide their targets.
        wsl::write_file(&format!("{repo}/debug.log"), b"x\n");
        wsl::must(&["mkdir", "-p", &format!("{repo}/build")]);
        wsl::write_file(&format!("{repo}/build/out.bin"), b"x\n");
        ignore_path(&repo, b"/debug.log", false, None).expect("file rule");
        assert!(ignored("debug.log"), "the file rule must hide it");
        ignore_path(&repo, b"*.bin", false, None).expect("ext rule");
        assert!(ignored("build/out.bin"), "the ext rule must hide it");
        ignore_path(&repo, b"/build/", false, None).expect("folder rule");
        let text = String::from_utf8_lossy(&gitignore()).into_owned();
        assert!(text.contains("/debug.log\n"));
        assert!(text.contains("/build/\n"));

        // Appending twice does not duplicate the line.
        ignore_path(&repo, b"/debug.log", false, None).expect("idempotent");
        let again = String::from_utf8_lossy(&gitignore()).into_owned();
        assert_eq!(
            again.matches("/debug.log").count(),
            1,
            "duplicate rules must not accumulate"
        );
        assert_eq!(text, again, "an existing rule must leave the file alone");

        // Shift behavior lands in the local exclude file instead.
        wsl::write_file(&format!("{repo}/local.tmp"), b"x\n");
        ignore_path(&repo, b"/local.tmp", true, None).expect("exclude rule");
        assert!(ignored("local.tmp"));
        assert!(
            !String::from_utf8_lossy(&gitignore()).contains("/local.tmp"),
            "exclude rules must not leak into .gitignore"
        );

        // Tracked files are untracked first, then hidden. The `rm --cached`
        // stages a deletion, so commit it before the status can go quiet.
        wsl::write_file(&format!("{repo}/file.txt"), b"changed\n");
        ignore_path(&repo, b"/file.txt", false, Some(&b"file.txt"[..]))
            .expect("untrack and ignore");
        assert!(
            wsl::must(&["git", "-C", &repo, "ls-files", "file.txt"]).is_empty(),
            "the path must leave the index"
        );
        wsl::must_env(&home, &["git", "-C", &repo, "commit", "-qm", "untrack file.txt"]);
        assert!(ignored("file.txt"));

        // Invalid inputs never touch the disk.
        assert!(ignore_path(&repo, b"", false, None).is_err());
        let empty: &[u8] = &[];
        assert!(ignore_path(&repo, b"/debug.log", false, Some(empty)).is_err());
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn copy_patch_round_trips_through_git_apply() {
        let dir = wsl::temp_dir("w21");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let repo = format!("{dir}/repo");
        init_repo(&home, &repo);
        // Each copied patch is written out and must `git apply --check`
        // against a clean worktree, then restore the exact bytes.
        let round_trip = |name: &str, target: &str, patch: &str, expect: &[u8]| {
            wsl::write_file(&format!("{dir}/{name}"), patch.as_bytes());
            wsl::must(&["git", "-C", &repo, "apply", "--check", &format!("{dir}/{name}")]);
            wsl::must(&["git", "-C", &repo, "apply", &format!("{dir}/{name}")]);
            assert_eq!(
                wsl::must(&["cat", &format!("{repo}/{target}")]),
                expect,
                "{name} must restore the exact bytes"
            );
        };
        let clean = || {
            wsl::must_env(&home, &["git", "-C", &repo, "reset", "-q"]);
            wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "--", "."]);
            wsl::must(&["rm", "-f", &format!("{repo}/new.txt")]);
        };

        // Unstaged modification.
        wsl::write_file(&format!("{repo}/file.txt"), b"one\nchanged\n");
        let patch = worktree_file_patch(&repo, b"file.txt", false, false).expect("unstaged patch");
        assert!(patch.contains("+changed\n"), "the patch must carry the edit");
        clean();
        round_trip("unstaged.patch", "file.txt", &patch, b"one\nchanged\n");

        // Staged modification (same content change, index side).
        clean();
        wsl::write_file(&format!("{repo}/file.txt"), b"one\nstaged\n");
        wsl::must_env(&home, &["git", "-C", &repo, "add", "file.txt"]);
        let patch = worktree_file_patch(&repo, b"file.txt", true, false).expect("staged patch");
        assert!(patch.contains("+staged\n"));
        clean();
        round_trip("staged.patch", "file.txt", &patch, b"one\nstaged\n");

        // Untracked file synthesizes a new-file patch.
        clean();
        wsl::write_file(&format!("{repo}/new.txt"), b"fresh\n");
        let patch = worktree_file_patch(&repo, b"new.txt", false, true).expect("untracked patch");
        assert!(patch.contains("new file mode"));
        wsl::must(&["rm", &format!("{repo}/new.txt")]);
        round_trip("untracked.patch", "new.txt", &patch, b"fresh\n");

        // Stash files: stash everything, then copy both sides out of the entry.
        clean();
        wsl::write_file(&format!("{repo}/new.txt"), b"fresh\n");
        wsl::write_file(&format!("{repo}/file.txt"), b"one\nstashed\n");
        wsl::write_file(&format!("{repo}/added.txt"), b"added\n");
        wsl::must_env(
            &home,
            &["git", "-C", &repo, "stash", "push", "-q", "--include-untracked", "-m", "w21"],
        );
        let patch =
            stash_file_patch(&repo, "stash@{0}", b"file.txt", false).expect("stash patch");
        assert!(patch.contains("+stashed\n"));
        round_trip("stash.patch", "file.txt", &patch, b"one\nstashed\n");
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "--", "."]);
        let patch =
            stash_file_patch(&repo, "stash@{0}", b"added.txt", true).expect("stash new patch");
        assert!(patch.contains("new file mode"));
        round_trip("stash-new.patch", "added.txt", &patch, b"added\n");

        // Invalid inputs never reach git.
        assert!(worktree_file_patch(&repo, b"", false, false).is_err());
        assert!(stash_file_patch(&repo, "", b"file.txt", false).is_err());
        assert!(stash_file_patch(&repo, "-x", b"file.txt", false).is_err());
        assert!(stash_file_patch(&repo, "stash@{0}", b"", false).is_err());
        // A clean file has no patch to copy.
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "--", "."]);
        assert!(worktree_file_patch(&repo, b"file.txt", false, false).is_err());
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn apply_patch_lands_in_the_worktree_unstaged() {
        let dir = wsl::temp_dir("w22");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let repo = format!("{dir}/repo");
        wsl::must(&["mkdir", "-p", &repo]);
        wsl::must_env(&home, &["git", "-C", &repo, "init", "-q", "-b", "main"]);
        let content = |line4: &str| {
            format!("l1\nl2\nl3\n{line4}\nl5\nl6\nl7\nl8\n")
        };
        wsl::write_file(&format!("{repo}/file.txt"), content("l4").as_bytes());
        wsl::must_env(&home, &["git", "-C", &repo, "add", "file.txt"]);
        wsl::must_env(&home, &["git", "-C", &repo, "commit", "-q", "-m", "base"]);
        let staged = || {
            String::from_utf8_lossy(&wsl::must(&[
                "git",
                "-C",
                &repo,
                "diff",
                "--cached",
                "--name-only",
            ]))
            .into_owned()
        };

        // A copied patch applies to the worktree and stays unstaged for
        // review (no `--3way`: it implies `--index`, staging the result and
        // refusing dirty worktrees).
        wsl::write_file(&format!("{repo}/file.txt"), content("L4").as_bytes());
        let patch =
            worktree_file_patch(&repo, b"file.txt", false, false).expect("copied patch");
        assert_eq!(patch_file_list(&patch), vec!["file.txt"]);
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "--", "."]);
        apply_patch(&repo, patch.as_bytes(), false, false).expect("apply must land");
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/file.txt")]),
            content("L4").as_bytes()
        );
        assert!(
            staged().trim().is_empty(),
            "an applied patch must not stage itself"
        );

        // Drifted context refuses with git's own error and leaves the file
        // alone.
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "--", "."]);
        wsl::write_file(
            &format!("{repo}/file.txt"),
            b"L1\nL2\nL3\nl4\nL5\nL6\nL7\nl8\n",
        );
        let err = apply_patch(&repo, patch.as_bytes(), false, false).expect_err("drift must fail");
        assert!(
            err.contains("patch does not apply"),
            "the refusal must name the cause, got: {err}"
        );
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/file.txt")]),
            b"L1\nL2\nL3\nl4\nL5\nL6\nL7\nl8\n".as_slice()
        );

        // Garbage never reaches the worktree.
        assert!(apply_patch(&repo, b"", false, false).is_err());
        assert!(apply_patch(&repo, b"not a patch\n", false, false).is_err());
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn command_journal_captures_argv_for_the_panel() {
        let dir = wsl::temp_dir("w23");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let repo = format!("{dir}/repo");
        init_repo(&home, &repo);
        // A backend read leaves its exact argv in the journal, attributed
        // to the worktree — what the log panel expands.
        let floor = crate::process::journal_len();
        wsl::write_file(&format!("{repo}/file.txt"), b"one\nchanged\n");
        worktree_file_patch(&repo, b"file.txt", false, false).expect("patch");
        let records = crate::process::journal_since_for(floor, &repo);
        assert!(
            records
                .iter()
                .any(|record| record.argv.contains(&"diff".to_string())
                    && record.success),
            "the diff call must be journaled: {records:?}"
        );
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn stash_pop_branch_and_apply_file() {
        let dir = wsl::temp_dir("w19");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let repo = format!("{dir}/repo");
        init_repo(&home, &repo);
        let stash_with = |tracked: &[u8], untracked: Option<(&str, &[u8])>| {
            wsl::write_file(&format!("{repo}/file.txt"), tracked);
            if let Some((name, content)) = untracked {
                wsl::write_file(&format!("{repo}/{name}"), content);
            }
            wsl::must_env(
                &home,
                &["git", "-C", &repo, "stash", "push", "-q", "--include-untracked", "-m", "w19"],
            );
        };
        let list = || {
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "stash", "list"])).into_owned()
        };

        // Pop restores both files and drops the entry.
        stash_with(b"popped\n", Some(("new.txt", b"new\n")));
        assert!(list().contains("w19"));
        stash_pop(&repo, "stash@{0}").expect("pop must work");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/file.txt")]), b"popped\n");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/new.txt")]), b"new\n");
        assert!(list().trim().is_empty(), "pop must drop the entry");

        // Branch-from-stash checks the branch out and drops the entry.
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "--", "file.txt"]);
        stash_with(b"branched\n", None);
        stash_branch(&repo, "stash@{0}", "from-stash").expect("branch must work");
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&[
                "git", "-C", &repo, "symbolic-ref", "--short", "HEAD"
            ]))
            .trim(),
            "from-stash"
        );
        assert_eq!(wsl::must(&["cat", &format!("{repo}/file.txt")]), b"branched\n");
        assert!(list().trim().is_empty(), "branch must drop the entry");

        // Apply-file-only restores one path and keeps the entry and index.
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "main"]);
        wsl::must_env(&home, &["git", "-C", &repo, "branch", "-q", "-D", "from-stash"]);
        stash_with(b"partial\n", Some(("other.txt", b"other\n")));
        stash_apply_file(&repo, "stash@{0}", b"file.txt", false).expect("tracked apply");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/file.txt")]), b"partial\n");
        assert!(
            !wsl::exists(&format!("{repo}/other.txt")),
            "only the requested file may come back"
        );
        assert!(list().contains("w19"), "apply-file must keep the entry");
        stash_apply_file(&repo, "stash@{0}", b"other.txt", true).expect("untracked apply");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/other.txt")]), b"other\n");

        // Invalid inputs never reach git.
        assert!(stash_pop(&repo, "").is_err());
        assert!(stash_branch(&repo, "stash@{0}", "").is_err());
        assert!(stash_branch(&repo, "stash@{0}", "-x").is_err());
        assert!(stash_branch(&repo, "nope", "ok").is_err());
        assert!(stash_apply_file(&repo, "stash@{0}", b"", false).is_err());
        assert!(stash_apply_file(&repo, "stash@{9}", b"file.txt", false).is_err());
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn remote_prune_set_url_rename_and_remove() {
        let dir = wsl::temp_dir("w18");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let remote = format!("{dir}/remote.git");
        wsl::must(&["git", "init", "-q", "--bare", &remote]);
        let repo = format!("{dir}/repo");
        init_repo(&home, &repo);
        wsl::must_env(&home, &["git", "-C", &repo, "remote", "add", "origin", &remote]);
        wsl::must_env(&home, &["git", "-C", &repo, "push", "-q", "origin", "main"]);

        // A deleted upstream branch prunes away locally.
        wsl::must_env(&home, &["git", "-C", &repo, "branch", "gone"]);
        wsl::must_env(&home, &["git", "-C", &repo, "push", "-q", "origin", "gone"]);
        wsl::must(&["git", "--git-dir", &remote, "branch", "-q", "-D", "gone"]);
        remote_prune(&repo, "origin").expect("prune must work");
        assert!(
            !wsl::wsl(&[
                "git", "-C", &repo, "show-ref", "--verify", "--quiet",
                "refs/remotes/origin/gone"
            ])
            .status
            .success(),
            "the stale tracking branch must be gone"
        );

        // URL, rename, and remove round-trip through the backend.
        let other = format!("{dir}/other.git");
        wsl::must(&["git", "init", "-q", "--bare", &other]);
        set_remote_url(&repo, "origin", &other).expect("set-url must work");
        let url = String::from_utf8_lossy(&wsl::must(&[
            "git", "-C", &repo, "remote", "get-url", "origin",
        ]))
        .trim()
        .to_string();
        assert_eq!(url, other);
        rename_remote(&repo, "origin", "upstream").expect("rename must work");
        assert!(
            wsl::wsl(&["git", "-C", &repo, "remote", "get-url", "upstream"])
                .status
                .success(),
            "the new name must resolve"
        );
        remove_remote(&repo, "upstream").expect("remove must work");
        assert!(
            !wsl::wsl(&["git", "-C", &repo, "remote", "get-url", "upstream"])
                .status
                .success(),
            "the removed remote must be gone"
        );

        // Invalid inputs never reach git.
        assert!(remote_prune(&repo, "").is_err());
        assert!(remote_prune(&repo, "-x").is_err());
        assert!(set_remote_url(&repo, "origin", "").is_err());
        assert!(set_remote_url(&repo, "", &other).is_err());
        assert!(rename_remote(&repo, "", "x").is_err());
        assert!(rename_remote(&repo, "origin", "-x").is_err());
        assert!(remove_remote(&repo, "").is_err());
        assert!(remove_remote(&repo, "missing").is_err());
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn creating_a_tag_can_push_it_and_never_pushes_a_taken_name() {
        wsl::watchdog(120);
        let dir = wsl::temp_dir("tag-create-push");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]
	name = Spur Test
	email = spur@test.invalid
",
        );
        let remote = format!("{dir}/remote.git");
        wsl::must(&["git", "init", "-q", "--bare", &remote]);
        let repo = format!("{dir}/repo");
        init_repo(&home, &repo);
        let first = rev(&repo, "HEAD");
        wsl::must_env(&home, &["git", "-C", &repo, "remote", "add", "origin", &remote]);
        let ls_remote = |reference: &str| {
            String::from_utf8_lossy(&wsl::must(&[
                "git", "-C", &repo, "ls-remote", &remote, reference,
            ]))
            .into_owned()
        };

        create_tag_and_push(&repo, "v-local", &first, "", None).expect("local only");
        assert!(ls_remote("refs/tags/v-local").is_empty(), "no remote given: nothing pushed");

        create_tag_and_push(&repo, "v-both", &first, "release", Some("origin")).expect("create and push");
        assert!(ls_remote("refs/tags/v-both").contains("refs/tags/v-both"));

        // A later commit tries the taken name: creation fails, and the old
        // tag on the remote is neither replaced nor re-pushed.
        wsl::write_file(&format!("{repo}/later.txt"), b"later
");
        wsl::must_env(&home, &["git", "-C", &repo, "add", "."]);
        wsl::must_env(&home, &["git", "-C", &repo, "commit", "-q", "-m", "later"]);
        let later = rev(&repo, "HEAD");
        assert!(create_tag_and_push(&repo, "v-local", &later, "", Some("origin")).is_err());
        assert!(ls_remote("refs/tags/v-local").is_empty(), "the existing tag must not be pushed");

        // A push failure after a successful creation says so and keeps the tag.
        let err = create_tag_and_push(&repo, "v-kept", &later, "", Some("nowhere")).unwrap_err();
        assert!(err.contains("the tag was created, but pushing it failed"), "{err}");
        assert_eq!(tag_commit(&repo, "v-kept").expect("tag stays"), later);
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn tag_checkout_push_and_delete() {
        let dir = wsl::temp_dir("w17");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        // A local bare remote stands in for origin.
        let remote = format!("{dir}/remote.git");
        wsl::must(&["git", "init", "-q", "--bare", &remote]);
        let repo = format!("{dir}/repo");
        init_repo(&home, &repo);
        let tip = rev(&repo, "HEAD");
        wsl::must_env(&home, &["git", "-C", &repo, "remote", "add", "origin", &remote]);

        // Annotated and lightweight tags resolve to the commit either way.
        create_tag(&repo, "v-a", &tip, "release").expect("annotated tag");
        create_tag(&repo, "v-b", &tip, "").expect("lightweight tag");
        assert_eq!(tag_commit(&repo, "v-a").expect("resolve"), tip);
        assert_eq!(tag_commit(&repo, "v-b").expect("resolve"), tip);
        assert!(tag_commit(&repo, "missing").is_err());
        assert!(tag_commit(&repo, "").is_err());

        // Checking a tag out detaches exactly at its commit.
        checkout_detached(&repo, &tag_commit(&repo, "v-a").expect("resolve"))
            .expect("detach at the tag");
        assert_eq!(rev(&repo, "HEAD"), tip);
        assert!(
            !wsl::wsl(&["git", "-C", &repo, "symbolic-ref", "-q", "HEAD"])
                .status
                .success(),
            "HEAD must be detached"
        );
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "main"]);

        // Push then delete: the remote ref appears and disappears, the local
        // tag follows only when asked.
        let ls_remote = |reference: &str| {
            String::from_utf8_lossy(&wsl::must(&[
                "git",
                "-C",
                &repo,
                "ls-remote",
                &remote,
                reference,
            ]))
            .into_owned()
        };
        push_tag(&repo, "origin", "v-a").expect("push tag");
        // The remote must carry exactly the pushed tag object. (Older git
        // versions also printed a `^{}` peeled line here; git 2.53 only
        // lists the tag object for an exact ref query, so compare objects.)
        let remote_tag = ls_remote("refs/tags/v-a")
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            remote_tag,
            rev(&repo, "refs/tags/v-a"),
            "the remote must carry the tag"
        );
        delete_tag(&repo, "v-a", false, &["origin".to_string()]).expect("remote delete");
        assert!(
            ls_remote("refs/tags/v-a").trim().is_empty(),
            "the remote tag must be gone"
        );
        assert_eq!(
            tag_commit(&repo, "v-a").expect("resolve"),
            tip,
            "local tag must survive"
        );
        delete_tag(&repo, "v-a", true, &[]).expect("local delete");
        assert!(
            !wsl::wsl(&["git", "-C", &repo, "show-ref", "--verify", "--quiet", "refs/tags/v-a"])
                .status
                .success(),
            "the local tag must be gone"
        );

        // Invalid inputs never reach git.
        assert!(push_tag(&repo, "", "v-b").is_err());
        assert!(push_tag(&repo, "origin", "").is_err());
        assert!(delete_tag(&repo, "", true, &[]).is_err());
        assert!(delete_tag(&repo, "v-b", true, &["-x".to_string()]).is_err());
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn rebase_onto_replays_the_branch_and_aborts_on_conflicts() {
        let dir = wsl::temp_dir("w-rebase");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let repo = format!("{dir}/repo");
        let commit_all = |message: &str| {
            wsl::must_env(&home, &["git", "-C", &repo, "add", "-A"]);
            wsl::must_env(&home, &["git", "-C", &repo, "commit", "-q", "-m", message]);
        };
        init_repo(&home, &repo);
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "-b", "feature"]);
        wsl::write_file(&format!("{repo}/feature.txt"), b"feature\n");
        commit_all("feature work");
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "main"]);
        wsl::write_file(&format!("{repo}/main.txt"), b"main\n");
        commit_all("main work");
        let main_tip = rev(&repo, "main");
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "feature"]);

        // Wrong expected branch and option-looking targets are refused.
        assert!(rebase_onto(&repo, "main", "main", false).is_err());
        assert!(rebase_onto(&repo, "feature", "-x", false).is_err());

        // A clean rebase puts the feature commit on top of main.
        rebase_onto(&repo, "feature", "main", false).expect("clean rebase");
        assert_eq!(rev(&repo, "feature~1"), main_tip);

        // A conflicting rebase is aborted and leaves the branch untouched.
        wsl::write_file(&format!("{repo}/file.txt"), b"feature side\n");
        commit_all("feature edit");
        let feature_tip = rev(&repo, "feature");
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "main"]);
        wsl::write_file(&format!("{repo}/file.txt"), b"main side\n");
        commit_all("main edit");
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "feature"]);
        let err = rebase_onto(&repo, "feature", "main", false).expect_err("conflict");
        assert!(err.contains("aborted"), "{err}");
        assert_eq!(rev(&repo, "feature"), feature_tip);
        assert!(!wsl::exists(&format!("{repo}/.git/rebase-merge")));
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn commit_menu_ops_apply_revert_tag_checkout_and_patch() {
        let dir = wsl::temp_dir("w15");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let repo = format!("{dir}/repo");
        let commit_all = |message: &str| {
            wsl::must_env(&home, &["git", "-C", &repo, "add", "-A"]);
            wsl::must_env(&home, &["git", "-C", &repo, "commit", "-q", "-m", message]);
        };
        init_repo(&home, &repo);
        let main_tip = rev(&repo, "HEAD");
        // A side branch touching the tracked file plus its own file.
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "-b", "side"]);
        wsl::write_file(&format!("{repo}/file.txt"), b"one\nside\n");
        wsl::write_file(&format!("{repo}/side.txt"), b"side\n");
        commit_all("side work");
        let side_tip = rev(&repo, "HEAD");
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "main"]);

        // Cherry-pick lands as a new commit; no sequencer state survives.
        cherry_pick(&repo, &side_tip).expect("cherry-pick must apply cleanly");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/side.txt")]), b"side\n");
        let picked = rev(&repo, "HEAD");
        assert_ne!(picked, side_tip, "the pick must be a new commit");
        assert!(
            !wsl::exists(&format!("{repo}/.git/CHERRY_PICK_HEAD")),
            "no cherry-pick state may survive a clean pick"
        );

        // Reverting the pick drops its file again.
        revert_commit(&repo, &picked).expect("revert must apply cleanly");
        assert!(
            !wsl::exists(&format!("{repo}/side.txt")),
            "revert must remove the picked file"
        );

        // Tags: annotated with a message, lightweight without one.
        let head = rev(&repo, "HEAD");
        create_tag(&repo, "v-test", &head, "release").expect("annotated tag");
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&[
                "git", "-C", &repo, "cat-file", "-t", "v-test"
            ]))
            .trim(),
            "tag",
            "a message must make an annotated tag object"
        );
        create_tag(&repo, "v-plain", &main_tip, "").expect("lightweight tag");
        assert_eq!(rev(&repo, "v-plain"), main_tip);
        assert!(
            String::from_utf8_lossy(&wsl::must(&[
                "git", "-C", &repo, "cat-file", "-t", "v-plain"
            ]))
            .trim()
            == "commit",
            "an empty message must make a lightweight tag"
        );
        assert!(
            create_tag(&repo, "v-plain", &main_tip, "").is_err(),
            "a duplicate tag name must be refused"
        );

        // Invalid inputs never reach git.
        assert!(cherry_pick(&repo, "zzz-not-hex").is_err());
        assert!(revert_commit(&repo, "").is_err());
        assert!(checkout_detached(&repo, "../evil").is_err());
        assert!(create_tag(&repo, "", &main_tip, "").is_err());
        assert!(create_tag(&repo, "-x", &main_tip, "").is_err());
        assert!(commit_patch(&repo, "nope").is_err());

        // The patch of a known commit carries its message and diff.
        let patch = commit_patch(&repo, &side_tip).expect("patch must read");
        assert!(patch.contains("side work"), "{patch}");
        assert!(patch.contains("+side"), "{patch}");

        // Detached checkout lands on the commit with no branch attached.
        checkout_detached(&repo, &main_tip).expect("detach at the main tip");
        assert_eq!(rev(&repo, "HEAD"), main_tip);
        assert!(
            !wsl::wsl(&["git", "-C", &repo, "symbolic-ref", "-q", "HEAD"])
                .status
                .success(),
            "HEAD must be detached"
        );

        // Back on main, a conflicting dirty file refuses the checkout
        // instead of discarding work.
        wsl::must_env(&home, &["git", "-C", &repo, "checkout", "-q", "main"]);
        wsl::write_file(&format!("{repo}/file.txt"), b"dirty\n");
        assert!(
            checkout_detached(&repo, &side_tip).is_err(),
            "a checkout that would overwrite local changes must fail"
        );
        assert_eq!(rev(&repo, "HEAD"), rev(&repo, "main"));
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/file.txt")]),
            b"dirty\n",
            "the dirty worktree must survive the refusal"
        );
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn requested_repo_wins_over_inherited_git_variables() {
        let dir = wsl::temp_dir("r17");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            format!(
                "[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n\
                 [include]\n\tpath = {dir}/extra.gitconfig\n"
            )
            .as_bytes(),
        );
        wsl::write_file(&format!("{dir}/extra.gitconfig"), b"[spur]\n\tmarker = retained\n");

        let a = format!("{dir}/repo-a");
        let b = format!("{dir}/repo-b");
        let weird = format!("{dir}/weird repo; touch {dir}/pwned");
        for repo in [&a, &b, &weird] {
            init_repo(&home, repo);
        }
        wsl::write_file(&format!("{b}/only-in-b.txt"), b"b\n");
        wsl::must_env(&home, &["git", "-C", &b, "add", "only-in-b.txt"]);

        let env = linux_env(&home);
        // One routing variable at a time: combinations must not mask a defect.
        for extra in [
            vec![("GIT_DIR", format!("{b}/.git"))],
            vec![("GIT_WORK_TREE", b.clone())],
            vec![("GIT_INDEX_FILE", format!("{b}/.git/index"))],
        ] {
            let out = run_with_env(&["-C", &a, "status", "--porcelain"], &extra, &env).unwrap();
            assert!(out.success(), "{}", out.error_context());
            assert!(
                out.stdout.is_empty(),
                "a routing variable redirected the query: {}",
                out.stdout_text()
            );
        }

        // Combined routing variables plus injected configuration.
        let vars = vec![
            ("GIT_DIR", format!("{b}/.git")),
            ("GIT_WORK_TREE", b.clone()),
            ("GIT_INDEX_FILE", format!("{b}/.git/index")),
            ("GIT_CONFIG_COUNT", "1".to_string()),
            ("GIT_CONFIG_KEY_0", "core.hooksPath".to_string()),
            ("GIT_CONFIG_VALUE_0", format!("{dir}/no-hooks")),
        ];

        let out = run_with_env(&["-C", &a, "status", "--porcelain"], &vars, &env).unwrap();
        assert!(out.success(), "{}", out.error_context());
        assert!(out.stdout.is_empty(), "{}", out.stdout_text());

        // A path full of shell metacharacters is an argument, never a command.
        let weird_out =
            run_with_env(&["-C", &weird, "status", "--porcelain"], &[], &env).unwrap();
        assert!(weird_out.success(), "{}", weird_out.error_context());
        assert!(
            !wsl::exists(&format!("{dir}/pwned")),
            "a shell interpreted the repository path"
        );

        // Intended user configuration and hooks are preserved.
        let marker =
            run_with_env(&["-C", &a, "config", "--get", "spur.marker"], &[], &env).unwrap();
        assert!(marker.success(), "{}", marker.error_context());
        assert_eq!(marker.stdout_text().trim(), "retained");

        wsl::write_script(
            &format!("{a}/.git/hooks/pre-commit"),
            &format!("#!/bin/sh\ntouch {dir}/hook-ran\n"),
        );
        let commit = run_with_env(
            &["-C", &a, "commit", "--allow-empty", "-m", "hooked"],
            &vars,
            &env,
        )
        .unwrap();
        assert!(commit.success(), "{}", commit.error_context());
        assert!(wsl::exists(&format!("{dir}/hook-ran")), "pre-commit hook did not run");
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn identity_separates_worktree_git_dir_and_common_storage() {
        let dir = wsl::temp_dir("identity");
        let main = format!("{dir}/main");
        let linked = format!("{dir}/linked worktree");
        let identity_config = ["-c", "user.name=t", "-c", "user.email=t@t"];
        wsl::must(&["mkdir", "-p", &main]);
        wsl::must(&["git", "-C", &main, "init", "-q", "-b", "main"]);
        wsl::write_file(&format!("{main}/file.txt"), b"one\n");
        let mut add = vec!["git", "-C", &main];
        add.extend(identity_config);
        add.push("add");
        add.push("file.txt");
        wsl::must(&add);
        let mut commit = vec!["git", "-C", &main];
        commit.extend(identity_config);
        commit.extend(["commit", "-q", "-m", "init"]);
        wsl::must(&commit);
        wsl::must(&["git", "-C", &main, "worktree", "add", "-q", "--detach", &linked]);

        let main_id = identity(&main).unwrap();
        let linked_id = identity(&linked).unwrap();
        assert_eq!(main_id.worktree, PathBuf::from(&main));
        assert_eq!(linked_id.worktree, PathBuf::from(&linked));
        assert_eq!(main_id.git_dir, PathBuf::from(format!("{main}/.git")));
        assert_eq!(
            linked_id.git_dir,
            PathBuf::from(format!("{main}/.git/worktrees/linked-worktree"))
        );
        assert_ne!(main_id.git_dir, linked_id.git_dir);
        assert_eq!(main_id.common_dir, linked_id.common_dir);

        assert!(identity(&format!("{dir}/missing")).is_err());
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn failed_commands_report_their_exit_and_context() {
        let out = run(&["-C", "/definitely/not/a/repo", "status"]).unwrap();
        assert!(!out.success());
        assert!(out.status.is_some_and(|code| code != 0));
        assert!(
            out.error_context().contains("/definitely/not/a/repo"),
            "{}",
            out.error_context()
        );
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn ref_heavy_history_pages_via_stdin_not_argv() {
        let dir = wsl::temp_dir("r19");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let repo = format!("{dir}/repo");
        init_repo(&home, &repo);

        // 900 tags at HEAD: passed as arguments that is ~37 kB of tips, over
        // the Windows CreateProcess command-line limit that produced
        // "Der Dateiname oder die Erweiterung ist zu lang. (os error 206)".
        let refs_script = format!("{dir}/refs.sh");
        wsl::write_script(
            &refs_script,
            &format!(
                r#"#!/bin/bash
set -e
H=$(git -C "{repo}" rev-parse HEAD)
for i in $(seq 0 899); do printf 'create refs/tags/t%04d %s\n' "$i" "$H"; done | git -C "{repo}" update-ref --stdin
"#
            ),
        );
        wsl::must(&[&refs_script]);

        let tips = revision_tips(&repo);
        assert!(tips.len() >= 901, "expected >=901 tips, got {}", tips.len());
        let page = log_history(&repo, &tips, &HashSet::new(), 0, 300).expect("history must page via stdin");
        assert_eq!(page.len(), 1, "one commit expected, got {}", page.len());
        assert_eq!(page[0].subject, "init");
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn history_reads_in_date_order_like_sourcegit() {
        let dir = wsl::temp_dir("r20");
        let repo = format!("{dir}/repo");
        let script = format!("{dir}/build.sh");
        wsl::write_script(
            &script,
            &format!(
                r#"#!/bin/bash
set -e
git init -q -b main "{repo}"
git -C "{repo}" config user.name t
git -C "{repo}" config user.email t@t
c() {{
  GIT_AUTHOR_DATE="$2" GIT_COMMITTER_DATE="$2" git -C "{repo}" commit -q --allow-empty -m "$1"
}}
c init 2020-01-01T10:00:00
git -C "{repo}" branch side
c main-1 2020-01-10T10:00:00
git -C "{repo}" checkout -q side
c side-1 2020-01-05T10:00:00
c side-2 2020-01-12T10:00:00
"#
            ),
        );
        wsl::must(&[&script]);

        let tips = revision_tips(&repo);
        let page = log_history(&repo, &tips, &HashSet::new(), 0, 300).expect("history");
        let subjects: Vec<&str> = page.iter().map(|c| c.subject.as_str()).collect();
        // `--topo-order` would group the side branch: side-2, side-1, main-1.
        assert_eq!(
            subjects,
            ["side-2", "main-1", "side-1", "init"],
            "history must use date order, not topo order"
        );
    }

    /// Index-relative names currently staged (`git diff --cached --name-only`).
    fn staged_names(repo: &str) -> Vec<Vec<u8>> {
        wsl::must(&["git", "-C", repo, "diff", "--cached", "--name-only", "-z"])
            .split(|&byte| byte == 0)
            .filter(|name| !name.is_empty())
            .map(<[u8]>::to_vec)
            .collect()
    }

    fn diff_text(diff: &FileDiff) -> String {
        diff.lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn stage_unstage_touch_only_the_requested_literal_paths() {
        let dir = wsl::temp_dir("w01");
        let repo = format!("{dir}/repo");
        let script = format!("{dir}/build.sh");
        wsl::write_script(
            &script,
            &format!(
                r#"#!/bin/bash
set -e
R="{dir}/repo"
mkdir -p "$R"
git -C "$R" init -q -b main
git -C "$R" config user.name t
git -C "$R" config user.email t@t
cd "$R"
printf 'keep\n' > '*.txt'
printf 'one\n' > normal.txt
printf 'dash\n' > -n
printf 'spaced\n' > 'sp ace.txt'
printf 'glob\n' > ':(glob)*'
printf 'weird\n' > "$(printf 'bad\xffname')" || true  # APFS rejects non-UTF-8 names
git add -A
git commit -q -m base
printf 'two\n' >> normal.txt
printf 'changed\n' >> '*.txt'
printf 'new\n' > 'new file.txt'
printf 'glob-change\n' >> ':(glob)*'
printf 'weird-change\n' >> "$(printf 'bad\xffname')" || true
"#
            ),
        );
        wsl::must(&[&script]);

        let mut targets: Vec<Vec<u8>> = vec![
            b"*.txt".to_vec(),
            b"new file.txt".to_vec(),
            b":(glob)*".to_vec(),
        ];
        // APFS cannot hold a non-UTF-8 file name.
        if cfg!(not(target_os = "macos")) {
            targets.push(b"bad\xffname".to_vec());
        }
        stage_paths(&repo, &targets).expect("stage must succeed");

        let staged = staged_names(&repo);
        for target in &targets {
            assert!(
                staged.contains(target),
                "missing {:?} in staged set",
                String::from_utf8_lossy(target)
            );
        }
        assert!(
            !staged.contains(&b"normal.txt".to_vec()),
            "a wildcard pathspec staged an unrelated file: {staged:?}"
        );

        // Worktree content is untouched by staging; the file is still dirty.
        let status = wsl::must(&["git", "-C", &repo, "diff", "--name-only", "-z"]);
        assert!(status.windows(10).any(|w| w == b"normal.txt"));

        unstage_paths(&repo, &targets).expect("unstage must succeed");
        assert!(
            staged_names(&repo).is_empty(),
            "unstage left entries staged: {:?}",
            staged_names(&repo)
        );
        // `git status` covers both modified tracked files and the untracked
        // new file, so it proves unstaging kept every worktree byte.
        let after = wsl::must(&["git", "-C", &repo, "status", "--porcelain", "-z"]);
        for target in &targets {
            assert!(
                after.windows(target.len()).any(|w| w == target.as_slice()),
                "unstaging discarded worktree content for {:?}",
                String::from_utf8_lossy(target)
            );
        }

        // Unstage must also work before the first commit (`restore --staged`
        // cannot resolve HEAD there).
        let unborn = format!("{dir}/unborn");
        let unborn_script = format!("{dir}/unborn.sh");
        wsl::write_script(
            &unborn_script,
            &format!(
                r#"#!/bin/bash
set -e
git init -q -b main "{unborn}"
printf 'born\n' > "{unborn}/born.txt"
"#
            ),
        );
        wsl::must(&[&unborn_script]);
        stage_paths(&unborn, &[b"born.txt".to_vec()]).expect("stage in unborn repo");
        assert_eq!(staged_names(&unborn), vec![b"born.txt".to_vec()]);
        unstage_paths(&unborn, &[b"born.txt".to_vec()]).expect("unstage in unborn repo");
        assert!(staged_names(&unborn).is_empty());
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn diff_sides_match_git_objects_and_never_run_display_helpers() {
        let dir = wsl::temp_dir("w02");
        let repo = format!("{dir}/repo");
        let script = format!("{dir}/build.sh");
        wsl::write_script(
            &script,
            &format!(
                r#"#!/bin/bash
set -e
R="{dir}/repo"
mkdir -p "$R"
git -C "$R" init -q -b main
git -C "$R" config user.name t
git -C "$R" config user.email t@t
cd "$R"
printf 'A1\nA2\n' > tracked.txt
printf 'gone\n' > deleted.txt
printf 'BIN\000DATA\n' > binary.bin
printf 'U1\nU2\n' > loose-untracked.txt
git add -A
git commit -q -m base
printf 'B1\nB2\n' > tracked.txt
git add tracked.txt
printf 'C1\nC2\n' > tracked.txt
printf 'BIN2\000DATA\n' > binary.bin
rm deleted.txt
yes 'line' | head -c 3000000 > big-untracked.txt

cat > "{dir}/ext-diff.sh" <<'EOF'
#!/bin/sh
touch "$0.sentinel"
exit 0
EOF
cat > "{dir}/textconv.sh" <<'EOF'
#!/bin/sh
touch "$0.sentinel"
cat "$1"
EOF
chmod +x "{dir}/ext-diff.sh" "{dir}/textconv.sh"
git -C "$R" config diff.external "{dir}/ext-diff.sh"
git -C "$R" config diff.tr.textconv "{dir}/textconv.sh"
printf 'tracked.txt diff=tr\n' > "$R/.gitattributes"
git -C "$R" add .gitattributes
"#
            ),
        );
        wsl::must(&[&script]);

        let staged = diff_file(&repo, b"tracked.txt", true, false).expect("staged diff");
        let staged_text = diff_text(&staged);
        assert!(staged_text.contains("A1") && staged_text.contains("B1"), "{staged_text}");
        assert!(!staged_text.contains("C1"), "staged diff showed worktree content");

        let unstaged = diff_file(&repo, b"tracked.txt", false, false).expect("unstaged diff");
        let unstaged_text = diff_text(&unstaged);
        assert!(unstaged_text.contains("B1") && unstaged_text.contains("C1"), "{unstaged_text}");
        assert!(!unstaged_text.contains("A1"), "unstaged diff showed committed content");

        // Deleted tracked file: removals only.
        let deleted = diff_file(&repo, b"deleted.txt", false, false).expect("deleted diff");
        assert!(deleted.deletions > 0 && deleted.additions == 0, "{deleted:?}");

        // Modified binary: explicit binary state, no text lines parsed.
        let binary = diff_file(&repo, b"binary.bin", false, false).expect("binary diff");
        assert!(binary.binary, "{binary:?}");

        // Untracked preview: synthesized all-additions.
        let untracked = diff_file(&repo, b"loose-untracked.txt", false, true).expect("untracked");
        assert_eq!(untracked.additions, 2);
        assert_eq!(untracked.deletions, 0);
        let untracked_text = diff_text(&untracked);
        assert!(untracked_text.contains("U1") && untracked_text.contains("U2"));

        // Oversized untracked file: capped and flagged, not an error.
        let big = diff_file(&repo, b"big-untracked.txt", false, true).expect("big untracked");
        assert!(big.truncated, "3 MB preview must be truncated");

        // The internal viewer never invokes external diff or textconv helpers.
        assert!(
            !wsl::exists(&format!("{dir}/ext-diff.sh.sentinel")),
            "external diff helper ran"
        );
        assert!(
            !wsl::exists(&format!("{dir}/textconv.sh.sentinel")),
            "textconv helper ran"
        );
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn single_hunk_patches_stage_unstage_and_discard_one_block() {
        let dir = wsl::temp_dir("w06");
        let repo = format!("{dir}/repo");
        let script = format!("{dir}/build.sh");
        wsl::write_script(
            &script,
            &format!(
                r#"#!/bin/bash
set -e
R="{repo}"
mkdir -p "$R"
git -C "$R" init -q -b main
git -C "$R" config user.name t
git -C "$R" config user.email t@t
cd "$R"
printf 'l01\nl02\nl03\nl04\nl05\nl06\nl07\nl08\nl09\nl10\nl11\nl12\nl13\nl14\n' > f.txt
git add -A
git commit -q -m base
printf 'l01\nCHANGED-TOP\nl03\nl04\nl05\nl06\nl07\nl08\nl09\nl10\nl11\nl12\nCHANGED-BOTTOM\nl14\n' > f.txt
"#
            ),
        );
        wsl::must(&[&script]);

        let diff = diff_file(&repo, b"f.txt", false, false).expect("diff");
        assert_eq!(diff.hunks.len(), 2, "fixture must produce two hunks");

        // Stage only the second hunk: the index gets it, the worktree keeps both.
        let patch = diff.patch_for_hunk(1).expect("hunk patch");
        apply_patch(&repo, &patch, true, false).expect("stage hunk");
        let staged = wsl::must(&["git", "-C", &repo, "show", ":f.txt"]);
        let staged = String::from_utf8_lossy(&staged);
        assert!(staged.contains("CHANGED-BOTTOM"), "{staged}");
        assert!(!staged.contains("CHANGED-TOP"), "{staged}");
        let worktree = wsl::must(&["cat", &format!("{repo}/f.txt")]);
        let worktree = String::from_utf8_lossy(&worktree);
        assert!(
            worktree.contains("CHANGED-TOP") && worktree.contains("CHANGED-BOTTOM"),
            "staging a hunk must not touch the worktree: {worktree}"
        );

        // Unstage it again.
        apply_patch(&repo, &patch, true, true).expect("unstage hunk");
        assert!(staged_names(&repo).is_empty());

        // Discard only the first hunk from the worktree.
        let first = diff.patch_for_hunk(0).expect("first hunk patch");
        apply_patch(&repo, &first, false, true).expect("discard hunk");
        let worktree = wsl::must(&["cat", &format!("{repo}/f.txt")]);
        let worktree = String::from_utf8_lossy(&worktree);
        assert!(!worktree.contains("CHANGED-TOP"), "{worktree}");
        assert!(worktree.contains("CHANGED-BOTTOM"), "{worktree}");
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn discard_and_stash_touch_only_the_requested_path() {
        let dir = wsl::temp_dir("w05");
        let repo = format!("{dir}/repo");
        let script = format!("{dir}/build.sh");
        wsl::write_script(
            &script,
            &format!(
                r#"#!/bin/bash
set -e
R="{repo}"
mkdir -p "$R"
git -C "$R" init -q -b main
git -C "$R" config user.name t
git -C "$R" config user.email t@t
cd "$R"
printf 'A\n' > a.txt
printf 'B\n' > b.txt
git add -A
git commit -q -m base
"#
            ),
        );
        wsl::must(&[&script]);

        // Worktree-only discard reverts the requested path and nothing else.
        wsl::write_file(&format!("{repo}/a.txt"), b"A-changed\n");
        wsl::write_file(&format!("{repo}/b.txt"), b"B-changed\n");
        restore_paths(&repo, &[b"a.txt".to_vec()], false).expect("discard a.txt");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/a.txt")]), b"A\n");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/b.txt")]), b"B-changed\n");

        // Staged discard resets index and worktree to HEAD for that path.
        wsl::write_file(&format!("{repo}/a.txt"), b"A-staged\n");
        stage_paths(&repo, &[b"a.txt".to_vec()]).unwrap();
        restore_paths(&repo, &[b"a.txt".to_vec()], true).expect("discard staged a.txt");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/a.txt")]), b"A\n");
        assert!(staged_names(&repo).is_empty());
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/b.txt")]),
            b"B-changed\n",
            "discard must not touch other paths"
        );

        // Stash frees the worktree and records the change; the stash message
        // identifies the path.
        wsl::write_file(&format!("{repo}/a.txt"), b"A-stashed\n");
        stash_paths(
            &repo,
            &[b"a.txt".to_vec()],
            false,
            "Spur: a.txt",
            StashMode::Discard,
        )
        .expect("stash a.txt");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/a.txt")]), b"A\n");
        let listed = wsl::must(&["git", "-C", &repo, "stash", "list"]);
        let stash_list = String::from_utf8_lossy(&listed).into_owned();
        assert!(stash_list.contains("Spur: a.txt"), "{stash_list}");

        // Deleting an untracked path removes only that file.
        wsl::write_file(&format!("{repo}/u.txt"), b"loose\n");
        delete_untracked_paths(&repo, &[b"u.txt".to_vec()]).expect("delete u.txt");
        assert!(!wsl::exists(&format!("{repo}/u.txt")));
        assert_eq!(wsl::must(&["cat", &format!("{repo}/b.txt")]), b"B-changed\n");

        // Untracked stash includes the file in the stash entry.
        wsl::write_file(&format!("{repo}/u2.txt"), b"loose2\n");
        stash_paths(
            &repo,
            &[b"u2.txt".to_vec()],
            true,
            "Spur: u2.txt",
            StashMode::Discard,
        )
        .expect("stash u2");
        assert!(!wsl::exists(&format!("{repo}/u2.txt")));
        let listed = wsl::must(&["git", "-C", &repo, "stash", "list"]);
        let stash_list = String::from_utf8_lossy(&listed).into_owned();
        assert!(stash_list.contains("Spur: u2.txt"), "{stash_list}");

        // Keep Index: staged changes stay intact, the unstaged part is stashed.
        wsl::write_file(&format!("{repo}/a.txt"), b"A-staged2\n");
        stage_paths(&repo, &[b"a.txt".to_vec()]).unwrap();
        wsl::write_file(&format!("{repo}/a.txt"), b"A-staged2\nunstaged\n");
        stash_paths(
            &repo,
            &[b"a.txt".to_vec()],
            false,
            "Spur: a.txt keep-index",
            StashMode::KeepIndex,
        )
        .expect("stash keep-index");
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/a.txt")]),
            b"A-staged2\n",
            "keep-index must leave the staged content in the worktree"
        );
        assert_eq!(staged_names(&repo), vec![b"a.txt".to_vec()]);
        unstage_paths(&repo, &[b"a.txt".to_vec()]).unwrap();
        restore_paths(&repo, &[b"a.txt".to_vec()], false).unwrap();
        assert_eq!(wsl::must(&["cat", &format!("{repo}/a.txt")]), b"A\n");

        // Keep All: a stash entry exists and the worktree is untouched.
        wsl::write_file(&format!("{repo}/b.txt"), b"B-keepall\n");
        stash_paths(
            &repo,
            &[b"b.txt".to_vec()],
            false,
            "Spur: b.txt keep-all",
            StashMode::KeepAll,
        )
        .expect("stash keep-all");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/b.txt")]), b"B-keepall\n");
        let listed = wsl::must(&["git", "-C", &repo, "stash", "list"]);
        let stash_list = String::from_utf8_lossy(&listed).into_owned();
        assert!(stash_list.contains("Spur: b.txt keep-all"), "{stash_list}");
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn commit_means_the_staged_snapshot_and_hooks_stay_authoritative() {
        let dir = wsl::temp_dir("w03");
        let repo = format!("{dir}/repo");
        let script = format!("{dir}/build.sh");
        wsl::write_script(
            &script,
            &format!(
                r#"#!/bin/bash
set -e
R="{dir}/repo"
mkdir -p "$R"
git -C "$R" init -q -b main
git -C "$R" config user.name t
git -C "$R" config user.email t@t
cd "$R"
printf 'A1\nA2\n' > tracked.txt
printf 'K\n' > keep.txt
git add -A
git commit -q -m base
printf 'B1\nB2\n' > tracked.txt
git add tracked.txt
printf 'C1\nC2\n' > tracked.txt
printf 'loose\n' > loose.txt
"#
            ),
        );
        wsl::must(&[&script]);

        let before = collect_status(&repo).expect("status before commit");
        commit_staged(&repo, b"subject line\n\nbody line\n").expect("commit");
        let after = collect_status(&repo).expect("status after commit");
        assert_ne!(
            before.history_signature(),
            after.history_signature(),
            "a commit must change the history signature"
        );

        let subject = String::from_utf8_lossy(&wsl::must(&[
            "git", "-C", &repo, "log", "-1", "--format=%s",
        ]))
        .trim()
        .to_string();
        assert_eq!(subject, "subject line");
        let body = String::from_utf8_lossy(&wsl::must(&[
            "git", "-C", &repo, "log", "-1", "--format=%B",
        ]))
        .replace("\r\n", "\n");
        assert_eq!(body.trim_end(), "subject line\n\nbody line");

        // Only the staged path is in the commit; the worktree C content and
        // the untracked file stay out.
        let files = String::from_utf8_lossy(&wsl::must(&[
            "git", "-C", &repo, "show", "--name-only", "--format=", "HEAD",
        ]))
        .replace("\r\n", "\n");
        assert_eq!(files.trim(), "tracked.txt", "{files}");
        assert_eq!(
            wsl::must(&["git", "-C", &repo, "show", "HEAD:tracked.txt"]),
            b"B1\nB2\n",
            "the commit must contain the staged blob, not worktree content"
        );
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/tracked.txt")]),
            b"C1\nC2\n",
            "the commit must not touch worktree content"
        );
        let status = wsl::must(&["git", "-C", &repo, "status", "--porcelain", "-z"]);
        assert!(
            status.windows(9).any(|window| window == b"loose.txt"),
            "untracked file disappeared: {status:?}"
        );

        // A rejecting pre-commit hook leaves HEAD, index, and worktree bytes
        // unchanged, and its output surfaces as the error.
        wsl::write_script(
            &format!("{repo}/.git/hooks/pre-commit"),
            "#!/bin/sh\necho 'hook says no' >&2\nexit 1\n",
        );
        wsl::write_file(&format!("{repo}/keep.txt"), b"K2\n");
        stage_paths(&repo, &[b"keep.txt".to_vec()]).unwrap();
        let head_before = wsl::must(&["git", "-C", &repo, "rev-parse", "HEAD"]);
        let index_before = wsl::must(&["git", "-C", &repo, "ls-files", "--stage", "-z"]);
        let signature_before_failure =
            collect_status(&repo).expect("status").history_signature();
        let err = commit_staged(&repo, b"must fail\n").unwrap_err();
        assert!(err.contains("hook says no"), "{err}");
        assert_eq!(wsl::must(&["git", "-C", &repo, "rev-parse", "HEAD"]), head_before);
        assert_eq!(
            wsl::must(&["git", "-C", &repo, "ls-files", "--stage", "-z"]),
            index_before
        );
        assert_eq!(wsl::must(&["cat", &format!("{repo}/keep.txt")]), b"K2\n");
        assert_eq!(
            collect_status(&repo).expect("status").history_signature(),
            signature_before_failure,
            "a rejected commit must not change the history signature"
        );

        // Initial commit in an unborn repository.
        let unborn = format!("{dir}/unborn");
        let unborn_script = format!("{dir}/unborn.sh");
        wsl::write_script(
            &unborn_script,
            &format!(
                r#"#!/bin/bash
set -e
git init -q -b main "{unborn}"
git -C "{unborn}" config user.name t
git -C "{unborn}" config user.email t@t
printf 'born\n' > "{unborn}/born.txt"
"#
            ),
        );
        wsl::must(&[&unborn_script]);
        stage_paths(&unborn, &[b"born.txt".to_vec()]).unwrap();
        commit_staged(&unborn, b"initial commit\n").expect("unborn commit");
        let root_subject = String::from_utf8_lossy(&wsl::must(&[
            "git", "-C", &unborn, "log", "-1", "--format=%s",
        ]))
        .trim()
        .to_string();
        assert_eq!(root_subject, "initial commit");
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn create_branch_honors_base_checkout_overwrite_and_changes() {
        wsl::watchdog(240);
        let dir = wsl::temp_dir("w07");
        let repo = format!("{dir}/repo");
        let script = format!("{dir}/build.sh");
        wsl::write_script(
            &script,
            &format!(
                r#"#!/bin/bash
set -e
git init -q -b main "{repo}"
git -C "{repo}" config user.name t
git -C "{repo}" config user.email t@t
printf 'one\n' > "{repo}/f.txt"
git -C "{repo}" add f.txt
git -C "{repo}" commit -q -m one
printf 'two\n' > "{repo}/f.txt"
git -C "{repo}" commit -qam two
"#
            ),
        );
        wsl::must(&[&script]);
        let head = || -> String {
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "rev-parse", "HEAD"]))
                .trim()
                .to_string()
        };
        let parent = || -> String {
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "rev-parse", "HEAD~1"]))
                .trim()
                .to_string()
        };
        let root_commit = parent();
        let rev = |reference: &str| -> String {
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "rev-parse", reference]))
                .trim()
                .to_string()
        };

        // Dirty worktree: one tracked modification and one untracked file.
        wsl::write_file(&format!("{repo}/f.txt"), b"dirty\n");
        wsl::write_file(&format!("{repo}/untracked.txt"), b"loose\n");

        // Create without checkout: the branch points at the base, HEAD and
        // every worktree byte stay put.
        create_branch(&repo, "feature/x", "main", false, false, BranchChanges::Keep)
            .expect("create without checkout");
        assert_eq!(rev("feature/x"), head());
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "symbolic-ref", "--short", "HEAD"])).trim(),
            "main"
        );
        assert_eq!(wsl::must(&["cat", &format!("{repo}/f.txt")]), b"dirty\n");
        assert!(wsl::exists(&format!("{repo}/untracked.txt")));

        // Existing branch without overwrite is refused.
        let err = create_branch(&repo, "feature/x", "main", false, false, BranchChanges::Keep)
            .unwrap_err();
        assert!(err.contains("already exists"), "{err}");

        // Overwrite moves it to the requested base.
        create_branch(&repo, "feature/x", "HEAD~1", false, true, BranchChanges::Keep)
            .expect("overwrite");
        assert_eq!(rev("feature/x"), root_commit);

        // Checkout with Stash: changes are stashed and reapplied on the new
        // branch, leaving no stash entry behind.
        create_branch(&repo, "stash-switch", "main", true, false, BranchChanges::Stash)
            .expect("checkout with stash");
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "symbolic-ref", "--short", "HEAD"])).trim(),
            "stash-switch"
        );
        assert_eq!(wsl::must(&["cat", &format!("{repo}/f.txt")]), b"dirty\n");
        assert!(wsl::exists(&format!("{repo}/untracked.txt")));
        assert_eq!(wsl::must(&["git", "-C", &repo, "stash", "list"]), b"");

        // Checkout with Discard: the modification is gone, untracked files
        // that do not obstruct the switch remain untouched.
        create_branch(&repo, "clean-switch", "main", true, false, BranchChanges::Discard)
            .expect("checkout with discard");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/f.txt")]), b"two\n");

        // Context-menu checkout: a conflicting local change is refused and
        // HEAD stays put; after clearing it, the switch lands on the branch.
        let head_before_conflict = head();
        wsl::write_file(&format!("{repo}/f.txt"), b"conflict\n");
        let err = checkout_branch(&repo, "feature/x").unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(head(), head_before_conflict, "a refused checkout must not move HEAD");
        wsl::must(&["git", "-C", &repo, "checkout", "--", "f.txt"]);
        checkout_branch(&repo, "feature/x").expect("checkout after clearing");
        assert_eq!(head(), root_commit);
        assert_eq!(wsl::must(&["cat", &format!("{repo}/f.txt")]), b"one\n");

        // Invalid names and an invalid base fail with Git's own message.
        let err = create_branch(&repo, "bad..name", "main", false, false, BranchChanges::Keep)
            .unwrap_err();
        assert!(!err.is_empty());
        let err = create_branch(&repo, "no-base", "does-not-exist", false, false, BranchChanges::Keep)
            .unwrap_err();
        assert!(!err.is_empty());
        assert!(
            wsl::must(&["git", "-C", &repo, "branch", "--list", "no-base"]).is_empty(),
            "a failed creation must not leave the branch behind"
        );

        // Remotes: add a remote, push a branch that has no local counterpart,
        // then check it out: the local branch is created with an upstream.
        let bare = format!("{dir}/remote.git");
        wsl::must(&["git", "init", "-q", "--bare", "-b", "main", &bare]);
        add_remote(&repo, "origin2", &bare).expect("add remote");
        assert_eq!(
            wsl::must(&["git", "-C", &repo, "remote", "get-url", "origin2"]),
            format!("{bare}\n").as_bytes()
        );
        let duplicate = add_remote(&repo, "origin2", &bare).unwrap_err();
        assert!(!duplicate.is_empty(), "a duplicate remote must fail");
        wsl::must(&[
            "git",
            "-C",
            &repo,
            "push",
            "-q",
            "origin2",
            "feature/x:refs/heads/remote-only",
        ]);
        let refs = ref_info(&repo).expect("refs");
        assert!(
            refs.remote_branches
                .iter()
                .any(|branch| branch.remote == "origin2" && branch.name == "remote-only"),
            "{:?}",
            refs.remote_branches
        );
        checkout_remote_branch(&repo, "origin2", "remote-only").expect("checkout remote branch");
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&[
                "git",
                "-C",
                &repo,
                "symbolic-ref",
                "--short",
                "HEAD",
            ]))
            .trim(),
            "remote-only"
        );
        assert_eq!(
            rev("remote-only"),
            rev("origin2/remote-only"),
            "the tracking branch must start at the remote ref"
        );
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&[
                "git",
                "-C",
                &repo,
                "rev-parse",
                "--abbrev-ref",
                "remote-only@{upstream}",
            ]))
            .trim(),
            "origin2/remote-only"
        );
        // An existing local branch just switches (no -b failure).
        checkout_remote_branch(&repo, "origin2", "main").expect("checkout existing local");
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "symbolic-ref", "--short", "HEAD"])).trim(),
            "main"
        );
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn commit_detail_reads_metadata_files_and_file_diffs() {
        wsl::watchdog(240);
        let dir = wsl::temp_dir("w08");
        let repo = format!("{dir}/repo");
        wsl::must(&["git", "init", "-q", "-b", "main", &repo]);
        let config_name = ["git", "-C", &repo, "config", "user.name", "Ada"];
        wsl::must(&config_name);
        wsl::must(&["git", "-C", &repo, "config", "user.email", "ada@example.com"]);
        wsl::must(&["git", "-C", &repo, "config", "diff.renames", "true"]);
        wsl::write_file(&format!("{repo}/keep.txt"), b"keep\n");
        wsl::write_file(&format!("{repo}/edit.txt"), b"one\n");
        wsl::write_file(&format!("{repo}/old name.txt"), b"rename me\n");
        wsl::must(&["git", "-C", &repo, "add", "."]);
        wsl::must(&[
            "git", "-C", &repo, "commit", "-q", "-m", "root commit\n\nbody line",
        ]);
        wsl::write_file(&format!("{repo}/edit.txt"), b"one\ntwo\n");
        wsl::write_file(&format!("{repo}/added.txt"), b"new\n");
        wsl::must(&["git", "-C", &repo, "rm", "-q", "keep.txt"]);
        wsl::must(&["git", "-C", &repo, "mv", "old name.txt", "renamed.txt"]);
        wsl::must(&["git", "-C", &repo, "add", "."]);
        wsl::must(&["git", "-C", &repo, "commit", "-q", "-m", "second"]);

        let head_detail = commit_detail(&repo, "HEAD").expect("head detail");
        assert_eq!(head_detail.author, "Ada");
        assert_eq!(head_detail.email, "ada@example.com");
        assert!(head_detail.date.starts_with("20"), "{}", head_detail.date);
        assert_eq!(head_detail.parents.len(), 1);
        assert_eq!(head_detail.message.trim(), "second");
        let by_path = |path: &[u8]| {
            head_detail
                .files
                .iter()
                .find(|file| file.path == path)
                .unwrap_or_else(|| panic!("missing file {}", String::from_utf8_lossy(path)))
        };
        assert_eq!(by_path(b"edit.txt").status, 'M');
        assert_eq!(by_path(b"added.txt").status, 'A');
        assert_eq!(by_path(b"keep.txt").status, 'D');
        let renamed = by_path(b"renamed.txt");
        assert_eq!(renamed.status, 'R');
        assert_eq!(
            renamed.orig_path.as_deref(),
            Some(b"old name.txt".as_ref())
        );
        // Line totals come from the same first-parent diff: edit +1,
        // added +1, deleted -1, the rename contributes nothing.
        assert_eq!((head_detail.additions, head_detail.deletions), (2, 1));

        // The patch for one file matches the worktree oracle.
        let diff = commit_file_diff(&repo, Some(&head_detail.parents[0]), "HEAD", b"edit.txt")
            .expect("file diff");
        assert_eq!(diff.additions, 1);
        assert_eq!(diff.deletions, 0);
        assert!(
            diff.lines
                .iter()
                .any(|line| line.kind == crate::git::DiffLineKind::Added && line.text == "two"),
            "{:?}",
            diff.lines.iter().map(|line| &line.text).collect::<Vec<_>>()
        );

        // Root commit: no parents, every file is an addition, and the patch
        // works through the `--root` path.
        let root_hash = String::from_utf8_lossy(&wsl::must(&[
            "git", "-C", &repo, "rev-parse", "HEAD~1",
        ]))
        .trim()
        .to_string();
        let root = commit_detail(&repo, &root_hash).expect("root detail");
        assert!(root.parents.is_empty());
        assert!(root.files.iter().all(|file| file.status == 'A'));
        assert!(root.files.iter().any(|file| file.path == b"keep.txt"));
        assert!(root.message.contains("root commit"));
        assert_eq!((root.additions, root.deletions), (3, 0));
        let root_diff = commit_file_diff(&repo, None, &root_hash, b"keep.txt").expect("root diff");
        assert!(
            root_diff
                .lines
                .iter()
                .any(|line| line.kind == crate::git::DiffLineKind::Added && line.text == "keep")
        );
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn push_targets_explicit_refspecs_and_refuses_non_fast_forward() {
        wsl::watchdog(240);
        let dir = wsl::temp_dir("w09");
        let origin = format!("{dir}/origin.git");
        let mirror = format!("{dir}/mirror.git");
        let repo = format!("{dir}/repo");
        let other = format!("{dir}/other");
        wsl::must(&["git", "init", "-q", "--bare", "-b", "main", &origin]);
        wsl::must(&["git", "clone", "-q", &origin, &repo]);
        wsl::must(&["git", "-C", &repo, "config", "user.name", "Ada"]);
        wsl::must(&["git", "-C", &repo, "config", "user.email", "ada@example.com"]);
        let rev = |path: &str, reference: &str| -> String {
            String::from_utf8_lossy(&wsl::must(&["git", "-C", path, "rev-parse", reference]))
                .trim()
                .to_string()
        };

        // First push: no upstream yet, the explicit refspec creates the remote
        // branch and --set-upstream records it.
        wsl::write_file(&format!("{repo}/a.txt"), b"one\n");
        wsl::must(&["git", "-C", &repo, "add", "."]);
        wsl::must(&["git", "-C", &repo, "commit", "-q", "-m", "first"]);
        push_branch(&repo, "origin", "main", "main", true).expect("first push");
        assert_eq!(rev(&origin, "refs/heads/main"), rev(&repo, "HEAD"));
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&[
                "git", "-C", &repo, "rev-parse", "--abbrev-ref", "main@{upstream}",
            ]))
            .trim(),
            "origin/main"
        );

        // Existing upstream: a plain update without -u.
        wsl::write_file(&format!("{repo}/a.txt"), b"one\ntwo\n");
        wsl::must(&["git", "-C", &repo, "commit", "-qam", "second"]);
        push_branch(&repo, "origin", "main", "main", false).expect("update push");
        assert_eq!(rev(&origin, "refs/heads/main"), rev(&repo, "HEAD"));

        // Second remote with a different target branch name: the refspec is
        // explicit, so the destination exists under the chosen name.
        wsl::must(&["git", "init", "-q", "--bare", "-b", "main", &mirror]);
        wsl::must(&["git", "-C", &repo, "remote", "add", "mirror", &mirror]);
        push_branch(&repo, "mirror", "main", "main-mirror", true).expect("mirror push");
        assert_eq!(rev(&mirror, "refs/heads/main-mirror"), rev(&repo, "HEAD"));
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&[
                "git", "-C", &repo, "rev-parse", "--abbrev-ref", "main@{upstream}",
            ]))
            .trim(),
            "mirror/main-mirror"
        );

        // Another clone moves origin/main forward; our local branch diverges.
        wsl::must(&["git", "clone", "-q", &origin, &other]);
        wsl::must(&["git", "-C", &other, "config", "user.name", "Bob"]);
        wsl::must(&["git", "-C", &other, "config", "user.email", "bob@example.com"]);
        wsl::write_file(&format!("{other}/b.txt"), b"theirs\n");
        wsl::must(&["git", "-C", &other, "add", "."]);
        wsl::must(&["git", "-C", &other, "commit", "-q", "-m", "theirs"]);
        push_branch(&other, "origin", "main", "main", false).expect("other push");
        let theirs = rev(&origin, "refs/heads/main");
        wsl::write_file(&format!("{repo}/c.txt"), b"ours\n");
        wsl::must(&["git", "-C", &repo, "add", "."]);
        wsl::must(&["git", "-C", &repo, "commit", "-q", "-m", "ours"]);
        let local_before = rev(&repo, "HEAD");

        let err = push_branch(&repo, "origin", "main", "main", false).unwrap_err();
        assert!(
            err.contains("rejected") || err.contains("non-fast-forward") || err.contains("fetch first"),
            "expected a rejection message, got: {err}"
        );
        assert_eq!(rev(&origin, "refs/heads/main"), theirs, "no force push may happen");
        assert_eq!(rev(&repo, "HEAD"), local_before, "a rejected push must not move HEAD");
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn fetch_updates_remotes_with_force_and_tag_options() {
        wsl::watchdog(240);
        let dir = wsl::temp_dir("w10");
        let origin = format!("{dir}/origin.git");
        let mirror = format!("{dir}/mirror.git");
        let repo = format!("{dir}/repo");
        let other = format!("{dir}/other");
        wsl::must(&["git", "init", "-q", "--bare", "-b", "main", &origin]);
        wsl::must(&["git", "clone", "-q", &origin, &repo]);
        wsl::must(&["git", "-C", &repo, "config", "user.name", "Ada"]);
        wsl::must(&["git", "-C", &repo, "config", "user.email", "ada@example.com"]);
        wsl::write_file(&format!("{repo}/a.txt"), b"one\n");
        wsl::must(&["git", "-C", &repo, "add", "."]);
        wsl::must(&["git", "-C", &repo, "commit", "-q", "-m", "init"]);
        push_branch(&repo, "origin", "main", "main", true).expect("seed push");
        let seeded = String::from_utf8_lossy(&wsl::must(&[
            "git", "-C", &repo, "rev-parse", "refs/remotes/origin/main",
        ]))
        .trim()
        .to_string();

        // A publisher moves origin/main forward; nothing updates locally until
        // the fetch runs.
        wsl::must(&["git", "clone", "-q", &origin, &other]);
        wsl::must(&["git", "-C", &other, "config", "user.name", "Bob"]);
        wsl::must(&["git", "-C", &other, "config", "user.email", "bob@example.com"]);
        wsl::write_file(&format!("{other}/b.txt"), b"theirs\n");
        wsl::must(&["git", "-C", &other, "add", "."]);
        wsl::must(&["git", "-C", &other, "commit", "-q", "-m", "theirs"]);
        wsl::must(&["git", "-C", &other, "tag", "v9"]);
        wsl::must(&["git", "-C", &other, "push", "-q", "origin", "main", "v9"]);
        let theirs = String::from_utf8_lossy(&wsl::must(&[
            "git", "-C", &other, "rev-parse", "HEAD",
        ]))
        .trim()
        .to_string();
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&[
                "git", "-C", &repo, "rev-parse", "refs/remotes/origin/main",
            ]))
            .trim(),
            seeded,
            "the remote-tracking ref must not move without a fetch"
        );

        // A normal fetch updates the remote-tracking ref and takes tags.
        fetch_remote(&repo, Some("origin"), false, false).expect("fetch");
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&[
                "git", "-C", &repo, "rev-parse", "refs/remotes/origin/main",
            ]))
            .trim(),
            theirs
        );
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "tag", "--list", "v9"])).trim(),
            "v9"
        );

        // `--no-tags` leaves new tags behind; `--force` still succeeds.
        wsl::must(&["git", "-C", &other, "tag", "v10"]);
        wsl::must(&["git", "-C", &other, "push", "-q", "origin", "v10"]);
        fetch_remote(&repo, Some("origin"), false, true).expect("fetch without tags");
        assert!(
            wsl::must(&["git", "-C", &repo, "tag", "--list", "v10"]).is_empty(),
            "fetch --no-tags must not create the tag locally"
        );
        fetch_remote(&repo, Some("origin"), true, false).expect("forced fetch");
        assert!(
            !wsl::must(&["git", "-C", &repo, "tag", "--list", "v10"]).is_empty(),
            "a later normal fetch takes the tag"
        );

        // Fetching all remotes updates every remote-tracking ref.
        wsl::must(&["git", "init", "-q", "--bare", "-b", "main", &mirror]);
        wsl::must(&["git", "-C", &repo, "remote", "add", "mirror", &mirror]);
        push_branch(&repo, "mirror", "main", "main", false).expect("mirror seed");
        wsl::write_file(&format!("{other}/c.txt"), b"more\n");
        wsl::must(&["git", "-C", &other, "add", "."]);
        wsl::must(&["git", "-C", &other, "commit", "-q", "-m", "more"]);
        wsl::must(&["git", "-C", &other, "push", "-q", "origin", "main"]);
        fetch_remote(&repo, None, false, false).expect("fetch all");
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&[
                "git", "-C", &repo, "rev-parse", "refs/remotes/origin/main",
            ]))
            .trim(),
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &other, "rev-parse", "HEAD"])).trim()
        );
        assert!(
            !wsl::must(&["git", "-C", &repo, "show-ref", "refs/remotes/mirror/main"]).is_empty(),
            "fetch --all must keep the second remote's tracking ref"
        );

        // An unknown remote is an error, not a silent no-op.
        assert!(fetch_remote(&repo, Some("nope"), false, false).is_err());
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn stash_view_lists_files_and_diffs_tracked_and_untracked() {
        wsl::watchdog(240);
        let dir = wsl::temp_dir("w11");
        let repo = format!("{dir}/repo");
        wsl::must(&["git", "init", "-q", "-b", "main", &repo]);
        wsl::must(&["git", "-C", &repo, "config", "user.name", "Ada"]);
        wsl::must(&["git", "-C", &repo, "config", "user.email", "ada@example.com"]);
        wsl::write_file(&format!("{repo}/a.txt"), b"one\n");
        wsl::write_file(&format!("{repo}/b.txt"), b"bee\n");
        wsl::must(&["git", "-C", &repo, "add", "."]);
        wsl::must(&["git", "-C", &repo, "commit", "-q", "-m", "base"]);

        // A stash with a tracked modification, a deletion, and an untracked
        // file.
        wsl::write_file(&format!("{repo}/a.txt"), b"one\ntwo\n");
        wsl::must(&["git", "-C", &repo, "rm", "-q", "b.txt"]);
        wsl::write_file(&format!("{repo}/u.txt"), b"new\n");
        wsl::must(&["git", "-C", &repo, "stash", "push", "-q", "-u", "-m", "mixed"]);
        let mixed = stash_changes(&repo, "stash@{0}").expect("mixed changes");
        let names: Vec<String> = mixed
            .files
            .iter()
            .map(|file| file.display_path())
            .collect();
        assert_eq!(names, ["a.txt", "b.txt", "u.txt"]);
        assert_eq!(mixed.files[0].status, 'M');
        assert_eq!(mixed.files[1].status, 'D');
        assert_eq!(mixed.files[2].status, 'A');
        assert_eq!(mixed.untracked, vec![b"u.txt".to_vec()]);

        // `refs/stash` must not leak into the history graph: neither the stash
        // commit nor its `index on …` parent may appear.
        let stash_oid = String::from_utf8_lossy(&wsl::must(&[
            "git", "-C", &repo, "rev-parse", "refs/stash",
        ]))
        .trim()
        .to_string();
        let tips = revision_tips(&repo);
        assert!(
            !tips.iter().any(|tip| tip == &stash_oid),
            "refs/stash leaked into the revision tips"
        );
        let page = log_history(&repo, &tips, &HashSet::new(), 0, 50).expect("history");
        assert!(
            !page.iter().any(|commit| commit.subject.starts_with("index on")),
            "stash commit in history: {:?}",
            page.iter().map(|commit| &commit.subject).collect::<Vec<_>>()
        );

        let tracked = stash_file_diff(&repo, "stash@{0}", b"a.txt", false).expect("tracked diff");
        assert!(
            tracked
                .lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Added && line.text == "two"),
            "{:?}",
            tracked.lines.iter().map(|line| &line.text).collect::<Vec<_>>()
        );
        let untracked = stash_file_diff(&repo, "stash@{0}", b"u.txt", true).expect("untracked diff");
        assert!(!untracked.binary);
        assert!(
            untracked
                .lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Added && line.text == "new")
        );

        // A tracked-only stash has no untracked commit.
        wsl::write_file(&format!("{repo}/a.txt"), b"one\nthree\n");
        wsl::must(&["git", "-C", &repo, "stash", "push", "-q", "-m", "tracked"]);
        let plain = stash_changes(&repo, "stash@{0}").expect("tracked changes");
        assert!(plain.untracked.is_empty(), "{:?}", plain.untracked);
        assert_eq!(plain.files.len(), 1);
        assert_eq!(plain.files[0].path, b"a.txt");

        // Apply restores the content and keeps the entry.
        stash_apply(&repo, "stash@{0}").expect("apply");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/a.txt")]), b"one\nthree\n");
        let listed = wsl::must(&["git", "-C", &repo, "stash", "list"]);
        let after_apply = String::from_utf8_lossy(&listed);
        assert_eq!(after_apply.lines().count(), 2, "{after_apply}");

        // Drop removes it; the other entry survives.
        stash_drop(&repo, "stash@{0}").expect("drop");
        let listed = wsl::must(&["git", "-C", &repo, "stash", "list"]);
        let after_drop = String::from_utf8_lossy(&listed);
        assert_eq!(after_drop.lines().count(), 1, "{after_drop}");
        assert!(after_drop.contains("mixed"), "{after_drop}");
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn stash_reads_by_object_id_survive_a_newer_stash() {
        wsl::watchdog(240);
        let dir = wsl::temp_dir("w13");
        let repo = format!("{dir}/repo");
        wsl::must(&["git", "init", "-q", "-b", "main", &repo]);
        wsl::must(&["git", "-C", &repo, "config", "user.name", "Ada"]);
        wsl::must(&["git", "-C", &repo, "config", "user.email", "ada@example.com"]);
        wsl::write_file(&format!("{repo}/a.txt"), b"one\n");
        wsl::write_file(&format!("{repo}/b.txt"), b"bee\n");
        wsl::must(&["git", "-C", &repo, "add", "."]);
        wsl::must(&["git", "-C", &repo, "commit", "-q", "-m", "base"]);

        // Stash A changes a.txt; capture its object id like the UI does.
        wsl::write_file(&format!("{repo}/a.txt"), b"one\ntwo\n");
        wsl::must(&["git", "-C", &repo, "stash", "push", "-q", "-m", "old"]);
        let old_object = String::from_utf8_lossy(&wsl::must(&[
            "git", "-C", &repo, "rev-parse", "stash@{0}",
        ]))
        .trim()
        .to_string();

        // An external tool adds a newer stash B (b.txt); `stash@{0}` now
        // names B while the captured object still names A.
        wsl::write_file(&format!("{repo}/b.txt"), b"bee\nbuzz\n");
        wsl::must(&["git", "-C", &repo, "stash", "push", "-q", "-m", "new"]);

        let by_label = stash_changes(&repo, "stash@{0}").expect("label read");
        assert!(
            by_label.files.iter().any(|file| file.path == b"b.txt"),
            "the label read must follow the moved label"
        );
        let by_object = stash_changes(&repo, &old_object).expect("object read");
        let names: Vec<String> = by_object
            .files
            .iter()
            .map(|file| file.display_path())
            .collect();
        assert_eq!(names, ["a.txt"], "the object read must stay on stash A");

        // The preview read follows the same identity.
        let pinned = stash_file_diff(&repo, &old_object, b"a.txt", false).expect("pinned diff");
        assert!(
            pinned
                .lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Added && line.text == "two"),
            "{:?}",
            pinned.lines.iter().map(|line| &line.text).collect::<Vec<_>>()
        );
        let moved = stash_file_diff(&repo, "stash@{0}", b"a.txt", false).expect("label diff");
        assert!(
            !moved
                .lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Added && line.text == "two"),
            "the moved label must not serve the old object's content"
        );
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn conflict_detection_compare_and_resolution() {
        wsl::watchdog(300);
        let dir = wsl::temp_dir("w14");
        let repo = format!("{dir}/repo");
        wsl::must(&["git", "init", "-q", "-b", "main", &repo]);
        wsl::must(&["git", "-C", &repo, "config", "user.name", "Ada"]);
        wsl::must(&["git", "-C", &repo, "config", "user.email", "ada@example.com"]);
        wsl::write_file(&format!("{repo}/conflict.txt"), b"base\n");
        wsl::must(&["git", "-C", &repo, "add", "."]);
        wsl::must(&["git", "-C", &repo, "commit", "-q", "-m", "base"]);
        wsl::must(&["git", "-C", &repo, "checkout", "-q", "-b", "side"]);
        wsl::write_file(&format!("{repo}/conflict.txt"), b"theirs\n");
        wsl::must(&["git", "-C", &repo, "commit", "-qam", "side change"]);
        wsl::must(&["git", "-C", &repo, "checkout", "-q", "main"]);
        wsl::write_file(&format!("{repo}/conflict.txt"), b"ours\n");
        wsl::must(&["git", "-C", &repo, "commit", "-qam", "main change"]);
        let merge = wsl::wsl(&["git", "-C", &repo, "merge", "side"]);
        assert!(!merge.status.success(), "the merge must conflict");

        // Detection: status marks the path unmerged, and the resolver parses
        // one conflict with both sides.
        let collected = collect_status(&repo).expect("status");
        let conflicted = crate::status::change_lists(&collected.snapshot)
            .unstaged
            .iter()
            .find(|item| item.path == b"conflict.txt")
            .map(|item| item.kind);
        assert_eq!(conflicted, Some(crate::status::Change::Unmerged));

        let sections = conflict_sections(&repo, b"conflict.txt").expect("sections");
        assert_eq!(conflict_count(&sections), 1);
        match &sections.iter().find(|s| s.is_conflict()).expect("conflict") {
            ConflictSection::Conflict {
                ours_label,
                theirs_label,
                ours,
                theirs,
            } => {
                assert_eq!(ours_label, "HEAD");
                assert_eq!(theirs_label, "side");
                assert_eq!(ours, b"ours\n");
                assert_eq!(theirs, b"theirs\n");
            }
            other => panic!("expected a conflict, got {other:?}"),
        }

        // A wrong choice count is refused and leaves the file untouched.
        assert!(resolve_conflict_file(&repo, b"conflict.txt", &[]).is_err());
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/conflict.txt")]),
            b"<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> side\n"
        );

        // Choosing Theirs writes their side and stages the path: resolved.
        resolve_conflict_file(&repo, b"conflict.txt", &[ConflictChoice::Theirs]).expect("resolve");
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/conflict.txt")]),
            b"theirs\n"
        );
        let porcelain = String::from_utf8_lossy(&wsl::must(&[
            "git", "-C", &repo, "status", "--porcelain",
        ]))
        .to_string();
        assert!(porcelain.contains("M  conflict.txt"), "{porcelain}");
        assert!(!porcelain.contains('U'), "{porcelain}");

        // Abort and recreate the conflict to prove Ours and Both.
        wsl::must(&["git", "-C", &repo, "merge", "--abort"]);
        assert!(!wsl::wsl(&["git", "-C", &repo, "merge", "side"]).status.success());
        resolve_conflict_file(&repo, b"conflict.txt", &[ConflictChoice::Both]).expect("resolve both");
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/conflict.txt")]),
            b"ours\ntheirs\n"
        );

        wsl::must(&["git", "-C", &repo, "merge", "--abort"]);
        assert!(!wsl::wsl(&["git", "-C", &repo, "merge", "side"]).status.success());
        resolve_conflict_file(&repo, b"conflict.txt", &[ConflictChoice::Ours]).expect("resolve ours");
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/conflict.txt")]),
            b"ours\n"
        );
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn pull_selects_remote_branch_mode_and_local_changes_policy() {
        wsl::watchdog(300);
        let dir = wsl::temp_dir("w12");
        let origin = format!("{dir}/origin.git");
        let repo = format!("{dir}/repo");
        let other = format!("{dir}/other");
        wsl::must(&["git", "init", "-q", "--bare", "-b", "main", &origin]);
        wsl::must(&["git", "clone", "-q", &origin, &repo]);
        wsl::must(&["git", "-C", &repo, "config", "user.name", "Ada"]);
        wsl::must(&["git", "-C", &repo, "config", "user.email", "ada@example.com"]);
        wsl::write_file(&format!("{repo}/a.txt"), b"one\n");
        wsl::must(&["git", "-C", &repo, "add", "."]);
        wsl::must(&["git", "-C", &repo, "commit", "-q", "-m", "init"]);
        push_branch(&repo, "origin", "main", "main", true).expect("seed push");

        // The publisher moves origin/main forward; the remote ref is only
        // fetched by the pull itself.
        wsl::must(&["git", "clone", "-q", &origin, &other]);
        wsl::must(&["git", "-C", &other, "config", "user.name", "Bob"]);
        wsl::must(&["git", "-C", &other, "config", "user.email", "bob@example.com"]);
        let publish = |name: &str, bytes: &[u8]| -> String {
            wsl::write_file(&format!("{other}/{name}"), bytes);
            wsl::must(&["git", "-C", &other, "add", "."]);
            wsl::must(&["git", "-C", &other, "commit", "-q", "-m", name]);
            wsl::must(&["git", "-C", &other, "push", "-q", "origin", "main"]);
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &other, "rev-parse", "HEAD"]))
                .trim()
                .to_string()
        };
        let theirs = publish("b.txt", b"theirs\n");

        // A plain pull (merge mode) fast-forwards to the published commit.
        pull_remote(&repo, "origin", "main", "main", false, PullChanges::Nothing).expect("pull");
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "rev-parse", "HEAD"])).trim(),
            theirs
        );

        // "Do nothing" with a dirty file the update must overwrite: Git
        // refuses, HEAD and the edit stay.
        wsl::write_file(&format!("{repo}/a.txt"), b"local edit\n");
        publish("a.txt", b"theirs edit\n");
        assert!(
            pull_remote(&repo, "origin", "main", "main", false, PullChanges::Nothing).is_err(),
            "a conflicting local change must refuse the pull"
        );
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "rev-parse", "HEAD"])).trim(),
            theirs,
            "a refused pull must not move HEAD"
        );
        assert_eq!(wsl::must(&["cat", &format!("{repo}/a.txt")]), b"local edit\n");

        // "Stash & reapply": a non-conflicting edit comes back after the pull.
        wsl::write_file(&format!("{repo}/a.txt"), b"one\n");
        wsl::write_file(&format!("{repo}/local.txt"), b"local\n");
        let theirs3 = publish("c.txt", b"more\n");
        pull_remote(
            &repo,
            "origin",
            "main",
            "main",
            false,
            PullChanges::StashReapply,
        )
        .expect("stash pull");
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "rev-parse", "HEAD"])).trim(),
            theirs3
        );
        assert_eq!(wsl::must(&["cat", &format!("{repo}/local.txt")]), b"local\n");
        assert!(
            wsl::must(&["git", "-C", &repo, "stash", "list"]).is_empty(),
            "the reapply must empty the stash"
        );

        // "Discard": the change is moved into a stash and not reapplied.
        wsl::write_file(&format!("{repo}/a.txt"), b"local edit two\n");
        let theirs4 = publish("d.txt", b"again\n");
        pull_remote(
            &repo,
            "origin",
            "main",
            "main",
            false,
            PullChanges::Discard,
        )
        .expect("discard pull");
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "rev-parse", "HEAD"])).trim(),
            theirs4
        );
        assert!(
            wsl::must(&["git", "-C", &repo, "status", "--porcelain"]).is_empty(),
            "the worktree must be clean after a discard pull"
        );
        assert_eq!(
            String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "stash", "list"]))
                .lines()
                .count(),
            1,
            "the discarded changes must be recoverable in the stash list"
        );

        // Diverged history: merge (default) keeps both sides in a merge
        // commit, rebase replays the local commit on top of the remote one.
        wsl::write_file(&format!("{repo}/local.txt"), b"local\n");
        wsl::must(&["git", "-C", &repo, "add", "."]);
        wsl::must(&["git", "-C", &repo, "commit", "-q", "-m", "local work"]);
        let local = String::from_utf8_lossy(&wsl::must(&["git", "-C", &repo, "rev-parse", "HEAD"]))
            .trim()
            .to_string();
        let theirs5 = publish("e.txt", b"remote work\n");

        pull_remote(&repo, "origin", "main", "main", false, PullChanges::Nothing)
            .expect("merge pull");
        let merged_parents = String::from_utf8_lossy(&wsl::must(&[
            "git", "-C", &repo, "rev-list", "--parents", "-n1", "HEAD",
        ]))
        .trim()
        .to_string();
        assert_eq!(
            merged_parents.split_whitespace().count(),
            3,
            "the default pull must create a merge commit: {merged_parents}"
        );
        assert!(merged_parents.contains(&local) && merged_parents.contains(&theirs5));

        // Rewind to the divergent local commit and rebase instead.
        wsl::must(&["git", "-C", &repo, "reset", "--hard", "-q", &local]);
        pull_remote(&repo, "origin", "main", "main", true, PullChanges::Nothing)
            .expect("rebase pull");
        let rebased = String::from_utf8_lossy(&wsl::must(&[
            "git", "-C", &repo, "rev-list", "--parents", "-n1", "HEAD",
        ]))
        .trim()
        .to_string();
        assert_eq!(
            rebased.split_whitespace().count(),
            2,
            "a rebase pull must keep the history linear: {rebased}"
        );
        assert!(
            rebased.split_whitespace().nth(1) == Some(theirs5.as_str()),
            "the local commit must sit on top of the remote commit: {rebased}"
        );

        // The checked-out branch is part of the operation identity.
        assert!(
            pull_remote(&repo, "origin", "main", "other", false, PullChanges::Nothing).is_err(),
            "a changed checked-out branch must refuse the pull"
        );
        // An unknown remote is an error, not a silent no-op.
        assert!(pull_remote(&repo, "nope", "main", "main", false, PullChanges::Nothing).is_err());
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn undo_backup_refs_restore_branches_tags_head_and_content() {
        let dir = wsl::temp_dir("w24");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let repo = format!("{dir}/repo");
        init_repo(&home, &repo);
        let main_tip = rev(&repo, "HEAD");

        // Branch delete round-trips through the backup ref.
        wsl::must_env(&home, &["git", "-C", &repo, "branch", "doomed"]);
        let tip = rev_parse(&repo, "doomed").expect("tip resolves");
        assert_eq!(tip, main_tip);
        let backup = write_backup_ref(&repo, &tip, "branch doomed").expect("backup");
        assert!(backup.starts_with(UNDO_REF_PREFIX));
        delete_branch(&repo, "doomed", false).expect("delete");
        assert!(
            rev_parse(&repo, "doomed").is_err(),
            "the branch must be gone"
        );
        recreate_branch_at(&repo, "doomed", &tip).expect("recreate");
        assert_eq!(rev(&repo, "doomed"), tip);
        delete_backup_ref(&repo, &backup).expect("cleanup");
        assert!(
            rev_parse(&repo, &backup).is_err(),
            "the backup ref must be gone"
        );

        // Tag delete restores the exact object (annotated or lightweight).
        create_tag(&repo, "v-doomed", &main_tip, "release").expect("tag");
        let tag_obj = rev_parse(&repo, "v-doomed").expect("tag object");
        let tag_backup = write_backup_ref(&repo, &tag_obj, "tag v-doomed").expect("backup");
        delete_tag(&repo, "v-doomed", true, &[]).expect("delete tag");
        assert!(rev_parse(&repo, "v-doomed").is_err());
        restore_tag_to(&repo, "v-doomed", &tag_obj).expect("restore tag");
        assert_eq!(rev_parse(&repo, "v-doomed").expect("tag back"), tag_obj);
        delete_backup_ref(&repo, &tag_backup).expect("cleanup");

        // `reset --hard` rewinds HEAD; the backup keeps the commit alive.
        wsl::write_file(&format!("{repo}/file.txt"), b"two\n");
        wsl::must_env(&home, &["git", "-C", &repo, "add", "-A"]);
        wsl::must_env(&home, &["git", "-C", &repo, "commit", "-q", "-m", "second"]);
        let second = rev(&repo, "HEAD");
        let head_backup =
            write_backup_ref(&repo, &second, "HEAD before reset").expect("backup");
        reset_hard_to(&repo, &main_tip).expect("rewind");
        assert_eq!(rev(&repo, "HEAD"), main_tip);
        reset_hard_to(&repo, &second).expect("restore");
        assert_eq!(rev(&repo, "HEAD"), second);
        assert_eq!(
            prune_undo_refs(&repo),
            0,
            "fresh backups must survive pruning"
        );
        delete_backup_ref(&repo, &head_backup).expect("cleanup");

        // Tracked discard round-trips through `stash create` + store + apply.
        wsl::write_file(&format!("{repo}/file.txt"), b"edited\n");
        let stash_id = stash_create_id(&repo)
            .expect("create")
            .expect("must back up edits");
        restore_paths(&repo, &[b"file.txt".to_vec()], false).expect("discard");
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/file.txt")]),
            b"two\n"
        );
        stash_store_id(&repo, &stash_id, "Spur undo test").expect("store");
        stash_apply(&repo, "stash@{0}").expect("apply the stored entry");
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/file.txt")]),
            b"edited\n"
        );
        let top = rev_parse(&repo, "stash@{0}").expect("top entry");
        assert_eq!(top, stash_id);
        stash_drop(&repo, "stash@{0}").expect("drop the temp entry");
        // A clean tree creates nothing.
        restore_paths(&repo, &[b"file.txt".to_vec()], false).expect("clean up");
        assert_eq!(stash_create_id(&repo).expect("clean"), None);

        // Untracked bytes round-trip through blobs.
        wsl::write_file(&format!("{repo}/lost.txt"), b"precious\n");
        let saved = read_untracked_for_undo(&repo, b"lost.txt")
            .expect("read")
            .expect("present");
        assert_eq!(saved, b"precious\n");
        let blob = hash_blob(&repo, &saved).expect("store blob");
        let blob_backup =
            write_backup_ref(&repo, &blob, "untracked lost.txt").expect("backup");
        wsl::must(&["rm", &format!("{repo}/lost.txt")]);
        assert_eq!(
            read_untracked_for_undo(&repo, b"lost.txt").expect("gone"),
            None
        );
        let back = cat_blob(&repo, &blob).expect("read blob");
        restore_untracked_file(&repo, b"lost.txt", &back).expect("rewrite");
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/lost.txt")]),
            b"precious\n"
        );
        assert!(
            restore_untracked_file(&repo, b"lost.txt", &back).is_err(),
            "an existing file must never be overwritten by undo"
        );
        delete_backup_ref(&repo, &blob_backup).expect("cleanup");

        // Guards refuse before git runs.
        assert!(write_backup_ref(&repo, "not-an-oid", "x").is_err());
        assert!(recreate_branch_at(&repo, "doomed", "not-an-oid").is_err());
        assert!(rev_parse(&repo, "refs/spur/undo/nope").is_err());
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn reset_moves_the_branch_per_mode_with_identity_recheck() {
        let dir = wsl::temp_dir("w25");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let repo = format!("{dir}/repo");
        init_repo(&home, &repo);
        let first = rev(&repo, "HEAD");
        wsl::write_file(&format!("{repo}/file.txt"), b"two\n");
        wsl::must_env(&home, &["git", "-C", &repo, "add", "-A"]);
        wsl::must_env(&home, &["git", "-C", &repo, "commit", "-q", "-m", "second"]);
        let second = rev(&repo, "HEAD");

        // Soft keeps the index: HEAD moves, the diff stays staged.
        reset_branch(&repo, &first, ResetMode::Soft, Some("main"), &second).expect("soft");
        assert_eq!(rev(&repo, "HEAD"), first);
        let staged_bytes = wsl::must(&["git", "-C", &repo, "diff", "--cached", "--name-only"]);
        let staged = String::from_utf8_lossy(&staged_bytes);
        assert!(
            staged.contains("file.txt"),
            "soft must keep the changes staged: {staged}"
        );

        // Mixed keeps the worktree unstaged.
        reset_branch(&repo, &second, ResetMode::Mixed, Some("main"), &first).expect("there");
        reset_branch(&repo, &first, ResetMode::Mixed, Some("main"), &second).expect("mixed");
        assert_eq!(rev(&repo, "HEAD"), first);
        assert!(
            wsl::must(&["git", "-C", &repo, "diff", "--cached", "--name-only"]).is_empty(),
            "mixed must not stage"
        );
        let unstaged_bytes = wsl::must(&["git", "-C", &repo, "diff", "--name-only"]);
        let unstaged = String::from_utf8_lossy(&unstaged_bytes);
        assert!(
            unstaged.contains("file.txt"),
            "mixed must keep the worktree changes: {unstaged}"
        );

        // Hard discards everything tracked.
        reset_branch(&repo, &first, ResetMode::Hard, Some("main"), &first).expect("hard");
        assert_eq!(rev(&repo, "HEAD"), first);
        assert!(
            wsl::must(&["git", "-C", &repo, "status", "--porcelain"]).is_empty(),
            "hard must leave a clean tree"
        );
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/file.txt")]),
            b"one\n"
        );

        // The identity recheck refuses a moved branch or HEAD.
        wsl::write_file(&format!("{repo}/file.txt"), b"two\n");
        wsl::must_env(&home, &["git", "-C", &repo, "add", "-A"]);
        wsl::must_env(&home, &["git", "-C", &repo, "commit", "-q", "-m", "second again"]);
        let moved = rev(&repo, "HEAD");
        assert_ne!(moved, second);
        assert!(
            reset_branch(&repo, &first, ResetMode::Mixed, Some("main"), &second).is_err(),
            "a moved HEAD must refuse the reset"
        );
        assert!(
            reset_branch(&repo, &first, ResetMode::Mixed, Some("other"), &moved).is_err(),
            "a changed branch must refuse the reset"
        );
        assert_eq!(rev(&repo, "HEAD"), moved, "refusals must not move HEAD");
        // Invalid inputs never reach git.
        assert!(reset_branch(&repo, "zzz", ResetMode::Mixed, Some("main"), &moved).is_err());
        assert!(reset_branch(&repo, &first, ResetMode::Mixed, Some("-evil"), &moved).is_err());
        assert!(reset_branch(&repo, &first, ResetMode::Mixed, Some("main"), "zzz").is_err());

        // Pushed-ahead counts: nothing pushed past the target reads zero.
        let bare = format!("{dir}/remote.git");
        wsl::must(&["git", "init", "-q", "--bare", &bare]);
        wsl::must_env(&home, &["git", "-C", &repo, "remote", "add", "origin", &bare]);
        wsl::must_env(&home, &["git", "-C", &repo, "push", "-q", "origin", "main"]);
        assert_eq!(
            pushed_ahead_count(&repo, &first, "origin/main").expect("count"),
            1,
            "origin/main holds one commit past the first"
        );
        assert_eq!(
            pushed_ahead_count(&repo, &moved, "origin/main").expect("count"),
            0,
            "nothing sits past the pushed tip"
        );
        assert!(pushed_ahead_count(&repo, &moved, "origin/missing").is_err());
        assert!(pushed_ahead_count(&repo, &moved, "-evil").is_err());
        assert!(pushed_ahead_count(&repo, "zzz", "origin/main").is_err());
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn look_and_happy_signals_after_fetch_and_pull() {
        // The branchling's signals: Look keys on the
        // behind count growing after a fetch, Happy on a pull leaving the
        // repository clean and 0 behind.
        wsl::watchdog(240);
        let dir = wsl::temp_dir("w26");
        let origin = format!("{dir}/origin.git");
        let repo = format!("{dir}/repo");
        let other = format!("{dir}/other");
        wsl::must(&["git", "init", "-q", "--bare", "-b", "main", &origin]);
        wsl::must(&["git", "clone", "-q", &origin, &repo]);
        wsl::must(&["git", "-C", &repo, "config", "user.name", "Ada"]);
        wsl::must(&["git", "-C", &repo, "config", "user.email", "ada@example.com"]);
        wsl::write_file(&format!("{repo}/a.txt"), b"one\n");
        wsl::must(&["git", "-C", &repo, "add", "."]);
        wsl::must(&["git", "-C", &repo, "commit", "-q", "-m", "init"]);
        push_branch(&repo, "origin", "main", "main", true).expect("seed push");
        let seeded = collect_status(&repo).expect("status");
        assert_eq!(seeded.snapshot.behind, 0);
        assert!(seeded.snapshot.is_clean());

        // A publisher moves origin/main on; only a fetch can raise `behind`.
        wsl::must(&["git", "clone", "-q", &origin, &other]);
        wsl::must(&["git", "-C", &other, "config", "user.name", "Bob"]);
        wsl::must(&["git", "-C", &other, "config", "user.email", "bob@example.com"]);
        wsl::write_file(&format!("{other}/b.txt"), b"theirs\n");
        wsl::must(&["git", "-C", &other, "add", "."]);
        wsl::must(&["git", "-C", &other, "commit", "-q", "-m", "theirs"]);
        wsl::must(&["git", "-C", &other, "push", "-q", "origin", "main"]);
        assert_eq!(
            collect_status(&repo).expect("status").snapshot.behind,
            0,
            "behind counts local refs: nothing moved before the fetch"
        );

        fetch_remote(&repo, Some("origin"), false, false).expect("fetch");
        assert_eq!(
            collect_status(&repo).expect("status").snapshot.behind,
            1,
            "Look: the fetch raised behind"
        );

        pull_remote(&repo, "origin", "main", "main", false, PullChanges::Nothing).expect("pull");
        let settled = collect_status(&repo).expect("status");
        assert_eq!(settled.snapshot.behind, 0, "Happy: not behind");
        assert!(settled.snapshot.is_clean(), "Happy: clean");
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    #[cfg(windows)]
    fn wsl_created_worktree_on_a_windows_drive_falls_back_to_wsl_git() {
        wsl::watchdog(120);
        let base = std::env::temp_dir().join(format!("spur-wt-fallback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let dir = crate::model::to_linux_path(&base.to_string_lossy()).unwrap();
        let main = format!("{dir}/main");
        let side = format!("{dir}/side");
        wsl::must(&["git", "init", "-q", "-b", "main", &main]);
        wsl::must(&["git", "-C", &main, "-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "-q", "--allow-empty", "-m", "init"]);
        wsl::must(&["git", "-C", &main, "worktree", "add", "-q", "-b", "side", &side]);

        assert!(has_linux_gitdir(&side));
        let out = run_for(&side, &["-C", &side, "rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert!(out.success(), "{}", out.error_context());
        assert_eq!(out.stdout_text().trim(), "side");
        let _ = std::fs::remove_dir_all(&base);
    }
}

#[cfg(test)]
mod conflict_tests {
    use super::*;

    #[test]
    fn markers_parse_into_common_and_conflict_sections() {
        let raw = b"one\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\ntwo\n";
        let sections = parse_conflict_markers(raw).expect("parse");
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0], ConflictSection::Common(b"one\n".to_vec()));
        assert_eq!(sections[2], ConflictSection::Common(b"two\n".to_vec()));
        match &sections[1] {
            ConflictSection::Conflict {
                ours_label,
                theirs_label,
                ours,
                theirs,
            } => {
                assert_eq!(ours_label, "HEAD");
                assert_eq!(theirs_label, "feature");
                assert_eq!(ours, b"ours\n");
                assert_eq!(theirs, b"theirs\n");
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
        assert_eq!(conflict_count(&sections), 1);
    }

    #[test]
    fn diff3_bases_are_skipped_and_choices_rebuild_byte_exact_content() {
        let raw = b"<<<<<<< HEAD\na\n||||||| merged common ancestors\nbase\n=======\nb\n>>>>>>> side\nmid\n<<<<<<< HEAD\nc\n=======\nd\n>>>>>>> side\ntail";
        let sections = parse_conflict_markers(raw).expect("parse");
        assert_eq!(conflict_count(&sections), 2);
        let resolved =
            resolve_conflict_sections(&sections, &[ConflictChoice::Ours, ConflictChoice::Both])
                .expect("resolve");
        assert_eq!(resolved, b"a\nmid\nc\nd\ntail");
        // One side chosen: only that side's exact bytes survive.
        let resolved =
            resolve_conflict_sections(&sections, &[ConflictChoice::Theirs, ConflictChoice::Theirs])
                .expect("resolve");
        assert_eq!(resolved, b"b\nmid\nd\ntail");
    }

    #[test]
    fn plain_separator_lookalikes_are_content_and_bad_markers_error() {
        // A separator line outside a conflict is ordinary content.
        let sections = parse_conflict_markers(b"=======\nplain\n").expect("parse");
        assert_eq!(conflict_count(&sections), 0);
        assert_eq!(
            sections,
            vec![ConflictSection::Common(b"=======\nplain\n".to_vec())]
        );
        // Unterminated, nested, and misplaced markers are explicit errors.
        assert!(parse_conflict_markers(b"<<<<<<< HEAD\nours\n")
            .unwrap_err()
            .contains("unterminated"));
        assert!(parse_conflict_markers(b"<<<<<<< HEAD\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> z\n")
            .is_err());
        assert!(parse_conflict_markers(b"<<<<<<< HEAD\nx\n=======\ny\n=======\nz\n>>>>>>> s\n")
            .is_err());
        // The resolver refuses a choice count that does not match.
        let sections = parse_conflict_markers(b"<<<<<<< a\nx\n=======\ny\n>>>>>>> b\n").unwrap();
        assert!(resolve_conflict_sections(&sections, &[]).is_err());
    }
}

