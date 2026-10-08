//! Local Changes: the UNSTAGED/STAGED sections with stage/unstage interaction,
//! filter, and the selection that drives the diff pane. Layout follows
//! SourceGit's changes page (list column + diff pane).

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::base::ElementExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::{ActiveTheme, Icon, Sizable as _};
use gpui_kit::{
    App, InteractiveElement as _, IntoElement, SharedString, StatefulInteractiveElement as _,
    WeakEntity,
};

use crate::i18n::t;
use crate::status::{Change, ChangeItem, ChangeLists};

use super::motion::{hover_blend, hover_listener};

/// File-list column width clamp: a 220 px floor and 50 % of the window
/// ceiling (capped absolutely too). Pure, unit-tested.
pub(super) fn clamp_col_width(w: f32, viewport_w: f32) -> f32 {
    let max = (viewport_w * 0.5).min(crate::settings::CHANGES_LIST_W_MAX);
    crate::settings::clamp_changes_list_w(w.min(max))
}
/// Uniform row height for section headers and file rows.
const CHANGE_ROW_H: f32 = 32.0;
/// Section header band height.
const SECTION_HEADER_H: f32 = 30.0;
/// Empty-note reserve: an explanatory note must stay visible.
const SECTION_NOTE_H: f32 = 48.0;
/// Vertical padding above and below a section's file list.
const SECTION_LIST_PAD_Y: f32 = 4.0;

/// Height of `count` file rows including the list padding, so the last
/// row's highlight is never clipped.
fn rows_h(count: usize) -> f32 {
    if count == 0 {
        0.0
    } else {
        count as f32 * CHANGE_ROW_H + 2.0 * SECTION_LIST_PAD_Y
    }
}

/// Minimum section height: the header, up to three rows, and room for
/// an explanatory note when one shows. Pure, unit-tested.
pub(super) fn section_min_h(count: usize, has_note: bool) -> f32 {
    SECTION_HEADER_H
        + rows_h(count.min(3))
        + if count == 0 && has_note {
            SECTION_NOTE_H
        } else {
            0.0
        }
}

/// Explicit section heights: natural heights when they fit, otherwise
/// the remainder split in proportion to row counts over the floors. Pure,
/// unit-tested. Leftover space stays empty above the pinned composer.
pub(super) fn section_heights(
    u_count: usize,
    s_count: usize,
    u_note: bool,
    s_note: bool,
    avail: f32,
) -> (f32, f32) {
    fn natural(count: usize, note: bool) -> f32 {
        SECTION_HEADER_H
            + rows_h(count)
            + if count == 0 && note {
                SECTION_NOTE_H
            } else {
                0.0
            }
    }
    let (want_u, want_s) = (natural(u_count, u_note), natural(s_count, s_note));
    let (floor_u, floor_s) = (
        section_min_h(u_count, u_note),
        section_min_h(s_count, s_note),
    );
    if avail <= 0.0 {
        // Unmeasured (first frame): naturals, floors respected.
        return (want_u.max(floor_u), want_s.max(floor_s));
    }
    if want_u + want_s <= avail {
        return (want_u, want_s);
    }
    let floors = floor_u + floor_s;
    let total = (u_count + s_count) as f32;
    if floors >= avail {
        // Degenerate (tiny window): both headers first so neither section
        // vanishes, then the rest in proportion to counts.
        let headers = 2.0 * SECTION_HEADER_H;
        if avail <= headers || total <= 0.0 {
            return (avail / 2.0, avail / 2.0);
        }
        let hu = SECTION_HEADER_H + (avail - headers) * u_count as f32 / total;
        return (hu, avail - hu);
    }
    if total <= 0.0 {
        return (floor_u, floor_s);
    }
    let rest = avail - floors;
    let mut hu = (floor_u + rest * u_count as f32 / total).min(want_u);
    let mut hs = (floor_s + rest * s_count as f32 / total).min(want_s);
    // A section capped at its natural height hands its unused share to the
    // other one, which would otherwise scroll above an empty band. Both
    // cannot be capped here: that would mean everything fits.
    let spare = avail - hu - hs;
    if spare > 0.0 {
        if hu < want_u {
            hu = (hu + spare).min(want_u);
        } else {
            hs = (hs + spare).min(want_s);
        }
    }
    (hu, hs)
}

/// What the diff pane is currently showing.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Selection {
    pub path: Vec<u8>,
    /// Index side (`--cached`) or worktree side.
    pub staged: bool,
    pub untracked: bool,
    /// Unmerged path: the pane shows the conflict resolver instead of a diff.
    pub conflicted: bool,
    pub display: String,
}

impl Selection {
    pub(super) fn from_item(item: &ChangeItem, staged: bool) -> Self {
        Self {
            path: item.path.clone(),
            staged,
            untracked: item.untracked,
            conflicted: item.kind == Change::Unmerged,
            display: item.display_path(),
        }
    }
}

/// One path an action applies to, with the flags the operation needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ChangeTarget {
    pub path: Vec<u8>,
    pub untracked: bool,
}

/// One visible-row range in a section, for shift-click selection.
pub(super) fn selection_range(
    rows: &ChangeRows,
    staged: bool,
    from: usize,
    to: usize,
) -> Vec<Selection> {
    let indices = rows.indices(staged);
    if indices.is_empty() {
        return Vec::new();
    }
    let a = from.min(indices.len() - 1);
    let b = to.min(indices.len() - 1);
    let (lo, hi) = (a.min(b), a.max(b));
    indices[lo..=hi]
        .iter()
        .filter_map(|&item_ix| rows.item(staged, item_ix))
        .map(|item| Selection::from_item(item, staged))
        .collect()
}

/// What a queued Local Changes operation does to its paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChangeOpKind {
    Stage,
    Unstage,
    /// Discard worktree changes; a staged path also resets to HEAD, an
    /// untracked path is deleted.
    Discard { staged: bool, untracked: bool },
    /// Stash the paths (untracked content included when asked).
    Stash {
        untracked: bool,
        mode: crate::git::StashMode,
    },
}

impl ChangeOpKind {
    fn verb(self) -> &'static str {
        match self {
            ChangeOpKind::Stage => "stage",
            ChangeOpKind::Unstage => "unstage",
            ChangeOpKind::Discard { .. } => "discard",
            ChangeOpKind::Stash { .. } => "stash",
        }
    }
}

/// One queued Local Changes operation: either path batches or a single-hunk
/// patch (the diff pane's hover buttons).
#[derive(Clone)]
pub(super) enum ChangeOp {
    Paths(PendingOp),
    Hunk(HunkPatch),
    /// Create Branch dialog operation (same queue, never overlaps a write).
    Branch(branch_dialog::BranchCreateOp),
    /// Branch context menu: switch to an existing local branch.
    Checkout(CheckoutOp),
    /// Remotes context menu: check out a remote-tracking branch.
    CheckoutRemote(CheckoutRemoteOp),
    /// Add Remote dialog: register a new remote.
    AddRemote(AddRemoteOp),
    /// Prune stale tracking branches of one remote (remote row menu).
    PruneRemote(PruneRemoteOp),
    /// Point one remote at a new URL (remote row menu).
    SetRemoteUrl(SetRemoteUrlOp),
    /// Rename one remote (remote row menu).
    RenameRemote(RenameRemoteOp),
    /// Remove one remote (remote row menu, confirmed).
    RemoveRemote(RemoveRemoteOp),
    /// Push the active branch to an explicit remote branch.
    Push(PushOp),
    /// Fetch one remote or all remotes (Fetch dialog).
    Fetch(FetchOp),
    /// Pull one remote branch into the checked-out branch (Pull dialog).
    Pull(PullOp),
    /// Apply a stash entry, keeping it (stash context menu).
    StashApply(StashOp),
    /// Drop a stash entry (stash context menu, confirmed).
    StashDrop(StashOp),
    /// Pop a stash entry (stash row menu).
    StashPop(StashPopOp),
    /// Append an ignore rule, untracking the path when asked (row menu).
    Ignore(IgnoreOp),
    /// Create and check out a branch at a stash entry (stash row menu).
    StashBranch(StashBranchOp),
    /// Restore one stashed file to the worktree (stash file menu).
    StashApplyFile(StashApplyFileOp),
    /// Apply a clipboard patch to the worktree (palette).
    ApplyPatch(ApplyPatchOp),
    /// Check out one commit detached (commit context menu).
    CheckoutDetached(CheckoutDetachedOp),
    /// Rename a local branch (branch row menu / palette).
    RenameBranch(RenameBranchOp),
    /// Delete a local branch (branch row menu, confirmed).
    DeleteBranch(DeleteBranchOp),
    /// Cherry-pick one commit onto the checked-out branch (commit menu).
    CherryPick(CherryPickOp),
    /// Revert one commit (commit context menu).
    Revert(RevertOp),
    /// Rebase the checked-out branch onto another ref (rebase dialog).
    Rebase(RebaseOp),
    /// Reset the checked-out branch to one commit (reset dialog).
    Reset(ResetOp),
    /// Create a tag at one commit (tag dialog).
    TagCreate(TagCreateOp),
    /// Push one tag to one remote (tag row menu).
    TagPush(TagPushOp),
    /// Delete one tag locally and/or on remotes (tag delete dialog).
    TagDelete(TagDeleteOp),
    /// Conflict resolver: rewrite the worktree file with the chosen sections
    /// and stage it.
    Resolve(ResolveConflictOp),
    /// Undo (or redo) one entry through its backup.
    Undo(undo::UndoOp),
}

/// A queued conflict resolution.
#[derive(Clone, Debug)]
pub(super) struct ResolveConflictOp {
    pub repo_id: String,
    pub path: Vec<u8>,
    pub choices: Vec<crate::git::ConflictChoice>,
}

/// A queued checkout of an existing local branch.
#[derive(Clone, Debug)]
pub(super) struct CheckoutOp {
    pub repo_id: String,
    pub name: String,
}

/// A queued checkout of a remote-tracking branch (creates a tracking local
/// branch when none exists).
#[derive(Clone, Debug)]
pub(super) struct CheckoutRemoteOp {
    pub repo_id: String,
    pub remote: String,
    pub branch: String,
}

/// A queued `git remote add`.
#[derive(Clone, Debug)]
pub(super) struct AddRemoteOp {
    pub repo_id: String,
    pub name: String,
    pub url: String,
}

/// A queued `git remote prune` (remote row menu).
#[derive(Clone, Debug)]
pub(super) struct PruneRemoteOp {
    pub repo_id: String,
    pub remote: String,
}

/// A queued `git remote set-url` (remote row menu).
#[derive(Clone, Debug)]
pub(super) struct SetRemoteUrlOp {
    pub repo_id: String,
    pub remote: String,
    pub url: String,
}

/// A queued `git remote rename` (remote row menu).
#[derive(Clone, Debug)]
pub(super) struct RenameRemoteOp {
    pub repo_id: String,
    pub old: String,
    pub new: String,
}

/// A queued `git remote remove` (remote row menu, confirmed).
#[derive(Clone, Debug)]
pub(super) struct RemoveRemoteOp {
    pub repo_id: String,
    pub remote: String,
}

/// A queued single-branch push with an explicit destination.
#[derive(Clone, Debug)]
pub(super) struct PushOp {
    pub repo_id: String,
    pub remote: String,
    pub local: String,
    pub target: String,
    pub set_upstream: bool,
}

/// A queued fetch: one remote, or all when `remote` is `None`.
#[derive(Clone, Debug)]
pub(super) struct FetchOp {
    pub repo_id: String,
    pub remote: Option<String>,
    pub force: bool,
    pub no_tags: bool,
    /// Queued by the Fetch dialog, which stays open (busy) until it ends.
    /// Background fetches (a remote's own Fetch, the first fetch after
    /// adding a remote) must not close whatever dialog is open by then.
    pub from_dialog: bool,
}

/// A queued explicit pull into the checked-out branch.
#[derive(Clone, Debug)]
pub(super) struct PullOp {
    pub repo_id: String,
    pub remote: String,
    pub branch: String,
    /// Checked-out branch at confirm time; rechecked before the pull runs.
    pub local: String,
    pub rebase: bool,
    pub changes: crate::git::PullChanges,
}

/// A queued apply/drop of one stash entry (`stash@{n}`).
#[derive(Clone, Debug)]
pub(super) struct StashOp {
    pub repo_id: String,
    pub id: String,
    /// Entry commit at click time; `id` is re-resolved from it on run.
    pub oid: String,
}

/// A queued stash pop: apply, then drop on success (stash row menu).
#[derive(Clone, Debug)]
pub(super) struct StashPopOp {
    pub repo_id: String,
    pub id: String,
    pub oid: String,
}

/// A queued ignore-rule append, optionally untracking the path itself
/// (change row menu).
#[derive(Clone, Debug)]
pub(super) struct IgnoreOp {
    pub repo_id: String,
    pub rule: Vec<u8>,
    pub exclude: bool,
    pub untrack: Option<Vec<u8>>,
}

/// A queued `git stash branch`: new branch checked out at the entry, which
/// is dropped on success (stash row menu).
#[derive(Clone, Debug)]
pub(super) struct StashBranchOp {
    pub repo_id: String,
    pub id: String,
    pub oid: String,
    pub branch: String,
}

/// A queued single-file restore from a stash entry (stash file menu).
#[derive(Clone, Debug)]
pub(super) struct StashApplyFileOp {
    pub repo_id: String,
    pub id: String,
    pub oid: String,
    pub path: Vec<u8>,
    pub untracked: bool,
}

/// A queued clipboard patch (`git apply --3way` to the worktree).
#[derive(Clone, Debug)]
pub(super) struct ApplyPatchOp {
    pub repo_id: String,
    pub patch: Vec<u8>,
    /// Previewed file list, kept for the operation-log line.
    pub files: Vec<String>,
}

/// A queued local-branch rename (branch row menu).
#[derive(Clone, Debug)]
pub(super) struct RenameBranchOp {
    pub repo_id: String,
    pub old: String,
    pub new: String,
}

/// A queued local-branch delete (branch row menu, confirmed; `force` only
/// after git refused the safe delete for unmerged commits).
#[derive(Clone, Debug)]
pub(super) struct DeleteBranchOp {
    pub repo_id: String,
    pub name: String,
    pub force: bool,
}

/// A queued detached checkout of one commit (commit context menu).
#[derive(Clone, Debug)]
pub(super) struct CheckoutDetachedOp {
    pub repo_id: String,
    pub hash: String,
    pub short: String,
}

/// A queued cherry-pick onto the checked-out branch (commit context menu).
#[derive(Clone, Debug)]
pub(super) struct CherryPickOp {
    pub repo_id: String,
    pub hash: String,
    pub short: String,
}

/// A queued revert of one commit (commit context menu).
#[derive(Clone, Debug)]
pub(super) struct RevertOp {
    pub repo_id: String,
    pub hash: String,
    pub short: String,
}

/// A queued rebase of the checked-out branch (rebase dialog).
#[derive(Clone, Debug)]
pub(super) struct RebaseOp {
    pub repo_id: String,
    pub branch: String,
    pub onto: String,
    pub autostash: bool,
}

/// A queued branch reset to one commit (reset dialog). The branch and
/// HEAD captured at confirm time are rechecked immediately before the reset
/// runs; a mismatch refuses instead of resetting the wrong branch.
#[derive(Clone, Debug)]
pub(super) struct ResetOp {
    pub repo_id: String,
    pub target: String,
    pub short: String,
    pub mode: crate::git::ResetMode,
    /// Checked-out branch at confirm time (`None` = detached).
    pub branch: Option<String>,
    /// HEAD at confirm time.
    pub head: String,
}

/// A queued tag creation at one commit (tag dialog).
#[derive(Clone, Debug)]
pub(super) struct TagCreateOp {
    pub repo_id: String,
    pub name: String,
    pub hash: String,
    pub message: String,
    /// Remote to push the new tag to right after creating it.
    pub push_remote: Option<String>,
}

/// A queued tag push to one remote (tag row menu).
#[derive(Clone, Debug)]
pub(super) struct TagPushOp {
    pub repo_id: String,
    pub remote: String,
    pub name: String,
}

/// A queued tag delete, locally and/or on remotes (tag delete dialog).
#[derive(Clone, Debug)]
pub(super) struct TagDeleteOp {
    pub repo_id: String,
    pub name: String,
    pub local: bool,
    pub remotes: Vec<String>,
}

/// A one-hunk `git apply` (stage/unstage/discard from the diff pane).
#[derive(Clone)]
pub(super) struct HunkPatch {
    pub repo_id: String,
    pub patch: Vec<u8>,
    /// Apply to the index instead of the worktree.
    pub cached: bool,
    pub reverse: bool,
    /// Operation-log line.
    pub label: String,
}

/// A pending destructive hunk Discard awaiting confirmation.
#[derive(Clone, Debug)]
pub(super) struct HunkDiscardRequest {
    pub patch: Vec<u8>,
    pub staged: bool,
    pub label: String,
}

/// One queued Local Changes operation. Paths are batched so "stage all" runs
/// a single Git command instead of one process per file.
#[derive(Clone)]
pub(super) struct PendingOp {
    pub repo_id: String,
    pub paths: Vec<Vec<u8>>,
    pub kind: ChangeOpKind,
    /// Stash description (only used by [`ChangeOpKind::Stash`]).
    pub message: Option<String>,
}

/// A pending destructive Discard; confirmed through an overlay before it runs.
#[derive(Clone, Debug)]
pub(super) struct DiscardRequest {
    pub repo_id: String,
    /// Every target is on the same index side (`staged`); the list may mix
    /// tracked and untracked paths.
    pub staged: bool,
    pub targets: Vec<ChangeTarget>,
    pub display: String,
}

impl DiscardRequest {
    /// Discard for one file on one side, from its current list entry.
    pub fn single(repo_id: String, staged: bool, item: &ChangeItem) -> Self {
        Self {
            repo_id,
            staged,
            targets: vec![ChangeTarget {
                path: item.path.clone(),
                untracked: item.untracked,
            }],
            display: item.display_path(),
        }
    }

    pub fn count(&self) -> usize {
        self.targets.len()
    }

    pub fn all_untracked(&self) -> bool {
        self.targets.iter().all(|target| target.untracked)
    }
}

/// Filtered display rows for both sections: visible INDICES into the shared
/// immutable change-list snapshot the model was built from. A published row
/// model owns the exact snapshot its indices address, so an older model
/// stays valid after the shell publishes a newer one.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ChangeRows {
    pub lists: Rc<ChangeLists>,
    pub unstaged: Vec<usize>,
    pub staged: Vec<usize>,
}

impl ChangeRows {
    pub fn indices(&self, staged: bool) -> &[usize] {
        if staged { &self.staged } else { &self.unstaged }
    }

    pub fn len(&self, staged: bool) -> usize {
        self.indices(staged).len()
    }

    pub fn item(&self, staged: bool, visible_ix: usize) -> Option<&ChangeItem> {
        let items = if staged {
            &self.lists.staged
        } else {
            &self.lists.unstaged
        };
        self.indices(staged).get(visible_ix).and_then(|&ix| items.get(ix))
    }
}

/// The row to show after `gone` left its section: the first row that
/// followed it in `before` and is still listed in `after`, else the nearest
/// one above it. `None` when the section has no rows left.
pub(super) fn next_selection(
    before: &ChangeRows,
    after: &ChangeRows,
    gone: &Selection,
) -> Option<Selection> {
    fn visible(rows: &ChangeRows, staged: bool) -> Vec<&ChangeItem> {
        (0..rows.len(staged)).filter_map(|ix| rows.item(staged, ix)).collect()
    }
    let staged = gone.staged;
    let old = visible(before, staged);
    let at = old.iter().position(|item| item.path == gone.path)?;
    let now: HashMap<&[u8], &ChangeItem> = visible(after, staged)
        .into_iter()
        .map(|item| (item.path.as_slice(), item))
        .collect();
    old[at + 1..]
        .iter()
        .chain(old[..at].iter().rev())
        .find_map(|item| now.get(item.path.as_slice()))
        .map(|item| Selection::from_item(item, staged))
}

/// Filter each section by path, keeping only the visible indices.
pub(super) fn build_rows(lists: Rc<ChangeLists>, filter: &str) -> ChangeRows {
    let needle = filter.trim().to_lowercase();
    let visible = |items: &[ChangeItem]| -> Vec<usize> {
        items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                needle.is_empty() || item.display_path().to_lowercase().contains(&needle)
            })
            .map(|(ix, _)| ix)
            .collect()
    };
    ChangeRows {
        unstaged: visible(&lists.unstaged),
        staged: visible(&lists.staged),
        lists,
    }
}

impl SpurShell {
    pub(super) fn render_changes_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.change_rows.clone();
        let has_snapshot = self.active_snapshot().is_some();
        let repo_id: SharedString = match self.active_repo() {
            Some(repo) => repo.path.to_string_lossy().into_owned().into(),
            None => "none".into(),
        };
        // Recently-discarded: a header button only while the active
        // repository actually has backups.
        let show_discarded = self
            .active_repo_id()
            .is_some_and(|id| !self.discarded_for(&id).is_empty());
        let entity = cx.entity().downgrade();

        // Both sections always render (header, count, note only when it adds
        // information), so the layout never collapses an area that simply has
        // no rows right now. "No matches" is kept because it explains a filter
        // hiding rows; a genuinely empty section stays blank.
        let unstaged_empty = if self.change_lists.is_empty() && !has_snapshot {
            Some(t().no_status_collected())
        } else if !self.change_lists.unstaged.is_empty() {
            Some(t().no_matches)
        } else {
            None
        };
        let staged_empty = if !self.change_lists.staged.is_empty() {
            Some(t().no_matches)
        } else {
            None
        };
        // Explicit section heights: measured wrapper space split by
        // content, so an empty Staged collapses to its header and the
        // composer always fits. Unmeasured first frame falls back to
        // naturals via section_heights.
        let (unstaged_h, staged_h) = section_heights(
            rows.len(false),
            rows.len(true),
            unstaged_empty.is_some(),
            staged_empty.is_some(),
            (self.change_sections_h - 1.0).max(0.0),
        );
        let measure_entity = entity.clone();

        div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .w(px(self.list_col_w()))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .overflow_hidden()
                    .border_r_1()
                    .border_color(hairline(0.05))
                    .child(
                        div()
                            .flex_none()
                            .h(px(36.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .border_b_1()
                            .border_color(hairline(0.05))
                            .child(
                                Icon::new(IconName::Search)
                                    .size(px(13.))
                                    .flex_none()
                                    .text_color(text_faint(cx)),
                            )
                            .child(div().flex_1().min_w_0().child(
                                Input::new(&self.change_filter)
                                    .appearance(false)
                                    .focus_bordered(false)
                                    .w_full(),
                            ))
                            .children(show_discarded.then(|| {
                                wash_icon_button(
                                    "recently-discarded",
                                    IconName::ArchiveRestore,
                                    t().discarded_action(),
                                    {
                                        let entity = entity.clone();
                                        move |_, _, cx| {
                                            entity
                                                .update(cx, |this, cx| this.open_discarded(cx))
                                                .ok();
                                        }
                                    },
                                    cx,
                                )
                            })),
                    )
 // Sections wrapper: the two flex_1 sections split
                    // the space left by the filter row and the composer
                    // below.
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .on_prepaint(move |bounds, _, cx| {
                                measure_entity
                                    .update(cx, |this, cx| {
                                        let h = f32::from(bounds.size.height);
                                        if (this.change_sections_h - h).abs() > 0.5 {
                                            this.change_sections_h = h;
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                            })
                            .child(change_section(
                                false,
                                &rows,
                                unstaged_empty,
                                unstaged_h,
                                &repo_id,
                                self,
                                cx,
                            ))
                            .child(
                                div()
                                    .flex_none()
                                    .h(px(1.))
                                    .bg(hairline(0.05)),
                            )
                            .child(change_section(
                                true,
                                &rows,
                                staged_empty,
                                staged_h,
                                &repo_id,
                                self,
                                cx,
                            )),
                    )
                    // Commit composer: under the Staged list it commits,
                    // so staging and committing never cross the diff.
                    .child(self.render_commit_box(cx)),
            )
            .child(
                // Column drag handle: a quiet grab strip between the
                // file list and the diff.
                super::panes::pane_handle(super::panes::Pane::ChangesList, entity.clone())
                    .flex_none()
                    .w(px(6.)),
            )
            .child(
                // The right column clips its content: a wide diff must scroll
                // horizontally instead of stretching the column and pushing the
                // commit box off-window. `w(px(0.))` gives the flex item a
                // definite basis so the diff content cannot size it.
                div()
                    .flex()
                    .flex_1()
                    .w(px(0.))
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .flex_col()
                    .child(if self.conflict_view.is_some() {
                        self.render_conflict_pane(cx)
                    } else {
                        self.render_diff_pane(cx).into_any_element()
                    }),
            )
    }
}

/// One section: fixed header (label, count, bulk action) plus an independently
/// scrolling list. Sections size to their content: no growth, shrink
/// shared in proportion to content, floored at `section_min_h`. An empty
/// section keeps its header (count 0) and shows `empty` in place of the list
/// when there is something to explain (a filter, missing status).
fn change_section(
    staged: bool,
    rows: &Rc<ChangeRows>,
    empty: Option<&str>,
    height: f32,
    repo_id: &SharedString,
    shell: &SpurShell,
    cx: &mut Context<SpurShell>,
) -> gpui_kit::AnyElement {
    let count = rows.len(staged);
    let body: gpui_kit::AnyElement = if count == 0 {
        match empty {
            Some(text) => change_section_empty(text, cx),
            None => div().flex_1().min_h_0().into_any_element(),
        }
    } else {
        let list_id: SharedString = format!(
            "changes-{}-{repo_id}",
            if staged { "staged" } else { "unstaged" }
        )
        .into();
        // One shared model per frame: indices + the snapshot they address.
        let rows = rows.clone();
        let repo = repo_id.clone();
        let selected_rows = shell.change_selected.clone();
        let this = cx.entity().downgrade();
        let scroll = if staged {
            shell.staged_scroll.clone()
        } else {
            shell.unstaged_scroll.clone()
        };
        let scrollbar_id: &'static str = if staged {
            "staged-scrollbar"
        } else {
            "unstaged-scrollbar"
        };
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px(px(6.))
            .py(px(SECTION_LIST_PAD_Y))
            .child(scrollbar_overlay(scrollbar_id, &scroll))
            .child(
                gpui_kit::uniform_list(list_id, count, move |range, _window, cx| {
                    range
                        .filter_map(|ix| {
                            let item = rows.item(staged, ix)?;
                            let selected = selected_rows
                                .iter()
                                .any(|sel| sel.path == item.path && sel.staged == staged);
                            Some(change_file_row(
                                ix,
                                staged,
                                item,
                                selected,
                                this.clone(),
                                repo.clone(),
                                cx,
                            ))
                        })
                        .collect()
                })
                .track_scroll(&scroll)
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    };

    div()
        .flex_none()
        .h(px(height))
        .min_h_0()
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(change_section_header(staged, count, cx.entity().downgrade(), cx))
        .child(body)
        .into_any_element()
}

fn change_section_empty(text: &str, cx: &Context<SpurShell>) -> gpui_kit::AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .px(px(14.))
        .text_size(px(TEXT_SM))
        .text_color(text_faint(cx))
        .child(text.to_string())
        .into_any_element()
}

impl SpurShell {
    /// Select a single file (right-click) and load its diff.
    pub(super) fn select_change(&mut self, staged: bool, item: ChangeItem, cx: &mut Context<Self>) {
        let selection = Selection::from_item(&item, staged);
        self.change_selected = vec![selection.clone()];
        self.change_selection = Some(selection);
        self.change_anchor = None;
        self.load_diff(cx);
    }

    /// Left click on a row: plain replaces the selection, Ctrl/Cmd toggles the
    /// row, Shift selects the range from the last anchor (same section).
    pub(super) fn select_change_click(
        &mut self,
        staged: bool,
        ix: usize,
        item: ChangeItem,
        shift: bool,
        additive: bool,
        cx: &mut Context<Self>,
    ) {
        let clicked = Selection::from_item(&item, staged);
        if shift {
            let anchor = self
                .change_anchor
                .filter(|(anchor_staged, _)| *anchor_staged == staged);
            let range = anchor.and_then(|(_, anchor_ix)| {
                (self.change_rows.len(staged) > 0)
                    .then(|| selection_range(&self.change_rows, staged, anchor_ix, ix))
            });
            self.change_selected = range.unwrap_or_else(|| vec![clicked.clone()]);
        } else if additive {
            match self
                .change_selected
                .iter()
                .position(|sel| sel.path == clicked.path && sel.staged == staged)
            {
                Some(position) => {
                    self.change_selected.remove(position);
                }
                None => self.change_selected.push(clicked.clone()),
            }
        } else {
            self.change_selected = vec![clicked.clone()];
        }
        self.change_anchor = Some((staged, ix));
        self.change_selection = Some(clicked);
        self.load_diff(cx);
    }

    /// Targets an action (stage/unstage/discard/stash) applies to: the whole
    /// multi-selection when the context row is part of it, otherwise just the
    /// context row. Only paths on the context row's side are taken.
    pub(super) fn action_targets_for(&self, item: &ChangeItem, staged: bool) -> Vec<ChangeTarget> {
        let in_selection = self
            .change_selected
            .iter()
            .any(|sel| sel.path == item.path && sel.staged == staged);
        if in_selection {
            self.change_selected
                .iter()
                .filter(|sel| sel.staged == staged)
                .map(|sel| ChangeTarget {
                    path: sel.path.clone(),
                    untracked: sel.untracked,
                })
                .collect()
        } else {
            vec![ChangeTarget {
                path: item.path.clone(),
                untracked: item.untracked,
            }]
        }
    }

    /// Rows the Copy actions apply to (both sections of the multi-selection).
    fn copy_targets_for(&self, item: &ChangeItem, staged: bool) -> Vec<Selection> {
        let in_selection = self
            .change_selected
            .iter()
            .any(|sel| sel.path == item.path && sel.staged == staged);
        if in_selection {
            self.change_selected.clone()
        } else {
            vec![Selection::from_item(item, staged)]
        }
    }

    /// Queue a checkbox toggle: a staged row unstages, an unstaged row stages.
    /// The live list entry for `path` on one side, or `None` once the file
    /// has left that side (staged, unstaged, discarded or committed).
    pub(super) fn current_change_item(&self, staged: bool, path: &[u8]) -> Option<ChangeItem> {
        let items = if staged {
            &self.change_lists.staged
        } else {
            &self.change_lists.unstaged
        };
        items.iter().find(|item| item.path == path).cloned()
    }

    pub(super) fn toggle_change(&mut self, staged_now: bool, path: Vec<u8>, cx: &mut Context<Self>) {
        if let Some(repo_id) = self.active_repo_id() {
            let kind = if staged_now {
                ChangeOpKind::Unstage
            } else {
                ChangeOpKind::Stage
            };
            self.queue_change_op(repo_id, vec![path], kind, None, cx);
        }
    }

    pub(super) fn stage_all(&mut self, cx: &mut Context<Self>) {
        if let Some(repo_id) = self.active_repo_id() {
            let paths: Vec<Vec<u8>> = self
                .change_lists
                .unstaged
                .iter()
                .map(|item| item.path.clone())
                .collect();
            self.queue_change_op(repo_id, paths, ChangeOpKind::Stage, None, cx);
        }
    }

    pub(super) fn unstage_all(&mut self, cx: &mut Context<Self>) {
        if let Some(repo_id) = self.active_repo_id() {
            let paths: Vec<Vec<u8>> = self
                .change_lists
                .staged
                .iter()
                .map(|item| item.path.clone())
                .collect();
            self.queue_change_op(repo_id, paths, ChangeOpKind::Unstage, None, cx);
        }
    }

    /// Queue an ignore-rule append. `untrack` carries the path when it is
    /// already tracked and must leave the index first; `None` skips the
    /// `git rm --cached` step (which would fail on untracked paths). Shift
    /// in the row menu targets `.git/info/exclude` instead of `.gitignore`.
    pub(super) fn ignore_change(
        &mut self,
        rule: Vec<u8>,
        exclude: bool,
        untrack: Option<Vec<u8>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(repo_id) = self.active_repo_id() {
            self.change_ops.push_back(ChangeOp::Ignore(IgnoreOp {
                repo_id,
                rule,
                exclude,
                untrack,
            }));
            self.pump_change_ops(cx);
        }
    }

    /// Copy an apply-able patch of the targets' side: one background
    /// read per file, joined in selection order. A failure surfaces instead
    /// of a partial clipboard.
    pub(super) fn copy_change_patch(
        &mut self,
        targets: Vec<ChangeTarget>,
        staged: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let worktree = repo.path.to_string_lossy().into_owned();
        cx.spawn(async move |this, cx| {
            let patches = cx
                .background_executor()
                .spawn(async move {
                    let mut out = String::new();
                    for target in &targets {
                        let patch = crate::git::worktree_file_patch(
                            &worktree,
                            &target.path,
                            staged,
                            target.untracked,
                        )
                        .map_err(|err| {
                            format!("{}: {err}", String::from_utf8_lossy(&target.path))
                        })?;
                        out.push_str(&patch);
                    }
                    Ok::<String, String>(out)
                })
                .await;
            this.update(cx, |this, cx| match patches {
                Ok(text) => {
                    let bytes = text.len();
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                    this.ops.push_info(t().log_copied_patch(bytes));
                    cx.notify();
                }
                Err(err) => {
                    this.note_error(t().log_action_failed("copy patch", &err), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// The file-list width as rendered: the stored preference, capped at half
    /// the current window so a shrunk window never hands the list the diff's
    /// space. The preference itself is kept for when the window grows again.
    pub(super) fn list_col_w(&self) -> f32 {
        if self.viewport_w > 0.0 {
            clamp_col_width(self.changes_list_w, self.viewport_w)
        } else {
            self.changes_list_w
        }
    }

    pub(super) fn queue_change_op(
        &mut self,
        repo_id: String,
        paths: Vec<Vec<u8>>,
        kind: ChangeOpKind,
        message: Option<String>,
        cx: &mut Context<Self>,
    ) {        if paths.is_empty() {
            return;
        }
        // A path already queued for the same operation must not run twice.
        let queued: Vec<Vec<u8>> = self
            .change_ops
            .iter()
            .filter_map(|op| match op {
                ChangeOp::Paths(op) if op.repo_id == repo_id && op.kind == kind => Some(op),
                _ => None,
            })
            .flat_map(|op| op.paths.iter().cloned())
            .collect();
        let paths: Vec<Vec<u8>> = paths
            .into_iter()
            .filter(|path| !queued.contains(path))
            .collect();
        if paths.is_empty() {
            return;
        }
        self.change_ops.push_back(ChangeOp::Paths(PendingOp {
            repo_id,
            paths,
            kind,
            message,
        }));
        self.pump_change_ops(cx);
    }

    /// Queue a single-hunk patch (diff pane buttons).
    pub(super) fn queue_hunk_patch(
        &mut self,
        repo_id: String,
        patch: Vec<u8>,
        cached: bool,
        reverse: bool,
        label: String,
        cx: &mut Context<Self>,
    ) {
        if patch.is_empty() {
            return;
        }
        self.change_ops.push_back(ChangeOp::Hunk(HunkPatch {
            repo_id,
            patch,
            cached,
            reverse,
            label,
        }));
        self.pump_change_ops(cx);
    }

    /// Run queued operations sequentially; one Git process per batch and no
    /// overlapping index writes.
    pub(super) fn pump_change_ops(&mut self, cx: &mut Context<Self>) {
        if self.change_busy {
            return;
        }
        let Some(op) = self.change_ops.pop_front() else {
            return;
        };
        let repo_id = match &op {
            ChangeOp::Paths(op) => op.repo_id.clone(),
            ChangeOp::Hunk(op) => op.repo_id.clone(),
            ChangeOp::Branch(op) => op.repo_id.clone(),
            ChangeOp::Checkout(op) => op.repo_id.clone(),
            ChangeOp::CheckoutRemote(op) => op.repo_id.clone(),
            ChangeOp::AddRemote(op) => op.repo_id.clone(),
            ChangeOp::PruneRemote(op) => op.repo_id.clone(),
            ChangeOp::SetRemoteUrl(op) => op.repo_id.clone(),
            ChangeOp::RenameRemote(op) => op.repo_id.clone(),
            ChangeOp::RemoveRemote(op) => op.repo_id.clone(),
            ChangeOp::Push(op) => op.repo_id.clone(),
            ChangeOp::Fetch(op) => op.repo_id.clone(),
            ChangeOp::Pull(op) => op.repo_id.clone(),
            ChangeOp::StashApply(op) => op.repo_id.clone(),
            ChangeOp::StashDrop(op) => op.repo_id.clone(),
            ChangeOp::StashPop(op) => op.repo_id.clone(),
            ChangeOp::Ignore(op) => op.repo_id.clone(),
            ChangeOp::StashBranch(op) => op.repo_id.clone(),
            ChangeOp::StashApplyFile(op) => op.repo_id.clone(),
            ChangeOp::ApplyPatch(op) => op.repo_id.clone(),
            ChangeOp::CheckoutDetached(op) => op.repo_id.clone(),
            ChangeOp::RenameBranch(op) => op.repo_id.clone(),
            ChangeOp::DeleteBranch(op) => op.repo_id.clone(),
            ChangeOp::CherryPick(op) => op.repo_id.clone(),
            ChangeOp::Revert(op) => op.repo_id.clone(),
            ChangeOp::Rebase(op) => op.repo_id.clone(),
            ChangeOp::Reset(op) => op.repo_id.clone(),
            ChangeOp::TagCreate(op) => op.repo_id.clone(),
            ChangeOp::TagPush(op) => op.repo_id.clone(),
            ChangeOp::TagDelete(op) => op.repo_id.clone(),
            ChangeOp::Resolve(op) => op.repo_id.clone(),
            ChangeOp::Undo(op) => op.repo_id.clone(),
        };
        let Some(row) = self.overview.row(&repo_id) else {
            return;
        };
        let worktree = row.path.to_string_lossy().into_owned();
        let repo_name = row.name.clone();
        self.apply_profile(&worktree);

        // Log metadata is computed before the operation is moved into the
        // worker thread; the remote list and the stash list only exist in the
        // inspector cache.
        let reload_details = match &op {
            ChangeOp::AddRemote(_)
            | ChangeOp::StashPop(_)
            | ChangeOp::StashBranch(_)
            | ChangeOp::PruneRemote(_)
            | ChangeOp::SetRemoteUrl(_)
            | ChangeOp::RenameRemote(_)
            | ChangeOp::RemoveRemote(_)
            | ChangeOp::StashApply(_)
            | ChangeOp::StashDrop(_)
            | ChangeOp::TagCreate(_)
            | ChangeOp::TagPush(_)
            | ChangeOp::TagDelete(_)
            | ChangeOp::Undo(_) => true,
            // Creating a stash changes the stash list.
            ChangeOp::Paths(op) => matches!(op.kind, ChangeOpKind::Stash { .. }),
            _ => false,
        };
        // A successful fetch (or pull) stamps the row's "last fetched" time.
        let note_fetch = matches!(&op, ChangeOp::Fetch(_) | ChangeOp::Pull(_));
        // The branchling's post-operation watch: the
        // fetch's behind count from before it runs, so Look can compare, and
        // whether a pull is the op that finished.
        let fetch_behind_before = match &op {
            ChangeOp::Fetch(fetch) => self
                .overview
                .row(&fetch.repo_id)
                .and_then(|row| row.snapshot.as_ref())
                .map(|collected| collected.snapshot.behind),
            _ => None,
        };
        let is_pull = matches!(&op, ChangeOp::Pull(_));
        // The modal that queued this operation stays open (busy) while it
        // runs; close it when the operation completes.
        let closes_push = matches!(&op, ChangeOp::Push(_));
        let closes_fetch = matches!(&op, ChangeOp::Fetch(fetch) if fetch.from_dialog);
        let closes_pull = matches!(&op, ChangeOp::Pull(_));
        let closes_reset = matches!(&op, ChangeOp::Reset(_));
        // Resolving clears the resolver view and the file selection once the
        // path is staged (it is no longer an unmerged row).
        let closes_conflict = matches!(&op, ChangeOp::Resolve(_));
        // A freshly added remote is fetched once it exists:
        // until then it has no tracking refs to show.
        let added_remote = match &op {
            ChangeOp::AddRemote(op) => Some(op.name.clone()),
            _ => None,
        };
        let (ok_line, verb) = match &op {
            ChangeOp::Paths(op) => {
                let label = if op.paths.len() == 1 {
                    String::from_utf8_lossy(&op.paths[0]).into_owned()
                } else {
                    format!("{} files", op.paths.len())
                };
                let ok_line = match op.kind {
                    ChangeOpKind::Stage => t().log_change_action(true, &label),
                    ChangeOpKind::Unstage => t().log_change_action(false, &label),
                    ChangeOpKind::Discard { .. } => t().log_discarded(&label),
                    ChangeOpKind::Stash { .. } => t().log_stashed(&label),
                };
                (ok_line, op.kind.verb())
            }
            ChangeOp::Hunk(op) => (op.label.clone(), "apply hunk"),
            ChangeOp::Branch(op) => (t().log_branch_created(&op.name), "create branch"),
            ChangeOp::Checkout(op) => (t().log_branch_checked_out(&op.name), "checkout"),
            ChangeOp::CheckoutRemote(op) => (
                t().log_remote_checked_out(&op.remote, &op.branch),
                "checkout remote branch",
            ),
            ChangeOp::AddRemote(op) => (t().log_remote_added(&op.name), "add remote"),
            ChangeOp::PruneRemote(op) => (t().log_pruned(&op.remote), "prune remote"),
            ChangeOp::SetRemoteUrl(op) => {
                (t().log_remote_url_saved(&op.remote), "set remote URL")
            }
            ChangeOp::RenameRemote(op) => (
                t().log_branch_renamed(&op.old, &op.new),
                "rename remote",
            ),
            ChangeOp::RemoveRemote(op) => {
                (t().log_remote_removed(&op.remote), "remove remote")
            }
            ChangeOp::Push(op) => (
                t().log_pushed(&op.local, &op.remote, &op.target),
                "push",
            ),
            ChangeOp::Fetch(op) => (
                match &op.remote {
                    Some(remote) => t().log_fetched(remote),
                    None => t().log_fetched_all(),
                },
                "fetch",
            ),
            ChangeOp::Pull(op) => (
                t().log_pulled(&op.remote, &op.branch, &op.local),
                "pull",
            ),
            ChangeOp::StashApply(op) => (t().log_stash_applied(&op.id), "apply stash"),
            ChangeOp::StashDrop(op) => (t().log_stash_dropped(&op.id), "drop stash"),
            ChangeOp::StashPop(op) => (t().log_stash_popped(&op.id), "pop stash"),
            ChangeOp::Ignore(op) => {
                let rule = String::from_utf8_lossy(&op.rule).into_owned();
                let mut detail = if op.exclude {
                    "exclude".to_string()
                } else {
                    ".gitignore".to_string()
                };
                if op.untrack.is_some() {
                    detail.push_str(", untracked");
                }
                (t().log_ignored(&rule, &detail), "ignore")
            }
            ChangeOp::StashBranch(op) => (
                t().log_stash_branched(&op.id, &op.branch),
                "create branch from stash",
            ),
            ChangeOp::StashApplyFile(op) => (
                t().log_stash_file_applied(
                    &op.id,
                    &String::from_utf8_lossy(&op.path),
                ),
                "apply stashed file",
            ),
            ChangeOp::ApplyPatch(op) => (t().log_patch_applied(&op.files), "apply patch"),
            ChangeOp::CheckoutDetached(op) => {
                (t().log_detached_checked_out(&op.short), "checkout detached")
            }
            ChangeOp::CherryPick(op) => (t().log_cherry_picked(&op.short), "cherry-pick"),
            ChangeOp::Revert(op) => (t().log_reverted(&op.short), "revert"),
            ChangeOp::Rebase(op) => (t().log_rebased(&op.branch, &op.onto), "rebase"),
            ChangeOp::Reset(op) => (
                t().log_reset_to(op.branch.as_deref().unwrap_or("HEAD"), &op.short),
                "reset",
            ),
            ChangeOp::TagCreate(op) => (
                match &op.push_remote {
                    Some(remote) => t().log_tag_created_pushed(&op.name, remote),
                    None => t().log_tag_created(&op.name),
                },
                "create tag",
            ),
            ChangeOp::TagPush(op) => (t().log_tag_pushed(&op.name, &op.remote), "push tag"),
            ChangeOp::TagDelete(op) => {
                let mut destinations = Vec::new();
                if op.local {
                    destinations.push(t().tag_delete_local.to_string());
                }
                destinations.extend(op.remotes.clone());
                (
                    t().log_tag_deleted(&op.name, &destinations),
                    "delete tag",
                )
            }
            ChangeOp::RenameBranch(op) => (
                t().log_branch_renamed(&op.old, &op.new),
                "rename branch",
            ),
            ChangeOp::DeleteBranch(op) => (t().log_branch_deleted(&op.name), "delete branch"),
            ChangeOp::Resolve(op) => (
                t().log_conflict_resolved(&String::from_utf8_lossy(&op.path)),
                "resolve conflict",
            ),
            ChangeOp::Undo(op) => {
                if op.is_redo {
                    (t().log_redid(&op.entry.label), "redo")
                } else {
                    (t().log_undid(&op.entry.label), "undo")
                }
            }
        };
        let stash_message = match &op {
            ChangeOp::Paths(op) => op.message.clone().unwrap_or_default(),
            ChangeOp::Hunk(_)
            | ChangeOp::Branch(_)
            | ChangeOp::Checkout(_)
            | ChangeOp::CheckoutRemote(_)
            | ChangeOp::AddRemote(_)
            | ChangeOp::PruneRemote(_)
            | ChangeOp::SetRemoteUrl(_)
            | ChangeOp::RenameRemote(_)
            | ChangeOp::RemoveRemote(_)
            | ChangeOp::Push(_)
            | ChangeOp::Fetch(_)
            | ChangeOp::Pull(_)
            | ChangeOp::StashApply(_)
            | ChangeOp::StashDrop(_)
            | ChangeOp::StashPop(_)
            | ChangeOp::Ignore(_)
            | ChangeOp::StashBranch(_)
                            | ChangeOp::StashApplyFile(_)
                            | ChangeOp::ApplyPatch(_)
                            | ChangeOp::CheckoutDetached(_)
            | ChangeOp::RenameBranch(_)
            | ChangeOp::DeleteBranch(_)
            | ChangeOp::CherryPick(_)
            | ChangeOp::Revert(_)
            | ChangeOp::Rebase(_)
            | ChangeOp::Reset(_)
            | ChangeOp::TagCreate(_)
            | ChangeOp::TagPush(_)
            | ChangeOp::TagDelete(_)
            | ChangeOp::Resolve(_)
            | ChangeOp::Undo(_) => String::new(),
        };
        // A safe branch delete refused for unmerged commits offers the
        // explicit force delete instead of just failing.
        let delete_refused = match &op {
            ChangeOp::DeleteBranch(op) if !op.force => {
                Some((op.repo_id.clone(), op.name.clone()))
            }
            _ => None,
        };
        self.change_busy = true;
        self.running_op = Some(oplog::RunningOp {
            repo: repo_name.clone(),
            op: verb.to_string(),
            network: matches!(&op, ChangeOp::Fetch(_) | ChangeOp::Pull(_) | ChangeOp::Push(_)),
            started: Instant::now(),
        });
        // Journal floor for this operation's own commands. Refresh
        // polls interleave globally; the worktree filter below keeps them
        // out of the entry's expansion.
        let journal_start = crate::process::journal_len();
        let journal_worktree = worktree.clone();
        // The stash message for a drop comes from the inspector cache
        // (UI thread); the backup itself is captured on the worker below.
        let undo_stash_message = match &op {
            ChangeOp::StashDrop(op) => undo::stash_message_for(&self.details, &op.repo_id, &op.id),
            _ => None,
        };
        let capture_label = ok_line.clone();
        // Cancel snapshot: a cancel that lands during the backup capture
        // kills a capture command, and the mutation must then not run
        // without its backup.
        let cancel_gen = crate::process::cancel_generation();
        // A failed undo/redo from the stack goes back where it came from.
        // Pins follow a renamed branch and leave with a deleted one.
        let pin_retarget = match &op {
            ChangeOp::RenameBranch(op) => Some((op.repo_id.clone(), op.old.clone(), Some(op.new.clone()))),
            ChangeOp::DeleteBranch(op) => Some((op.repo_id.clone(), op.name.clone(), None)),
            _ => None,
        };
        let undo_retry = match &op {
            ChangeOp::Undo(uop) if uop.from_stack => Some(uop.clone()),
            _ => None,
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (result, worker_aftermath) = cx
                .background_executor()
                .spawn(async move {
                    // Capture the backup BEFORE the mutation. A capture that
                    // finds nothing (or fails) never blocks the op below.
                    // Stash ops re-resolve their entry by commit id first:
                    // a shifted list must never hit a different entry.
                    let mut op = op;
                    let stash_check = match &mut op {
                        ChangeOp::StashApply(StashOp { id, oid, .. })
                        | ChangeOp::StashDrop(StashOp { id, oid, .. })
                        | ChangeOp::StashPop(StashPopOp { id, oid, .. })
                        | ChangeOp::StashBranch(StashBranchOp { id, oid, .. })
                        | ChangeOp::StashApplyFile(StashApplyFileOp { id, oid, .. }) => {
                            crate::git::resolve_stash_ref(&worktree, oid).map(|now| *id = now)
                        }
                        _ => Ok(()),
                    };
                    let mut capture = if matches!(op, ChangeOp::Undo(_)) || stash_check.is_err() {
                        undo::UndoCapture::empty()
                    } else {
                        undo::capture_backup(
                            &worktree,
                            &op,
                            &capture_label,
                            undo_stash_message.as_deref(),
                        )
                    };
                    let mut undone: Option<undo::UndoneInfo> = None;
                    let r: Result<(), String> = match op {
                        _ if stash_check.is_err() => stash_check.clone(),
                        _ if crate::process::cancel_generation() != cancel_gen => {
                            Err("cancelled".to_string())
                        }
                        ChangeOp::Paths(op) => {
                            let paths = op.paths.clone();
                            match op.kind {
                                ChangeOpKind::Stage => crate::git::stage_paths(&worktree, &paths),
                                ChangeOpKind::Unstage => {
                                    crate::git::unstage_paths(&worktree, &paths)
                                }
                                ChangeOpKind::Discard { staged, untracked } => {
                                    if untracked && !staged {
                                        crate::git::delete_untracked_paths(&worktree, &paths)
                                    } else {
                                        crate::git::restore_paths(&worktree, &paths, staged)
                                    }
                                }
                                ChangeOpKind::Stash { untracked, mode } => crate::git::stash_paths(
                                    &worktree,
                                    &paths,
                                    untracked,
                                    &stash_message,
                                    mode,
                                ),
                            }
                        }
                        ChangeOp::Hunk(op) => crate::git::apply_patch(
                            &worktree,
                            &op.patch,
                            op.cached,
                            op.reverse,
                        ),
                        ChangeOp::Branch(op) => crate::git::create_branch(
                            &worktree,
                            &op.name,
                            &op.base,
                            op.checkout,
                            op.overwrite,
                            op.changes,
                        ),
                        ChangeOp::Checkout(op) => {
                            crate::git::checkout_branch(&worktree, &op.name)
                        }
                        ChangeOp::CheckoutRemote(op) => {
                            crate::git::checkout_remote_branch(
                                &worktree,
                                &op.remote,
                                &op.branch,
                            )
                        }
                        ChangeOp::AddRemote(op) => {
                            crate::git::add_remote(&worktree, &op.name, &op.url)
                        }
                        ChangeOp::PruneRemote(op) => {
                            crate::git::remote_prune(&worktree, &op.remote)
                        }
                        ChangeOp::SetRemoteUrl(op) => {
                            crate::git::set_remote_url(&worktree, &op.remote, &op.url)
                        }
                        ChangeOp::RenameRemote(op) => {
                            crate::git::rename_remote(&worktree, &op.old, &op.new)
                        }
                        ChangeOp::RemoveRemote(op) => {
                            crate::git::remove_remote(&worktree, &op.remote)
                        }
                        ChangeOp::Push(op) => crate::git::push_branch(
                            &worktree,
                            &op.remote,
                            &op.local,
                            &op.target,
                            op.set_upstream,
                        ),
                        ChangeOp::Fetch(op) => crate::git::fetch_remote(
                            &worktree,
                            op.remote.as_deref(),
                            op.force,
                            op.no_tags,
                        ),
                        ChangeOp::Pull(op) => crate::git::pull_remote(
                            &worktree,
                            &op.remote,
                            &op.branch,
                            &op.local,
                            op.rebase,
                            op.changes,
                        ),
                        ChangeOp::StashApply(op) => crate::git::stash_apply(&worktree, &op.id),
                        ChangeOp::StashDrop(op) => crate::git::stash_drop(&worktree, &op.id),
                        ChangeOp::StashPop(op) => crate::git::stash_pop(&worktree, &op.id),
                        ChangeOp::Ignore(op) => crate::git::ignore_path(
                            &worktree,
                            &op.rule,
                            op.exclude,
                            op.untrack.as_deref(),
                        ),
                        ChangeOp::StashBranch(op) => {
                            crate::git::stash_branch(&worktree, &op.id, &op.branch)
                        }
                        ChangeOp::StashApplyFile(op) => crate::git::stash_apply_file(
                            &worktree,
                            &op.id,
                            &op.path,
                            op.untracked,
                        ),
                        ChangeOp::ApplyPatch(op) => {
                            crate::git::apply_patch(&worktree, &op.patch, false, false)
                        }
                        ChangeOp::CheckoutDetached(op) => {
                            crate::git::checkout_detached(&worktree, &op.hash)
                        }
                        ChangeOp::RenameBranch(op) => {
                            crate::git::rename_branch(&worktree, &op.old, &op.new)
                        }
                        ChangeOp::DeleteBranch(op) => {
                            crate::git::delete_branch(&worktree, &op.name, op.force)
                        }
                        ChangeOp::CherryPick(op) => crate::git::cherry_pick(&worktree, &op.hash),
                        ChangeOp::Revert(op) => crate::git::revert_commit(&worktree, &op.hash),
                        ChangeOp::Rebase(op) => crate::git::rebase_onto(
                            &worktree,
                            &op.branch,
                            &op.onto,
                            op.autostash,
                        ),
                        ChangeOp::Reset(op) => crate::git::reset_branch(
                            &worktree,
                            &op.target,
                            op.mode,
                            op.branch.as_deref(),
                            &op.head,
                        ),
                        ChangeOp::TagCreate(op) => crate::git::create_tag_and_push(
                            &worktree,
                            &op.name,
                            &op.hash,
                            &op.message,
                            op.push_remote.as_deref(),
                        ),
                        ChangeOp::TagPush(op) => {
                            crate::git::push_tag(&worktree, &op.remote, &op.name)
                        }
                        ChangeOp::TagDelete(op) => crate::git::delete_tag(
                            &worktree,
                            &op.name,
                            op.local,
                            &op.remotes,
                        ),
                        ChangeOp::Resolve(op) => {
                            crate::git::resolve_conflict_file(&worktree, &op.path, &op.choices)
                        }
                        ChangeOp::Undo(uop) => {
                            match undo::run_undo_restore(&worktree, &uop.entry.restore) {
                                Ok(redo) => {
                                    undone = Some(undo::UndoneInfo {
                                        is_redo: uop.is_redo,
                                        redo,
                                        label: uop.entry.label,
                                        verb: uop.entry.verb,
                                    });
                                    Ok(())
                                }
                                Err(err) => Err(err),
                            }
                        }
                    };
                    if r.is_err() {
                        // A failed op leaves no undo behind (and no litter).
                        capture.cleanup(&worktree);
                    } else {
                        // Pin HEAD-move undos to the post-op branch/HEAD.
                        capture.seal(&worktree);
                    }
                    (
                        r,
                        undo::WorkerAftermath {
                            capture,
                            undone,
                        },
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                this.change_busy = false;
                this.running_op = None;
                let succeeded = result.is_ok();
                let commands =
                    crate::process::journal_since_for(journal_start, &journal_worktree);
                match result {
                    Ok(()) => {
                        if let Some((repo, old, new)) = pin_retarget {
                            this.retarget_pin(&repo, &old, new.as_deref(), cx);
                        }
                        let detail = worker_aftermath.log_detail(&ok_line);
                        let op_aftermath = worker_aftermath.into_op_aftermath(
                            repo_id.clone(),
                            ok_line,
                            verb.to_string(),
                        );
                        let log_id = this.ops.push_entry(
                            Some(repo_name),
                            verb.to_string(),
                            crate::model::OpResult::Success,
                            detail,
                            commands,
                        );
                        match op_aftermath {
                            Some(undo::OpAftermath::UndoReady { entry, discarded }) => {
                                let toast_repo = entry.repo_id.clone();
                                if let Some(meta) = discarded {
                                    this.push_discarded(
                                        entry.repo_id.clone(),
                                        entry.label.clone(),
                                        meta,
                                        entry.restore.clone(),
                                    );
                                }
                                this.push_undo_entry(entry);
                                this.raise_undo_toast(&toast_repo, log_id, cx);
                            }
                            Some(undo::OpAftermath::Undone { is_redo, redo }) => {
                                if is_redo {
                                    this.redo_slots.remove(&repo_id);
                                } else if let Some(redo) = redo {
                                    this.redo_slots.insert(repo_id.clone(), redo);
                                } else {
                                    this.redo_slots.remove(&repo_id);
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(err) => {
                        crate::logging::log!("changes: {verb}: {err}");
                        if let Some(uop) = undo_retry {
                            this.requeue_failed_undo(uop);
                        }
                        if err == "cancelled" || err == "cancelled before start" {
                            // User-cancelled: a quiet row, never an alert.
                            this.ops.push_entry(
                                Some(repo_name),
                                verb.to_string(),
                                crate::model::OpResult::Cancelled,
                                t().log_cancelled(verb),
                                commands,
                            );
                        } else {
                            match delete_refused {
                                Some((repo_id, name))
                                    if err.contains("not fully merged") =>
                                {
                                    this.branch_delete_refused = Some(branch_menu::RefusedDelete {
                                        repo_id,
                                        name,
                                    });
                                    this.open_modal(cx);
                                }
                                _ => {
                                    let id = this.ops.push_entry(
                                        Some(repo_name),
                                        verb.to_string(),
                                        crate::model::OpResult::Failed,
                                        t().log_action_failed(verb, &err),
                                        commands,
                                    );
                                    this.raise_toast(id, cx);
                                }
                            }
                        }
                    }
                }
                if reload_details {
                    // The Remotes inspector reads its list from the details
                    // cache, which a plain status refresh does not reload.
                    this.details.remove(&repo_id);
                    this.refresh_active_details(true, cx);
                }
                if note_fetch && succeeded {
                    this.overview.note_fetch(&repo_id, Instant::now());
                }
                // Arm the branchling's watch: the follow-up refresh below
                // evaluates it and clears it. A
                // failure arms nothing — the character stays at rest and the
                // alert carries the news.
                if succeeded {
                    let after = this.overview.row(&repo_id).map_or(0, |row| row.serial());
                    if let Some(before) = fetch_behind_before {
                        this.op_watch = Some(OpWatch::Fetch {
                            repo: repo_id.clone(),
                            before,
                            after,
                        });
                    } else if is_pull {
                        this.op_watch = Some(OpWatch::Pull {
                            repo: repo_id.clone(),
                            after,
                        });
                    }
                }
                // The add succeeded, so the remote exists now: queue its first
                // fetch (the pump below runs it next).
                if succeeded && let Some(remote) = added_remote.clone() {
                    this.change_ops.push_back(ChangeOp::Fetch(FetchOp {
                        repo_id: repo_id.clone(),
                        remote: Some(remote),
                        force: false,
                        no_tags: false,
                        from_dialog: false,
                    }));
                }
                if closes_push || closes_fetch || closes_pull || closes_reset {
                    this.close_modal(cx);
                }
                if closes_conflict && succeeded {
                    this.conflict_view = None;
                    this.change_selection = None;
                    this.change_selected.clear();
                    this.change_anchor = None;
                }
                this.refresh_repo(repo_id.clone(), true, cx);
                this.pump_change_ops(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Re-collect status for one repository after an index mutation, with the
    /// usual generation/serial protection. A query already running for this
    /// repository is not duplicated: the request (and its diff-reload intent)
    /// is remembered and runs when the in-flight one finishes. `reload_diff`
    /// is only set by mutations: background polls must not re-read the open
    /// diff.
    /// Periodic active-repository tick: disposable when a query is already
    /// in flight — that query's result is fresher than queueing another
    /// read, so a 2 s poll must not schedule a successor (SPEEDUP_REVIEW P2).
    pub(super) fn poll_repo(&mut self, id: String, cx: &mut Context<Self>) {
        if self.repo_refresh_in_flight.contains(&id) {
            return;
        }
        self.refresh_repo(id, false, cx);
    }

    pub(super) fn refresh_repo(&mut self, id: String, reload_diff: bool, cx: &mut Context<Self>) {
        if self.repo_refresh_in_flight.contains(&id) {
            let pending = self.repo_refresh_pending.entry(id).or_insert(false);
            *pending |= reload_diff;
            return;
        }
        let Some(row) = self.overview.row(&id) else {
            return;
        };
        let path = row.path.to_string_lossy().into_owned();
        let Some(ticket) = self.overview.begin_refresh(&id) else {
            return;
        };
        self.repo_refresh_in_flight.insert(id.clone());
        self.sync_repo_rows();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::git::collect_status(&path) })
                .await;
            this.update(cx, |this, cx| {
                this.repo_refresh_in_flight.remove(&id);
                let applied = this
                    .overview
                    .apply_result(ticket, &id, result, Instant::now());
                // A scan merged while this query ran: the workspace
                // generation moved and the (still valid) result was dropped.
                // Queue a fresh read instead of leaving the row stale.
                if !applied
                    && ticket.generation != this.overview.generation()
                    && this.overview.row(&id).is_some()
                {
                    this.refresh_repo(id.clone(), reload_diff, cx);
                }
                this.sync_repo_rows();
                this.sync_detail_lists(cx);
                // The operation's follow-up refresh landed: the branchling
                // reacts once — Look when the fetch
                // gained commits, Happy when the pull left everything current.
                // Only an applied result from a query begun after the
                // operation finished can judge it (see `OpWatch::judged_by`).
                let watch = this
                    .op_watch
                    .clone()
                    .filter(|watch| applied && watch.judged_by(&id, ticket.serial));
                match watch {
                    Some(OpWatch::Fetch { before, .. }) => {
                        this.op_watch = None;
                        let now = this
                            .overview
                            .row(&id)
                            .and_then(|row| row.snapshot.as_ref())
                            .map(|collected| collected.snapshot.behind)
                            .unwrap_or(0);
                        if now > before {
                            this.hold_face(
                                brand::Face::Look,
                                Duration::from_millis(2500),
                                cx,
                            );
                        }
                    }
                    Some(OpWatch::Pull { .. }) => {
                        this.op_watch = None;
                        if this.workspace_is_clean_and_current() {
                            this.hold_face(
                                brand::Face::Happy,
                                Duration::from_millis(2500),
                                cx,
                            );
                        }
                    }
                    _ => {}
                }
                if reload_diff {
                    this.reload_diff(cx);
                } else if this.active_repo_id().as_deref() == Some(id.as_str()) {
                    // A poll can find a change the status snapshot cannot see
                    // (saving an already-modified file): re-read the open diff
                    // quietly so the pane never goes stale.
                    this.refresh_open_diff(cx);
                }
                this.maybe_reload_history(cx);
                this.refresh_details_if_invalid(false, cx);
                // Preserve a refresh requested while this one ran.
                if let Some(pending_reload) = this.repo_refresh_pending.remove(&id) {
                    this.refresh_repo(id, pending_reload, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Forget the selection and diff when the active repository changes.
    pub(super) fn reset_changes_view(&mut self) {
        self.change_selection = None;
        self.change_selected.clear();
        self.change_anchor = None;
        self.discard_confirm = None;
        self.stash_request = None;
        self.stash_drop_confirm = None;
        self.branch_request = None;
        self.remote_request = None;
        self.push_request = None;
        self.fetch_request = None;
        self.reset_stash_view();
        self.hovered_hunk = None;
        self.hunk_discard = None;
        self.diff = diff::DiffState::Empty;
        self.diff_gen = self.diff_gen.wrapping_add(1);
        self.diff_loading = false;
        self.conflict_gen = self.conflict_gen.wrapping_add(1);
        self.conflict_view = None;
        // The diff cache survives repository switches: keys are
        // worktree-scoped, worktree entries revalidate when shown, immutable
        // object entries stay valid, and the byte budget bounds memory.
    }

    /// Open the destructive-Discard confirmation.
    pub(super) fn request_discard(&mut self, request: DiscardRequest, cx: &mut Context<Self>) {
        self.discard_confirm = Some(request);
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_discard(&mut self, cx: &mut Context<Self>) {
        if self.discard_confirm.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed Discard: runs through the operation queue so it cannot
    /// overlap another index write. Tracked and untracked targets use
    /// different Git operations, so they are queued as separate batches.
    pub(super) fn confirm_discard(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.discard_confirm.clone() else {
            return;
        };
        if request.staged {
            let paths = request.targets.iter().map(|target| target.path.clone()).collect();
            self.queue_change_op(
                request.repo_id,
                paths,
                ChangeOpKind::Discard {
                    staged: true,
                    untracked: false,
                },
                None,
                cx,
            );
            self.close_modal(cx);
            return;
        }
        let tracked: Vec<Vec<u8>> = request
            .targets
            .iter()
            .filter(|target| !target.untracked)
            .map(|target| target.path.clone())
            .collect();
        let untracked: Vec<Vec<u8>> = request
            .targets
            .iter()
            .filter(|target| target.untracked)
            .map(|target| target.path.clone())
            .collect();
        if !tracked.is_empty() {
            self.queue_change_op(
                request.repo_id.clone(),
                tracked,
                ChangeOpKind::Discard {
                    staged: false,
                    untracked: false,
                },
                None,
                cx,
            );
        }
        if !untracked.is_empty() {
            self.queue_change_op(
                request.repo_id,
                untracked,
                ChangeOpKind::Discard {
                    staged: false,
                    untracked: true,
                },
                None,
                cx,
            );
        }
        self.close_modal(cx);
    }

    /// Full-window scrim + card shown while a Discard awaits confirmation.
    pub(super) fn render_discard_confirm(
        &self,
        request: DiscardRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let body = if request.count() == 1 {
            t().discard_body(&request.display, request.all_untracked())
        } else {
            t().discard_body_files(request.count(), request.all_untracked())
        };
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0.0, 0.0, 0.0, 0.35))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, cx| this.cancel_discard(cx)),
            )
            .child(
                div()
                    .w(px(420.))
                    .rounded(px(PANEL_RADIUS))
                    .border_1()
                    .border_color(hairline(0.10))
                    .bg(cx.theme().popover)
                    .shadow_lg()
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_LG))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_primary(cx))
                            .child(t().discard_title),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_MD))
                            .text_color(text_muted(cx))
                            .child(body),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("discard-cancel")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_discard(cx)),
                                    ),
                            )
                            .child(
                                Button::new("discard-confirm")
                                    .label(t().discard_confirm)
                                    .small()
                                    .danger()
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.confirm_discard(cx)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn change_section_header(
    staged: bool,
    count: usize,
    this: WeakEntity<SpurShell>,
    cx: &mut App,
) -> gpui_kit::AnyElement {
    let label = if staged {
        t().section_staged
    } else {
        t().section_unstaged
    };
    let mut row = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .h(px(SECTION_HEADER_H))
        .px(px(8.))
        // A hair lighter than the pane so the section reads as its own band.
        .bg(ink(0.04))
        .border_b_1()
        .border_color(hairline(0.04))
        .child(
            div()
                .text_size(px(TEXT_XS))
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(text_faint(cx))
                .child(t().section_count(label, count)),
        )
        .child(div().flex_1());
    if count > 0 {
        row = row.child(wash_icon_button(
            if staged { "unstage-all" } else { "stage-all" },
            if staged {
                IconName::Minus
            } else {
                IconName::Plus
            },
            if staged {
                t().unstage_all
            } else {
                t().stage_all
            },
            move |_, _, cx| {
                this.update(cx, |this, cx| {
                    if staged {
                        this.unstage_all(cx);
                    } else {
                        this.stage_all(cx);
                    }
                })
                .ok();
            },
            cx,
        ));
    }
    row.into_any_element()
}

fn change_file_row(
    ix: usize,
    staged: bool,
    item: &ChangeItem,
    selected: bool,
    this: WeakEntity<SpurShell>,
    repo_id: SharedString,
    cx: &mut App,
) -> gpui_kit::AnyElement {
    let code = kind_code(item.kind);
    let color = match item.kind {
        // Modified stays neutral: the letter carries the meaning, and
        // the accent is reserved for primary actions and selection.
        Change::Modified => text_muted(cx),
        Change::Added => cx.theme().success,
        Change::Deleted | Change::Unmerged => cx.theme().danger,
        Change::Renamed | Change::Copied => cx.theme().warning,
        _ => text_muted(cx),
    };
    let path_color = if selected {
        text_primary(cx)
    } else {
        text_muted(cx)
    };
    let side = if staged { "staged" } else { "unstaged" };
    let check_id: SharedString = format!("change-check-{side}-{ix}").into();
    let row_id: (&'static str, usize) = if staged {
        ("staged-row", ix)
    } else {
        ("unstaged-row", ix)
    };
    // Hover: the row wash lifts on hover like every other list in the app.
    let fade_key = format!("change-row-{side}-{ix}");
    let base_bg = if selected { ink(0.08) } else { ink(0.0) };
    let hover_bg = if selected { ink(0.12) } else { ink(0.055) };
    let click_item = item.clone();
    let toggle_item = item.clone();
    let check_entity = this.clone();
    let right_item = item.clone();
    let right_entity = this.clone();
    let menu_item = item.clone();
    let menu_repo = repo_id.clone();
    let menu_entity = this.clone();

    div()
        .id(row_id)
        .w_full()
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(CHANGE_ROW_H))
        .px(px(8.))
        .rounded(px(CONTROL_RADIUS))
        .bg(hover_blend(&fade_key, base_bg, hover_bg))
        .on_hover(hover_listener(fade_key))
        .on_mouse_down(gpui_kit::MouseButton::Right, move |_, _, cx| {
            let item = right_item.clone();
            right_entity
                .update(cx, |this, cx| {
                    // Right-clicking outside the selection selects that row so
                    // the menu always acts on something visible.
                    if !this
                        .change_selected
                        .iter()
                        .any(|sel| sel.path == item.path && sel.staged == staged)
                    {
                        this.select_change(staged, item, cx);
                    }
                })
                .ok();
        })
        .child(
            // The checkbox stages or unstages this one file — it says
            // so, instead of looking like a bulk selector. The section
            // header's +/− stays the bulk action ("Stage all").
            Checkbox::new(check_id)
                .checked(staged)
                .tooltip(if staged {
                    t().check_unstage_file
                } else {
                    t().check_stage_file
                })
                .accessibility_label(if staged {
                    t().check_unstage_file
                } else {
                    t().check_stage_file
                })
                .on_change(move |_, _, cx| {
                    check_entity
                        .update(cx, |this, cx| {
                            this.toggle_change(staged, toggle_item.path.clone(), cx);
                        })
                        .ok();
                }),
        )
        .child(
            div()
                .w(px(14.))
                .flex_none()
                .font_family(MONO)
                .text_size(px(TEXT_SM))
                .text_color(color)
                .child(code.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(MONO)
                .text_size(px(TEXT_SM))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(path_color)
                .child(item.display_path()),
        )
        .on_click(move |event, _, cx| {
            let modifiers = event.modifiers();
            this.update(cx, |this, cx| {
                this.select_change_click(
                    staged,
                    ix,
                    click_item.clone(),
                    modifiers.shift,
                    modifiers.control || modifiers.platform,
                    cx,
                );
            })
            .ok();
        })
        .context_menu(move |menu, _window, _cx| {
            // `repo_id` is the canonical absolute worktree, so the full path
            // is worktree + relative path.
            let relative = String::from_utf8_lossy(&menu_item.path).into_owned();
            let absolute = format!("{}/{}", menu_repo.trim_end_matches('/'), relative);

            let reveal_entity = menu_entity.clone();
            let reveal_path = absolute.clone();
            let resolve_entity = menu_entity.clone();
            let resolve_item = menu_item.clone();
            let stage_entity = menu_entity.clone();
            let stage_item = menu_item.clone();
            let discard_entity = menu_entity.clone();
            let discard_item = menu_item.clone();
            let stash_entity = menu_entity.clone();
            let stash_item = menu_item.clone();
            let copy_entity = menu_entity.clone();
            let copy_item = menu_item.clone();
            let copy_full_entity = menu_entity.clone();
            let copy_full_item = menu_item.clone();
            let copy_full_repo = menu_repo.clone();
            let patch_entity = menu_entity.clone();
            let patch_item = menu_item.clone();

            let mut menu = menu
                .item(context_menu_item(
                    crate::i18n::file_manager(t().context_reveal),
                    IconName::FolderOpen,
                    move |_, _, cx| {
                        reveal_entity
                            .update(cx, |this, cx| this.reveal_in_explorer(&reveal_path, cx))
                            .ok();
                    },
                ))
                .separator();
            if !staged && menu_item.kind == Change::Unmerged {
                // Conflicted paths open the resolver instead of a diff.
                menu = menu
                    .item(context_menu_item(
                        t().context_resolve.into(),
                        IconName::GitBranch,
                        move |_, _, cx| {
                            resolve_entity
                                .update(cx, |this, cx| {
                                    this.select_change(false, resolve_item.clone(), cx)
                                })
                                .ok();
                        },
                    ))
                    .separator();
            }
            menu = menu.item(context_menu_item(
                if staged {
                    t().context_unstage.into()
                } else {
                    t().context_stage.into()
                },
                if staged {
                    IconName::Minus
                } else {
                    IconName::Plus
                },
                move |_, _, cx| {
                    stage_entity
                        .update(cx, |this, cx| {
                            let targets = this.action_targets_for(&stage_item, staged);
                            if targets.is_empty() {
                                return;
                            }
                            let paths = targets.into_iter().map(|target| target.path).collect();
                            let kind = if staged {
                                ChangeOpKind::Unstage
                            } else {
                                ChangeOpKind::Stage
                            };
                            if let Some(repo_id) = this.active_repo_id() {
                                this.queue_change_op(repo_id, paths, kind, None, cx);
                            }
                        })
                        .ok();
                },
            ))
            .item(context_menu_item(
                t().context_discard.into(),
                IconName::Trash,
                move |_, _, cx| {
                    discard_entity
                        .update(cx, |this, cx| {
                            let targets = this.action_targets_for(&discard_item, staged);
                            if targets.is_empty() {
                                return;
                            }
                            let display = if targets.len() == 1 {
                                discard_item.display_path()
                            } else {
                                t().file_count(targets.len())
                            };
                            if let Some(repo_id) = this.active_repo_id() {
                                this.request_discard(
                                    DiscardRequest {
                                        repo_id,
                                        staged,
                                        targets,
                                        display,
                                    },
                                    cx,
                                );
                            }
                        })
                        .ok();
                },
            ))
            .item(context_menu_item(
                t().context_stash.into(),
                IconName::Archive,
                move |_, window, cx| {
                    stash_entity
                        .update(cx, |this, cx| {
                            let targets = this.action_targets_for(&stash_item, staged);
                            if targets.is_empty() {
                                return;
                            }
                            let display = if targets.len() == 1 {
                                stash_item.display_path()
                            } else {
                                t().file_count(targets.len())
                            };
                            if let Some(repo_id) = this.active_repo_id() {
                                this.request_stash(repo_id, targets, display, window, cx);
                            }
                        })
                        .ok();
                },
            ))
            .separator();
            if menu_item.kind != Change::Unmerged {
                // Conflicted paths open the resolver, not a diff; there is
                // no meaningful patch to copy mid-conflict.
                menu = menu.item(context_menu_item(
                    t().context_copy_patch.into(),
                    IconName::ClipboardCopy,
                    move |_, _, cx| {
                        patch_entity
                            .update(cx, |this, cx| {
                                let targets = this.action_targets_for(&patch_item, staged);
                                if !targets.is_empty() {
                                    this.copy_change_patch(targets, staged, cx);
                                }
                            })
                            .ok();
                    },
                ));
            }
            menu = menu.item(context_menu_item(
                t().context_copy_path.into(),
                IconName::ClipboardCopy,
                move |_, _, cx| {
                    copy_entity
                        .update(cx, |this, cx| {
                            let selections = this.copy_targets_for(&copy_item, staged);
                            let joined = selections
                                .iter()
                                .map(|sel| String::from_utf8_lossy(&sel.path).into_owned())
                                .collect::<Vec<_>>()
                                .join("\n");
                            this.copy_path(&joined, cx);
                        })
                        .ok();
                },
            ))
            .item(context_menu_item(
                t().context_copy_full_path.into(),
                IconName::Clipboard,
                move |_, _, cx| {
                    copy_full_entity
                        .update(cx, |this, cx| {
                            let selections = this.copy_targets_for(&copy_full_item, staged);
                            // Windows-native paths (`C:\…` / `\\wsl.localhost\…`),
                            // not the internal `/mnt/…` identity.
                            let joined = selections
                                .iter()
                                .map(|sel| {
                                    explorer_target(&format!(
                                        "{}/{}",
                                        copy_full_repo.trim_end_matches('/'),
                                        String::from_utf8_lossy(&sel.path)
                                    ))
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            this.copy_path(&joined, cx);
                        })
                        .ok();
                },
            ));
            menu = ignore_menu(menu, &menu_item, menu_entity.clone());
            menu
        })
        .into_any_element()
}

/// Ignore items for one change row: untracked files offer file, extension
/// (when the basename has a usable suffix), and folder (except repo-root
/// files); tracked files offer untrack-and-ignore instead. Conflicted rows
/// get nothing — there is nothing sensible to ignore mid-conflict. Shift on
/// click targets `.git/info/exclude` instead of `.gitignore`.
fn ignore_menu(
    menu: gpui_kit::component::menu::PopupMenu,
    item: &ChangeItem,
    this: WeakEntity<SpurShell>,
) -> gpui_kit::component::menu::PopupMenu {
    use crate::git::IgnoreKind;
    if item.kind == Change::Unmerged {
        return menu;
    }
    if !item.untracked {
        let entity = this.clone();
        let path = item.path.clone();
        return menu.item(context_menu_item(
            t().context_untrack_ignore.into(),
            IconName::EyeOff,
            move |event, _, cx| {
                let exclude = event.modifiers().shift;
                let path = path.clone();
                entity
                    .update(cx, |shell, cx| {
                        if let Some(rule) = crate::git::ignore_rule(&path, IgnoreKind::File) {
                            shell.ignore_change(rule, exclude, Some(path), cx);
                        }
                    })
                    .ok();
            },
        ));
    }
    let mut menu = menu;
    let file_rule = crate::git::ignore_rule(&item.path, IgnoreKind::File);
    let ext_rule = crate::git::ignore_rule(&item.path, IgnoreKind::Extension);
    let folder_rule = crate::git::ignore_rule(&item.path, IgnoreKind::Folder);
    if let Some(rule) = file_rule {
        let entity = this.clone();
        menu = menu.item(context_menu_item(
            t().context_ignore_file.into(),
            IconName::EyeOff,
            move |event, _, cx| {
                let exclude = event.modifiers().shift;
                let rule = rule.clone();
                entity
                    .update(cx, |shell, cx| shell.ignore_change(rule, exclude, None, cx))
                    .ok();
            },
        ));
    }
    if let Some(rule) = ext_rule {
        let entity = this.clone();
        let ext = String::from_utf8_lossy(rule.strip_prefix(b"*.").unwrap_or(&rule)).into_owned();
        menu = menu.item(context_menu_item(
            t().context_ignore_ext(&ext),
            IconName::EyeOff,
            move |event, _, cx| {
                let exclude = event.modifiers().shift;
                let rule = rule.clone();
                entity
                    .update(cx, |shell, cx| shell.ignore_change(rule, exclude, None, cx))
                    .ok();
            },
        ));
    }
    if let Some(rule) = folder_rule {
        let entity = this.clone();
        menu = menu.item(context_menu_item(
            t().context_ignore_folder.into(),
            IconName::EyeOff,
            move |event, _, cx| {
                let exclude = event.modifiers().shift;
                let rule = rule.clone();
                entity
                    .update(cx, |shell, cx| shell.ignore_change(rule, exclude, None, cx))
                    .ok();
            },
        ));
    }
    menu
}

/// One-letter code for a change kind (index or worktree side).
fn kind_code(kind: Change) -> char {
    match kind {
        Change::Unchanged => ' ',
        Change::Modified => 'M',
        Change::TypeChanged => 'T',
        Change::Added => 'A',
        Change::Deleted => 'D',
        Change::Renamed => 'R',
        Change::Copied => 'C',
        Change::Unmerged => 'U',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_min_h_covers_header_rows_and_notes() {
        // Empty without a note: header only (no dead band).
        assert_eq!(section_min_h(0, false), SECTION_HEADER_H);
        // An explanatory note stays visible.
        assert!(section_min_h(0, true) > SECTION_HEADER_H);
        // Up to three rows, then capped.
        // The list padding is included, so the last row is never clipped.
        assert_eq!(
            section_min_h(1, false),
            SECTION_HEADER_H + CHANGE_ROW_H + 2.0 * SECTION_LIST_PAD_Y
        );
        assert_eq!(section_min_h(3, false), SECTION_HEADER_H + rows_h(3));
        assert_eq!(section_min_h(6, false), section_min_h(3, false));
        assert_eq!(section_min_h(100, false), section_min_h(3, false));
    }

    #[test]
    fn section_heights_fit_share_and_floor() {
        // Everything fits: naturals (6 unstaged + header-only
        // staged + room to spare).
        let (hu, hs) = section_heights(6, 0, false, false, 2000.0);
        assert_eq!(hu, SECTION_HEADER_H + rows_h(6));
        assert_eq!(hs, SECTION_HEADER_H);
        // Unmeasured first frame behaves the same.
        assert_eq!(
            section_heights(6, 0, false, false, 0.0),
            section_heights(6, 0, false, false, 2000.0)
        );
        // Overflow splits the remainder in proportion to counts over floors:
        // 8 unstaged + empty staged in 300px keeps a header-only staged.
        let (hu, hs) = section_heights(8, 0, false, false, 300.0);
        assert_eq!(hs, SECTION_HEADER_H);
        assert!(hu > section_min_h(8, false) && hu < SECTION_HEADER_H + rows_h(8));
        assert!((hu + hs - 300.0).abs() < 0.01);
        // Two big sections share evenly-ish above their floors.
        let (hu, hs) = section_heights(50, 50, false, false, 500.0);
        assert!((hu - hs).abs() < 0.01);
        assert!(hu >= section_min_h(50, false));
        // A section capped at its natural height gives its unused share to
        // the other: 4 unstaged fit exactly, staged takes all the rest.
        let (hu, hs) = section_heights(4, 10, false, false, 480.0);
        assert_eq!(hu, SECTION_HEADER_H + rows_h(4));
        assert!((hu + hs - 480.0).abs() < 0.01, "no dead band: {hu} + {hs}");
        // Degenerate window: both headers keep their band before any rows.
        let (hu, hs) = section_heights(10, 0, false, false, 100.0);
        assert!(hs >= SECTION_HEADER_H && hu >= SECTION_HEADER_H, "{hu} {hs}");
        assert!((hu + hs - 100.0).abs() < 0.01);
        let (hu, hs) = section_heights(4, 0, false, false, 40.0);
        assert_eq!((hu, hs), (20.0, 20.0));
        // Both empty: headers.
        let (hu, hs) = section_heights(0, 0, false, false, 500.0);
        assert_eq!((hu, hs), (SECTION_HEADER_H, SECTION_HEADER_H));
    }

    #[test]
    fn op_watch_waits_for_a_query_begun_after_the_operation() {
        let fetch = super::super::OpWatch::Fetch {
            repo: "/w/a".into(),
            before: 0,
            after: 7,
        };
        // A poll begun before the fetch finished (serial 7) reports the old
        // refs: it must not use the watch up.
        assert!(!fetch.judged_by("/w/a", 7));
        assert!(fetch.judged_by("/w/a", 8), "the follow-up refresh judges it");
        assert!(!fetch.judged_by("/w/b", 8), "another repository never does");
        let pull = super::super::OpWatch::Pull {
            repo: "/w/a".into(),
            after: 3,
        };
        assert!(!pull.judged_by("/w/a", 3));
        assert!(pull.judged_by("/w/a", 4));
    }

    #[test]
    fn column_width_clamps_floor_and_half_window_ceiling() {
        assert_eq!(clamp_col_width(300.0, 1920.0), 300.0);
        assert_eq!(
            clamp_col_width(100.0, 1920.0),
            crate::settings::CHANGES_LIST_W_MIN
        );
        assert_eq!(clamp_col_width(900.0, 1920.0), 800.0);
        // A narrow window caps at half its width, never below the floor.
        assert_eq!(clamp_col_width(500.0, 600.0), 300.0);
        assert_eq!(
            clamp_col_width(500.0, 300.0),
            crate::settings::CHANGES_LIST_W_MIN
        );
    }

    fn item(path: &str, kind: Change, untracked: bool) -> ChangeItem {
        ChangeItem {
            path: path.as_bytes().to_vec(),
            orig_path: None,
            kind,
            untracked,
        }
    }

    #[test]
    fn rows_filter_both_sections_independently() {
        let lists = || ChangeLists {
            unstaged: vec![
                item("src/a.rs", Change::Modified, false),
                item("README.md", Change::Added, true),
            ],
            staged: vec![item("src/b.rs", Change::Added, false)],
        };
        let rows = build_rows(Rc::new(lists()), "");
        assert_eq!(rows.len(false), 2);
        assert_eq!(rows.len(true), 1);

        // Case-insensitive path search over both sections.
        let rows = build_rows(Rc::new(lists()), "SRC");
        assert_eq!(rows.len(false), 1);
        assert_eq!(rows.item(false, 0).unwrap().path, b"src/a.rs");
        assert_eq!(rows.len(true), 1);
        assert_eq!(rows.item(true, 0).unwrap().path, b"src/b.rs");

        let rows = build_rows(Rc::new(lists()), "nothing-matches");
        assert!(rows.indices(false).is_empty() && rows.indices(true).is_empty());
    }

    #[test]
    fn r23a_published_rows_keep_their_own_snapshot() {
        // An older published model must resolve against the snapshot it was
        // built from, even after a newer one is published.
        let old = build_rows(
            Rc::new(ChangeLists {
                unstaged: vec![item("old/a.rs", Change::Modified, false)],
                staged: Vec::new(),
            }),
            "",
        );
        let new = build_rows(
            Rc::new(ChangeLists {
                unstaged: vec![item("new/b.rs", Change::Renamed, false)],
                staged: Vec::new(),
            }),
            "",
        );
        assert_eq!(old.item(false, 0).unwrap().path, b"old/a.rs");
        assert_eq!(new.item(false, 0).unwrap().path, b"new/b.rs");
    }

    #[test]
    fn next_selection_skips_rows_that_left_too() {
        let lists = |names: &[&str]| {
            let items = names.iter().map(|n| item(n, Change::Modified, false)).collect();
            Rc::new(ChangeLists {
                unstaged: items,
                staged: Vec::new(),
            })
        };
        let gone = |name: &str| Selection::from_item(&item(name, Change::Modified, false), false);
        let before = build_rows(lists(&["a", "b", "c", "d"]), "");
        // "b" and "c" were staged together: the next survivor is "d".
        let after = build_rows(lists(&["a", "d"]), "");
        let next = next_selection(&before, &after, &gone("b")).expect("next row");
        assert_eq!(next.path, b"d".to_vec());
        // The last row falls back to the nearest one above.
        let after = build_rows(lists(&["a", "b", "c"]), "");
        let next = next_selection(&before, &after, &gone("d")).expect("previous row");
        assert_eq!(next.path, b"c".to_vec());
        // Nothing left.
        let after = build_rows(lists(&[]), "");
        assert!(next_selection(&before, &after, &gone("a")).is_none());
    }

    #[test]
    fn selection_range_covers_the_clicked_span() {
        let items = || {
            vec![
                item("a", Change::Modified, false),
                item("b", Change::Modified, false),
                item("c", Change::Modified, false),
                item("d", Change::Modified, false),
            ]
        };
        let rows = build_rows(
            Rc::new(ChangeLists {
                unstaged: items(),
                staged: items(),
            }),
            "",
        );
        // Anchor above the click selects every row in between, in order.
        let range = selection_range(&rows, false, 3, 1);
        let paths: Vec<String> = range
            .iter()
            .map(|selection| String::from_utf8_lossy(&selection.path).into_owned())
            .collect();
        assert_eq!(paths, ["b", "c", "d"]);
        assert!(range.iter().all(|selection| !selection.staged));

        // Bounds are clamped; a single row works; the staged side is carried.
        let all = selection_range(&rows, true, 0, 99);
        assert_eq!(all.len(), 4);
        assert!(all.iter().all(|selection| selection.staged));
        assert_eq!(selection_range(&rows, false, 2, 2).len(), 1);
        let empty = build_rows(Rc::new(ChangeLists::default()), "");
        assert!(selection_range(&empty, false, 0, 2).is_empty());
    }
}
