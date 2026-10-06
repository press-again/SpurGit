//! Undo safety net: per-repository undo stacks over backup refs.
//!
//! Before any ref-moving or content-destroying queue operation runs, the pump
//! captures a backup through [`capture_backup`]: the old HEAD, a deleted
//! branch/tag tip, a `git stash create` commit, blobs holding deleted
//! untracked files or a discarded hunk, or a dropped stash entry's object —
//! each pinned by a ref under `refs/spur/undo/…` (local refs, never pushed,
//! pruned after 14 days). [`run_undo_restore`] replays the inverse through
//! the same sequential queue, so undo never overlaps an index write.
//!
//! Deliberate cuts: redo exists for ref-moving restores only (content
//! restores return no redo); commits bypass the queue so commit/amend are not
//! undoable yet; stash pop/branch/apply move content into the worktree or a
//! branch instead of destroying it, so they carry no backup. Backup plumbing
//! never blocks an operation: a failed capture only costs the undo, noted on
//! the log line.

use super::*;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, Icon, Sizable as _};
use gpui_kit::{IntoElement, StatefulInteractiveElement as _};

use crate::i18n::t;

/// Undoable restores, captured BEFORE the mutation runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum UndoRestore {
    /// Move HEAD back (cherry-pick, revert, reset). `guard` pins the branch
    /// and HEAD right after the op ([`UndoCapture::seal`]): a switched branch
    /// or newer commits refuse the undo instead of being reset away.
    ResetHead {
        oid: String,
        guard: Option<HeadGuard>,
    },
    /// Recreate a deleted branch at its tip.
    RecreateBranch { name: String, oid: String },
    /// Delete a branch undo recreated (redo of a branch-delete undo; runs
    /// only when the tip still matches, so a moved branch is never lost).
    DeleteBranch { name: String, oid: String },
    /// Point a deleted tag back at its object.
    RestoreTag { name: String, oid: String },
    /// Delete a tag undo restored (redo; guarded by the same tip check).
    DeleteTag { name: String, oid: String },
    /// A `git stash create` commit holding discarded tracked changes; only
    /// the discarded `paths` come back (the commit holds the whole tree).
    RestorePaths {
        oid: String,
        paths: Vec<Vec<u8>>,
        staged: bool,
    },
    /// A `git stash create` commit holding the whole dirty tree (hard-reset
    /// undo): store it as an entry, apply, then drop the entry again.
    ApplyStashed {
        oid: String,
        message: String,
        staged: bool,
    },
    /// Blobs (`oid`) holding deleted untracked files (`path`).
    RewriteUntracked { files: Vec<(Vec<u8>, String)> },
    /// A forward hunk patch re-applying a discarded hunk.
    ReapplyHunk {
        patch: Vec<u8>,
        cached: bool,
    },
    /// A dropped stash entry's object: store it back as an entry.
    RestashEntry { oid: String, message: String },
    /// Drop the entry undo stored (redo of a stash-drop undo; guarded by the
    /// top entry's object id, so a newer stash is never dropped).
    DropStash { oid: String },
    /// Ordered sub-restores (hard-reset undo: move HEAD back, then re-apply
    /// the stashed changes). Aborts on the first failure and banks no redo:
    /// re-applying a destructive op is never automatic.
    Sequence(Vec<UndoRestore>),
}

/// Branch (`None` = detached) and HEAD an undo expects to find.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HeadGuard {
    pub branch: Option<String>,
    pub head: String,
}

/// One undoable operation: what to show plus how to reverse it.
#[derive(Clone, Debug)]
pub(super) struct UndoEntry {
    pub repo_id: String,
    /// Human line, e.g. the op's log detail (`"deleted branch side"`).
    pub label: String,
    pub verb: String,
    pub restore: UndoRestore,
}

/// A queued undo (or redo) of one entry.
#[derive(Clone, Debug)]
pub(super) struct UndoOp {
    pub repo_id: String,
    pub entry: UndoEntry,
    pub is_redo: bool,
    /// Popped from the undo stack / redo slot (not a Recently-discarded
    /// restore): a failed run goes back there.
    pub from_stack: bool,
}

/// What a finished queue operation hands back for undo bookkeeping. Notes
/// (no undo, or a partial undo) travel on the log line itself — see
/// [`WorkerAftermath::log_detail`] — so they are not repeated here.
#[derive(Clone, Debug)]
pub(super) enum OpAftermath {
    /// A normal op succeeded with an undo entry, plus the Recently-discarded
    /// metadata when the op was a discard.
    UndoReady {
        entry: UndoEntry,
        discarded: Option<DiscardedMeta>,
    },
    /// An undo/redo op succeeded, with the single redo step when one exists.
    Undone { is_redo: bool, redo: Option<UndoEntry> },
}

/// A finished undo/redo op crossing the worker boundary (no Git here).
#[derive(Clone, Debug)]
pub(super) struct UndoneInfo {
    pub is_redo: bool,
    pub redo: Option<UndoRestore>,
    pub label: String,
    pub verb: String,
}

/// Everything the worker hands back to the completion handler.
pub(super) struct WorkerAftermath {
    pub capture: UndoCapture,
    pub undone: Option<UndoneInfo>,
}

impl WorkerAftermath {
    /// The log detail: the op line, with the no/partial-undo note appended.
    pub fn log_detail(&self, base: &str) -> String {
        let note = match &self.undone {
            Some(_) => None,
            None => match (&self.capture.restore, &self.capture.note) {
                (_, Some(note)) => Some(note.clone()),
                _ => None,
            },
        };
        match note {
            Some(note) => format!("{base} ({note})"),
            None => base.to_string(),
        }
    }

    /// Assemble the bookkeeping for a succeeded op.
    pub fn into_op_aftermath(
        self,
        repo_id: String,
        label: String,
        verb: String,
    ) -> Option<OpAftermath> {
        if let Some(undone) = self.undone {
            let redo = undone.redo.map(|restore| UndoEntry {
                repo_id: repo_id.clone(),
                label: undone.label.clone(),
                verb: undone.verb.clone(),
                restore,
            });
            return Some(OpAftermath::Undone {
                is_redo: undone.is_redo,
                redo,
            });
        }
        self.capture.into_aftermath(repo_id, label, verb)
    }
}

/// The transient info-toned alert offering Undo after an undoable op.
#[derive(Clone, Debug)]
pub(super) struct UndoToast {
    pub log_id: u64,
    pub repo_id: String,
}

/// Entries kept per repository (oldest dropped first).
pub(super) const UNDO_STACK_CAP: usize = 20;

/// Recently-discarded entries kept per repository.
pub(super) const DISCARDED_CAP: usize = 20;

/// Drain a discarded list to its cap (pure, unit-tested).
fn cap_discarded(list: &mut Vec<DiscardedBackup>) {
    if list.len() > DISCARDED_CAP {
        let excess = list.len() - DISCARDED_CAP;
        list.drain(..excess);
    }
}

/// Push with the per-repository cap (pure, unit-tested).
pub(super) fn push_capped(stack: &mut Vec<UndoEntry>, entry: UndoEntry) {
    stack.push(entry);
    if stack.len() > UNDO_STACK_CAP {
        let excess = stack.len() - UNDO_STACK_CAP;
        stack.drain(..excess);
    }
}

/// `12 B`, `3.1 KiB`, `2.0 MiB` — pure, unit-tested.
pub(super) fn format_bytes(n: u64) -> String {
    const KI: f64 = 1024.0;
    if n < 1024 {
        return format!("{n} B");
    }
    let kib = n as f64 / KI;
    if kib < 1024.0 {
        return format!("{kib:.1} KiB");
    }
    format!("{:.1} MiB", kib / KI)
}

/// One discarded file: raw bytes for Git reads, display form for rows.
#[derive(Clone, Debug)]
pub(super) struct DiscardedFile {
    pub path: Vec<u8>,
    pub display: String,
    pub untracked: bool,
}

/// Display metadata captured with a discard backup (worker thread).
#[derive(Clone, Debug)]
pub(super) struct DiscardedMeta {
    pub files: Vec<DiscardedFile>,
    pub size_line: String,
}

/// One Recently-discarded entry: what, when, how big, how to bring it back.
#[derive(Clone, Debug)]
pub(super) struct DiscardedBackup {
    pub id: u64,
    pub repo_id: String,
    pub label: String,
    pub at: std::time::Instant,
    pub files: Vec<DiscardedFile>,
    pub size_line: String,
    pub restore: UndoRestore,
}

/// A backup captured before the mutation, plus how it becomes an entry.
pub(super) struct UndoCapture {
    backup_refs: Vec<String>,
    restore: Option<UndoRestore>,
    note: Option<String>,
    discarded: Option<DiscardedMeta>,
}

impl UndoCapture {
    pub fn empty() -> Self {
        Self {
            backup_refs: Vec::new(),
            restore: None,
            note: None,
            discarded: None,
        }
    }

    fn ready(refs: Vec<String>, restore: UndoRestore) -> Self {
        Self {
            backup_refs: refs,
            restore: Some(restore),
            note: None,
            discarded: None,
        }
    }

    /// A discard backup: also lands in Recently-discarded.
    fn ready_discarded(
        refs: Vec<String>,
        restore: UndoRestore,
        discarded: DiscardedMeta,
    ) -> Self {
        Self {
            backup_refs: refs,
            restore: Some(restore),
            note: None,
            discarded: Some(discarded),
        }
    }

    fn ready_noted(refs: Vec<String>, restore: UndoRestore, note: String) -> Self {
        Self {
            backup_refs: refs,
            restore: Some(restore),
            note: Some(note),
            discarded: None,
        }
    }

    fn note(reason: &str) -> Self {
        Self {
            backup_refs: Vec::new(),
            restore: None,
            note: Some(t().log_no_undo(reason)),
            discarded: None,
        }
    }

    /// After the op succeeded: pin every HEAD move to the branch and HEAD
    /// it left behind. An unreadable HEAD drops the undo rather than keeping
    /// an unguarded `reset --hard`.
    pub fn seal(&mut self, worktree: &str) {
        fn needs_guard(restore: &UndoRestore) -> bool {
            match restore {
                UndoRestore::ResetHead { .. } => true,
                UndoRestore::Sequence(steps) => steps.iter().any(needs_guard),
                _ => false,
            }
        }
        fn apply(restore: &mut UndoRestore, guard: &HeadGuard) {
            match restore {
                UndoRestore::ResetHead { guard: slot, .. } => *slot = Some(guard.clone()),
                UndoRestore::Sequence(steps) => steps.iter_mut().for_each(|s| apply(s, guard)),
                _ => {}
            }
        }
        let Some(restore) = self.restore.as_mut().filter(|r| needs_guard(r)) else {
            return;
        };
        let guard = crate::git::current_branch_name(worktree).and_then(|branch| {
            Ok(HeadGuard {
                branch,
                head: crate::git::rev_parse(worktree, "HEAD")?,
            })
        });
        match guard {
            Ok(guard) => apply(restore, &guard),
            Err(_) => {
                self.cleanup(worktree);
                self.restore = None;
                self.discarded = None;
                self.note = Some(t().log_no_undo("could not read HEAD"));
            }
        }
    }

    /// Drop backup refs after a failed op: no undo, no litter.
    pub fn cleanup(&self, worktree: &str) {
        for name in &self.backup_refs {
            let _ = crate::git::delete_backup_ref(worktree, name);
        }
    }

    /// Assemble the aftermath for a succeeded op. `repo_id`/`label`/`verb`
    /// only matter when an entry exists.
    pub fn into_aftermath(
        self,
        repo_id: String,
        label: String,
        verb: String,
    ) -> Option<OpAftermath> {
        self.restore.map(|restore| {
            OpAftermath::UndoReady {
                entry: UndoEntry {
                    repo_id,
                    label,
                    verb,
                    restore,
                },
                discarded: self.discarded,
            }
        })
    }
}

/// Pin one object id under a backup ref; `None` when the write fails (the op
/// proceeds without undo rather than blocking on plumbing).
fn keep_ref(worktree: &str, oid: &str, label: &str) -> Option<String> {
    crate::git::write_backup_ref(worktree, oid, label).ok()
}

/// Stash entry message for a dropped entry: the original message when the
/// details cache still has it, so the restored entry reads the same.
pub(super) fn stash_message_for(
    details: &HashMap<String, crate::git::RepoDetails>,
    repo_id: &str,
    id: &str,
) -> Option<String> {
    details
        .get(repo_id)?
        .stashes
        .iter()
        .find(|(entry_id, _, _)| entry_id == id)
        .map(|(_, _, message)| message.clone())
}

/// Capture the backup for one queued op. Runs on the worker thread BEFORE the
/// mutation (the queue is sequential, so nothing interleaves). Anything that
/// finds nothing to save (a clean discard, an already-gone file) yields an
/// empty capture: no entry, no note, no noise.
pub(super) fn capture_backup(
    worktree: &str,
    op: &changes::ChangeOp,
    label: &str,
    stash_message: Option<&str>,
) -> UndoCapture {
    match op {
        changes::ChangeOp::DeleteBranch(op) => match crate::git::rev_parse(worktree, &op.name) {
            Ok(tip) => match keep_ref(worktree, &tip, &format!("branch {}", op.name)) {
                Some(name) => UndoCapture::ready(
                    vec![name],
                    UndoRestore::RecreateBranch {
                        name: op.name.clone(),
                        oid: tip,
                    },
                ),
                None => UndoCapture::note("could not write the backup ref"),
            },
            Err(_) => UndoCapture::note("could not read the branch tip"),
        },
        changes::ChangeOp::Paths(op) => match op.kind {
            changes::ChangeOpKind::Discard { staged, untracked } => {
                if untracked {
                    capture_untracked_discard(worktree, &op.paths)
                } else {
                    capture_tracked_discard(worktree, staged, label, &op.paths)
                }
            }
            _ => UndoCapture::empty(),
        },
        changes::ChangeOp::Hunk(hunk) => {
            if !hunk.reverse {
                return UndoCapture::empty();
            }
            match crate::git::hash_blob(worktree, &hunk.patch) {
                Ok(blob) => match keep_ref(worktree, &blob, "hunk") {
                    Some(name) => {
                        let mut files: Vec<DiscardedFile> =
                            crate::git::patch_file_list(&String::from_utf8_lossy(&hunk.patch))
                                .into_iter()
                                .map(|display| DiscardedFile {
                                    path: display.as_bytes().to_vec(),
                                    display,
                                    untracked: false,
                                })
                                .collect();
                        // A hunk patch always names its file, but the list
                        // must never render empty.
                        if files.is_empty() {
                            files.push(DiscardedFile {
                                path: b"hunk".to_vec(),
                                display: "hunk".to_string(),
                                untracked: false,
                            });
                        }
                        UndoCapture::ready_discarded(
                            vec![name],
                            UndoRestore::ReapplyHunk {
                                patch: hunk.patch.clone(),
                                cached: hunk.cached,
                            },
                            DiscardedMeta {
                                files,
                                size_line: format_bytes(hunk.patch.len() as u64),
                            },
                        )
                    }
                    None => UndoCapture::note("could not write the backup ref"),
                },
                Err(_) => UndoCapture::note("could not back up the hunk"),
            }
        }
        changes::ChangeOp::StashDrop(op) => match crate::git::rev_parse(worktree, &op.id) {
            Ok(oid) => match keep_ref(worktree, &oid, &format!("stash {}", op.id)) {
                Some(name) => UndoCapture::ready(
                    vec![name],
                    UndoRestore::RestashEntry {
                        oid,
                        message: stash_message
                            .filter(|message| !message.trim().is_empty())
                            .unwrap_or("Spur undo")
                            .to_string(),
                    },
                ),
                None => UndoCapture::note("could not write the backup ref"),
            },
            Err(_) => UndoCapture::note("could not read the stash entry"),
        },
        changes::ChangeOp::CherryPick(op) => capture_head(worktree, &op.short),
        changes::ChangeOp::Revert(op) => capture_head(worktree, &op.short),
        changes::ChangeOp::Rebase(op) => {
            capture_head(worktree, &format!("rebase onto {}", op.onto))
        }
        changes::ChangeOp::Reset(op) => capture_reset(worktree, op, label),
        changes::ChangeOp::TagDelete(op) => {
            if !op.local {
                return UndoCapture::note("remote tag deletions are not undoable");
            }
            match crate::git::rev_parse(worktree, &op.name) {
                Ok(oid) => match keep_ref(worktree, &oid, &format!("tag {}", op.name)) {
                    Some(name) => {
                        let capture = UndoRestore::RestoreTag {
                            name: op.name.clone(),
                            oid,
                        };
                        if op.remotes.is_empty() {
                            UndoCapture::ready(vec![name], capture)
                        } else {
                            UndoCapture::ready_noted(
                                vec![name],
                                capture,
                                t().log_no_undo("remote tag deletions are not undoable"),
                            )
                        }
                    }
                    None => UndoCapture::note("could not write the backup ref"),
                },
                Err(_) => UndoCapture::note("could not read the tag"),
            }
        }
        _ => UndoCapture::empty(),
    }
}

/// HEAD backup for commit-creating ops (cherry-pick, revert).
fn capture_head(worktree: &str, short: &str) -> UndoCapture {
    match crate::git::rev_parse(worktree, "HEAD") {
        Ok(head) => match keep_ref(worktree, &head, &format!("HEAD before {short}")) {
            Some(name) => UndoCapture::ready(vec![name], UndoRestore::ResetHead { oid: head, guard: None }),
            None => UndoCapture::note("could not write the backup ref"),
        },
        Err(_) => UndoCapture::note("could not read HEAD"),
    }
}

/// Reset backup: the live HEAD always; plus the dirty changes for a hard
/// reset (soft/mixed lose no worktree content). The identity check runs here
/// and again at execution: a moved branch refuses in both places.
fn capture_reset(worktree: &str, op: &changes::ResetOp, label: &str) -> UndoCapture {
    if crate::git::verify_reset_identity(worktree, op.branch.as_deref(), &op.head).is_err() {
        return UndoCapture::empty();
    }
    let head = match crate::git::rev_parse(worktree, "HEAD") {
        Ok(head) => head,
        Err(_) => return UndoCapture::note("could not read HEAD"),
    };
    let Some(head_ref) = keep_ref(worktree, &head, label) else {
        return UndoCapture::note("could not write the backup ref");
    };
    if op.mode != crate::git::ResetMode::Hard {
        return UndoCapture::ready(vec![head_ref], UndoRestore::ResetHead { oid: head, guard: None });
    }
    match crate::git::stash_create_id(worktree) {
        // Clean tree: moving HEAD back is the whole undo.
        Ok(None) => UndoCapture::ready(vec![head_ref], UndoRestore::ResetHead { oid: head, guard: None }),
        Ok(Some(oid)) => match keep_ref(worktree, &oid, label) {
            Some(stash_ref) => UndoCapture::ready(
                vec![head_ref, stash_ref],
                UndoRestore::Sequence(vec![
                    UndoRestore::ResetHead { oid: head, guard: None },
                    // `--index` applies: the reset above restored the exact
                    // index the stash was created against.
                    UndoRestore::ApplyStashed {
                        oid,
                        message: format!("Spur undo: {label}"),
                        staged: true,
                    },
                ]),
            ),
            None => UndoCapture::note("could not write the backup ref"),
        },
        Err(_) => UndoCapture::note("could not back up the changes"),
    }
}

/// Tracked discard: a `git stash create` commit holds the changes without
/// touching the stash list. `None` means a clean tree — the discard is a
/// no-op, so no entry and no note.
fn capture_tracked_discard(
    worktree: &str,
    staged: bool,
    label: &str,
    paths: &[Vec<u8>],
) -> UndoCapture {
    let files: Vec<DiscardedFile> = paths
        .iter()
        .map(|path| DiscardedFile {
            path: path.clone(),
            display: String::from_utf8_lossy(path).into_owned(),
            untracked: false,
        })
        .collect();
    match crate::git::stash_create_id(worktree) {
        Ok(None) => UndoCapture::empty(),
        Ok(Some(oid)) => match keep_ref(worktree, &oid, label) {
            Some(name) => {
                // Size line: added/deleted totals, falling back to the file
                // count when numstat fails (the backup itself is fine).
                let size_line = crate::git::stash_numstat(worktree, &oid, paths)
                    .map(|(added, deleted)| format!("+{added} -{deleted}"))
                    .unwrap_or_else(|_| t().discarded_files(files.len()));
                UndoCapture::ready_discarded(
                    vec![name],
                    UndoRestore::RestorePaths {
                        oid,
                        paths: paths.to_vec(),
                        staged,
                    },
                    DiscardedMeta { files, size_line },
                )
            }
            None => UndoCapture::note("could not write the backup ref"),
        },
        Err(_) => UndoCapture::note("could not back up the changes"),
    }
}

/// Untracked discard: every file becomes a blob under one backup ref each.
/// Already-gone files are skipped; any real failure drops the partial refs
/// and notes the missing undo instead of pretending.
fn capture_untracked_discard(worktree: &str, paths: &[Vec<u8>]) -> UndoCapture {
    let mut files = Vec::new();
    let mut blobs = Vec::new();
    let mut refs = Vec::new();
    let mut bytes_total: u64 = 0;
    for path in paths {
        let bytes = match crate::git::read_untracked_for_undo(worktree, path) {
            Ok(None) => continue,
            Ok(Some(bytes)) => bytes,
            Err(_) => {
                UndoCapture {
                    backup_refs: refs,
                    restore: None,
                    note: None,
                    discarded: None,
                }
                .cleanup(worktree);
                return UndoCapture::note("could not back up every file");
            }
        };
        bytes_total += bytes.len() as u64;
        let blob = match crate::git::hash_blob(worktree, &bytes) {
            Ok(blob) => blob,
            Err(_) => {
                UndoCapture {
                    backup_refs: refs,
                    restore: None,
                    note: None,
                    discarded: None,
                }
                .cleanup(worktree);
                return UndoCapture::note("could not back up every file");
            }
        };
        match keep_ref(worktree, &blob, "untracked") {
            Some(name) => {
                refs.push(name);
                blobs.push((path.clone(), blob));
                files.push(DiscardedFile {
                    path: path.clone(),
                    display: String::from_utf8_lossy(path).into_owned(),
                    untracked: true,
                });
            }
            None => {
                UndoCapture {
                    backup_refs: refs,
                    restore: None,
                    note: None,
                    discarded: None,
                }
                .cleanup(worktree);
                return UndoCapture::note("could not write the backup ref");
            }
        }
    }
    if files.is_empty() {
        UndoCapture::empty()
    } else {
        UndoCapture::ready_discarded(
            refs,
            UndoRestore::RewriteUntracked { files: blobs },
            DiscardedMeta {
                files,
                size_line: format_bytes(bytes_total),
            },
        )
    }
}

/// Replay one restore. Ref-moving restores return their redo inverse;
/// content restores return `None` (redo covers ref moves only). Guards refuse
/// instead of destroying work the user did after the op.
pub(super) fn run_undo_restore(
    worktree: &str,
    restore: &UndoRestore,
) -> Result<Option<UndoRestore>, String> {
    match restore {
        UndoRestore::ResetHead { oid, guard } => {
            if let Some(guard) = guard {
                crate::git::verify_reset_identity(worktree, guard.branch.as_deref(), &guard.head)
                    .map_err(|_| t().undo_target_moved().to_string())?;
            }
            if !crate::git::status_snapshot(worktree)?.is_clean() {
                return Err(t().undo_needs_clean_worktree().to_string());
            }
            let current = crate::git::rev_parse(worktree, "HEAD")?;
            let branch = crate::git::current_branch_name(worktree)?;
            crate::git::reset_hard_to(worktree, oid)?;
            Ok(Some(UndoRestore::ResetHead {
                oid: current,
                guard: Some(HeadGuard {
                    branch,
                    head: oid.clone(),
                }),
            }))
        }
        UndoRestore::RestorePaths { oid, paths, staged } => {
            // Only the discarded paths must be untouched since; the rest of
            // the tree may carry any other work.
            if !crate::git::paths_unmodified(worktree, paths, *staged)? {
                return Err(t().undo_target_moved().to_string());
            }
            crate::git::restore_paths_from_stash(worktree, oid, paths, *staged)?;
            Ok(None)
        }
        UndoRestore::RecreateBranch { name, oid } => {
            crate::git::recreate_branch_at(worktree, name, oid)?;
            Ok(Some(UndoRestore::DeleteBranch {
                name: name.clone(),
                oid: oid.clone(),
            }))
        }
        UndoRestore::DeleteBranch { name, oid } => {
            if crate::git::rev_parse(worktree, name)? != *oid {
                return Err(t().undo_target_moved().to_string());
            }
            crate::git::delete_branch(worktree, name, true)?;
            Ok(None)
        }
        UndoRestore::RestoreTag { name, oid } => {
            crate::git::restore_tag_to(worktree, name, oid)?;
            Ok(Some(UndoRestore::DeleteTag {
                name: name.clone(),
                oid: oid.clone(),
            }))
        }
        UndoRestore::DeleteTag { name, oid } => {
            if crate::git::rev_parse(worktree, name)? != *oid {
                return Err(t().undo_target_moved().to_string());
            }
            crate::git::delete_tag(worktree, name, true, &[])?;
            Ok(None)
        }
        UndoRestore::ApplyStashed {
            oid,
            message,
            staged,
        } => {
            if !crate::git::status_snapshot(worktree)?.is_clean() {
                return Err(t().undo_needs_clean_worktree().to_string());
            }
            crate::git::stash_store_id(worktree, oid, message)?;
            // `--index` keeps staged-ness; when the index moved since, plain
            // apply still restores the worktree.
            let applied = if *staged {
                crate::git::stash_apply_index(worktree, "stash@{0}", true)
                    .or_else(|_| crate::git::stash_apply(worktree, "stash@{0}"))
            } else {
                crate::git::stash_apply(worktree, "stash@{0}")
            };
            match applied {
                Ok(()) => {
                    // Drop the temp entry only while it is still on top: a
                    // concurrent stash must never be dropped by undo.
                    if crate::git::rev_parse(worktree, "stash@{0}").is_ok_and(|top| top == *oid)
                    {
                        let _ = crate::git::stash_drop(worktree, "stash@{0}");
                    }
                    Ok(None)
                }
                Err(err) => Err(format!("{err} (kept as a stash entry)")),
            }
        }
        UndoRestore::RewriteUntracked { files } => {
            for (path, blob) in files {
                let bytes = crate::git::cat_blob(worktree, blob)?;
                crate::git::restore_untracked_file(worktree, path, &bytes).map_err(|err| {
                    format!("{}: {err}", String::from_utf8_lossy(path))
                })?;
            }
            Ok(None)
        }
        UndoRestore::ReapplyHunk { patch, cached } => {
            crate::git::apply_patch(worktree, patch, *cached, false)?;
            Ok(None)
        }
        UndoRestore::RestashEntry { oid, message } => {
            crate::git::stash_store_id(worktree, oid, message)?;
            Ok(Some(UndoRestore::DropStash { oid: oid.clone() }))
        }
        UndoRestore::DropStash { oid } => {
            if crate::git::rev_parse(worktree, "stash@{0}")? != *oid {
                return Err(t().undo_target_moved().to_string());
            }
            crate::git::stash_drop(worktree, "stash@{0}")?;
            Ok(None)
        }
        UndoRestore::Sequence(steps) => {
            for step in steps {
                run_undo_restore(worktree, step)?;
            }
            Ok(None)
        }
    }
}

impl SpurShell {
    /// Record an undo entry; a new undoable op clears the one redo.
    pub(super) fn push_undo_entry(&mut self, entry: UndoEntry) {
        let repo = entry.repo_id.clone();
        push_capped(self.undo_stacks.entry(repo.clone()).or_default(), entry);
        self.redo_slots.remove(&repo);
    }

    pub(super) fn undo_top_label(&self, repo_id: &str) -> Option<String> {
        self.undo_stacks
            .get(repo_id)
            .and_then(|stack| stack.last())
            .map(|entry| entry.label.clone())
    }

    pub(super) fn redo_label(&self, repo_id: &str) -> Option<String> {
        self.redo_slots.get(repo_id).map(|entry| entry.label.clone())
    }

    /// Pop the newest entry and run it through the sequential queue.
    pub(super) fn queue_undo_for(&mut self, repo_id: String, cx: &mut Context<Self>) {
        let Some(entry) = self
            .undo_stacks
            .get_mut(&repo_id)
            .and_then(|stack| stack.pop())
        else {
            self.note_error(t().undo_empty().to_string(), cx);
            return;
        };
        self.dismiss_undo_toast(cx);
        self.change_ops.push_back(changes::ChangeOp::Undo(UndoOp {
            repo_id,
            entry,
            is_redo: false,
            from_stack: true,
        }));
        self.pump_change_ops(cx);
    }

    /// Put a failed (or refused) undo/redo back, so fixing the cause and
    /// retrying works instead of silently moving on to an older entry.
    pub(super) fn requeue_failed_undo(&mut self, op: UndoOp) {
        if op.is_redo {
            self.redo_slots.insert(op.repo_id, op.entry);
        } else {
            push_capped(self.undo_stacks.entry(op.repo_id).or_default(), op.entry);
        }
    }

    /// Replay the single redo step, if one is banked.
    pub(super) fn queue_redo_for(&mut self, repo_id: String, cx: &mut Context<Self>) {
        let Some(entry) = self.redo_slots.remove(&repo_id) else {
            self.note_error(t().redo_empty().to_string(), cx);
            return;
        };
        self.dismiss_undo_toast(cx);
        self.change_ops.push_back(changes::ChangeOp::Undo(UndoOp {
            repo_id,
            entry,
            is_redo: true,
            from_stack: true,
        }));
        self.pump_change_ops(cx);
    }

    /// `Ctrl+Alt+Z`: undo in the active repository (no repo, no-op).
    pub(super) fn undo_active_repo(&mut self, cx: &mut Context<Self>) {
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        self.queue_undo_for(repo_id, cx);
    }

    /// Info-toned transient alert for an undoable op: the log detail plus an
    /// Undo button. Shares the configured alert lifetime with errors.
    pub(super) fn raise_undo_toast(
        &mut self,
        repo_id: &str,
        log_id: u64,
        cx: &mut Context<Self>,
    ) {
        self.undo_toast_gen = self.undo_toast_gen.wrapping_add(1);
        let generation = self.undo_toast_gen;
        self.undo_toast = Some(UndoToast {
            log_id,
            repo_id: repo_id.to_string(),
        });
        cx.notify();
        let Some(timeout) = self.alert_timeout.duration() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(timeout).await;
            this.update(cx, |this, cx| {
                if this.undo_toast_gen == generation {
                    this.undo_toast = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn dismiss_undo_toast(&mut self, cx: &mut Context<Self>) {
        if self.undo_toast.take().is_some() {
            self.undo_toast_gen = self.undo_toast_gen.wrapping_add(1);
            cx.notify();
        }
    }

    /// Record a Recently-discarded entry. Restores run through the undo
    /// queue, so the same guards apply; the backup stays listed afterwards.
    pub(super) fn push_discarded(
        &mut self,
        repo_id: String,
        label: String,
        meta: DiscardedMeta,
        restore: UndoRestore,
    ) {
        self.next_discarded_id += 1;
        let id = self.next_discarded_id;
        let list = self.discarded.entry(repo_id.clone()).or_default();
        list.push(DiscardedBackup {
            id,
            repo_id,
            label,
            at: std::time::Instant::now(),
            files: meta.files,
            size_line: meta.size_line,
            restore,
        });
        cap_discarded(list);
        // Drop cached diffs of evicted backups.
        let live: HashSet<u64> = list.iter().map(|backup| backup.id).collect();
        self.discarded_diffs.retain(|(id, _), _| live.contains(id));
        if self.discarded_expanded.is_some_and(|open| !live.contains(&open)) {
            self.discarded_expanded = None;
            self.discarded_file = None;
        }
    }

    pub(super) fn discarded_for(&self, repo_id: &str) -> &[DiscardedBackup] {
        self.discarded
            .get(repo_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(super) fn open_discarded(&mut self, cx: &mut Context<Self>) {
        self.discarded_repo = self.active_repo_id();
        self.discarded_open = true;
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn close_discarded(&mut self, cx: &mut Context<Self>) {
        if self.discarded_open {
            self.close_modal(cx);
        }
    }

    fn find_discarded(&self, id: u64) -> Option<DiscardedBackup> {
        self.discarded
            .values()
            .flatten()
            .find(|backup| backup.id == id)
            .cloned()
    }

    /// Restore one backup through the sequential queue (the same path as
    /// Undo, so the guards apply; the backup stays listed afterwards).
    pub(super) fn restore_discarded(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(backup) = self.find_discarded(id) else {
            return;
        };
        self.close_modal(cx);
        self.change_ops.push_back(changes::ChangeOp::Undo(UndoOp {
            repo_id: backup.repo_id.clone(),
            entry: UndoEntry {
                repo_id: backup.repo_id,
                label: backup.label,
                verb: "restore".to_string(),
                restore: backup.restore,
            },
            is_redo: false,
            from_stack: false,
        }));
        self.pump_change_ops(cx);
    }

    pub(super) fn toggle_discarded_backup(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.discarded_expanded == Some(id) {
            self.discarded_expanded = None;
            self.discarded_file = None;
        } else {
            self.discarded_expanded = Some(id);
            self.discarded_file = None;
        }
        cx.notify();
    }

    /// Toggle one backup file's diff: hunk patches parse inline, everything
    /// else reads on the worker thread.
    pub(super) fn open_discarded_file(&mut self, id: u64, ix: usize, cx: &mut Context<Self>) {
        if self.discarded_file == Some((id, ix)) {
            self.discarded_file = None;
            cx.notify();
            return;
        }
        let Some(backup) = self.find_discarded(id) else {
            return;
        };
        let Some(file) = backup.files.get(ix).cloned() else {
            return;
        };
        if let UndoRestore::ReapplyHunk { patch, .. } = &backup.restore {
            self.discarded_diffs.insert(
                (id, ix),
                Rc::new(crate::git::parse_unified_diff(patch)),
            );
            self.discarded_file = Some((id, ix));
            cx.notify();
            return;
        }
        let worktree = match self.overview.row(&backup.repo_id) {
            Some(row) => row.path.to_string_lossy().into_owned(),
            None => {
                self.note_error(t().log_nothing_to_reveal(), cx);
                return;
            }
        };
        enum DiffSource {
            Stash { oid: String },
            Blob { oid: String },
        }
        let source = match &backup.restore {
            UndoRestore::ApplyStashed { oid, .. } | UndoRestore::RestorePaths { oid, .. } => {
                DiffSource::Stash { oid: oid.clone() }
            }
            UndoRestore::RewriteUntracked { files } => match files
                .iter()
                .find(|(path, _)| *path == file.path)
                .map(|(_, oid)| oid.clone())
            {
                Some(oid) => DiffSource::Blob { oid },
                None => {
                    self.note_error(
                        t().log_action_failed("show diff", "file is no longer backed up"),
                        cx,
                    );
                    return;
                }
            },
            _ => return,
        };
        let path = file.path.clone();
        let untracked = file.untracked;
        cx.spawn(async move |this, cx| {
            let diff = cx
                .background_executor()
                .spawn(async move {
                    match source {
                        DiffSource::Stash { oid } => {
                            crate::git::stash_file_diff(&worktree, &oid, &path, untracked)
                        }
                        DiffSource::Blob { oid } => {
                            let bytes = crate::git::cat_blob(&worktree, &oid)?;
                            Ok(crate::git::all_added_diff(bytes))
                        }
                    }
                })
                .await;
            this.update(
                cx,
                |this, cx| match diff {
                    Ok(diff) => {
                        // The dialog may have closed or the backup pruned
                        // meanwhile: only land when it is still listed.
                        if this.discarded_open && this.find_discarded(id).is_some() {
                            this.discarded_diffs.insert((id, ix), Rc::new(diff));
                            this.discarded_file = Some((id, ix));
                        }
                        cx.notify();
                    }
                    Err(err) => {
                        this.note_error(t().log_action_failed("show diff", &err), cx);
                    }
                },
            )
            .ok();
        })
        .detach();
    }

    /// Recently-discarded modal: backups newest-first, each expandable to its
    /// files with a read-only diff, plus a Restore button per backup.
    pub(super) fn render_discarded_modal(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let entity = cx.entity().downgrade();
        let backups: Vec<DiscardedBackup> = self
            .discarded_repo
            .as_deref()
            .map(|repo| self.discarded_for(repo).to_vec())
            .unwrap_or_default();
        let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
        for backup in backups.iter().rev() {
            let id = backup.id;
            let expanded = self.discarded_expanded == Some(id);
            let files_label = if backup.files.len() == 1 {
                truncate_path(&backup.files[0].display, 44)
            } else {
                t().discarded_files(backup.files.len())
            };
            let meta = t().discarded_meta(
                &files_label,
                &backup.size_line,
                &ago_label(backup.at.elapsed()),
            );
            let open_entity = entity.clone();
            let restore_entity = entity.clone();
            let mut row = div()
                .id(("discarded-row", id))
                .flex()
                .flex_col()
                .px(px(12.))
                .py(px(5.))
                .gap(px(2.))
                .rounded(px(6.))
                .mx(px(6.))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_discarded_backup(id, cx)
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .font_family(MONO)
                                .text_size(px(TEXT_XS))
                                .text_color(text_muted(cx))
                                .child(backup.label.clone()),
                        )
                        .child(
                            Button::new(("discarded-restore", id))
                                .label(t().discarded_restore())
                                .small()
                                .outline()
                                .cursor_pointer()
                                .on_click(move |_, _, cx| {
                                    restore_entity
                                        .update(cx, |this, cx| this.restore_discarded(id, cx))
                                        .ok();
                                }),
                        )
                        .children(expanded.then(|| {
                            div().flex_none().child(
                                Icon::new(IconName::ChevronDown)
                                    .size(px(12.))
                                    .text_color(text_faint(cx)),
                            )
                        }))
                        .children((!expanded).then(|| {
                            div().flex_none().child(
                                Icon::new(IconName::ChevronRight)
                                    .size(px(12.))
                                    .text_color(text_faint(cx)),
                            )
                        })),
                )
                .child(
                    div()
                        .pl(px(2.))
                        .overflow_hidden()
                        .text_size(px(TEXT_SM))
                        .text_color(text_muted(cx))
                        .child(meta),
                );
            if expanded {
                for (ix, file) in backup.files.iter().enumerate() {
                    let open = self.discarded_file == Some((id, ix));
                    let open_entity = open_entity.clone();
                    row = row.child(
                        div()
                            // One backup expands at a time, so the file index
                            // is a unique element id.
                            .id(("discarded-file", ix))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .pl(px(14.))
                            .py(px(3.))
                            .cursor_pointer()
                            .on_click(move |_, _, cx| {
                                open_entity
                                    .update(cx, |this, cx| this.open_discarded_file(id, ix, cx))
                                    .ok();
                            })
                            .child(
                                Icon::new(if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(12.))
                                .text_color(text_faint(cx)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .font_family(MONO)
                                    .text_size(px(TEXT_XS))
                                    .text_color(text_primary(cx))
                                    .child(file.display.clone()),
                            ),
                    );
                    if open
                        && let Some(diff) = self.discarded_diffs.get(&(id, ix))
                    {
                        row = row.child(
                            div()
                                .h(px(240.))
                                .mx(px(6.))
                                .rounded(px(6.))
                                .bg(ink(0.04))
                                .overflow_hidden()
                                .child(readonly_diff_list(
                                    ReadonlyDiffIds {
                                        list: "discarded-diff",
                                        v_scrollbar: "discarded-diff-v",
                                        h_wrapper: "discarded-diff-h",
                                        h_scrollbar: "discarded-diff-hbar",
                                    },
                                    diff.clone(),
                                    &self.discarded_vscroll,
                                    &self.discarded_hscroll,
                                )
                            ));
                    }
                }
            }
            rows.push(row.into_any_element());
        }
        if rows.is_empty() {
            rows.push(empty_note(t().discarded_empty(), cx).into_any_element());
        }
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
                cx.listener(|this, _, _, cx| this.close_discarded(cx)),
            )
            .child(
                div()
                    .w(px(520.))
                    .max_h(px(560.))
                    .rounded(px(PANEL_RADIUS))
                    .border_1()
                    .border_color(hairline(0.10))
                    .bg(cx.theme().popover)
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(12.))
                            .h(px(36.))
                            .border_b_1()
                            .border_color(hairline(0.05))
                            .child(
                                Icon::new(IconName::ArchiveRestore)
                                    .size(px(14.))
                                    .text_color(violet(cx)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(TEXT_MD))
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(text_primary(cx))
                                    .child(t().discarded_title()),
                            )
                            .child(
                                div()
                                    .id("discarded-close")
                                    .aria_label(t().discarded_title())
                                    .cursor_pointer()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .w(px(18.))
                                    .h(px(18.))
                                    .rounded(px(4.))
                                    .bg(hover_blend(
                                        "discarded-close",
                                        ink(0.0),
                                        ink(0.10),
                                    ))
                                    .on_hover(hover_listener("discarded-close"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_discarded(cx)
                                    }))
                                    .child(
                                        Icon::new(IconName::Close)
                                            .size(px(12.))
                                            .text_color(text_muted(cx)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("discarded-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.discarded_scroll)
                            .py(px(6.))
                            .flex()
                            .flex_col()
                            .children(rows),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::wsl_support as wsl;

    fn entry(label: &str) -> UndoEntry {
        UndoEntry {
            repo_id: "repo".to_string(),
            label: label.to_string(),
            verb: "test".to_string(),
            restore: UndoRestore::ResetHead {
                oid: "abc".to_string(),
                guard: None,
            },
        }
    }

    #[test]
    fn undo_stack_caps_at_twenty_keeping_the_newest() {
        let mut stack = Vec::new();
        for n in 0..UNDO_STACK_CAP + 5 {
            push_capped(&mut stack, entry(&format!("op {n}")));
        }
        assert_eq!(stack.len(), UNDO_STACK_CAP);
        assert_eq!(stack.first().unwrap().label, "op 5");
        assert_eq!(stack.last().unwrap().label, format!("op {}", UNDO_STACK_CAP + 4));
    }

    #[test]
    fn empty_capture_maps_to_no_aftermath() {
        let capture = UndoCapture::empty();
        assert!(
            capture
                .into_aftermath("r".into(), "l".into(), "v".into())
                .is_none()
        );
    }

    #[test]
    fn worker_aftermath_carries_notes_onto_the_log_line() {
        let aftermath = WorkerAftermath {
            capture: UndoCapture::note("remote deletions are gone"),
            undone: None,
        };
        assert_eq!(
            aftermath.log_detail("deleted tag v"),
            "deleted tag v (no undo: remote deletions are gone)"
        );
        let plain = WorkerAftermath {
            capture: UndoCapture::empty(),
            undone: None,
        };
        assert_eq!(plain.log_detail("staged a.txt"), "staged a.txt");
    }

    #[test]
    fn undone_assembles_the_single_redo_entry() {
        let aftermath = WorkerAftermath {
            capture: UndoCapture::empty(),
            undone: Some(UndoneInfo {
                is_redo: false,
                redo: Some(UndoRestore::ResetHead {
                    oid: "cur".to_string(),
                    guard: None,
                }),
                label: "cherry-picked abc".to_string(),
                verb: "undo".to_string(),
            }),
        };
        let Some(OpAftermath::Undone { is_redo, redo }) =
            aftermath.into_op_aftermath("r".into(), "l".into(), "v".into())
        else {
            panic!("must assemble the redo");
        };
        assert!(!is_redo);
        let redo = redo.expect("ref restores bank a redo");
        assert_eq!(redo.label, "cherry-picked abc");
        assert_eq!(
            redo.restore,
            UndoRestore::ResetHead {
                oid: "cur".to_string(),
                guard: None,
            }
        );
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn hard_reset_sequence_restores_head_and_dirty_changes() {
        let dir = wsl::temp_dir("undo-reset");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let repo = format!("{dir}/repo");
        wsl::must(&["mkdir", "-p", &repo]);
        wsl::must_env(&home, &["git", "-C", &repo, "init", "-q", "-b", "main"]);
        wsl::write_file(&format!("{repo}/file.txt"), b"one\n");
        wsl::must_env(&home, &["git", "-C", &repo, "add", "file.txt"]);
        wsl::must_env(&home, &["git", "-C", &repo, "commit", "-q", "-m", "init"]);
        let v1 = crate::git::rev_parse(&repo, "HEAD").expect("head");
        wsl::write_file(&format!("{repo}/file.txt"), b"two\n");
        wsl::must_env(&home, &["git", "-C", &repo, "add", "-A"]);
        wsl::must_env(&home, &["git", "-C", &repo, "commit", "-q", "-m", "second"]);
        let v2 = crate::git::rev_parse(&repo, "HEAD").expect("head");

        // A dirty edit, then the hard reset exactly like the queue runs it.
        wsl::write_file(&format!("{repo}/file.txt"), b"edited\n");
        let stash = crate::git::stash_create_id(&repo)
            .expect("create")
            .expect("dirty");
        crate::git::reset_branch(
            &repo,
            &v1,
            crate::git::ResetMode::Hard,
            Some("main"),
            &v2,
        )
        .expect("reset");
        assert_eq!(crate::git::rev_parse(&repo, "HEAD").expect("head"), v1);

        // Undo: HEAD back first, then the stashed edit — the shape
        // `capture_reset` assembles for a dirty hard reset.
        let restore = UndoRestore::Sequence(vec![
            UndoRestore::ResetHead { oid: v2.clone(), guard: None },
            UndoRestore::ApplyStashed {
                oid: stash,
                message: "test".to_string(),
                staged: true,
            },
        ]);
        assert_eq!(run_undo_restore(&repo, &restore).expect("undo"), None);
        assert_eq!(crate::git::rev_parse(&repo, "HEAD").expect("head"), v2);
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/file.txt")]),
            b"edited\n"
        );
        assert!(
            wsl::must(&["git", "-C", &repo, "stash", "list"]).is_empty(),
            "the temp entry must be dropped after a clean apply"
        );

        // The dirty guard refuses a HEAD restore over user changes.
        wsl::write_file(&format!("{repo}/file.txt"), b"more\n");
        assert!(
            run_undo_restore(&repo, &UndoRestore::ResetHead { oid: v1, guard: None }).is_err(),
            "undo must not discard worktree changes"
        );
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn review_fixes_scope_restores_and_guard_head_moves() {
        let dir = wsl::temp_dir("undo-review");
        let home = format!("{dir}/home");
        wsl::must(&["mkdir", "-p", &home]);
        wsl::write_file(
            &format!("{home}/.gitconfig"),
            b"[user]\n\tname = Spur Test\n\temail = spur@test.invalid\n",
        );
        let repo = format!("{dir}/repo");
        wsl::must(&["mkdir", "-p", &repo]);
        wsl::must_env(&home, &["git", "-C", &repo, "init", "-q", "-b", "main"]);
        wsl::write_file(&format!("{repo}/a.txt"), b"a\n");
        wsl::write_file(&format!("{repo}/b.txt"), b"b\n");
        wsl::must_env(&home, &["git", "-C", &repo, "add", "-A"]);
        wsl::must_env(&home, &["git", "-C", &repo, "commit", "-q", "-m", "init"]);
        let v1 = crate::git::rev_parse(&repo, "HEAD").expect("head");

        // Same-second backups get distinct names (one per untracked file).
        let one = crate::git::hash_blob(&repo, b"one").expect("blob");
        let two = crate::git::hash_blob(&repo, b"two").expect("blob");
        let r1 = crate::git::write_backup_ref(&repo, &one, "untracked").expect("ref");
        let r2 = crate::git::write_backup_ref(&repo, &two, "untracked").expect("ref");
        assert_ne!(r1, r2);
        assert_eq!(crate::git::rev_parse(&repo, &r1).expect("kept"), one);

        // Discard a.txt while b.txt stays dirty: restoring brings back only
        // a.txt, with b.txt's live edit untouched.
        wsl::write_file(&format!("{repo}/a.txt"), b"a edited\n");
        wsl::write_file(&format!("{repo}/b.txt"), b"b edited\n");
        let paths = vec![b"a.txt".to_vec()];
        let capture = capture_tracked_discard(&repo, false, "discard a.txt", &paths);
        crate::git::restore_paths(&repo, &paths, false).expect("discard");
        wsl::write_file(&format!("{repo}/b.txt"), b"b later\n");
        let restore = capture.restore.clone().expect("backup");
        assert!(matches!(restore, UndoRestore::RestorePaths { .. }));
        run_undo_restore(&repo, &restore).expect("restore a.txt");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/a.txt")]), b"a edited\n");
        assert_eq!(wsl::must(&["cat", &format!("{repo}/b.txt")]), b"b later\n");
        // Edited again since: the restore refuses instead of overwriting.
        wsl::write_file(&format!("{repo}/a.txt"), b"newer\n");
        assert!(run_undo_restore(&repo, &restore).is_err());
        crate::git::restore_paths(&repo, &[b"a.txt".to_vec(), b"b.txt".to_vec()], false)
            .expect("clean");

        // A sealed HEAD undo refuses on another branch and after new commits.
        wsl::write_file(&format!("{repo}/a.txt"), b"two\n");
        wsl::must_env(&home, &["git", "-C", &repo, "commit", "-qam", "two"]);
        let mut capture = UndoCapture::ready(
            Vec::new(),
            UndoRestore::ResetHead {
                oid: v1.clone(),
                guard: None,
            },
        );
        capture.seal(&repo);
        let restore = capture.restore.clone().expect("sealed");
        wsl::must_env(&home, &["git", "-C", &repo, "switch", "-q", "-c", "side"]);
        assert!(run_undo_restore(&repo, &restore).is_err(), "other branch");
        wsl::must_env(&home, &["git", "-C", &repo, "switch", "-q", "main"]);
        wsl::must_env(&home, &["git", "-C", &repo, "commit", "-q", "--allow-empty", "-m", "3"]);
        assert!(run_undo_restore(&repo, &restore).is_err(), "newer commit");
        wsl::must_env(&home, &["git", "-C", &repo, "reset", "-q", "--hard", "HEAD~1"]);
        run_undo_restore(&repo, &restore).expect("matching guard undoes");
        assert_eq!(crate::git::rev_parse(&repo, "HEAD").expect("head"), v1);

        // Tag restore never repoints a tag recreated since.
        wsl::must_env(&home, &["git", "-C", &repo, "tag", "t", "side"]);
        assert!(crate::git::restore_tag_to(&repo, "t", &v1).is_err());
    }

    #[test]
    fn format_bytes_uses_bytes_kib_and_mib() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(12), "12 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn discarded_list_caps_at_twenty_keeping_the_newest() {
        fn backup(id: u64) -> DiscardedBackup {
            DiscardedBackup {
                id,
                repo_id: "repo".to_string(),
                label: "discarded x".to_string(),
                at: std::time::Instant::now(),
                files: Vec::new(),
                size_line: "1.0 KiB".to_string(),
                restore: UndoRestore::ResetHead {
                    oid: "abc".to_string(),
                    guard: None,
                },
            }
        }
        let mut list = Vec::new();
        for id in 0..DISCARDED_CAP as u64 + 3 {
            list.push(backup(id));
            cap_discarded(&mut list);
        }
        assert_eq!(list.len(), DISCARDED_CAP);
        assert_eq!(list.first().unwrap().id, 3);
        assert_eq!(list.last().unwrap().id, DISCARDED_CAP as u64 + 2);
    }

    #[test]
    fn aftermath_carries_discard_meta_with_the_entry() {
        let capture = UndoCapture::ready_discarded(
            vec!["refs/spur/undo/1_x".to_string()],
            UndoRestore::ResetHead {
                oid: "abc".to_string(),
                guard: None,
            },
            DiscardedMeta {
                files: vec![DiscardedFile {
                    path: b"a.txt".to_vec(),
                    display: "a.txt".to_string(),
                    untracked: false,
                }],
                size_line: "+1 -0".to_string(),
            },
        );
        let Some(OpAftermath::UndoReady { entry, discarded }) =
            capture.into_aftermath("r".into(), "l".into(), "v".into())
        else {
            panic!("must assemble the entry");
        };
        assert_eq!(entry.label, "l");
        let meta = discarded.expect("discard meta must travel along");
        assert_eq!(meta.files.len(), 1);
        assert_eq!(meta.size_line, "+1 -0");
    }
}
