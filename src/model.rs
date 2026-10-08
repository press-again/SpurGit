//! Domain model for the workspace overview: workspaces, repository snapshots,
//! gis-style status flags, and pull eligibility.
//!
//! Types here are GPUI-free on purpose.
#![allow(dead_code)]

use std::path::PathBuf;

use crate::graph::GraphRow;

/// Gis-compatible status symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    Clean,            // ✓
    Stash,            // $
    Untracked,        // ?
    Modified,         // !
    AddedStaged,      // +
    Deleted,          // -
    Renamed,          // »
    Conflicted,       // =
    Diverged,         // ⇕
    Ahead,            // ⇡
    Behind,           // ⇣
    UpstreamMissing,  // ✗
}

impl Flag {
    pub fn symbol(self) -> &'static str {
        match self {
            Flag::Clean => "✓",
            Flag::Stash => "$",
            Flag::Untracked => "?",
            Flag::Modified => "!",
            Flag::AddedStaged => "+",
            Flag::Deleted => "-",
            Flag::Renamed => "»",
            Flag::Conflicted => "=",
            Flag::Diverged => "⇕",
            Flag::Ahead => "⇡",
            Flag::Behind => "⇣",
            Flag::UpstreamMissing => "✗",
        }
    }

    /// Symbol with its count for the sync flags (`⇡3`, `⇣2`, `⇕3/4`); other
    /// flags are their bare symbol.
    pub fn symbol_with_counts(self, ahead: u32, behind: u32) -> String {
        match self {
            Flag::Ahead => format!("⇡{ahead}"),
            Flag::Behind => format!("⇣{behind}"),
            Flag::Diverged => format!("⇕{ahead}/{behind}"),
            _ => self.symbol().to_string(),
        }
    }

    /// Human-readable name for inspector chips and tooltips.
    pub fn label(self) -> &'static str {
        match self {
            Flag::Clean => "clean",
            Flag::Stash => "stash",
            Flag::Untracked => "untracked",
            Flag::Modified => "modified",
            Flag::AddedStaged => "added (staged)",
            Flag::Deleted => "deleted",
            Flag::Renamed => "renamed",
            Flag::Conflicted => "conflicted",
            Flag::Diverged => "diverged",
            Flag::Ahead => "ahead",
            Flag::Behind => "behind",
            Flag::UpstreamMissing => "upstream missing",
        }
    }

    /// What the flag means, for tooltips.
    pub fn description(self) -> &'static str {
        match self {
            Flag::Clean => "no local changes",
            Flag::Stash => "there are stashed changes in this repository",
            Flag::Untracked => "new files Git doesn't track yet",
            Flag::Modified => "tracked files have changes (staged or not)",
            Flag::AddedStaged => "new files are staged for the next commit",
            Flag::Deleted => "tracked files were deleted",
            Flag::Renamed => "staged files were renamed",
            Flag::Conflicted => "files have merge conflicts to resolve",
            Flag::Diverged => "local and remote both have new commits; pull before pushing",
            Flag::Ahead => "local commits not pushed yet",
            Flag::Behind => "remote commits not pulled yet",
            Flag::UpstreamMissing => "the tracked remote branch no longer exists",
        }
    }

    /// Color role for rendering: (semantic role, symbol). Color reinforces,
    /// never replaces, the symbol itself.
    pub fn color_role(self) -> FlagColor {
        match self {
            Flag::Clean => FlagColor::Success,
            Flag::Stash
            | Flag::Untracked
            | Flag::Modified
            | Flag::AddedStaged
            | Flag::Deleted
            | Flag::Renamed => FlagColor::Neutral,
            Flag::Conflicted | Flag::UpstreamMissing => FlagColor::Danger,
            Flag::Diverged => FlagColor::Warning,
            Flag::Ahead | Flag::Behind => FlagColor::Accent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagColor {
    Neutral,
    Success,
    Warning,
    Danger,
    Accent,
}

/// Which lifecycle stage a repository row is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    Ready,
    /// Status collection currently running (spinner row).
    Loading,
    /// Last status query failed; previous snapshot kept and marked stale.
    Stale { error: String },
}

/// One repository row (a worktree identity).
#[derive(Debug, Clone)]
pub struct RepoEntry {
    pub name: String,
    /// Workspace-relative path, kept as bytes for later Git use; display
    /// sanitization happens at render time.
    pub path: PathBuf,
    pub branch: Option<String>, // None = detached HEAD
    pub unborn: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub extra_branches: u32,
    pub flags: Vec<Flag>,
    pub last_fetch: Option<String>, // humanized, app-known only
    /// When the last successful status collection finished.
    pub last_collected: Option<std::time::Instant>,
    pub state: RowState,
}

impl RepoEntry {
    /// The live error behind a stale row, if any.
    pub fn stale_error(&self) -> Option<&str> {
        match &self.state {
            RowState::Stale { error } => Some(error),
            _ => None,
        }
    }

    pub fn sync_label(&self) -> String {
        match (self.ahead, self.behind) {
            (0, 0) => "–".into(),
            (a, 0) => format!("⇡{a}"),
            (0, b) => format!("⇣{b}"),
            (a, b) => format!("⇕{a}/{b}"),
        }
    }

    pub fn matches_filter(&self, filter: Filter) -> bool {
        match filter {
            Filter::All => true,
            Filter::Changed => self
                .flags
                .iter()
                .any(|f| !matches!(f, Flag::Clean | Flag::Ahead | Flag::Behind | Flag::Diverged)),
            Filter::Behind => self.behind > 0,
            Filter::Conflicts => self.flags.contains(&Flag::Conflicted),
            Filter::Errors => matches!(self.state, RowState::Stale { .. }),
        }
    }

    /// Local (uncommitted) worktree or index changes worth a tab highlight.
    /// Stash entries, sync state (ahead/behind/diverged), and upstream
    /// problems are not local changes — a committed and pushed repository
    /// must not keep its dot because an old stash exists.
    pub fn has_local_changes(&self) -> bool {
        self.flags.iter().any(|f| {
            matches!(
                f,
                Flag::Untracked
                    | Flag::Modified
                    | Flag::AddedStaged
                    | Flag::Deleted
                    | Flag::Renamed
                    | Flag::Conflicted
            )
        })
    }

    pub fn matches_search(&self, needle: &str) -> bool {
        let n = needle.to_lowercase();
        n.is_empty()
            || self.name.to_lowercase().contains(&n)
            || self.path.to_string_lossy().to_lowercase().contains(&n)
            || self.branch.as_deref().unwrap_or("").to_lowercase().contains(&n)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Changed,
    Behind,
    Conflicts,
    Errors,
}

impl Filter {
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Changed => "Changed",
            Filter::Behind => "Behind",
            Filter::Conflicts => "Conflicts",
            Filter::Errors => "Errors",
        }
    }

    pub const ALL: [Filter; 5] = [
        Filter::All,
        Filter::Changed,
        Filter::Behind,
        Filter::Conflicts,
        Filter::Errors,
    ];
}

/// Where a scan root lives. Discovery and Git execution pick the matching
/// environment: WSL roots run Linux Git via `wsl.exe -e`, Windows roots run
/// the native Git executable with Windows paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootKind {
    Wsl,
    Windows,
}

/// Classify a root path by shape: drive-lettered (`C:\…`, `C:/…`) and UNC
/// (`\\server\share`, `//server/share`) paths are Windows; everything else
/// (absolute, `~/…`) is treated as a Linux path inside WSL.
pub fn root_kind(path: &str) -> RootKind {
    let p = path.trim();
    let bytes = p.as_bytes();
    let drive_lettered =
        bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if p.starts_with("\\\\") || p.starts_with("//") || drive_lettered {
        RootKind::Windows
    } else {
        RootKind::Wsl
    }
}

/// Normalize a path picked in the Windows file dialog (or dropped from
/// Explorer, typed, or passed at launch) into a root string. Git only runs
/// inside `crate::git::distro()`, so:
/// - `\\wsl.localhost\<distro>\home\…` (or the legacy `\\wsl$\…`) becomes the
///   Linux path `/home/…`; a different distribution is rejected with its name;
/// - a Windows drive path becomes its WSL mount (`C:\dev` -> `/mnt/c/dev`),
///   so Windows folders picked or typed in the app stay WSL-backed;
/// - anything else passes through and is validated by discovery.
pub fn to_root_string(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    for prefix in ["\\\\wsl.localhost\\", "\\\\wsl$\\"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            if let Some((distro, path)) = rest.split_once('\\') {
                if !distro.eq_ignore_ascii_case(crate::git::distro()) {
                    return Err(distro.to_string());
                }
                return Ok(format!("/{}", path.replace('\\', "/")));
            }
        }
    }
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = s[2..].trim_start_matches(['\\', '/']).replace('\\', "/");
        return Ok(if rest.is_empty() {
            format!("/mnt/{drive}")
        } else {
            format!("/mnt/{drive}/{rest}")
        });
    }
    Ok(s.to_string())
}

/// True when a worktree path is native to the WSL distro (`/home/…`, `/srv/…`).
/// Windows drives reached through their `/mnt/<drive>` mount are the
/// non-WSL repositories: they run through native Windows Git
/// ([`to_windows_path`]) and display their Windows path in the UI.
pub fn is_wsl_worktree(path: &str) -> bool {
    cfg!(windows) && to_windows_path(path).is_none()
}

/// Map a `/mnt/<drive>/…` Linux path back to its Windows path, so
/// Windows-mounted repositories can run through native Git (WSL Git over 9p
/// takes tens of seconds for a large status; native Git takes well under a
/// second).
pub fn to_windows_path(path: &str) -> Option<String> {
    let rest = path.trim().strip_prefix("/mnt/")?;
    let (drive, tail) = match rest.split_once('/') {
        Some((drive, tail)) => (drive, tail),
        None => (rest, ""),
    };
    if drive.len() != 1 || !drive.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    let drive = drive.to_ascii_uppercase();
    Some(if tail.is_empty() {
        format!("{drive}:\\")
    } else {
        format!("{drive}:\\{}", tail.replace('/', "\\"))
    })
}

/// Path to hand to `std::fs` for a worktree path: the Windows spelling of a
/// `/mnt/<drive>` mount on Windows, the path itself everywhere else.
pub fn host_path(path: &str) -> Option<String> {
    if cfg!(windows) {
        to_windows_path(path)
    } else {
        Some(path.to_string())
    }
}

/// Reverse of [`to_windows_path`]: a Windows drive path as its WSL mount.
pub fn to_linux_path(path: &str) -> Option<String> {
    let trimmed = path.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return None;
    }
    let drive = (bytes[0] as char).to_ascii_lowercase();
    let rest = trimmed[2..].trim_start_matches(['\\', '/']).replace('\\', "/");
    Some(if rest.is_empty() {
        format!("/mnt/{drive}")
    } else {
        format!("/mnt/{drive}/{rest}")
    })
}

/// Operation log. Its newest line surfaces as a transient bottom-left alert.
/// Informational lines stay plain text; ran operations carry their repo,
/// result, and the command journal they spawned.
#[derive(Debug, Clone, Default)]
pub struct OpSummary {
    pub log: Vec<OpEntry>,
    next_id: u64,
}

/// One log row: newest first in the panel.
#[derive(Debug, Clone)]
pub struct OpEntry {
    pub id: u64,
    pub at: std::time::Instant,
    /// Repository display name for ran operations; `None` for ambient lines.
    pub repo: Option<String>,
    /// Short verb ("fetch", "commit", "copy path", …).
    pub op: String,
    pub result: OpResult,
    /// The human-readable line (also the alert text on failure).
    pub detail: String,
    /// Command records the operation spawned (usually 0–3).
    pub commands: Vec<crate::process::CommandRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpResult {
    Success,
    Failed,
    Cancelled,
    /// Ambient lines (copies, discovery notes, settings saves).
    Info,
}

impl OpSummary {
    /// Retained rows; the panel renders newest first from this order.
    pub const CAP: usize = 200;

    pub fn push_info(&mut self, detail: String) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.log.push(OpEntry {
            id,
            at: std::time::Instant::now(),
            repo: None,
            op: String::new(),
            result: OpResult::Info,
            detail,
            commands: Vec::new(),
        });
        if self.log.len() > Self::CAP {
            let excess = self.log.len() - Self::CAP;
            self.log.drain(..excess);
        }
        id
    }

    pub fn push_entry(
        &mut self,
        repo: Option<String>,
        op: String,
        result: OpResult,
        detail: String,
        commands: Vec<crate::process::CommandRecord>,
    ) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.log.push(OpEntry {
            id,
            at: std::time::Instant::now(),
            repo,
            op,
            result,
            detail,
            commands,
        });
        if self.log.len() > Self::CAP {
            let excess = self.log.len() - Self::CAP;
            self.log.drain(..excess);
        }
        id
    }

    pub fn entry(&self, id: u64) -> Option<&OpEntry> {
        self.log.iter().find(|entry| entry.id == id)
    }
}

// ---- Repo detail view ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Head,
    Branch,
    Remote,
    Tag,
}

/// One commit from `git log` (History section). Lane placement lives in
/// `graph.rs`; `graph` is filled by `graph::layout` after loading/re-loading.
#[derive(Debug, Clone, Default)]
pub struct HistoryCommit {
    pub hash: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub refs: Vec<(String, RefKind)>,
    pub graph: GraphRow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_log_caps_ids_and_looks_up() {
        let mut ops = OpSummary::default();
        for n in 0..OpSummary::CAP + 5 {
            ops.push_info(format!("line {n}"));
        }
        assert_eq!(ops.log.len(), OpSummary::CAP, "the log must stay capped");
        assert!(
            ops.log.iter().all(|entry| entry.result == OpResult::Info),
            "info lines carry no repo or result"
        );
        let id = ops.push_entry(
            Some("repo".to_string()),
            "fetch".to_string(),
            OpResult::Failed,
            "could not fetch: boom".to_string(),
            Vec::new(),
        );
        let entry = ops.entry(id).expect("the entry must be findable");
        assert_eq!(entry.repo.as_deref(), Some("repo"));
        assert_eq!(entry.result, OpResult::Failed);
        assert!(ops.entry(id + 1000).is_none());
    }

    #[test]
    fn to_root_string_maps_the_current_distro_and_rejects_others() {
        let distro = crate::git::distro();
        assert_eq!(
            to_root_string(&format!(r"\\wsl.localhost\{distro}\home\me\dev")).unwrap(),
            "/home/me/dev"
        );
        assert_eq!(
            to_root_string(&format!(r"\\wsl$\{distro}\home\me")).unwrap(),
            "/home/me"
        );
        assert_eq!(
            to_root_string(r"\\wsl.localhost\OtherDistro\home\me").unwrap_err(),
            "OtherDistro"
        );
        assert_eq!(to_root_string(r"C:\src\dev").unwrap(), "/mnt/c/src/dev");
        assert_eq!(to_root_string("D:/work/repo").unwrap(), "/mnt/d/work/repo");
        assert_eq!(to_root_string("c:").unwrap(), "/mnt/c");
        assert_eq!(to_root_string("/home/me/dev").unwrap(), "/home/me/dev");
        // A colon inside an explicit Linux path is not a drive letter.
        assert_eq!(to_root_string("/tmp/a:b").unwrap(), "/tmp/a:b");
    }

    #[test]
    fn windows_paths_map_back_for_native_git() {
        assert_eq!(
            to_windows_path("/mnt/c/Users/me/dev").as_deref(),
            Some(r"C:\Users\me\dev")
        );
        assert_eq!(to_windows_path("/mnt/d").as_deref(), Some(r"D:\"));
        assert_eq!(to_windows_path("/home/me/dev"), None);
        assert_eq!(to_windows_path("/mnt/work/x"), None);

        assert_eq!(
            to_linux_path(r"C:\Users\me\dev").as_deref(),
            Some("/mnt/c/Users/me/dev")
        );
        assert_eq!(to_linux_path(r"d:\").as_deref(), Some("/mnt/d"));
        assert_eq!(to_linux_path("/home/me/dev"), None);
    }

    #[test]
    #[cfg(windows)]
    fn wsl_worktrees_are_everything_but_windows_mounts() {
        assert!(is_wsl_worktree("/home/me/dev"));
        assert!(is_wsl_worktree("/srv/git/app"));
        assert!(!is_wsl_worktree("/mnt/c/Users/me/dev"));
        assert!(!is_wsl_worktree("/mnt/d"));
    }

    #[test]
    fn gis_symbols_match_the_concept_table() {
        let expected = [
            (Flag::Clean, "✓"),
            (Flag::Stash, "$"),
            (Flag::Untracked, "?"),
            (Flag::Modified, "!"),
            (Flag::AddedStaged, "+"),
            (Flag::Deleted, "-"),
            (Flag::Renamed, "»"),
            (Flag::Conflicted, "="),
            (Flag::Diverged, "⇕"),
            (Flag::Ahead, "⇡"),
            (Flag::Behind, "⇣"),
            (Flag::UpstreamMissing, "✗"),
        ];
        for (flag, symbol) in expected {
            assert_eq!(flag.symbol(), symbol, "{flag:?}");
        }
        assert_eq!(Flag::Ahead.symbol_with_counts(3, 0), "⇡3");
        assert_eq!(Flag::Behind.symbol_with_counts(0, 2), "⇣2");
        assert_eq!(Flag::Diverged.symbol_with_counts(3, 4), "⇕3/4");
        assert_eq!(Flag::Modified.symbol_with_counts(3, 4), "!");
        let mut entry = RepoEntry {
            name: "x".into(),
            path: PathBuf::from("/x"),
            branch: Some("main".into()),
            unborn: false,
            upstream: None,
            ahead: 2,
            behind: 0,
            extra_branches: 0,
            flags: vec![],
            last_fetch: None,
            last_collected: None,
            state: RowState::Ready,
        };
        assert_eq!(entry.sync_label(), "⇡2");
        assert!(!entry.has_local_changes());
        entry.flags = vec![
            Flag::Clean,
            Flag::Ahead,
            Flag::Behind,
            Flag::Stash,
            Flag::UpstreamMissing,
        ];
        assert!(
            !entry.has_local_changes(),
            "stashes and sync state are not local changes"
        );
        entry.flags.push(Flag::Modified);
        assert!(entry.has_local_changes());
        entry.flags = vec![Flag::Conflicted];
        assert!(entry.has_local_changes());
    }
}
