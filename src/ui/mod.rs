//! SpurGit shell: title chrome, repository tabs, per-repository view (section
//! sidebar + content), Ctrl+K repository palette, floating operations card.
//! Visual vocabulary mirrors Zeron @ 1fdcfe19 (theme tokens, surface_chrome
//! metrics, command-palette structure, motion.rs).
//!
//! Component files:
//! - `widgets` — tones, type scale, and small reusable building blocks
//! - `motion`  — hover blends and transition tweens (adapted from Zeron)
//! - `chrome`  — app bar, empty state, operations card
//! - `tabs`    — repository tab strip
//! - `palette` — Ctrl+K command palette (and its actions)
//! - `roots`   — scan-roots manager dialog
//! - `repo` — repository detail view (sections sidebar + lists)
//! - `reset_dialog` — reset-to-commit dialog with rechecked identity
//! - `branch_menu` — branch row menu, rename dialog, delete confirms
//! - `history` — commit history with the lane graph
//! - `commit_menu` — history row context menu
//! - `tag_dialog` — create-tag dialog
//! - `apply_patch` — "Apply patch from clipboard" preview
//! - `shortcuts` — remappable bindings, cheat sheet, section keys
//! - `oplog` — operation log panel with command details
//! - `undo` — undo stacks over backup refs with the Undo alert
//! - `explainer` — hover explainer popouts for context-menu actions

pub(crate) mod app_icon;
pub(crate) mod apply_patch;
pub(crate) mod brand;
pub(crate) mod branch_dialog;
pub(crate) mod branch_menu;
pub(crate) mod branch_tree;
pub(crate) mod changes;
pub(crate) mod chrome;
pub(crate) mod commit;
pub(crate) mod commit_detail;
pub(crate) mod commit_menu;
pub(crate) mod conflicts;
pub(crate) mod diff;
pub(crate) mod explainer;
pub(crate) mod fetch_dialog;
pub(crate) mod history;
pub(crate) mod icon_view;
pub(crate) mod motion;
pub(crate) mod oplog;
pub(crate) mod palette;
pub(crate) mod pull_dialog;
pub(crate) mod push_dialog;
pub(crate) mod profile_dialog;
pub(crate) mod remote_dialog;
pub(crate) mod repo;
pub(crate) mod reset_dialog;
pub(crate) mod roots;
pub(crate) mod settings;
pub(crate) mod shortcuts;
pub(crate) mod stash;
pub(crate) mod tabs;
pub(crate) mod tag_dialog;
pub(crate) mod undo;
pub(crate) mod widgets;

pub use palette::{ClosePalette, SubmitCommit, TogglePalette};
pub use settings::OpenSettings;
pub use tabs::{CloseTab, NextTab, PrevTab};

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::color_picker::{ColorPickerEvent, ColorPickerState};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::{ActiveTheme, Icon, Root};
use gpui_kit::{
    div, hsla, px, point, size, AnimationExt as _, App, AppContext, Bounds, Context, Entity,
    FocusHandle, InteractiveElement as _, IntoElement, ParentElement, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, UniformListScrollHandle,
    Window, WindowBounds, WindowOptions,
};

use crate::i18n::t;
use crate::logging::log;
use crate::model::{to_root_string, OpSummary, RepoEntry, RowState};

use self::motion::{hover_blend, hover_listener};
use self::widgets::*;

// ---- Zeron metric tokens (Theme::* and surface_chrome.rs) ----

const TITLEBAR_H: f32 = 38.0; // Theme::TITLEBAR_HEIGHT — the one chrome row
const HEADER_H: f32 = 44.0; // Theme::HEADER_HEIGHT — section/sidebar headers
/// How often the active repository is re-collected so every panel (history,
/// changes, diff, details) follows external edits without waiting for the
/// full 15 s round. Skipped while a query for the same repository is running.
const ACTIVE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
/// How often to ask GitHub for a newer release (also once at startup).
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// Inactive per-tab history sessions kept in memory (LRU); browsing more tabs
/// than this drops the oldest session instead of retaining every commit.
const HISTORY_CACHE_MAX: usize = 4;
/// Inactive repository detail sets (remotes/tags/stashes) kept in memory.
const DETAILS_CACHE_MAX: usize = 8;
/// Inspector details are re-read on this slow floor even when no ref moved,
/// so an external remote/tag config edit is still picked up.
const DETAILS_FALLBACK_INTERVAL: Duration = Duration::from_secs(60);
/// Immutable commit-detail snapshots kept in memory (FIFO).
const COMMIT_DETAIL_CACHE_MAX: usize = 16;
/// Immutable stash file lists kept in memory (FIFO).
const STASH_FILES_CACHE_MAX: usize = 16;
const CONTROL_RADIUS: f32 = 6.0;
const PANEL_RADIUS: f32 = 12.0;
const PALETTE_W: f32 = 560.0; // Zeron command palette width
const ROOTS_W: f32 = 520.0; // roots manager dialog width
const PALETTE_RADIUS: f32 = 16.0; // Zeron palette card radius
const PALETTE_ITEM_RADIUS: f32 = 6.0;
const PALETTE_ITEM_H: f32 = 32.0;

/// Geist Mono — same face Zeron bundles for refs, paths, counts.
const MONO: &str = "Geist Mono";


/// Tab identity is the canonical worktree path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PathKey(String);

impl PathKey {
    fn new(p: &std::path::Path) -> Self {
        Self(p.to_string_lossy().into_owned())
    }
}

/// Root precedence: repeatable `--root=<path>` arguments, then
/// the saved root list, then `GIS_PATH`, then the launch directory. A
/// `--desktop` launch with none of those opens selection instead of scanning
/// home. Entries are literal: no shell expansion, no colon splitting.
fn resolve_launch_inputs(desktop: bool, saved_roots: Vec<String>) -> crate::discovery::RootPlan {
    let explicit: Vec<String> = std::env::args()
        .filter_map(|arg| {
            arg.strip_prefix("--root=")
                .filter(|path| !path.is_empty())
                .map(str::to_string)
        })
        .collect();
    crate::discovery::resolve_roots(&crate::discovery::LaunchInputs {
        explicit,
        saved: saved_roots,
        gis_path: std::env::var("GIS_PATH").ok(),
        launch_dir: std::env::current_dir()
            .ok()
            .map(|dir| dir.to_string_lossy().into_owned()),
        desktop,
    })
}

/// The platform file manager, opening a folder or (`select`) showing a file
/// selected in its folder.
fn file_manager_command(target: &str, select: bool) -> std::process::Command {
    use std::process::Command;
    if cfg!(windows) {
        let mut c = Command::new("explorer.exe");
        c.arg(if select { format!("/select,{target}") } else { target.to_string() });
        c
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        if select {
            c.arg("-R");
        }
        c.arg(target);
        c
    } else {
        // xdg-open cannot select a file, so open its folder instead.
        let dir = std::path::Path::new(target)
            .parent()
            .and_then(|parent| parent.to_str())
            .filter(|_| select)
            .unwrap_or(target);
        let mut c = Command::new("xdg-open");
        c.arg(dir);
        c
    }
}

pub fn window_options(width: f32, height: f32) -> WindowOptions {
    let bounds = Bounds {
        origin: point(px(80.), px(60.)),
        size: size(px(width), px(height)),
    };
    let maximized = std::env::args().any(|a| a == "--maximized");
    WindowOptions {
        titlebar: Some(gpui_kit::TitlebarOptions {
            title: Some(t().window_title.into()),
            // Zeron-style custom title bar: hide the OS caption and draw the
            // minimize/maximize/close controls in the app bar. On Windows the
            // platform keeps the resize frame and answers the control-area
            // hit tests (drag, snap layouts, double-click zoom) natively.
            appears_transparent: true,
            // macOS keeps its native traffic lights; centered in the 38px bar.
            traffic_light_position: Some(point(px(14.), px(12.))),
        }),
        window_bounds: Some(if maximized {
            WindowBounds::Maximized(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        }),
        window_min_size: Some(size(px(860.), px(560.))),
        ..WindowOptions::default()
    }
}

/// Wrap a modal overlay in the shared appear/disappear fade (scrim + card
/// together), the same motion vocabulary the Ctrl+K palette uses.
fn modal_fade_layer(overlay: gpui_kit::AnyElement, t: f32) -> gpui_kit::AnyElement {
    if t >= 1.0 {
        return overlay;
    }
    div()
        .absolute()
        .inset_0()
        .opacity(t)
        .child(overlay)
        .into_any_element()
}

/// What the branchling watches for once an operation's follow-up refresh
/// lands.
#[derive(Debug, Clone)]
pub(super) enum OpWatch {
    /// A fetch: Look when this repository's behind count grew.
    Fetch { repo: String, before: u32, after: u64 },
    /// A pull: Happy when the workspace ended clean and 0 behind.
    Pull { repo: String, after: u64 },
}

impl OpWatch {
    /// Whether an applied status result for `id` with this ticket serial can
    /// judge the watch. `after` is the row's query serial when the operation
    /// finished, so only a query begun after it sees the new refs: a poll
    /// already in flight would report the old behind count and use the
    /// watch up before the real follow-up refresh lands.
    fn judged_by(&self, id: &str, serial: u64) -> bool {
        match self {
            OpWatch::Fetch { repo, after, .. } | OpWatch::Pull { repo, after } => {
                repo == id && serial > *after
            }
        }
    }
}

pub struct SpurShell {
    /// Scan roots: repositories are discovered under these paths.
    roots: Vec<String>,
    repos: Vec<RepoEntry>,
    /// Live overview controller: discovered rows, generations, filters.
    overview: crate::overview::Overview,
    /// Inspector details (remotes/tags/stashes) cached per repository path.
    details: HashMap<String, crate::git::RepoDetails>,
    details_loading: HashSet<String>,
    /// Insertion order of [`details`] for the LRU budget.
    details_order: std::collections::VecDeque<String>,
    /// When each repository's inspector details were last loaded (slow
    /// fallback freshness check for external remote/tag edits).
    details_checked_at: HashMap<String, Instant>,
    /// HEAD+refs signature at the time each repository's details were loaded.
    details_signature: HashMap<String, (Option<String>, u64)>,
    /// True while a discovery scan runs; auto-refresh waits for it.
    discovering: bool,
    /// A rescan requested while one was already running.
    rescan_pending: bool,
    /// Workspace exclusions: repositories at or under these paths are hidden.
    exclude: Vec<String>,
    auto_refresh: bool,
    external_client: Option<crate::settings::ExternalClient>,
    /// Measured sections-wrapper height for explicit section sizing (0
    /// unmeasured).
    change_sections_h: f32,
    /// File-list column width in logical pixels (from settings).
    changes_list_w: f32,
    /// Soft-wrap long diff lines (from settings).
    diff_wrap: bool,
    /// A column drag in progress, with its (start logical x, start width).
    col_drag: Option<(f32, f32)>,
    /// Window width in logical pixels at the last frame, so the file list
    /// honours its half-window ceiling after the window shrinks.
    viewport_w: f32,
    /// Memoized wrapped diff: (the diff itself, wrap cols, rows). Holding the
    /// `Rc` keeps identity by `Rc::ptr_eq` sound: a freed diff's address can
    /// never be reused while the cache still points at it.
    diff_visual: std::cell::RefCell<
        Option<(std::rc::Rc<crate::git::FileDiff>, usize, std::rc::Rc<diff::VisualRows>)>,
    >,
    /// Last measured diff content width, for wrap columns.
    diff_wrap_w: f32,
    /// Open repository tabs, in strip order; active index selects the shown tab.
    tabs: Vec<PathKey>,
    active: Option<usize>,
    /// Per-tab work context (section, selection, conflict choices, stash
    /// view) kept across tab switches and restored on return.
    tab_contexts: HashMap<String, tabs::TabContext>,
    /// Horizontal scroll of the tab strip when many repositories are open.
    tabs_scroll: ScrollHandle,
    /// Tabs remembered in settings, reopened after the first workspace scan.
    restore_tabs_pending: Option<(Vec<String>, Option<String>)>,
    /// True while remembered tabs are being reopened (no re-persist).
    restoring_tabs: bool,
    /// Scripted `--repo` launches never overwrite the user's saved tabs.
    persist_tabs_enabled: bool,
    section: repo::RepoSection,
    // history of the active tab (loaded on open, paged by the list)
    history: Rc<history::HistoryPages>,
    history_loading: bool,
    history_exhausted: bool,
    history_error: Option<String>,
    history_for: Option<PathKey>,
    /// Bumped for every new history session; results from an older generation
    /// are dropped (rapid A -> B -> A tab switching must not merge replies).
    history_gen: u64,
    /// Revision tips captured at session start. Pages keep using the same set,
    /// so refs moving between pages cannot shuffle the paginated list.
    history_tips: Option<Vec<String>>,
    /// Graph lanes reserved for this session (from the first page) so paging
    /// older history cannot move the commit text sideways.
    history_graph_cols: usize,
    /// Incremental lane state of the active history session; pages lay out
    /// only their new commits, in the background.
    history_layout: crate::graph::LayoutState,
    /// Pinned branches as `repo<US>branch` keys, in pin order.
    pinned_branches: Vec<String>,
    /// Local Branches sort order.
    branch_sort: crate::settings::BranchSort,
    /// Flatten input for the branch tree: branches minus the pinned ones.
    branch_tree_branches: Rc<Vec<crate::status::BranchInfo>>,
    /// Pinned branch infos in pin order, rendered above the tree.
    pinned_branch_rows: Rc<Vec<crate::status::BranchInfo>>,
    /// Rename-branch dialog state and its name input.
    rename_request: Option<branch_menu::RenameRequest>,
    rebase_request: Option<branch_menu::RebaseRequest>,
    commit_menu_open: Option<commit_menu::OpenCommitMenu>,
    branch_rename_input: Entity<InputState>,
    /// Delete-branch confirm (first) and force-delete confirm (unmerged).
    branch_delete_request: Option<branch_menu::BranchDeleteRequest>,
    branch_delete_refused: Option<branch_menu::RefusedDelete>,
    /// Remote names of the active history session, resolved once so pages do
    /// not re-read them.
    history_remotes: Option<HashSet<String>>,
    selected_commit: Option<String>,
    /// Commit detail panel: tab, loaded metadata/files, and the read-only
    /// per-file diff (double-click a changed file).
    commit_tab: commit_detail::CommitTab,
    /// False after the panel's collapse button; the selection stays.
    commit_panel_open: bool,
    commit_detail: commit_detail::DetailState,
    commit_detail_for: Option<String>,
    /// Bumped per detail load; late replies for a previous commit are dropped.
    commit_detail_gen: u64,
    /// Commit details by (worktree, object id): commit objects are immutable,
    /// so a revisit renders without a Git read (FIFO-capped).
    commit_detail_cache: HashMap<(String, String), Rc<crate::git::CommitDetail>>,
    commit_detail_order: std::collections::VecDeque<(String, String)>,
    commit_file_selected: Option<usize>,
    commit_file_diff: commit_detail::FileDiffState,
    commit_file_diff_gen: u64,
    commit_files_scroll: UniformListScrollHandle,
    commit_diff_scroll: UniformListScrollHandle,
    commit_diff_h_scroll: ScrollHandle,
    /// Own scroll for the capped commit-message block.
    commit_message_scroll: ScrollHandle,
    /// True while the INFORMATION tab's message description is expanded.
    commit_message_open: bool,
    // history ref search (branch locations)
    history_refs: Vec<(String, String)>,
    history_refs_loaded: bool,
    history_refs_loading: bool,
    history_query: String,
    history_target: Option<String>,
    history_searching: bool,
    history_no_match: bool,
    history_jump_pending: bool,
    history_scroll: UniformListScrollHandle,
    history_search_input: Entity<InputState>,
    /// Per-tab history sessions kept across tab switches.
    history_cache: HashMap<String, history::CachedHistory>,
    /// Insertion order of [`history_cache`] for the LRU budget.
    history_cache_order: std::collections::VecDeque<String>,
    /// HEAD oid + refs hash the active history session was loaded from. A
    /// status round that observes a different signature reloads the history.
    history_revision: Option<(Option<String>, u64)>,
    /// Repository ids with a targeted status query in flight (shared by the
    /// active poll and post-mutation refreshes so they never duplicate work).
    repo_refresh_in_flight: HashSet<String>,
    /// A refresh requested while one was already running (id -> whether the
    /// open diff should be re-read); runs when the in-flight one finishes.
    repo_refresh_pending: HashMap<String, bool>,
    /// A manual Refresh also re-reads the open diff when the round finishes.
    refresh_diff_on_finish: bool,
    /// A round was requested while one was in flight (e.g. by a rescan);
    /// it starts as soon as the current round finishes.
    round_pending: bool,
    /// Active-repository display rows for the virtualized Local Changes and
    /// Local Branches lists; rebuilt only when the row or query serial changes.
    change_lists: Rc<crate::status::ChangeLists>,
    /// Filtered display rows for the two Local Changes sections.
    change_rows: Rc<changes::ChangeRows>,
    change_filter: Entity<InputState>,
    /// Selected file in Local Changes, with the side its diff comes from.
    change_selection: Option<changes::Selection>,
    /// All selected rows (multi-select); contains [`change_selection`].
    change_selected: Vec<changes::Selection>,
    /// Shift-click anchor: (section side, row index).
    change_anchor: Option<(bool, usize)>,
    /// Pending destructive Discard awaiting confirmation.
    discard_confirm: Option<changes::DiscardRequest>,
    /// Pending Stash dialog (mode + description).
    stash_request: Option<stash::StashRequest>,
    /// Pending destructive Drop of one stash entry.
    stash_drop_confirm: Option<String>,
    /// Pending clipboard-patch preview.
    apply_patch_request: Option<apply_patch::ApplyPatchRequest>,
    /// Cheat-sheet card open.
    shortcuts_open: bool,
    /// Pending branch-from-stash dialog plus its branch-name input.
    stash_branch_request: Option<stash::StashBranchRequest>,
    stash_branch_input: Entity<InputState>,
    stash_message_input: Entity<InputState>,
    /// Remembered stash mode between dialogs.
    stash_mode: crate::git::StashMode,
    /// Sequential stage/unstage/hunk operations; never overlap on one index.
    change_ops: std::collections::VecDeque<changes::ChangeOp>,
    change_busy: bool,
    /// The executing queued operation, for the log panel's running row.
    running_op: Option<oplog::RunningOp>,
 /// A short-lived expression for the branchling:
    /// `(face, until)`. [`Self::brand_face`] reads it; Look and Happy set it.
    face_hold: Option<(brand::Face, Instant)>,
    /// False until the first root is added or the first repository opens:
    /// the empty state keeps its welcome line and the
    /// character winks once at startup.
    welcomed: bool,
    /// Commit identities, the default one and the per-repository picks.
    profiles: Vec<crate::accounts::Profile>,
    default_profile: Option<String>,
    repo_profiles: std::collections::BTreeMap<String, String>,
    /// GitHub accounts Git Credential Manager has a login for.
    stored_accounts: Vec<String>,
    profile_request: Option<profile_dialog::ProfileRequest>,
    profile_label_input: Entity<InputState>,
    profile_name_input: Entity<InputState>,
    profile_email_input: Entity<InputState>,
    profile_github_input: Entity<InputState>,
    /// The branchling's pending post-operation check:
    /// armed when a fetch or pull succeeds, evaluated when its refresh lands.
    op_watch: Option<OpWatch>,
    branches: Rc<Vec<crate::status::BranchInfo>>,
    /// Remote-tracking branches of the active repository (Remotes list).
    remote_branches: Rc<Vec<crate::status::RemoteBranch>>,
    /// Remote names of the Remotes list the user collapsed.
    remote_folders_closed: HashSet<String>,
    /// Flattened Local Branches rows: folders + branches (SourceGit-style).
    branch_rows: Rc<Vec<branch_tree::BranchRow>>,
    /// Folder paths of the branch tree the user collapsed (default expanded).
    branch_folders_closed: HashSet<String>,
    /// Flattened Remotes rows: remotes + their branches. Cached; rebuilt only
    /// when refs, inspector details, or the collapsed set change (a long ref
    /// list otherwise re-sorts and re-allocates every frame).
    remote_rows: Rc<Vec<branch_tree::RemoteRow>>,
    /// Create Branch dialog (base/name/changes/checkout/overwrite).
    branch_request: Option<branch_dialog::CreateBranchRequest>,
    /// Edit-URL, rename, and remove remote dialogs.
    edit_url_request: Option<remote_dialog::EditUrlRequest>,
    rename_remote_request: Option<remote_dialog::RenameRemoteRequest>,
    remove_remote_request: Option<remote_dialog::RemoveRemoteRequest>,
    /// Create Tag dialog (name/message for one commit).
    tag_request: Option<tag_dialog::TagRequest>,
    /// Push-tag and delete-tag dialogs.
    tag_push_request: Option<tag_dialog::TagPushRequest>,
    tag_delete_request: Option<tag_dialog::TagDeleteRequest>,
    /// Tag list filter text (the box shows past 20 tags).
    tag_filter_input: Entity<InputState>,
    tag_filter: String,
    tag_name_input: Entity<InputState>,
    tag_message_input: Entity<InputState>,
    branch_name_input: Entity<InputState>,
    create_branch_checkout: bool,
    create_branch_overwrite: bool,
    create_branch_changes: crate::git::BranchChanges,
    /// Add Remote dialog (name + URL).
    remote_request: Option<remote_dialog::AddRemoteRequest>,
    remote_name_input: Entity<InputState>,
    remote_url_input: Entity<InputState>,
    /// Push dialog: remote + target branch selection.
    push_request: Option<push_dialog::PushRequest>,
    /// Remembered remote between pushes.
    push_remote: Option<String>,
    /// Fetch dialog: remote + option flags (remembered between fetches).
    fetch_request: Option<fetch_dialog::FetchRequest>,
    fetch_remote: Option<String>,
    fetch_force: bool,
    fetch_all_remotes: bool,
    fetch_no_tags: bool,
    /// Pull dialog: remote + branch + local-changes policy (remembered).
    pull_request: Option<pull_dialog::PullRequest>,
    pull_remote: Option<String>,
    pull_changes: crate::git::PullChanges,
    pull_rebase: bool,
    /// Reset dialog: one target commit plus the rechecked identity.
    reset_request: Option<reset_dialog::ResetRequest>,
    detail_lists_for: Option<(String, u64)>,
    unstaged_scroll: UniformListScrollHandle,
    staged_scroll: UniformListScrollHandle,
    branches_scroll: ScrollHandle,
    /// Virtualized fallback handles for the branch/remote trees when a list
    /// is too long to animate row heights (see `VIRTUAL_TREE_THRESHOLD`).
    branches_vscroll: UniformListScrollHandle,
    remotes_vscroll: UniformListScrollHandle,
    stashes_scroll: UniformListScrollHandle,
    /// Stash view: selected entry id, its changed files, and the open diff.
    selected_stash: Option<String>,
    /// The selected entry's stash commit object id: reads, cache keys, and
    /// the file list are all keyed by it, so an external stash inserted while
    /// the inspector list is stale cannot make `stash@{n}` resolve to another
    /// entry mid-read.
    selected_stash_object: Option<String>,
    /// Immutable file list of the selected stash: the SAME `Rc` the cache
    /// holds, never a deep copy.
    stash_changes: Option<Rc<crate::git::StashChanges>>,
    /// Cached stash file lists by (worktree, stash object id); a stash commit
    /// is immutable, so a revisit paints without a Git read (FIFO-capped).
    stash_files_cache: HashMap<(String, String), Rc<crate::git::StashChanges>>,
    stash_files_cache_order: std::collections::VecDeque<(String, String)>,
    /// Per immutable stash object: last opened file and scroll offsets.
    stash_view_memory: HashMap<String, stash::StashMemory>,
    stash_files_for: Option<String>,
    stash_files_loading: bool,
    stash_files_error: Option<String>,
    stash_files_gen: u64,
    stash_file_selected: Option<usize>,
    stash_diff: commit_detail::FileDiffState,
    stash_diff_gen: u64,
    stash_files_scroll: UniformListScrollHandle,
    stash_diff_scroll: UniformListScrollHandle,
    stash_diff_h_scroll: ScrollHandle,
    remotes_scroll: ScrollHandle,
    tags_scroll: UniformListScrollHandle,
    /// Expanded sidebar collapsibles (Local Branches / Remotes / Tags).
    side_open: [bool; 3],
    /// Opening/closing progress of each collapsible (1 = fully open).
    side_fade: [motion::Transition; 3],
    /// Per-folder expand/collapse animations inside the sidebar trees; keys
    /// are `b/<branch folder path>` and `r/<remote name>`; folders animate
    /// like their collapsible hosts.
    folder_fades: HashMap<String, motion::Transition>,
    /// Diff pane state and its generation guard (stale replies are dropped).
    diff: diff::DiffState,
    diff_gen: u64,
    /// Parsed diffs per cache key: switching back to a file paints instantly
    /// while the fresh read revalidates in the background; immutable Git
    /// object diffs (commits, stashes) are served without any re-read.
    diff_cache: HashMap<diff::DiffKey, Rc<crate::git::FileDiff>>,
    /// Insertion order and retained bytes of [`diff_cache`] for the
    /// byte-budgeted eviction (a clear-all dropped useful small previews).
    diff_cache_order: std::collections::VecDeque<diff::DiffKey>,
    diff_cache_bytes: usize,
    /// A diff read is in flight; the previous content stays on screen.
    diff_loading: bool,
    /// Conflict resolver state (right pane when the selected row is
    /// unmerged) and its generation guard.
    conflict_view: Option<conflicts::ConflictView>,
    conflict_gen: u64,
    diff_scroll: UniformListScrollHandle,
    diff_h_scroll: ScrollHandle,
    /// Hunk under the pointer in the diff pane (hover actions).
    hovered_hunk: Option<usize>,
    /// Pending destructive hunk Discard awaiting confirmation.
    hunk_discard: Option<changes::HunkDiscardRequest>,
    /// Commit box: staged-snapshot commit, message retained on failure.
    commit_message: Entity<TextareaState>,
    commit_in_flight: bool,
    commit_error: Option<String>,
    /// Async success clears the textarea on the next render (needs a Window).
    commit_clear_pending: bool,
    ops: OpSummary,
    /// Newest error's log entry, shown as a transient bottom-left alert.
    ops_toast: Option<u64>,
    /// Operation log panel: open flag, expanded/highlighted entries,
    /// and its scroll position.
    oplog_open: bool,
    oplog_expanded: Option<u64>,
    oplog_highlight: Option<u64>,
    oplog_scroll: ScrollHandle,
    /// Invalidates the timer expiry when a newer error arrives.
    ops_toast_gen: u64,
    /// How long the alert stays (settings: 3 s, 5 s, 10 s, or click-to-remove).
    alert_timeout: crate::settings::AlertTimeout,
    /// Undo stacks per repository: newest last, capped.
    undo_stacks: HashMap<String, Vec<undo::UndoEntry>>,
    /// The single redo step banked by the last undo (ref moves only).
    redo_slots: HashMap<String, undo::UndoEntry>,
    /// Info-toned transient alert offering Undo after an undoable op.
    undo_toast: Option<undo::UndoToast>,
    /// Invalidates the undo timer when a newer undoable op lands.
    undo_toast_gen: u64,
    /// Newer GitHub release, if any; Settings offers to install it.
    update: Option<crate::update::Release>,
    /// The update toast is up (until dismissed or Settings opens).
    update_toast: bool,
    updating: bool,
    /// Recently-discarded backups per repository.
    discarded: HashMap<String, Vec<undo::DiscardedBackup>>,
    next_discarded_id: u64,
    discarded_open: bool,
    discarded_repo: Option<String>,
    discarded_expanded: Option<u64>,
    discarded_file: Option<(u64, usize)>,
    discarded_diffs: HashMap<(u64, usize), Rc<crate::git::FileDiff>>,
    discarded_scroll: ScrollHandle,
    discarded_vscroll: UniformListScrollHandle,
    discarded_hscroll: ScrollHandle,
    /// Menu explainer popout (history menu for now): pending/visible state
    /// and its generation.
    explainer: Option<explainer::ExplainerState>,
    explainer_gen: u64,
    // focus
    root_focus: FocusHandle,
    // palette
    palette_open: bool,
    /// True while the palette plays its exit fade (still rendered, then
    /// dropped by the render pass when the fade reaches zero).
    palette_closing: bool,
    /// Width morph only animates the palette <-> roots switch; the value
    /// re-anchors on interruption like [`dialog_fade`](Self::dialog_fade).
    dialog_width: motion::Transition,
    palette_active: usize,
    palette_focus: FocusHandle,
    palette_focus_pending: bool,
    palette_input: Entity<InputState>,
    palette_scroll: ScrollHandle,
    palette_enter: palette::EnterPress,
    /// Shared overlay fade (0 = hidden, 1 = open). Re-anchors on interruption.
    dialog_fade: motion::Transition,
    /// Appear/disappear fade for the request modals (branch, remote, push,
    /// fetch, pull, stash, confirmations): scrim and card fade together with
    /// the same motion vocabulary as the Ctrl+K palette.
    modal_fade: motion::Transition,
    /// Bumped on every modal open/close so a deferred clear cannot dismiss a
    /// newer modal.
    modal_gen: u64,
    /// Focus to restore when the overlay closes.
    dialog_previous_focus: Option<FocusHandle>,
    // roots manager
    roots_open: bool,
    roots_focus_pending: bool,
    root_input: Entity<InputState>,
    // settings page (theme store)
    settings_open: bool,
    settings_tab: settings::SettingsTab,
    /// Focus target for shortcut capture: clicking a capture chip
    /// moves focus here so key presses reach the Settings key handler.
    /// Without this, focus stays wherever it was and capture never fires.
    shortcut_focus: FocusHandle,
    /// Action awaiting a key press in the Shortcuts tab.
    shortcut_capture: Option<String>,
    /// Capture/save status line of the Shortcuts tab.
    shortcut_status: Option<(String, bool)>,
    settings_scroll: ScrollHandle,
    theme_menu_open: bool,
    theme_name_input: Entity<InputState>,
    theme_pickers: Vec<(crate::theme::Token, Entity<ColorPickerState>)>,
    theme_draft: crate::theme::ThemeConfig,
    theme_dirty: bool,
    /// Bumped on every draft edit; only the latest scheduled auto-save runs.
    theme_save_gen: u64,
    theme_status: Option<(String, bool)>,
    _subscriptions: Vec<Subscription>,
}

impl SpurShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Started from Finder or the Dock the working directory is `/`, which
        // is never a folder to scan: behave like `--desktop`.
        let desktop = std::env::args().any(|a| a == "--desktop")
            || std::env::current_dir().is_ok_and(|dir| dir == std::path::Path::new("/"));
        let (settings, settings_diagnostics) = crate::settings::load();
        for diagnostic in settings_diagnostics {
            log!("settings: {diagnostic}");
        }
        let launch = resolve_launch_inputs(desktop, settings.roots.clone());
        for diagnostic in &launch.diagnostics {
            log!("root: {diagnostic}");
        }
        let roots = launch.roots.clone();
        log!(
            "launch: {} ({} root(s)) {:?}",
            if desktop { "desktop" } else { "normal" },
            roots.len(),
            roots
        );
        let repos = Vec::new();
        // A desktop launch with no usable root opens selection instead of
        // scanning home.
        let roots_open = desktop
            && launch.source == crate::discovery::RootSource::Selection
            && roots.is_empty();
        // Repeatable: open several repositories as tabs in argument order.
        let open_repos: Vec<String> = std::env::args()
            .filter_map(|a| {
                a.strip_prefix("--repo=")
                    .filter(|p| !p.is_empty())
                    .map(|p| to_root_string(p).unwrap_or_else(|_| p.to_string()))
            })
            .collect();
        // Session persistence: remembered tabs reopen after the first scan;
        // explicit `--repo` launches never overwrite the user's saved list.
        let persist_tabs_enabled = open_repos.is_empty();
        let restore_tabs_pending = persist_tabs_enabled
            .then(|| (settings.open_repos.clone(), settings.active_repo.clone()))
            .filter(|(repos, _)| !repos.is_empty());
        let root_focus = cx.focus_handle();
        let root_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().path_input_placeholder())
        });
        let history_search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().history_search_placeholder)
        });
        let change_filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().filter_changes_placeholder)
        });
        let commit_message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(t().commit_placeholder)
                .rows(4)
        });
        let stash_message_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().stash_placeholder)
        });
        let stash_branch_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().stash_branch_placeholder)
        });
        let branch_name_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().branch_name_placeholder)
        });
        let branch_rename_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().rename_placeholder)
        });
        let tag_name_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().tag_name_placeholder)
        });
        let tag_message_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().tag_message)
        });
        let tag_filter_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().tag_filter_placeholder)
        });
        let remote_name_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().remote_name_placeholder)
        });
        let remote_url_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().remote_url_placeholder)
        });
        let profile_label_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().profile_label_placeholder)
        });
        let profile_name_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().profile_name_placeholder)
        });
        let profile_email_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().profile_email_placeholder)
        });
        let profile_github_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().profile_github_placeholder)
        });
        let palette_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().search_placeholder)
        });
        let theme_name_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t().theme_name_placeholder)
        });
        let theme_pickers: Vec<(crate::theme::Token, Entity<ColorPickerState>)> =
            crate::theme::TOKENS
                .iter()
                .map(|&token| (token, cx.new(|cx| ColorPickerState::new(window, cx))))
                .collect();

        let mut subs = Vec::new();
        // Shortcut capture (Settings) must see a press before any binding
        // fires: bindings dispatch ahead of key-down listeners, interceptors
        // ahead of bindings.
        let capture_shell = cx.entity().downgrade();
        subs.push(cx.intercept_keystrokes(move |event, _, cx| {
            let Some(shell) = capture_shell.upgrade() else {
                return;
            };
            if shell.read(cx).shortcut_capture.is_none() {
                return;
            }
            shell.update(cx, |this, cx| {
                this.capture_shortcut_key(&event.keystroke.modifiers, &event.keystroke.key, cx)
            });
            cx.stop_propagation();
        }));
        subs.push(cx.subscribe_in(&root_input, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.add_root(window, cx);
            }
        }));
        subs.push(
            cx.subscribe_in(&history_search_input, window, |this, _, event, _, cx| {
                match event {
                    InputEvent::Change => this.preview_history_search(cx),
                    InputEvent::PressEnter { .. } => this.jump_history_search(cx),
                    _ => {}
                }
            }),
        );
        subs.push(cx.subscribe_in(&change_filter, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.rebuild_change_rows(cx);
                cx.notify();
            }
        }));
        subs.push(
            cx.subscribe_in(&commit_message, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    // Editing dismisses the previous hook/failure output.
                    this.commit_error = None;
                    cx.notify();
                }
            }),
        );
        subs.push(
            cx.subscribe_in(&branch_name_input, window, |this, _, event, _, cx| {
                match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.confirm_create_branch(cx),
                    _ => {}
                }
            }),
        );
        subs.push(
            cx.subscribe_in(&branch_rename_input, window, |this, _, event, _, cx| {
                match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.confirm_rename_branch(cx),
                    _ => {}
                }
            }),
        );
        subs.push(
            cx.subscribe_in(&stash_branch_input, window, |this, _, event, _, cx| {
                match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.confirm_stash_branch(cx),
                    _ => {}
                }
            }),
        );
        for input in [&tag_name_input, &tag_message_input] {
            subs.push(cx.subscribe_in(input, window, |this, _, event, _, cx| {
                match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.confirm_create_tag(cx),
                    _ => {}
                }
            }));
        }
        subs.push(
            cx.subscribe_in(&tag_filter_input, window, |this, _, event, _, cx| {
                if let InputEvent::Change = event {
                    this.tag_filter =
                        this.tag_filter_input.read(cx).value().trim().to_string();
                    cx.notify();
                }
            }),
        );
        for input in [&remote_name_input, &remote_url_input] {
            subs.push(cx.subscribe_in(input, window, |this, _, event, _, cx| {
                match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.confirm_add_remote(cx),
                    _ => {}
                }
            }));
        }
        for input in [
            &profile_label_input,
            &profile_name_input,
            &profile_email_input,
            &profile_github_input,
        ] {
            subs.push(cx.subscribe_in(input, window, |this, _, event, _, cx| {
                match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.confirm_profile(cx),
                    _ => {}
                }
            }));
        }
        subs.push(
            cx.subscribe_in(&palette_input, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.palette_active = 0;
                    this.palette_scroll.set_offset(point(px(0.), px(0.)));
                    let query = this.palette_input.read(cx).value().to_string();
                    this.overview.set_search(query);
                    cx.notify();
                }
            }),
        );
        subs.push(
            cx.subscribe_in(&theme_name_input, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.save_theme(window, cx);
                }
            }),
        );
        for (token, picker) in &theme_pickers {
            let token = *token;
            subs.push(cx.subscribe(
                picker,
                move |this, _, event: &ColorPickerEvent, cx| {
                    if let ColorPickerEvent::Change(Some(color)) = event {
                        this.apply_theme_token(token, *color, cx);
                    }
                },
            ));
        }

        let theme_draft = {
            let selected = crate::theme::selected(cx);
            crate::theme::config(cx, &selected)
                .or_else(|| crate::theme::config(cx, crate::theme::PRESET_NAME))
                .expect("the built-in preset always exists")
        };

        let mut shell = Self {
            roots,
            repos,
            overview: {
                let mut overview = crate::overview::Overview::new();
                overview.refresh_loop.set_enabled(settings.auto_refresh);
                overview
            },
            details: HashMap::new(),
            details_loading: HashSet::new(),
            details_order: std::collections::VecDeque::new(),
            details_checked_at: HashMap::new(),
            details_signature: HashMap::new(),
            discovering: false,
            rescan_pending: false,
            exclude: {
                let mut exclude = settings.exclude.clone();
                exclude.extend(std::env::args().filter_map(|arg| {
                    arg.strip_prefix("--exclude=")
                        .filter(|path| !path.is_empty())
                        .and_then(|path| to_root_string(path).ok())
                }));
                exclude
            },
            auto_refresh: settings.auto_refresh,
            external_client: settings.external_client.clone(),
            change_sections_h: 0.0,
            changes_list_w: settings.changes_list_w,
            diff_wrap: settings.diff_wrap,
            col_drag: None,
            viewport_w: 0.0,
            diff_visual: std::cell::RefCell::new(None),
            diff_wrap_w: 0.0,
            tabs: Vec::new(),
            active: None,
            tab_contexts: HashMap::new(),
            tabs_scroll: ScrollHandle::new(),
            restore_tabs_pending,
            restoring_tabs: false,
            persist_tabs_enabled,
            section: repo::RepoSection::History,
            history: Rc::new(history::HistoryPages::default()),
            history_loading: false,
            history_exhausted: false,
            history_error: None,
            history_for: None,
            history_gen: 0,
            history_tips: None,
            history_graph_cols: 0,
            history_layout: crate::graph::LayoutState::default(),
            history_remotes: None,
            selected_commit: None,
            commit_tab: commit_detail::CommitTab::Information,
            commit_panel_open: false,
            commit_detail: commit_detail::DetailState::Empty,
            commit_detail_for: None,
            commit_detail_gen: 0,
            commit_detail_cache: HashMap::new(),
            commit_detail_order: std::collections::VecDeque::new(),
            commit_file_selected: None,
            commit_file_diff: commit_detail::FileDiffState::Empty,
            commit_file_diff_gen: 0,
            commit_files_scroll: UniformListScrollHandle::new(),
            commit_diff_scroll: UniformListScrollHandle::new(),
            commit_diff_h_scroll: ScrollHandle::new(),
            commit_message_scroll: ScrollHandle::new(),
            commit_message_open: false,
            history_refs: Vec::new(),
            history_refs_loaded: false,
            history_refs_loading: false,
            history_query: String::new(),
            history_target: None,
            history_searching: false,
            history_no_match: false,
            history_jump_pending: false,
            history_scroll: UniformListScrollHandle::new(),
            history_search_input,
            history_cache: HashMap::new(),
            history_cache_order: std::collections::VecDeque::new(),
            history_revision: None,
            repo_refresh_in_flight: HashSet::new(),
            refresh_diff_on_finish: false,
            round_pending: false,
            repo_refresh_pending: HashMap::new(),
            change_lists: Rc::new(crate::status::ChangeLists::default()),
            change_rows: Rc::new(changes::ChangeRows::default()),
            change_filter,
            change_selection: None,
            change_selected: Vec::new(),
            change_anchor: None,
            discard_confirm: None,
            stash_request: None,
            stash_drop_confirm: None,
            apply_patch_request: None,
            shortcuts_open: false,
            stash_branch_request: None,
            stash_branch_input,
            stash_message_input,
            stash_mode: crate::git::StashMode::default(),
            change_ops: std::collections::VecDeque::new(),
            change_busy: false,
            running_op: None,
            face_hold: None,
            welcomed: settings.welcomed,
            profiles: settings.profiles.clone(),
            default_profile: settings.default_profile.clone(),
            repo_profiles: settings.repo_profiles.clone(),
            stored_accounts: Vec::new(),
            profile_request: None,
            profile_label_input,
            profile_name_input,
            profile_email_input,
            profile_github_input,
            op_watch: None,
            branches: Rc::new(Vec::new()),
            remote_branches: Rc::new(Vec::new()),
            remote_folders_closed: HashSet::new(),
            pinned_branches: settings.pinned_branches.clone(),
            branch_sort: settings.branch_sort,
            branch_tree_branches: Rc::new(Vec::new()),
            pinned_branch_rows: Rc::new(Vec::new()),
            rename_request: None,
            rebase_request: None,
            commit_menu_open: None,
            branch_rename_input,
            branch_delete_request: None,
            branch_delete_refused: None,
            branch_rows: Rc::new(Vec::new()),
            remote_rows: Rc::new(Vec::new()),
            branch_folders_closed: HashSet::new(),
            branch_request: None,
            branch_name_input,
            edit_url_request: None,
            rename_remote_request: None,
            remove_remote_request: None,
            tag_request: None,
            tag_name_input,
            tag_message_input,
            tag_push_request: None,
            tag_delete_request: None,
            tag_filter_input,
            tag_filter: String::new(),
            create_branch_checkout: true,
            create_branch_overwrite: false,
            create_branch_changes: Default::default(),
            remote_request: None,
            remote_name_input,
            remote_url_input,
            push_request: None,
            push_remote: None,
            fetch_request: None,
            fetch_remote: None,
            fetch_force: false,
            fetch_all_remotes: false,
            fetch_no_tags: false,
            pull_request: None,
            pull_remote: None,
            pull_changes: Default::default(),
            pull_rebase: false,
            reset_request: None,
            detail_lists_for: None,
            unstaged_scroll: UniformListScrollHandle::new(),
            staged_scroll: UniformListScrollHandle::new(),
            branches_scroll: ScrollHandle::new(),
            branches_vscroll: UniformListScrollHandle::new(),
            remotes_vscroll: UniformListScrollHandle::new(),
            stashes_scroll: UniformListScrollHandle::new(),
            selected_stash: None,
            selected_stash_object: None,
            stash_changes: None,
            stash_view_memory: HashMap::new(),
            stash_files_for: None,
            stash_files_cache: HashMap::new(),
            stash_files_cache_order: std::collections::VecDeque::new(),
            stash_files_loading: false,
            stash_files_error: None,
            stash_files_gen: 0,
            stash_file_selected: None,
            stash_diff: commit_detail::FileDiffState::Empty,
            stash_diff_gen: 0,
            stash_files_scroll: UniformListScrollHandle::new(),
            stash_diff_scroll: UniformListScrollHandle::new(),
            stash_diff_h_scroll: ScrollHandle::new(),
            remotes_scroll: ScrollHandle::new(),
            tags_scroll: UniformListScrollHandle::new(),
            side_open: [false; 3],
            side_fade: std::array::from_fn(|_| motion::Transition::settled(0.0)),
            folder_fades: HashMap::new(),
            diff: diff::DiffState::Empty,
            diff_gen: 0,
            diff_cache: HashMap::new(),
            diff_cache_order: std::collections::VecDeque::new(),
            diff_cache_bytes: 0,
            diff_loading: false,
            conflict_view: None,
            conflict_gen: 0,
            diff_scroll: UniformListScrollHandle::new(),
            diff_h_scroll: ScrollHandle::new(),
            hovered_hunk: None,
            hunk_discard: None,
            commit_message,
            commit_in_flight: false,
            commit_error: None,
            commit_clear_pending: false,
            ops: OpSummary::default(),
            ops_toast: None,
            oplog_open: false,
            oplog_expanded: None,
            oplog_highlight: None,
            oplog_scroll: ScrollHandle::new(),
            ops_toast_gen: 0,
            alert_timeout: settings.alert_timeout,
            undo_stacks: HashMap::new(),
            redo_slots: HashMap::new(),
            undo_toast: None,
            undo_toast_gen: 0,
            update: None,
            update_toast: false,
            updating: false,
            discarded: HashMap::new(),
            next_discarded_id: 0,
            discarded_open: false,
            discarded_repo: None,
            discarded_expanded: None,
            discarded_file: None,
            discarded_diffs: HashMap::new(),
            discarded_scroll: ScrollHandle::new(),
            discarded_vscroll: UniformListScrollHandle::new(),
            discarded_hscroll: ScrollHandle::new(),
            explainer: None,
            explainer_gen: 0,
            root_focus: root_focus.clone(),
            palette_open: false,
            palette_closing: false,
            dialog_width: motion::Transition::settled(if roots_open { ROOTS_W } else { PALETTE_W }),
            palette_active: 0,
            palette_focus: cx.focus_handle(),
            palette_focus_pending: false,
            palette_input,
            palette_scroll: ScrollHandle::new(),
            palette_enter: palette::EnterPress::default(),
            dialog_fade: motion::Transition::settled(if roots_open { 1.0 } else { 0.0 }),
            modal_fade: motion::Transition::settled(0.0),
            modal_gen: 0,
            dialog_previous_focus: None,
            roots_open,
            roots_focus_pending: roots_open,
            root_input,
            settings_open: false,
            settings_tab: settings::SettingsTab::General,
            shortcut_capture: None,
            shortcut_status: None,
            settings_scroll: ScrollHandle::new(),
            shortcut_focus: cx.focus_handle(),
            theme_menu_open: false,
            theme_name_input,
            theme_pickers,
            theme_draft,
            theme_dirty: false,
            theme_save_gen: 0,
            theme_status: None,
            _subscriptions: subs,
        };
        if !open_repos.is_empty() {
            for path in &open_repos {
                shell.open_repo_path(path, cx);
            }
        } else if !shell.roots.is_empty()
            && let Some((saved, active)) = shell.restore_tabs_pending.clone()
        {
            // Tab metadata first: the strip is complete immediately, but only
            // the remembered (or first) tab activates and loads history,
            // details, and status. The rest load lazily when opened.
            // The scan later only prunes missing paths.
            shell.restoring_tabs = true;
            for path in &saved {
                log!("tab: restore {path}");
                shell.restore_tab(path, cx);
            }
            let chosen = active
                .filter(|active| shell.tabs.iter().any(|key| key.0 == *active))
                .or_else(|| shell.tabs.first().map(|key| key.0.clone()));
            if let Some(chosen) = chosen {
                log!("tab: activate restored {chosen}");
                shell.activate_restored_tab(&chosen, cx);
            }
            shell.restoring_tabs = false;
        }
        // Restored (or `--repo`) tabs are repositories open, so this is not a
        // first run: the welcome line and the wink are
        // for empty installs. `open_repo` covers later opens; the restore
        // branches above do not route through it. Saved roots mean the same:
        // an upgraded install already has its folders, so "Add a folder"
        // would be wrong (only saved roots count; a launch-directory root is
        // not the user's choice yet).
        if !shell.tabs.is_empty() || !settings.roots.is_empty() {
            shell.mark_welcomed(cx);
        }
        // The theme editor always opens with the selected theme's colors.
        shell.sync_theme_draft(window, cx);
        // Root owns focus so global actions (Ctrl+K, Esc) dispatch.
        window.focus(&root_focus, cx);
        // Live workspace: discovery, the first status round, and the bounded
        // auto-refresh tick. No roots leaves the empty state.
        if !shell.roots.is_empty() {
            shell.rescan_workspace(cx);
        }
        shell.spawn_auto_refresh_tick(cx);
        shell.spawn_active_refresh_tick(cx);
        shell.spawn_update_check(cx);
        // First run: one wink while the empty state is up; the welcome line
        // stays until a root or a repository arrives.
        if !shell.welcomed {
            shell.hold_face(brand::Face::Wink, Duration::from_millis(1600), cx);
        }
        shell
    }

    // ---- shared services used by several components ----

    fn active_repo(&self) -> Option<&RepoEntry> {
        let ix = self.active?;
        let key = self.tabs.get(ix)?;
        self.repos.iter().find(|r| PathKey::new(&r.path) == *key)
    }

    /// Canonical path of the active repository (row identity).
    pub(super) fn active_repo_id(&self) -> Option<String> {
        self.active_repo()
            .map(|row| row.path.to_string_lossy().into_owned())
    }

    /// Reveal a path in the native file manager. On Windows, Linux paths map to
    /// `\\wsl.localhost\<distro>\…` so Explorer can open them.
    fn open_in_explorer(&mut self, path: &str, cx: &mut Context<Self>) {        let target = explorer_target(path);
        match file_manager_command(&target, false).spawn() {
            Ok(_) => {
                log!("explorer: {target}");
                self.ops.push_info(t().log_explorer(&target));
            }
            Err(e) => {
                log!("explorer failed: {e}");
                self.note_error(t().log_explorer_failed(&e.to_string()), cx);
            }
        }
        cx.notify();
    }

    /// Select a file in the file manager (`explorer.exe /select,…`, `open -R`).
    /// Opening the file itself would launch its default application instead.
    fn reveal_in_explorer(&mut self, path: &str, cx: &mut Context<Self>) {
        let target = explorer_target(path);
        match file_manager_command(&target, true).spawn() {
            Ok(_) => {
                log!("reveal: {target}");
                self.ops.push_info(t().log_explorer(&target));
            }
            Err(e) => {
                log!("reveal failed: {e}");
                self.note_error(t().log_explorer_failed(&e.to_string()), cx);
            }
        }
        cx.notify();
    }

    /// Persist the scan-root list. A failed write leaves the previous settings
    /// file untouched and raises the error alert.
    pub(super) fn persist_roots(&mut self, cx: &mut Context<Self>) {
        // Reaching a root is the end of the first run.
        self.mark_welcomed(cx);
        if let Err(err) = crate::settings::set_roots(&self.roots) {
            self.note_error(t().log_settings_save_failed(&err), cx);
        }
    }

    /// The first-run welcome is over: drop the welcome line and remember it.
    /// Called after the first root or repository.
    pub(super) fn mark_welcomed(&mut self, cx: &mut Context<Self>) {
        if self.welcomed {
            return;
        }
        self.welcomed = true;
        if let Err(err) = crate::settings::set_welcomed(true) {
            self.note_error(t().log_settings_save_failed(&err), cx);
        }
        cx.notify();
    }

    /// Persist the open tab list and the active tab (skipped for `--repo`
    /// launches and while remembered tabs are being reopened).
    pub(super) fn persist_tabs(&mut self, cx: &mut Context<Self>) {
        if !self.persist_tabs_enabled || self.restoring_tabs {
            return;
        }
        let repos: Vec<String> = self.tabs.iter().map(|key| key.0.clone()).collect();
        let active = self
            .active
            .and_then(|ix| self.tabs.get(ix))
            .map(|key| key.0.clone());
        if let Err(err) = crate::settings::set_open_repos(&repos, active.as_deref()) {
            self.note_error(t().log_settings_save_failed(&err), cx);
        }
    }

    /// After discovery, close restored tabs whose repository is no longer
    /// under any root; the others were already opened at launch.
    fn prune_restored_tabs(&mut self, cx: &mut Context<Self>) {
        let Some((saved, _)) = self.restore_tabs_pending.take() else {
            return;
        };
        // Only rows discovery found count: a known row survives rescans
        // because its tab is open, which must not exempt it from this prune.
        let discovered: Vec<String> = self
            .overview
            .rows()
            .iter()
            .filter(|row| !row.is_known())
            .map(|row| row.path.to_string_lossy().into_owned())
            .collect();
        for path in saved {
            if discovered.contains(&path) {
                continue;
            }
            if let Some(ix) = self.tabs.iter().position(|key| key.0 == path) {
                log!("tab: close missing {path}");
                self.close_tab(ix, cx);
            }
        }
    }

    /// Rebuild the render rows from the overview controller (live data).
    fn sync_repo_rows(&mut self) {
        let mut repos: Vec<RepoEntry> = self
            .overview
            .rows()
            .iter()
            .map(|row| {
                let collected = row.snapshot.as_ref();
                RepoEntry {
                    name: row.name.clone(),
                    path: row.path.clone(),
                    branch: collected.and_then(|c| c.snapshot.branch.clone()),
                    unborn: collected.is_some_and(|c| c.snapshot.unborn),
                    upstream: collected.and_then(|c| c.snapshot.upstream.clone()),
                    ahead: collected.map(|c| c.snapshot.ahead).unwrap_or(0),
                    behind: collected.map(|c| c.snapshot.behind).unwrap_or(0),
                    extra_branches: collected.map(|c| c.refs.extra_branches()).unwrap_or(0),
                    flags: row.flags(),
                    last_fetch: row.last_fetched.map(|at| ago_label(at.elapsed())),
                    last_collected: row.last_collected,
                    state: row.state.clone(),
                }
            })
            .collect();
        // Open tabs whose repository the workspace does not know yet (startup
        // restore before discovery, or a path outside the roots) keep their
        // row so the tab never flickers or vanishes.
        for key in &self.tabs {
            let known = repos.iter().any(|row| PathKey::new(&row.path) == *key);
            if !known
                && let Some(existing) = self
                    .repos
                    .iter()
                    .find(|row| PathKey::new(&row.path) == *key)
            {
                repos.push(existing.clone());
            }
        }
        self.repos = repos;
    }

    /// Manual refresh (app-bar button): one local status round over the whole
    /// workspace. Overlapping rounds are refused. The round also reloads the
    /// active history when refs moved, refreshes inspector details, and
    /// re-reads the open diff.
    pub(super) fn refresh_workspace(&mut self, cx: &mut Context<Self>) {
        if self.change_selection.is_some() {
            self.refresh_diff_on_finish = true;
        }
        self.start_refresh_round(cx);
    }

    fn start_refresh_round(&mut self, cx: &mut Context<Self>) {
        if self.discovering
            || self.overview.refresh_loop.in_flight()
            || self.overview.rows().is_empty()
        {
            // A rescan finishing while a round runs must not lose its round:
            // remember it and start when the current one finishes.
            if self.overview.refresh_loop.in_flight() {
                self.round_pending = true;
            }
            return;
        }
        self.overview.refresh_loop.round_started(Instant::now());
        // Rows with a targeted query already in flight (active poll or a
        // post-mutation refresh) are left to that query: starting another
        // would duplicate the Git work and stale the targeted ticket.
        let mut targets: Vec<(String, String)> = self
            .overview
            .rows()
            .iter()
            .filter(|row| !self.repo_refresh_in_flight.contains(&row.id))
            .map(|row| (row.id.clone(), row.path.to_string_lossy().into_owned()))
            .collect();
        // Active/open tabs are collected first so the rows the user is
        // looking at stream in before the rest of the workspace.
        // Stable sort keeps the workspace order within each
        // priority class.
        let active = self.active_repo_id();
        let open_tabs: HashSet<&str> = self.tabs.iter().map(|key| key.0.as_str()).collect();
        targets.sort_by_key(|(id, _)| {
            if active.as_deref() == Some(id.as_str()) {
                0
            } else if open_tabs.contains(id.as_str()) {
                1
            } else {
                2
            }
        });
        let started = Instant::now();
        let jobs = crate::process::jobs_from_env().unwrap_or(crate::process::DEFAULT_JOBS);
        log!("refresh: {} repositories (jobs {jobs})", targets.len());
        let mut tickets = HashMap::new();
        for (id, _) in &targets {
            if let Some(ticket) = self.overview.begin_refresh(id) {
                tickets.insert(id.clone(), ticket);
                // The round owns the row until it finishes: a targeted poll or
                // mutation refresh arriving meanwhile queues (with its diff
                // intent) instead of starting a duplicate read of the same
                // repository.
                self.repo_refresh_in_flight.insert(id.clone());
            }
        }
        self.sync_repo_rows();
        cx.notify();
        let receiver = crate::overview::spawn_status_round(targets);
        cx.spawn(async move |this, cx| {
            // `cx.spawn` runs on the UI executor: never block on `recv` there.
            // Drain what has arrived, apply it in one batch, then yield back to
            // the event loop until the next burst.
            let mut ok = 0usize;
            let mut failed = 0usize;
            let mut failures: Vec<String> = Vec::new();
            loop {
                let mut batch = Vec::new();
                let mut finished = false;
                loop {
                    match receiver.try_recv() {
                        Ok(item) => batch.push(item),
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            finished = true;
                            break;
                        }
                    }
                }
                if !batch.is_empty() {
                    let alive = this
                        .update(cx, |this, cx| {
                            for (id, result) in batch {
                                match &result {
                                    Ok(_) => ok += 1,
                                    Err(err) => {
                                        failed += 1;
                                        if failures.len() < 10 {
                                            failures.push(format!("{id}: {err}"));
                                        }
                                    }
                                }
                                if let Some(ticket) = tickets.get(&id) {
                                    this.overview
                                        .apply_result(*ticket, &id, result, Instant::now());
                                    // Release this row as soon as its result is
                                    // processed: a targeted or post-mutation
                                    // refresh must not wait for the slowest
                                    // repository in the round (SPEEDUP_REVIEW P2).
                                    this.repo_refresh_in_flight.remove(&id);
                                    if let Some(reload) = this.repo_refresh_pending.remove(&id) {
                                        this.refresh_repo(id.clone(), reload, cx);
                                    }
                                }
                            }
                            this.sync_repo_rows();
                            this.maybe_reload_history(cx);
                            cx.notify();
                        })
                        .is_ok();
                    if !alive {
                        return;
                    }
                }
                if finished {
                    this.update(cx, |this, cx| {
                        this.overview.refresh_loop.round_finished();
                        this.sync_repo_rows();
                        this.refresh_details_if_invalid(true, cx);
                        if std::mem::take(&mut this.refresh_diff_on_finish) {
                            this.reload_diff(cx);
                        }
                        let again = std::mem::take(&mut this.round_pending);
                        // Release the round's per-row ownership and run the
                        // targeted refreshes (and diff reloads) that were
                        // queued while it held them.
                        let pending: Vec<(String, bool)> = tickets
                            .keys()
                            .filter_map(|id| {
                                this.repo_refresh_in_flight.remove(id);
                                this.repo_refresh_pending
                                    .remove(id)
                                    .map(|reload| (id.clone(), reload))
                            })
                            .collect();
                        for (id, reload) in pending {
                            this.refresh_repo(id, reload, cx);
                        }
                        if again {
                            // A rescan asked for a round while this one ran.
                            this.start_refresh_round(cx);
                        }
                        cx.notify();
                    })
                    .ok();
                    for failure in &failures {
                        log!("refresh: failed {failure}");
                    }
                    if failed > failures.len() {
                        log!("refresh: ... {} more failures", failed - failures.len());
                    }
                    log!(
                        "refresh: {ok} ok, {failed} failed in {:.2}s",
                        started.elapsed().as_secs_f32()
                    );
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(80))
                    .await;
            }
        })
        .detach();
    }

    /// Full rescan: discovery for the current roots, then a status round for
    /// every discovered row. A rescan requested while one is running is queued
    /// instead of being silently dropped.
    pub(super) fn rescan_workspace(&mut self, cx: &mut Context<Self>) {
        if self.discovering {
            self.rescan_pending = true;
            self.ops.push_info(t().log_rescan_queued());
            cx.notify();
            return;
        }
        if self.roots.is_empty() {
            self.overview.load_workspace(&[]);
            self.sync_repo_rows();
            // Nothing can be discovered, so remembered tabs cannot reopen.
            self.restore_tabs_pending = None;
            cx.notify();
            return;
        }
        self.discovering = true;
        self.ops.push_info(t().log_scanning(self.roots.len()));
        cx.notify();
        let started = Instant::now();
        log!("scan: {} root(s)...", self.roots.len());
        let roots = self.roots.clone();
        let exclude = self.exclude.clone();
        cx.spawn(async move |this, cx| {
            let discovery = cx
                .background_executor()
                .spawn(async move { crate::discovery::discover_with_excludes(&roots, &exclude) })
                .await;
            this.update(cx, |this, cx| {
                this.discovering = false;
                log!(
                    "scan: {} repositories in {:.2}s",
                    discovery.repos.len(),
                    started.elapsed().as_secs_f32()
                );
                for diagnostic in &discovery.diagnostics {
                    log!("root: {diagnostic}");
                    this.note_error(t().log_root_diagnostic(diagnostic), cx);
                }
                this.overview.load_workspace(&discovery.repos);
                this.sync_repo_rows();
                // Restored tabs were opened at launch; drop the ones whose
                // repository is no longer under a root.
                this.prune_restored_tabs(cx);
                if !discovery.repos.is_empty() {
                    this.ops.push_info(t().log_discovered(discovery.repos.len()));
                }
                // Keep inspector details for repositories that survived the
                // scan; only vanished ones are dropped. The signature checks
                // refresh what actually changed instead of refetching every
                // sidebar/stash page.
                this.retain_workspace_details();
                if std::mem::take(&mut this.rescan_pending) {
                    this.rescan_workspace(cx);
                    return;
                }
                cx.notify();
                this.start_refresh_round(cx);
            })
            .ok();
        })
        .detach();
    }

    fn spawn_auto_refresh_tick(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let alive = this
                .update(cx, |this, cx| {
                    if !this.discovering
                        && !this.overview.rows().is_empty()
                        && this.overview.refresh_loop.should_start(Instant::now())
                    {
                        this.start_refresh_round(cx);
                    }
                })
                .is_ok();
            if !alive {
                return;
            }
        })
        .detach();
    }

    /// Fast poll for the repository the user is looking at: one status query
    /// every [`ACTIVE_REFRESH_INTERVAL`], so history, changes, diff, and
    /// details follow external edits without waiting for the full round.
    fn spawn_active_refresh_tick(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(ACTIVE_REFRESH_INTERVAL)
                .await;
            let alive = this
                .update(cx, |this, cx| {
                    // The active repository's status is not gated on the
                    // workspace scan: a restored/open path is already known
                    // and must show its state promptly.
                    if !this.auto_refresh {
                        return;
                    }
                    if let Some(id) = this.active_repo_id() {
                        this.poll_repo(id, cx);
                    }
                })
                .is_ok();
            if !alive {
                return;
            }
        })
        .detach();
    }

    /// Check GitHub for a newer release at startup and every few hours; each
    /// new version raises the update toast once. Failures (offline) only log.
    fn spawn_update_check(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            let found = cx
                .background_executor()
                .spawn(async { crate::update::check() })
                .await;
            let alive = this
                .update(cx, |this, cx| match found {
                    Ok(Some(release)) => {
                        if this.update.as_ref().map(|r| &r.version) != Some(&release.version) {
                            this.update = Some(release);
                            this.update_toast = true;
                            cx.notify();
                        }
                    }
                    Ok(None) => {}
                    Err(err) => log!("update check failed: {err}"),
                })
                .is_ok();
            if !alive {
                return;
            }
            cx.background_executor().timer(UPDATE_CHECK_INTERVAL).await;
        })
        .detach();
    }

    /// Download and swap in the available release, then restart into it.
    pub(super) fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some(release) = self.update.clone().filter(|_| !self.updating) else {
            return;
        };
        self.updating = true;
        self.update_toast = false;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    crate::update::install(&release).and_then(|app| crate::update::relaunch(&app))
                })
                .await;
            this.update(cx, |this, cx| match result {
                Ok(()) => cx.quit(),
                Err(err) => {
                    this.updating = false;
                    this.note_error(t().update_failed(&err), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Hide repositories from the workspace (persisted exclude prefixes).
    pub(super) fn exclude_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let before = self.exclude.len();
        for path in paths {
            if path.is_empty() || self.exclude.iter().any(|entry| entry == &path) {
                continue;
            }
            self.exclude.push(path);
        }
        if self.exclude.len() == before {
            cx.notify();
            return;
        }
        match crate::settings::set_exclude(&self.exclude) {
            Ok(()) => {
                self.ops.push_info(t().log_excluded(self.exclude.len() - before));
            }
            Err(err) => self.note_error(t().log_settings_save_failed(&err), cx),
        }
        self.rescan_workspace(cx);
        cx.notify();
    }

    /// Drop every workspace exclusion and rescan.
    pub(super) fn clear_excludes(&mut self, cx: &mut Context<Self>) {
        if self.exclude.is_empty() {
            cx.notify();
            return;
        }
        self.exclude.clear();
        match crate::settings::set_exclude(&self.exclude) {
            Ok(()) => {
                self.ops.push_info(t().log_excludes_cleared());
            }
            Err(err) => self.note_error(t().log_settings_save_failed(&err), cx),
        }
        self.rescan_workspace(cx);
        cx.notify();
    }

    /// Toggle one repository in the action selection set (Ctrl/Cmd-click).
    pub(super) fn select_repo(&mut self, id: &str, cx: &mut Context<Self>) {        if self.overview.is_selected(id) {
            self.overview.deselect(id);
        } else {
            self.overview.select(id, true);
        }
        cx.notify();
    }

    /// Toggle the persisted auto-refresh preference (palette action).
    pub(super) fn toggle_auto_refresh(&mut self, cx: &mut Context<Self>) {
        self.auto_refresh = !self.auto_refresh;
        self.overview.refresh_loop.set_enabled(self.auto_refresh);
        match crate::settings::set_auto_refresh(self.auto_refresh) {
            Ok(()) => {
                self.ops.push_info(t().log_auto_refresh(self.auto_refresh));
            }
            Err(err) => self.note_error(t().log_settings_save_failed(&err), cx),
        }
        if self.auto_refresh {
            self.start_refresh_round(cx);
        }
        cx.notify();
    }

    /// Cycle the workspace status filter (palette action).
    pub(super) fn cycle_filter(&mut self, cx: &mut Context<Self>) {
        let all = crate::model::Filter::ALL;
        let current = self.overview.filter();
        let next = all[(all.iter().position(|f| *f == current).unwrap_or(0) + 1) % all.len()];
        self.overview.set_filter(next);
        cx.notify();
    }

    /// Cycle the repository sort key (palette action).
    pub(super) fn cycle_sort(&mut self, cx: &mut Context<Self>) {
        let all = crate::overview::SortKey::ALL;
        let current = self.overview.sort();
        let next = all[(all.iter().position(|s| *s == current).unwrap_or(0) + 1) % all.len()];
        self.overview.set_sort(next);
        cx.notify();
    }

    /// After a scan, keep inspector details for repositories that are still
    /// discovered and drop only vanished ones.
    /// Keep inspector data for every repository still in the workspace:
    /// the discovered ones and the known rows kept for open tabs, whose
    /// sidebar counts would otherwise blank on each rescan.
    fn retain_workspace_details(&mut self) {
        let ids: HashSet<String> = self
            .overview
            .rows()
            .iter()
            .map(|row| row.path.to_string_lossy().into_owned())
            .collect();
        self.details.retain(|path, _| ids.contains(path));
        self.details_loading.retain(|path| ids.contains(path));
        self.details_order.retain(|path| ids.contains(path));
        self.details_checked_at.retain(|path, _| ids.contains(path));
        self.details_signature.retain(|path, _| ids.contains(path));
    }

    /// Drop one repository's cached inspector data and freshness records.
    pub(super) fn drop_cached_details(&mut self, path: &str) {
        self.details.remove(path);
        self.details_loading.remove(path);
        self.details_order.retain(|cached| cached != path);
        self.details_checked_at.remove(path);
        self.details_signature.remove(path);
    }

    /// LRU-budget bookkeeping after a details load: the active repository is
    /// most recent; repository sets beyond [`DETAILS_CACHE_MAX`] are dropped
    /// oldest first.
    fn touch_details_lru(&mut self, path: &str) {
        self.details_order.retain(|cached| cached != path);
        self.details_order.push_back(path.to_string());
        while self.details_order.len() > DETAILS_CACHE_MAX {
            if let Some(oldest) = self.details_order.pop_front() {
                self.details.remove(&oldest);
                self.details_checked_at.remove(&oldest);
                self.details_signature.remove(&oldest);
            }
        }
    }

    /// Load remotes/tags/stashes for the active repository (inspector data).
    /// `force` re-reads while keeping the current values visible until the
    /// new ones arrive, so a refresh never blanks the sidebar counters.
    fn refresh_active_details(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(path) = self
            .active_repo()
            .map(|row| row.path.to_string_lossy().into_owned())
        else {
            return;
        };
        if self.details_loading.contains(&path) || (!force && self.details.contains_key(&path)) {
            return;
        }
        // Recorded with the load so a later freshness check can tell whether
        // refs moved while the values were cached.
        let signature = self.current_history_signature();
        self.details_loading.insert(path.clone());
        cx.spawn(async move |this, cx| {
            let load_path = path.clone();
            let details = cx
                .background_executor()
                .spawn(async move { crate::git::repo_details(&load_path) })
                .await;
            this.update(cx, |this, cx| {
                this.details_loading.remove(&path);
                match details {
                    Ok(details) => {
                        log!("details: loaded {path}");
                        // The selected stash label may now point at another
                        // object (an external stash was created/dropped): the
                        // view is reloaded for the new object, or dropped when
                        // the entry is gone. Reading a stale `stash@{n}` would
                        // otherwise show one entry under another's identity.
                        let selected_label = this.selected_stash.clone();
                        let selected_object_now = selected_label.as_ref().and_then(|id| {
                            details
                                .stashes
                                .iter()
                                .find(|(entry, _, _)| entry == id)
                                .map(|(_, object, _)| object.clone())
                        });
                        let selected_object_was = this.selected_stash_object.clone();
                        this.details.insert(path.clone(), details);
                        this.touch_details_lru(&path);
                        this.details_checked_at.insert(path.clone(), Instant::now());
                        if let Some(signature) = signature.clone() {
                            this.details_signature.insert(path.clone(), signature);
                        }
                        // The Remotes list reads from the cached rows.
                        this.rebuild_remote_rows();
                        if let Some(label) = selected_label {
                            match selected_object_now {
                                None => this.reset_stash_view(),
                                Some(object) if Some(&object) != selected_object_was.as_ref() => {
                                    // Force the reload: the guard keys on the
                                    // label, which did not change.
                                    this.stash_files_for = None;
                                    this.select_stash(label, cx);
                                }
                                Some(_) => {}
                            }
                        }
                    }
                    Err(err) => {
                        log!("details: {path}: {err}");
                        this.note_error(t().log_details_failed(&err), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Refresh the active repository's inspector data only when something it
    /// covers actually changed — the HEAD+refs signature (branches/tags), the
    /// status snapshot's stash count, or the slow fallback interval. The
    /// 15 s round used to force a full re-read every time.
    fn refresh_details_if_invalid(&mut self, allow_fallback: bool, cx: &mut Context<Self>) {
        let Some(id) = self.active_repo_id() else {
            return;
        };
        if self.details_loading.contains(&id) {
            return;
        }
        let signature = self.current_history_signature();
        let signature_moved = match (self.details_signature.get(&id), &signature) {
            (Some(loaded), Some(current)) => loaded != current,
            _ => false,
        };
        let stash_count_moved = match (self.active_snapshot(), self.active_details()) {
            (Some(collected), Some(details)) => {
                details.stashes.len() != collected.snapshot.stash as usize
            }
            (Some(_), None) => true, // no data yet: load it
            _ => false,
        };
        let fallback_due = allow_fallback
            && self
                .details_checked_at
                .get(&id)
                .is_none_or(|checked| checked.elapsed() >= DETAILS_FALLBACK_INTERVAL);
        if signature_moved || stash_count_moved || fallback_due {
            self.refresh_active_details(true, cx);
        }
    }

    /// Copy an absolute repository path to the clipboard (palette action).
    pub(super) fn copy_path(&mut self, path: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path.to_string()));
        self.ops.push_info(t().log_copied_path(path));
        cx.notify();
    }

    /// Append an operation-log line and raise it as the transient bottom-left
    /// error alert. The lifetime comes from settings ("3s" / "5s" / "10s" /
    /// click-to-remove). Informational lines only go to the log.
    pub(super) fn note_error(&mut self, line: String, cx: &mut Context<Self>) {
        let id = self.ops.push_info(line);
        self.raise_toast(id, cx);
    }

    /// Point the transient alert at one log entry (used by the pump for
    /// failed operations, which carry their repo and result).
    pub(super) fn raise_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        self.ops_toast_gen = self.ops_toast_gen.wrapping_add(1);
        let generation = self.ops_toast_gen;
        self.ops_toast = Some(id);
        cx.notify();
        let Some(timeout) = self.alert_timeout.duration() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(timeout).await;
            this.update(cx, |this, cx| {
                if this.ops_toast_gen == generation {
                    this.ops_toast = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Close the current error alert early (its close button).
    pub(super) fn dismiss_alert(&mut self, cx: &mut Context<Self>) {
        if self.ops_toast.take().is_some() {
            // Invalidate a pending expiry timer along with the alert.
            self.ops_toast_gen = self.ops_toast_gen.wrapping_add(1);
            cx.notify();
        }
    }

    /// Fade a request modal in (called by every `request_*` opener).
    pub(super) fn open_modal(&mut self, cx: &mut Context<Self>) {
        self.modal_gen = self.modal_gen.wrapping_add(1);
        self.modal_fade
            .retarget(1.0, motion::MENU_IN, cx.reduce_motion(), Instant::now());
        cx.notify();
    }

    /// Fade the open modal out, then clear the request so the exit is visible.
    /// Every request slot is cleared together (only one modal is ever open).
    pub(super) fn close_modal(&mut self, cx: &mut Context<Self>) {
        self.modal_gen = self.modal_gen.wrapping_add(1);
        let generation = self.modal_gen;
        self.modal_fade
            .retarget(0.0, motion::MENU_OUT, cx.reduce_motion(), Instant::now());
        if cx.reduce_motion() {
            self.clear_modal_requests();
            cx.notify();
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(motion::MENU_OUT).await;
            this.update(cx, |this, cx| {
                if this.modal_gen == generation {
                    this.clear_modal_requests();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn clear_modal_requests(&mut self) {
        self.tag_push_request = None;
        self.tag_delete_request = None;
        self.edit_url_request = None;
        self.rename_remote_request = None;
        self.remove_remote_request = None;
        self.profile_request = None;
        self.discard_confirm = None;
        self.stash_request = None;
        self.stash_drop_confirm = None;
        self.apply_patch_request = None;
        self.shortcuts_open = false;
        self.stash_branch_request = None;
        self.branch_request = None;
        self.tag_request = None;
        self.rename_request = None;
        self.rebase_request = None;
        self.branch_delete_request = None;
        self.branch_delete_refused = None;
        self.remote_request = None;
        self.push_request = None;
        self.fetch_request = None;
        self.pull_request = None;
        self.reset_request = None;
        self.discarded_open = false;
        self.discarded_expanded = None;
        self.discarded_file = None;
        self.hunk_discard = None;
    }

    /// Invoke the configured WSL-aware external client with the repository
    /// path as the final argument.
    pub(super) fn open_externally(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(client) = self.external_client.clone() else {
            self.note_error(t().log_external_not_configured(), cx);
            cx.notify();
            return;
        };
        let program = client.program.clone();
        match client.command_for(path).spawn() {
            Ok(_) => {
                self.ops.push_info(t().log_external_started(&program));
            }
            Err(err) => self.note_error(t().log_external_failed(&err.to_string()), cx),
        }
        cx.notify();
    }

    /// Snapshot for the active repository (Local Changes / Local Branches).
    pub(super) fn active_snapshot(&self) -> Option<&crate::git::CollectedStatus> {
        let row = self.active_repo()?;
        let id = row.path.to_string_lossy();
        self.overview.row(id.as_ref())?.snapshot.as_ref()
    }

    /// The branchling's face for this frame: a network
    /// operation the user started turns it to Focus; Look and Happy arrive as
    /// timed holds; everything else rests.
    fn brand_face(&self) -> brand::Face {
        brand::face_for(self.network_op_running(), self.face_hold, Instant::now())
    }
    /// True while a queued fetch, pull or push is running. Auto-refresh never
    /// qualifies: it is local inspection and never becomes a running op.
    fn network_op_running(&self) -> bool {
        self.running_op.as_ref().is_some_and(|running| running.network)
    }

    /// Happy's condition: every known repository has
    /// fresh data, no local changes and nothing behind.
    fn workspace_is_clean_and_current(&self) -> bool {
        let rows = self.overview.rows();
        !rows.is_empty()
            && rows.iter().all(|row| {
                row.snapshot.as_ref().is_some_and(|collected| {
                    collected.snapshot.behind == 0 && collected.snapshot.is_clean()
                })
            })
    }

    /// Show `face` for `hold`, then rest again. One timer per hold asks for
    /// the repaint that ends it — no per-frame checks. A newer hold replaces
    /// an older one, and the older timer cannot clear it (it checks the end
    /// time it was created with).
    fn hold_face(&mut self, face: brand::Face, hold: Duration, cx: &mut Context<Self>) {
        self.face_hold = Some((face, Instant::now() + hold));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(hold).await;
            this.update(cx, |this, cx| {
                if this
                    .face_hold
                    .is_some_and(|(_, until)| Instant::now() >= until)
                {
                    this.face_hold = None;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Rebuild the flattened Local Branches tree (status refresh or a folder
    /// toggle). Cheap: the branch list is small. Pinned branches render in
    /// pin order above the tree; the tree flattens the rest.
    pub(super) fn rebuild_branch_rows(&mut self) {
        let repo = self
            .active_repo()
            .map(|repo| repo.path.to_string_lossy().into_owned());
        let pinned: Vec<crate::status::BranchInfo> = self
            .pinned_branches
            .iter()
            .filter_map(|key| {
                let (key_repo, name) = crate::settings::split_pin(key)?;
                (Some(key_repo) == repo.as_deref()).then(|| {
                    self.branches.iter().find(|branch| branch.name == name)
                })?
            })
            .cloned()
            .collect();
        let pinned_names: std::collections::HashSet<&str> = pinned
            .iter()
            .map(|branch| branch.name.as_str())
            .collect();
        let rest: Vec<crate::status::BranchInfo> = self
            .branches
            .iter()
            .filter(|branch| !pinned_names.contains(branch.name.as_str()))
            .cloned()
            .collect();
        let animating = self.animating_folders("b/");
        self.pinned_branch_rows = Rc::new(pinned);
        self.branch_tree_branches = Rc::new(rest);
        let tree = self.branch_tree_branches.clone();
        self.branch_rows = Rc::new(branch_tree::flatten(
            &tree,
            &self.branch_folders_closed,
            &animating,
            self.branch_sort,
        ));
        self.rebuild_remote_rows();
    }

    /// Rebuild the flattened Remotes rows (ref/details refresh, folder
    /// toggle, or a remote animation settling). Cached otherwise.
    pub(super) fn rebuild_remote_rows(&mut self) {
        let remotes = self
            .active_details()
            .map(|details| details.remotes.clone())
            .unwrap_or_default();
        let animating = self.animating_folders("r/");
        self.remote_rows = Rc::new(branch_tree::remote_rows(
            &remotes,
            &self.remote_branches,
            &self.remote_folders_closed,
            &animating,
        ));
    }

    /// Current open fraction (0..1) of one sidebar folder; folders without an
    /// animation entry are fully open.
    pub(super) fn folder_fade(&self, key: &str, now: Instant) -> f32 {
        self.folder_fades
            .get(key)
            .map(|fade| fade.value_at(now))
            .unwrap_or(1.0)
    }

    /// Start (or retarget) a folder's expand/collapse animation with the
    /// collapsible's own motion vocabulary.
    pub(super) fn set_folder_fade(&mut self, key: &str, open: bool, cx: &mut Context<Self>) {
        let now = Instant::now();
        let entry = self
            .folder_fades
            .entry(key.to_string())
            .or_insert_with(|| motion::Transition::settled(if open { 0.0 } else { 1.0 }));
        entry.retarget_with(
            motion::Curve::Menu,
            if open { 1.0 } else { 0.0 },
            Duration::from_millis(150),
            cx.reduce_motion(),
            now,
        );
    }

    /// Closed folders whose collapse animation is still running: their rows
    /// stay in the flattened list until the fade settles.
    fn animating_folders(&self, prefix: &str) -> HashSet<String> {
        let now = Instant::now();
        self.folder_fades
            .iter()
            .filter(|(_, fade)| fade.value_at(now) > 0.0)
            .filter_map(|(key, _)| key.strip_prefix(prefix).map(str::to_string))
            .collect()
    }

    /// Drop settled folder fades and re-flatten once a closing folder's rows
    /// can leave the list. Called once per frame by the repository view.
    fn settle_folder_fades(&mut self) {
        let now = Instant::now();
        let before = self.folder_fades.len();
        self.folder_fades.retain(|_, fade| fade.is_animating(now));
        if self.folder_fades.len() != before {
            self.rebuild_branch_rows();
        }
    }

    /// HEAD + refs signature of the active repository's last snapshot.
    pub(super) fn current_history_signature(&self) -> Option<(Option<String>, u64)> {
        self.active_snapshot()
            .map(crate::git::CollectedStatus::history_signature)
    }

    /// Rebuild the active repository's virtualized display rows only when the
    /// row or its query serial changed (a 5 000-entry Local Changes list is
    /// not rebuilt on every frame).
    pub(super) fn sync_detail_lists(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self
            .active_repo()
            .map(|row| row.path.to_string_lossy().into_owned())
        else {
            self.change_lists = Rc::new(crate::status::ChangeLists::default());
            self.branches = Rc::new(Vec::new());
            self.remote_branches = Rc::new(Vec::new());
            self.detail_lists_for = None;
            self.rebuild_change_rows(cx);
            return;
        };
        let serial = self.overview.row_revision(&id);
        if self.detail_lists_for.as_ref() == Some(&(id.clone(), serial)) {
            return;
        }
        let (lists, branches, remote_branches) = match self.active_snapshot() {
            Some(collected) => (
                crate::status::change_lists(&collected.snapshot),
                collected.refs.branches.clone(),
                collected.refs.remote_branches.clone(),
            ),
            None => (
                crate::status::ChangeLists::default(),
                Vec::new(),
                Vec::new(),
            ),
        };
        self.change_lists = Rc::new(lists);
        self.branches = Rc::new(branches);
        self.remote_branches = Rc::new(remote_branches);
        self.rebuild_branch_rows();
        self.detail_lists_for = Some((id, serial));
        self.rebuild_change_rows(cx);
        // A refresh can remove rows (staged, discarded, committed): drop them
        // from the multi-selection and clear the diff focus when it is gone.
        self.change_selected
            .retain(|selection| self.change_lists.contains(&selection.path, selection.staged));
        if self
            .change_selection
            .as_ref()
            .is_some_and(|selection| !self.change_lists.contains(&selection.path, selection.staged))
        {
            self.change_selection = None;
        }
        // The selection's conflict flag follows the refreshed lists: a path
        // that is no longer unmerged leaves the resolver for a normal diff.
        if let Some(selection) = self.change_selection.clone() {
            let items = if selection.staged {
                &self.change_lists.staged
            } else {
                &self.change_lists.unstaged
            };
            let conflicted = items
                .iter()
                .find(|item| item.path == selection.path)
                .map(|item| item.kind == crate::status::Change::Unmerged);
            if let Some(conflicted) = conflicted.filter(|flag| *flag != selection.conflicted) {
                let updated = changes::Selection {
                    conflicted,
                    ..selection
                };
                self.change_selection = Some(updated.clone());
                for selected in &mut self.change_selected {
                    if selected.path == updated.path && selected.staged == updated.staged {
                        selected.conflicted = conflicted;
                    }
                }
                if !conflicted {
                    self.conflict_view = None;
                }
                self.load_diff(cx);
            }
        }
    }

    /// Rebuild the Local Changes rows for the current filter text. The
    /// published model shares the change-list snapshot it filters.
    pub(super) fn rebuild_change_rows(&mut self, cx: &App) {
        let filter = self.change_filter.read(cx).value().to_string();
        self.change_rows =
            Rc::new(changes::build_rows(self.change_lists.clone(), &filter));
    }

    /// Inspector details for the active repository, if loaded.
    pub(super) fn active_details(&self) -> Option<&crate::git::RepoDetails> {
        let row = self.active_repo()?;
        self.details.get(&row.path.to_string_lossy().into_owned())
    }

    /// Object id of a stash entry by its `stash@{n}` ref (stable cache key).
    pub(super) fn stash_object(&self, id: &str) -> Option<String> {
        self.active_details()?
            .stashes
            .iter()
            .find(|(entry, _, _)| entry == id)
            .map(|(_, object, _)| object.clone())
    }

    /// Native folder picker (IFileOpenDialog) to add scan roots.
    /// Picked `\\wsl.localhost\…` paths become Linux paths automatically.
    fn browse_for_root(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some(t().select_roots_prompt().into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |this, cx| {
                    for p in paths {
                        match to_root_string(&p.to_string_lossy()) {
                            Ok(root) if !root.is_empty() && !this.roots.contains(&root) => {
                                this.ops.push_info(t().log_scan_root_added(&root));
                                this.roots.push(root);
                            }
                            Ok(_) => {}
                            Err(distro) => {
                                this.note_error(t().log_foreign_distro(&distro), cx);
                            }
                        }
                    }
                    this.persist_roots(cx);
                    this.rescan_workspace(cx);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Explorer drops: a folder that is itself a Git worktree opens as a
    /// repository tab; any other folder becomes a scan root.
    fn handle_drop(&mut self, paths: &gpui_kit::ExternalPaths, cx: &mut Context<Self>) {
        for p in paths.paths() {
            let raw = p.to_string_lossy().into_owned();
            // `.git` may be a directory or a file (linked worktrees); `exists`
            // covers both and works over `\\wsl.localhost` too.
            if p.join(".git").exists() {
                self.import_dropped_repo(&raw, cx);
            } else {
                match to_root_string(&raw) {
                    Ok(root) if !root.is_empty() && !self.roots.contains(&root) => {
                        self.ops.push_info(t().log_scan_root_added(&root));
                        self.roots.push(root);
                    }
                    Ok(_) => {}
                    Err(distro) => self.note_error(t().log_foreign_distro(&distro), cx),
                }
            }
        }
        self.persist_roots(cx);
        self.rescan_workspace(cx);
        cx.notify();
    }

    fn import_dropped_repo(&mut self, raw: &str, cx: &mut Context<Self>) {
        let path_str = match to_root_string(raw) {
            Ok(path) if !path.is_empty() => path,
            Ok(_) => return,
            Err(distro) => {
                self.note_error(t().log_foreign_distro(&distro), cx);
                return;
            }
        };
        self.ops.push_info(t().log_repo_dropped(&path_str));
        self.open_repo_path(&path_str, cx);
    }

    /// Capture the focus that should return when the overlay closes. Called
    /// only when opening from a fully closed state, so a palette <-> roots
    /// switch keeps the original destination.
    fn remember_dialog_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.palette_open && !self.roots_open && !self.palette_closing {
            self.dialog_previous_focus = window.focused(cx);
        }
    }

    fn restore_dialog_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .dialog_previous_focus
            .take()
            .unwrap_or_else(|| self.root_focus.clone());
        window.focus(&target, cx);
    }

    /// Aim the shared card at a dialog width; a no-op when already aimed there
    /// so plain open/close never replays the morph.
    fn set_dialog_width(&mut self, target: f32, cx: &mut Context<Self>) {
        if (self.dialog_width.target() - target).abs() > 0.5 {
            self.dialog_width.retarget_with(
                motion::Curve::SoftBack,
                target,
                Duration::from_millis(170),
                cx.reduce_motion(),
                Instant::now(),
            );
        }
    }

    /// Shared dialog overlay for the palette and the roots manager.
    ///
    /// Both dialogs render inside ONE overlay: switching between them never
    /// unmounts the scrim/background, and the card width morphs between the
    /// two sizes with a soft bounce. Scrim opacity, panel
    /// opacity, panel movement, and card width all derive from
    /// interruption-safe values, so a reopen mid-close continues from where it
    /// was instead of replaying an entrance.
    fn render_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::Div {
        let now = Instant::now();
        let t = self.dialog_fade.value_at(now);
        let width = self.dialog_width.value_at(now);
        let roots_mode = self.roots_open;
        let card = if roots_mode {
            self.render_roots_card(window, cx)
        } else {
            self.render_palette_card(window, cx)
        };
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0.0, 0.0, 0.0, 0.35 * t))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.roots_open {
                        this.close_roots_to_palette(cx);
                    } else {
                        this.close_palette(window, cx);
                    }
                }),
            )
            .child(
                div()
                    .relative()
                    .opacity(0.3 + 0.7 * t)
                    .top(px(-2.0 * (1.0 - t)))
                    .child(card.w(px(width))),
            )
    }
}

impl Render for SpurShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        self.viewport_w = f32::from(window.viewport_size().width);
        // Menu explainer drive: reveal after the hover delay; retire once the
        // hovered row stops prepainting (its menu closed without a leave).
        // The row resets `unseen_frames` every frame it paints.
        if let Some(mut state) = self.explainer.take() {
            state.unseen_frames = state.unseen_frames.saturating_add(1);
            if state.unseen_frames <= explainer::UNSEEN_FRAME_LIMIT {
                if state.since.elapsed() >= explainer::EXPLAINER_DELAY {
                    state.visible = true;
                }
                window.request_animation_frame();
                self.explainer = Some(state);
            }
        }
        // Commit-message bookkeeping that needs a Window: clearing after an
        // async success.
        if self.commit_clear_pending {
            self.commit_clear_pending = false;
            self.commit_message
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
        // The exit fade landed: drop the overlay this frame instead of hiding
        // it on a timer (a reopen can no longer race a stale hide).
        if self.palette_closing && !self.dialog_fade.is_animating(now) {
            self.palette_closing = false;
            self.palette_open = false;
        }
        let has_repo = self.active_repo().is_some();
        let modal_t = self.modal_fade.value_at(now);
        let overlay = (self.palette_open || self.roots_open)
            .then(|| self.render_dialog(window, cx).into_any_element());
        let discard_overlay = self.discard_confirm.clone().map(|request| {
            modal_fade_layer(self.render_discard_confirm(request, cx).into_any_element(), modal_t)
        });
        let stash_overlay = self.stash_request.clone().map(|request| {
            modal_fade_layer(self.render_stash_dialog(request, cx).into_any_element(), modal_t)
        });
        let stash_drop_overlay = self.stash_drop_confirm.clone().map(|id| {
            modal_fade_layer(self.render_stash_drop_confirm(id, cx).into_any_element(), modal_t)
        });
        let apply_patch_overlay = self.apply_patch_request.clone().map(|request| {
            modal_fade_layer(
                self.render_apply_patch_dialog(request, cx).into_any_element(),
                modal_t,
            )
        });
        let shortcuts_overlay = self.shortcuts_open.then(|| {
            modal_fade_layer(self.render_shortcuts_dialog(cx).into_any_element(), modal_t)
        });
        let stash_branch_overlay = self.stash_branch_request.clone().map(|request| {
            modal_fade_layer(
                self.render_stash_branch_dialog(request, cx).into_any_element(),
                modal_t,
            )
        });
        let branch_overlay = self.branch_request.clone().map(|request| {
            modal_fade_layer(self.render_branch_dialog(request, cx).into_any_element(), modal_t)
        });
        let rename_overlay = self.rename_request.clone().map(|request| {
            modal_fade_layer(self.render_rename_dialog(request, cx).into_any_element(), modal_t)
        });
        let rebase_overlay = self.rebase_request.clone().map(|request| {
            modal_fade_layer(self.render_rebase_dialog(request, cx).into_any_element(), modal_t)
        });
        let branch_delete_overlay = self.branch_delete_request.clone().map(|request| {
            modal_fade_layer(
                self.render_branch_delete_confirm(request, cx).into_any_element(),
                modal_t,
            )
        });
        let branch_force_overlay = self.branch_delete_refused.clone().map(|refused| {
            modal_fade_layer(
                self.render_branch_force_confirm(refused, cx).into_any_element(),
                modal_t,
            )
        });
        let tag_overlay = self.tag_request.clone().map(|request| {
            modal_fade_layer(self.render_tag_dialog(request, cx).into_any_element(), modal_t)
        });
        let tag_push_overlay = self.tag_push_request.clone().map(|request| {
            modal_fade_layer(
                self.render_tag_push_dialog(request, cx).into_any_element(),
                modal_t,
            )
        });
        let tag_delete_overlay = self.tag_delete_request.clone().map(|request| {
            modal_fade_layer(
                self.render_tag_delete_dialog(request, cx).into_any_element(),
                modal_t,
            )
        });
        let remote_overlay = self.remote_request.clone().map(|request| {
            modal_fade_layer(self.render_add_remote_dialog(request, cx).into_any_element(), modal_t)
        });
        let edit_url_overlay = self.edit_url_request.clone().map(|request| {
            modal_fade_layer(
                self.render_edit_url_dialog(request, cx).into_any_element(),
                modal_t,
            )
        });
        let rename_remote_overlay = self.rename_remote_request.clone().map(|request| {
            modal_fade_layer(
                self.render_rename_remote_dialog(request, cx).into_any_element(),
                modal_t,
            )
        });
        let remove_remote_overlay = self.remove_remote_request.clone().map(|request| {
            modal_fade_layer(
                self.render_remove_remote_dialog(request, cx).into_any_element(),
                modal_t,
            )
        });
        let profile_overlay = self.profile_request.clone().map(|request| {
            modal_fade_layer(self.render_profile_dialog(request, cx).into_any_element(), modal_t)
        });
        let push_overlay = self.push_request.clone().map(|request| {
            modal_fade_layer(self.render_push_dialog(request, cx).into_any_element(), modal_t)
        });
        let fetch_overlay = self.fetch_request.clone().map(|request| {
            modal_fade_layer(self.render_fetch_dialog(request, cx).into_any_element(), modal_t)
        });
        let pull_overlay = self.pull_request.clone().map(|request| {
            modal_fade_layer(self.render_pull_dialog(request, cx).into_any_element(), modal_t)
        });
        let reset_overlay = self.reset_request.clone().map(|request| {
            modal_fade_layer(
                self.render_reset_dialog(request, cx).into_any_element(),
                modal_t,
            )
        });
        let discarded_overlay = self.discarded_open.then(|| {
            modal_fade_layer(
                self.render_discarded_modal(cx).into_any_element(),
                modal_t,
            )
        });
        let hunk_discard_overlay = self.hunk_discard.clone().map(|request| {
            modal_fade_layer(self.render_hunk_discard(request, cx).into_any_element(), modal_t)
        });
        let oplog_overlay = self.oplog_open.then(|| self.render_oplog(cx));

        let content: gpui_kit::AnyElement = if self.settings_open {
            self.render_settings(cx).into_any_element()
        } else if has_repo {
            self.render_repo_view(cx).into_any_element()
        } else {
            self.render_empty_state(window, cx).into_any_element()
        };

        // Error alert: newest failed/refused operation line, bottom-left. The
        // lifetime follows the settings choice; the click-to-remove mode adds
        // its close button inside the card. "Details" opens the operation
        // log panel at the entry.
        let manual_dismiss = self.alert_timeout == crate::settings::AlertTimeout::Manual;
        let error_toast = self.ops_toast.and_then(|id| {
            let line = self.ops.entry(id)?.detail.clone();
            let generation = self.ops_toast_gen;
            Some(
                div()
                    .max_w(px(560.))
                    .rounded(px(PANEL_RADIUS))
                    .border_1()
                    .border_color(cx.theme().danger.alpha(0.45))
                    .bg(cx.theme().popover)
                    .shadow_lg()
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .overflow_hidden()
                    .child(
                        Icon::new(IconName::CircleAlert)
                            .size(px(14.))
                            .flex_none()
                            .text_color(cx.theme().danger),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(TEXT_SM))
                            .text_color(text_primary(cx))
                            .child(line),
                    )
                    .child(
                        div()
                            .id("alert-details")
                            .flex_none()
                            .cursor_pointer()
                            .text_size(px(TEXT_XS))
                            .text_color(violet(cx))
                            .child(t().oplog_details)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_oplog(Some(id), cx)
                            })),
                    )
                    .children(manual_dismiss.then(|| {
                    div()
                        .id("alert-dismiss")
                        .aria_label(t().alert_dismiss)
                        .cursor_pointer()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(18.))
                        .h(px(18.))
                        .rounded(px(4.))
                        .bg(hover_blend("alert-dismiss", ink(0.0), ink(0.10)))
                        .on_hover(hover_listener("alert-dismiss"))
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_alert(cx)))
                        .child(
                            Icon::new(IconName::Close)
                                .size(px(12.))
                                .text_color(text_muted(cx)),
                        )
                }))
                .with_animation(
                    SharedString::from(format!("op-error-{generation}")),
                    motion::animation(motion::MENU_IN),
                    |el, t| el.opacity(t),
                )
            )
        });

        // Undo alert: info-toned card for the last undoable op, with an
        // Undo button next to Details. Same lifetime setting as errors.
        let undo_card = self.undo_toast.clone().and_then(|toast| {
            let line = self.ops.entry(toast.log_id)?.detail.clone();
            let generation = self.undo_toast_gen;
            let repo_id = toast.repo_id.clone();
            let log_id = toast.log_id;
            Some(
                div()
                    .max_w(px(560.))
                    .rounded(px(PANEL_RADIUS))
                    .border_1()
                    .border_color(cx.theme().success.alpha(0.45))
                    .bg(cx.theme().popover)
                    .shadow_lg()
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .overflow_hidden()
                    .child(
                        Icon::new(IconName::Undo)
                            .size(px(14.))
                            .flex_none()
                            .text_color(cx.theme().success),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(TEXT_SM))
                            .text_color(text_primary(cx))
                            .child(line),
                    )
                    .child(
                        div()
                            .id("undo-toast-action")
                            .flex_none()
                            .cursor_pointer()
                            .text_size(px(TEXT_XS))
                            .text_color(violet(cx))
                            .child(t().undo_action())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.queue_undo_for(repo_id.clone(), cx)
                            })),
                    )
                    .child(
                        div()
                            .id("undo-toast-details")
                            .flex_none()
                            .cursor_pointer()
                            .text_size(px(TEXT_XS))
                            .text_color(violet(cx))
                            .child(t().oplog_details)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_oplog(Some(log_id), cx)
                            })),
                    )
                    .children(manual_dismiss.then(|| {
                    div()
                        .id("undo-toast-dismiss")
                        .aria_label(t().alert_dismiss)
                        .cursor_pointer()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(18.))
                        .h(px(18.))
                        .rounded(px(4.))
                        .bg(hover_blend("undo-toast-dismiss", ink(0.0), ink(0.10)))
                        .on_hover(hover_listener("undo-toast-dismiss"))
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_undo_toast(cx)))
                        .child(
                            Icon::new(IconName::Close)
                                .size(px(12.))
                                .text_color(text_muted(cx)),
                        )
                }))
                .with_animation(
                    SharedString::from(format!("op-undo-{generation}")),
                    motion::animation(motion::MENU_IN),
                    |el, t| el.opacity(t),
                )
            )
        });

        // Update offer: stays until dismissed; the action opens Settings,
        // where the install button lives.
        let update_card = self.update.as_ref().filter(|_| self.update_toast).map(|release| {
            div()
                .max_w(px(560.))
                .rounded(px(PANEL_RADIUS))
                .border_1()
                .border_color(cx.theme().info.alpha(0.45))
                .bg(cx.theme().popover)
                .shadow_lg()
                .px(px(12.))
                .py(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .overflow_hidden()
                .child(
                    Icon::new(IconName::Download)
                        .size(px(14.))
                        .flex_none()
                        .text_color(cx.theme().info),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(TEXT_SM))
                        .text_color(text_primary(cx))
                        .child(t().update_available(&release.version)),
                )
                .child(
                    div()
                        .id("update-toast-action")
                        .flex_none()
                        .cursor_pointer()
                        .text_size(px(TEXT_XS))
                        .text_color(violet(cx))
                        .child(t().update_open_settings)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.update_toast = false;
                            this.open_settings(window, cx);
                        })),
                )
                .child(
                    div()
                        .id("update-toast-dismiss")
                        .aria_label(t().alert_dismiss)
                        .cursor_pointer()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(18.))
                        .h(px(18.))
                        .rounded(px(4.))
                        .bg(hover_blend("update-toast-dismiss", ink(0.0), ink(0.10)))
                        .on_hover(hover_listener("update-toast-dismiss"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.update_toast = false;
                            cx.notify();
                        }))
                        .child(
                            Icon::new(IconName::Close)
                                .size(px(12.))
                                .text_color(text_muted(cx)),
                        ),
                )
                .with_animation(
                    "update-toast",
                    motion::animation(motion::MENU_IN),
                    |el, t| el.opacity(t),
                )
        });

        let root = div()
            .id("root")
            .track_focus(&self.root_focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_size(px(TEXT_MD))
            .on_action(cx.listener(|this, _: &TogglePalette, window, cx| {
                this.toggle_palette(window, cx);
            }))
            .on_action(cx.listener(|this, _: &oplog::ToggleOpLog, _, cx| {
                this.toggle_oplog(cx);
            }))
            .on_action(cx.listener(|this, _: &shortcuts::UndoLast, _, cx| {
                this.undo_active_repo(cx);
            }))
            .on_action(cx.listener(|this, _: &shortcuts::ShowShortcuts, window, cx| {
                this.toggle_shortcuts(window, cx);
            }))
            .on_action(cx.listener(|this, _: &shortcuts::ShowHistory, window, cx| {
                this.show_history(window, cx);
            }))
            .on_action(cx.listener(|this, _: &shortcuts::ShowChanges, window, cx| {
                this.show_changes(window, cx);
            }))
            .on_action(cx.listener(|this, _: &shortcuts::ShowStashes, window, cx| {
                this.show_stashes(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                this.toggle_settings(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ClosePalette, window, cx| {
                if this.hunk_discard.is_some() {
                    this.cancel_hunk_discard(cx);
                    return;
                }
                if this.branch_request.is_some() {
                    this.cancel_create_branch(cx);
                    return;
                }
                if this.tag_request.is_some() {
                    this.cancel_create_tag(cx);
                    return;
                }
                if this.tag_push_request.is_some() {
                    this.cancel_tag_push(cx);
                    return;
                }
                if this.tag_delete_request.is_some() {
                    this.cancel_tag_delete(cx);
                    return;
                }
                if this.edit_url_request.is_some() {
                    this.cancel_edit_url(cx);
                    return;
                }
                if this.rename_remote_request.is_some() {
                    this.cancel_rename_remote(cx);
                    return;
                }
                if this.remove_remote_request.is_some() {
                    this.cancel_remove_remote(cx);
                    return;
                }
                if this.profile_request.is_some() {
                    this.cancel_profile(cx);
                    return;
                }
                if this.rename_request.is_some() {
                    this.cancel_rename_branch(cx);
                    return;
                }
                if this.rebase_request.is_some() {
                    this.cancel_rebase(cx);
                    return;
                }
                if this.branch_delete_request.is_some() {
                    this.cancel_branch_delete(cx);
                    return;
                }
                if this.branch_delete_refused.is_some() {
                    this.cancel_branch_force_delete(cx);
                    return;
                }
                if this.remote_request.is_some() {
                    this.cancel_add_remote(cx);
                    return;
                }
                if this.push_request.is_some() {
                    this.cancel_push(cx);
                    return;
                }
                if this.fetch_request.is_some() {
                    this.cancel_fetch(cx);
                    return;
                }
                if this.pull_request.is_some() {
                    this.cancel_pull(cx);
                    return;
                }
                if this.reset_request.is_some() {
                    this.cancel_reset(cx);
                    return;
                }
                if this.discarded_open {
                    this.close_discarded(cx);
                    return;
                }
                if this.stash_request.is_some() {
                    this.cancel_stash(cx);
                    return;
                }
                if this.stash_drop_confirm.is_some() {
                    this.cancel_stash_drop(cx);
                    return;
                }
                if this.apply_patch_request.is_some() {
                    this.cancel_apply_patch(cx);
                    return;
                }
                if this.shortcuts_open {
                    this.cancel_shortcuts(cx);
                    return;
                }
                if this.stash_branch_request.is_some() {
                    this.cancel_stash_branch(cx);
                    return;
                }
                if this.discard_confirm.is_some() {
                    this.cancel_discard(cx);
                    return;
                }
                if this.oplog_open {
                    this.close_oplog(cx);
                    return;
                }
                if this.palette_open {
                    this.close_palette(window, cx);
                } else if this.roots_open {
                    this.close_roots_to_palette(cx);
                } else if this.settings_open {
                    this.close_settings(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SubmitCommit, _, cx| {
                this.submit_commit(cx);
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                if !tabs::text_input_focused(this, window, cx) {
                    this.cycle_tab(true, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PrevTab, window, cx| {
                if !tabs::text_input_focused(this, window, cx) {
                    this.cycle_tab(false, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                // Ctrl+W is an editor shortcut too; never take it while typing.
                if !tabs::text_input_focused(this, window, cx)
                    && let Some(ix) = this.active
                {
                    this.close_tab(ix, cx);
                }
            }))
            .on_drop(cx.listener(|this, paths: &gpui_kit::ExternalPaths, _, cx| {
                this.handle_drop(paths, cx);
            }))
            // File-list column drag: moves update live, release persists.
            // A move without the button held ends a drag whose release happened
            // outside the window.
            .on_mouse_move(cx.listener(
                |this, event: &gpui_kit::MouseMoveEvent, window, cx| {
                    let Some((start_x, start_w)) = this.col_drag else {
                        return;
                    };
                    if event.pressed_button != Some(gpui_kit::MouseButton::Left) {
                        this.end_col_drag(cx);
                        return;
                    }
                    // Pixels are logical in gpui: no scale-factor division.
                    let viewport_w = f32::from(window.viewport_size().width);
                    let x = f32::from(event.position.x);
                    this.changes_list_w =
                        changes::clamp_col_width(start_w + x - start_x, viewport_w);
                    cx.notify();
                },
            ))
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.end_col_drag(cx);
                }),
            )
            .child(self.render_chrome(window, cx))
            .child(
                // Alerts float over the content region (no operations card):
                // the Undo offer above the error line.
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(content)
                    .child(
                        div()
                            .absolute()
                            .left(px(16.))
                            .bottom(px(14.))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .children(update_card)
                            .children(undo_card)
                            .children(error_toast),
                    ),
            )
            .children(stash_overlay)
            .children(stash_drop_overlay)
            .children(apply_patch_overlay)
            .children(shortcuts_overlay)
            .children(stash_branch_overlay)
            .children(branch_overlay)
            .children(rename_overlay)
            .children(rebase_overlay)
            .children(branch_delete_overlay)
            .children(branch_force_overlay)
            .children(tag_overlay)
            .children(tag_push_overlay)
            .children(tag_delete_overlay)
            .children(remote_overlay)
            .children(edit_url_overlay)
            .children(rename_remote_overlay)
            .children(remove_remote_overlay)
            .children(profile_overlay)
            .children(push_overlay)
            .children(fetch_overlay)
            .children(pull_overlay)
            .children(reset_overlay)
            .children(discarded_overlay)
            .children(hunk_discard_overlay)
            .children(oplog_overlay)
            .children(discard_overlay)
            .children(overlay)
            // Menu explainer: root-level like the other overlays, so its
            // window-space row coordinates land exactly (the card itself
            // paints deferred, beside the menu rather than under it).
            .children(self.render_commit_menu(window))
            .children(self.render_explainer(window, cx));
        let root = root.children(Root::render_notification_layer(window, cx));

        // Once-per-frame drives: the dialog fade/width and any mid-flight hover
        // blend. This is the window's root render, so it runs exactly once.
        if self.dialog_fade.is_animating(now)
            || self.modal_fade.is_animating(now)
            || self.dialog_width.is_animating(now)
            || self.side_fade.iter().any(|fade| fade.is_animating(now))
            || self.folder_fades.values().any(|fade| fade.is_animating(now))
            || motion::hover_fades_active()
        {
            window.request_animation_frame();
        }
        root
    }
}
