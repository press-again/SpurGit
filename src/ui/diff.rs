//! Diff pane for the selected Local Changes file: a virtualized unified diff
//! with old/new line gutters, bounded by the loader (`git::DIFF_BYTE_CAP`).
//! Binary, truncated, loading, and error states are explicit.

use super::*;

use std::rc::Rc;

use gpui_kit::base::ElementExt as _;
use gpui_kit::base::InteractiveElementExt as _;
use gpui_kit::base::SelectableText;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollbarHandle as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon, Sizable as _};
use gpui_kit::{
    Hsla, InteractiveElement as _, IntoElement, SharedString, StatefulInteractiveElement as _,
    Styled as _, WeakEntity,
};

use crate::git::{DiffLine, DiffLineKind, FileDiff};
use crate::i18n::t;

use super::changes::{DiscardRequest, HunkDiscardRequest, Selection};
use super::motion::{hover_blend, hover_listener};

/// Dense diff rows (mono, 19px).
const DIFF_ROW_H: f32 = 19.0;
/// Bottom reserve for the horizontal scrollbar: the bar overlays the
/// list's bottom edge, so a trailing spacer row scrolls the last line above
/// it instead of under it. The bar only renders on overflow.
const H_SCROLL_RESERVE_PX: f32 = 12.0;
/// Never build a horizontally scrollable strip wider than this.
const MAX_DIFF_WIDTH: f32 = 6000.0;
/// Retained parsed-diff budget; entries are evicted oldest first when a new
/// diff pushes the cache past it.
const DIFF_CACHE_BYTES: usize = 8 * 1024 * 1024;
/// Wrap-mode bounds: columns clamp, visual rows cap (the logical
/// row cap still applies underneath).
const WRAP_MIN_COLS: usize = 20;
const WRAP_MAX_COLS: usize = 1000;
const WRAP_VISUAL_CAP: usize = 100_000;
/// Tab expansion for wrap breaks (display only; patches stay byte-exact).
const WRAP_TAB_W: usize = 4;
/// Continuation indent for wrapped lines.
const WRAP_INDENT_PX: f32 = 16.0;
/// Row chrome left of the text: two 44 px number gutters and the 10 px
/// +/- marker (see `diff_row`).
const DIFF_GUTTER_PX: f32 = 98.0;
/// Right-edge reserve so the last column clears the vertical scrollbar.
const WRAP_RIGHT_RESERVE_PX: f32 = 12.0;

/// One wrapped visual row: the logical line plus one display chunk. Only
/// the first chunk of a line carries the gutter numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct VisualRow {
    pub line_ix: usize,
    pub text: String,
    pub first: bool,
}

/// A diff laid out for wrap mode: visual rows plus per-hunk visual ranges
/// aligned with `FileDiff::hunks`, for the hover overlay.
#[derive(Clone, Debug, Default)]
pub(super) struct VisualRows {
    pub rows: Vec<VisualRow>,
    pub hunks: Vec<(usize, usize)>,
    pub truncated: bool,
}

/// Text columns that fit a diff body `width_px` wide (pure, unit-tested).
/// The gutters, the continuation indent and the scrollbar reserve are taken
/// off first, so even an indented continuation chunk fits without the
/// horizontal scroller.
pub(super) fn wrap_cols_for(width_px: f32) -> usize {
    let text_w = width_px - DIFF_GUTTER_PX - WRAP_INDENT_PX - WRAP_RIGHT_RESERVE_PX;
    ((text_w.max(0.0) / MONO_CHAR_W) as usize).clamp(WRAP_MIN_COLS, WRAP_MAX_COLS)
}

/// Split one line into display chunks of at most `cols` columns. Tabs expand
/// (display only); breaks are by character. Always yields at least one chunk,
/// even for an empty line. Pure, unit-tested.
pub(super) fn wrap_chunks(text: &str, cols: usize) -> Vec<String> {
    const TAB: &str = "    ";
    debug_assert_eq!(TAB.len(), WRAP_TAB_W);
    let cols = cols.max(1);
    let expanded = text.replace('\t', TAB);
    if expanded.is_empty() {
        return vec![String::new()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut width = 0;
    for c in expanded.chars() {
        if width >= cols {
            chunks.push(std::mem::take(&mut current));
            width = 0;
        }
        current.push(c);
        width += 1;
    }
    chunks.push(current);
    chunks
}

/// Build the visual rows for a diff at `cols` text columns. Pure (no Git),
/// unit-tested. Hunk ranges map 1:1 onto `FileDiff::hunks`.
pub(super) fn build_visual_rows(diff: &FileDiff, cols: usize) -> VisualRows {
    let mut rows: Vec<VisualRow> = Vec::new();
    // Aligned 1:1 with `FileDiff::hunks` (untouched hunks stay (0, 0)).
    let mut hunks: Vec<(usize, usize)> = vec![(0, 0); diff.hunks.len()];
    let mut started = vec![false; diff.hunks.len()];
    let mut truncated = diff.truncated;
    // Hunk index by logical line: hunks arrive in order.
    let mut hi = 0;
    'lines: for (line_ix, line) in diff.lines.iter().enumerate() {
        while hi < diff.hunks.len() && diff.hunks[hi].end <= line_ix {
            hi += 1;
        }
        let in_hunk = hi < diff.hunks.len()
            && diff.hunks[hi].start <= line_ix
            && line_ix < diff.hunks[hi].end;
        let visual_start = rows.len();
        for (chunk_ix, chunk) in wrap_chunks(&line.text, cols).iter().enumerate() {
            if rows.len() >= WRAP_VISUAL_CAP {
                truncated = true;
                break 'lines;
            }
            rows.push(VisualRow {
                line_ix,
                text: chunk.clone(),
                first: chunk_ix == 0,
            });
        }
        if in_hunk {
            if !started[hi] {
                hunks[hi].0 = visual_start;
                started[hi] = true;
            }
            hunks[hi].1 = rows.len();
        }
    }
    VisualRows {
        rows,
        hunks,
        truncated,
    }
}

/// Cache key of one parsed diff: a Local Changes file side or an immutable
/// Git object pair (commit/stash and path).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum DiffKey {
    Worktree {
        worktree: String,
        path: Vec<u8>,
        staged: bool,
        untracked: bool,
    },
    Object {
        worktree: String,
        object: String,
        path: Vec<u8>,
    },
}

/// What the diff pane currently shows.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum DiffState {
    Empty,
    Loading,
    Ready(Rc<FileDiff>),
    Error(String),
}

/// A hover-button action for one hunk. (Discard goes through the confirmation
/// overlay and is queued by [`SpurShell::confirm_hunk_discard`].)
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HunkAction {
    Stage,
    Unstage,
}

impl SpurShell {
    /// Load the diff for the current selection. A generation counter drops
    /// replies that belong to a selection the user has already left.
    ///
    /// Speed and stability: a parsed diff is cached per
    /// (worktree, path, side) and painted instantly on a revisit, and while a
    /// fresh read runs the previously shown diff stays on screen instead of
    /// blanking — the header's spinner is the only sign of the load.
    pub(super) fn load_diff(&mut self, cx: &mut Context<Self>) {
        self.hovered_hunk = None;
        // Every call supersedes any read in flight, including the early
        // returns below: a cleared selection or a conflicted path must not be
        // overwritten by the previous file's late result (a load or the
        // poll's quiet refresh).
        self.diff_gen = self.diff_gen.wrapping_add(1);
        let generation = self.diff_gen;
        let Some(selection) = self.change_selection.clone() else {
            self.diff = DiffState::Empty;
            self.diff_loading = false;
            self.conflict_view = None;
            cx.notify();
            return;
        };
        // An unmerged path has no meaningful worktree diff (Git prints a
        // combined diff): the pane shows the conflict resolver instead.
        if selection.conflicted {
            self.open_conflict_view(selection, cx);
            return;
        }
        self.conflict_view = None;
        let Some(repo) = self.active_repo() else {
            self.diff = DiffState::Empty;
            self.diff_loading = false;
            cx.notify();
            return;
        };
        let worktree = repo.path.to_string_lossy().into_owned();
        let key = DiffKey::Worktree {
            worktree: worktree.clone(),
            path: selection.path.clone(),
            staged: selection.staged,
            untracked: selection.untracked,
        };
        match self.diff_cache.get(&key) {
            Some(cached) => self.diff = DiffState::Ready(cached.clone()),
            // No cache hit: keep the previous file's diff on screen (no blank
            // flash) unless there is nothing usable to show.
            None if !matches!(self.diff, DiffState::Ready(_)) => self.diff = DiffState::Loading,
            None => {}
        }
        self.diff_loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let path = selection.path.clone();
            let staged = selection.staged;
            let untracked = selection.untracked;
            let result = cx
                .background_executor()
                .spawn(async move { crate::git::diff_file(&worktree, &path, staged, untracked) })
                .await;
            this.update(cx, |this, cx| {
                if this.diff_gen != generation {
                    return; // a newer selection owns the pane
                }
                this.diff_loading = false;
                this.diff = match result {
                    Ok(diff) => {
                        let diff = Rc::new(diff);
                        this.cache_diff(key, diff.clone());
                        DiffState::Ready(diff)
                    }
                    Err(err) => DiffState::Error(err),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Store a parsed diff under the cache's byte budget; the oldest entries
    /// are evicted until the retained size fits. Re-inserting a key replaces
    /// (and re-orders) its entry.
    pub(super) fn cache_diff(&mut self, key: DiffKey, diff: Rc<FileDiff>) {
        self.uncache_diff(&key);
        self.diff_cache_bytes += diff.approx_bytes();
        self.diff_cache.insert(key.clone(), diff);
        self.diff_cache_order.push_back(key);
        while self.diff_cache_bytes > DIFF_CACHE_BYTES {
            let Some(oldest) = self.diff_cache_order.pop_front() else {
                break;
            };
            if let Some(entry) = self.diff_cache.remove(&oldest) {
                self.diff_cache_bytes = self.diff_cache_bytes.saturating_sub(entry.approx_bytes());
            }
        }
    }

    /// Drop one cached diff (mutation) and fix the budget bookkeeping.
    fn uncache_diff(&mut self, key: &DiffKey) {
        if let Some(entry) = self.diff_cache.remove(key) {
            self.diff_cache_bytes = self.diff_cache_bytes.saturating_sub(entry.approx_bytes());
        }
        self.diff_cache_order.retain(|cached| cached != key);
    }

    /// Reload after a mutation or refresh. A refresh can drop the selected
    /// path (commit, stage, discard); [`Self::load_diff`] clears the pane in
    /// that case, so the header's `+N −M` cannot outlive the selection.
    pub(super) fn reload_diff(&mut self, cx: &mut Context<Self>) {
        // Drop the cached copy of the open selection: a mutation just made it
        // stale, so the fresh read replaces it (the pane stays filled by the
        // old content until then).
        if let (Some(selection), Some(repo)) = (&self.change_selection, self.active_repo()) {
            let key = DiffKey::Worktree {
                worktree: repo.path.to_string_lossy().into_owned(),
                path: selection.path.clone(),
                staged: selection.staged,
                untracked: selection.untracked,
            };
            self.uncache_diff(&key);
        }
        self.load_diff(cx);
    }

    /// Quietly re-read the open diff. The active poll calls this because an
    /// editor saving an already-modified file changes the diff without
    /// changing `git status` — the snapshot revision cannot see it, so the
    /// pane would stay stale. Unlike [`Self::reload_diff`] it
    /// shows no spinner and keeps the hovered hunk, and it only repaints when
    /// the bytes actually differ.
    pub(super) fn refresh_open_diff(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.change_selection.clone() else {
            return;
        };
        if selection.conflicted {
            return; // the resolver owns that pane
        }
        // A load in flight already re-reads this file, and bumping the
        // generation here would discard its result before it clears
        // `diff_loading`, leaving the pane loading (hunk bar suppressed)
        // until the next selection. So the refresh never bumps the
        // generation; it only yields to any load that starts after it.
        // The pane is only on screen in Local Changes; elsewhere this would
        // spawn a `git diff` per poll for nothing (entering the section
        // refreshes it, see `show_section`).
        if self.diff_loading || self.section != super::repo::RepoSection::LocalChanges {
            return;
        }
        let Some(repo) = self.active_repo() else {
            return;
        };
        let worktree = repo.path.to_string_lossy().into_owned();
        let generation = self.diff_gen;
        let key = DiffKey::Worktree {
            worktree: worktree.clone(),
            path: selection.path.clone(),
            staged: selection.staged,
            untracked: selection.untracked,
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    crate::git::diff_file(
                        &worktree,
                        &selection.path,
                        selection.staged,
                        selection.untracked,
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                if this.diff_gen != generation {
                    return; // a newer read owns the pane
                }
                let Ok(diff) = result else {
                    return; // keep what is shown; the next poll retries
                };
                let diff = Rc::new(diff);
                let unchanged = matches!(
                    &this.diff,
                    DiffState::Ready(current) if **current == *diff
                );
                this.cache_diff(key, diff.clone());
                if !unchanged {
                    this.diff = DiffState::Ready(diff);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Apply a hover-button action to one hunk of the open diff.
    pub(super) fn apply_hunk_action(
        &mut self,
        hunk: usize,
        action: HunkAction,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.change_selection.clone() else {
            return;
        };
        if selection.untracked || self.diff_loading {
            return;
        }
        let DiffState::Ready(diff) = &self.diff else {
            return;
        };
        if diff.truncated {
            return; // a cut-off trailing hunk cannot be applied safely
        }
        let Some(patch) = diff.patch_for_hunk(hunk) else {
            return;
        };
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        let label = truncate_path(&selection.display, 40);
        match action {
            HunkAction::Stage => {
                self.queue_hunk_patch(repo_id, patch, true, false, t().log_hunk_staged(&label), cx);
            }
            HunkAction::Unstage => {
                self.queue_hunk_patch(repo_id, patch, true, true, t().log_hunk_unstaged(&label), cx);
            }
        }
    }

    /// Copy one hunk as an apply-able patch: the parsed diff is
    /// already in memory, so no Git call is needed.
    pub(super) fn copy_hunk_patch(&mut self, hunk: usize, cx: &mut Context<Self>) {
        if self.diff_loading {
            return;
        }
        let DiffState::Ready(diff) = &self.diff else {
            return;
        };
        if diff.truncated {
            return; // a cut-off trailing hunk cannot be copied safely
        }
        let Some(patch) = diff.patch_for_hunk(hunk) else {
            return;
        };
        let text = String::from_utf8_lossy(&patch).into_owned();
        let bytes = text.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        self.ops.push_info(t().log_copied_patch(bytes));
        cx.notify();
    }

    /// Discard is destructive: ask before applying the hunk.
    pub(super) fn request_hunk_discard(&mut self, hunk: usize, cx: &mut Context<Self>) {
        let Some(selection) = self.change_selection.clone() else {
            return;
        };
        if self.diff_loading {
            return; // the visible hunks may belong to the previous file
        }
        let DiffState::Ready(diff) = &self.diff else {
            return;
        };
        if diff.truncated {
            return;
        }
        let Some(patch) = diff.patch_for_hunk(hunk) else {
            return;
        };
        self.hunk_discard = Some(HunkDiscardRequest {
            patch,
            staged: selection.staged,
            label: truncate_path(&selection.display, 40),
        });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_hunk_discard(&mut self, cx: &mut Context<Self>) {
        if self.hunk_discard.is_some() {
            self.close_modal(cx);
        }
    }

    pub(super) fn confirm_hunk_discard(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.hunk_discard.clone() else {
            return;
        };
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        let label = t().log_hunk_discarded(&request.label);
        if request.staged {
            self.queue_hunk_patch(
                repo_id.clone(),
                request.patch.clone(),
                true,
                true,
                label.clone(),
                cx,
            );
            self.queue_hunk_patch(repo_id, request.patch, false, true, label, cx);
        } else {
            self.queue_hunk_patch(repo_id, request.patch, false, true, label, cx);
        }
        self.close_modal(cx);
    }

    /// Confirmation overlay for a destructive hunk Discard.
    pub(super) fn render_hunk_discard(
        &self,
        request: HunkDiscardRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let body = t().hunk_discard_body(&request.label);
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
                cx.listener(|this, _, _, cx| this.cancel_hunk_discard(cx)),
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
                                Button::new("hunk-discard-cancel")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_hunk_discard(cx)
                                    })),
                            )
                            .child(
                                Button::new("hunk-discard-confirm")
                                    .label(t().discard_confirm)
                                    .small()
                                    .danger()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_hunk_discard(cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_diff_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selection = self.change_selection.clone();
        let loading = self.diff_loading;
        let (stats, truncated) = match &self.diff {
            // Stale content stays visible while a new file loads: show neither
            // the previous file's counts nor its truncation chip.
            DiffState::Ready(diff) if !loading => (
                Some((diff.additions, diff.deletions)),
                diff.truncated,
            ),
            _ => (None, false),
        };

        let body: gpui_kit::AnyElement = match (&self.diff, &selection) {
            (_, None) => diff_note(t().diff_select, cx),
            // Selecting a file starts the load immediately; a "loading" note
            // only flashed for a frame. Keep the pane blank until the diff is
            // ready instead.
            (DiffState::Loading, _) => div().flex_1().min_h_0().into_any_element(),
            (DiffState::Empty, Some(_)) => diff_note(t().diff_select, cx),
            (DiffState::Error(err), _) => diff_error_note(err, cx),
            (DiffState::Ready(diff), Some(_)) if diff.binary => diff_note(t().diff_binary, cx),
            (DiffState::Ready(diff), Some(_)) if diff.lines.is_empty() => {
                diff_note(t().no_changes, cx)
            }
            (DiffState::Ready(diff), Some(selection)) => {
                diff_list(diff.clone(), selection, self, cx)
            }
        };

        let mut header = div()
            .flex_none()
            .h(px(HEADER_H))
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(hairline(0.05));
        if let Some(selection) = &selection {
            header = header
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(MONO)
                        .text_size(px(TEXT_SM))
                        .text_color(text_primary(cx))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(truncate_path(&selection.display, 72)),
                )
                .child(diff_side_chip(selection, cx));
        } else {
            header = header.child(div().flex_1());
        }
        if loading {
            // The load is signalled here instead of blanking the pane.
            header = header.child(Spinner::new().with_size(px(12.)).color(text_muted(cx)));
        }
        if let Some((additions, deletions)) = stats {
            header = header.child(
                div()
                    .flex_none()
                    .font_family(MONO)
                    .text_size(px(TEXT_SM))
                    .text_color(cx.theme().success)
                    .child(format!("+{additions}")),
            );
            header = header.child(
                div()
                    .flex_none()
                    .font_family(MONO)
                    .text_size(px(TEXT_SM))
                    .text_color(cx.theme().danger)
                    .child(format!("−{deletions}")),
            );
        }
        if truncated {
            header = header.child(chip(t().diff_truncated, cx.theme().warning));
        }
        // The selected file's own actions stay on screen — stage or
        // unstage mirrors the row checkbox, discard goes through the same
        // confirmation and undo safety net as the context menu. A conflicted
        // path is handled by the resolver, so it gets neither.
        if let Some(selection) = selection.as_ref().filter(|selection| !selection.conflicted) {
            let staged = selection.staged;
            let stage_path = selection.path.clone();
            let discard_path = selection.path.clone();
            let stage_entity = cx.entity().downgrade();
            header = header.child(wash_icon_button(
                if staged { "diff-unstage-file" } else { "diff-stage-file" },
                if staged {
                    IconName::Minus
                } else {
                    IconName::Plus
                },
                if staged {
                    t().context_unstage
                } else {
                    t().context_stage
                },
                move |_, _, cx| {
                    stage_entity
                        .update(cx, |this, cx| {
                            // Click time, not render time: a file that left
                            // this side meanwhile is not toggled back.
                            if this.current_change_item(staged, &stage_path).is_some() {
                                this.toggle_change(staged, stage_path.clone(), cx);
                            }
                        })
                        .ok();
                },
                cx,
            ));
            let discard_entity = cx.entity().downgrade();
            header = header.child(wash_icon_button(
                "diff-discard-file",
                IconName::Trash,
                t().context_discard,
                move |_, _, cx| {
                    discard_entity
                        .update(cx, |this, cx| {
                            // Resolve the file from the live lists at click
                            // time: whether it is untracked decides between
                            // restoring and deleting it, so a status change
                            // since this header rendered must win.
                            let Some(item) = this.current_change_item(staged, &discard_path)
                            else {
                                return;
                            };
                            let Some(repo_id) = this.active_repo_id() else {
                                return;
                            };
                            this.request_discard(DiscardRequest::single(repo_id, staged, &item), cx);
                        })
                        .ok();
                },
                cx,
            ));
        }
        // Soft-wrap toggle: accent icon while wrapped, persisted.
        {
            let entity = cx.entity().downgrade();
            let on = self.diff_wrap;
            header = header.child(
                div()
                    .id("diff-wrap-toggle")
                    .aria_label(t().diff_wrap_tooltip)
                    .cursor_pointer()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(20.))
                    .h(px(20.))
                    .rounded(px(4.))
                    .bg(hover_blend("diff-wrap", ink(0.0), ink(0.10)))
                    .on_hover(hover_listener("diff-wrap"))
                    .tooltip(move |window, cx| {
                        Tooltip::new(t().diff_wrap_tooltip).build(window, cx)
                    })
                    .child(
                        Icon::new(IconName::Pilcrow)
                            .size(px(13.))
                            .text_color(if on { violet(cx) } else { text_muted(cx) }),
                    )
                    .on_click(move |_, _, cx| {
                        entity
                            .update(cx, |this, cx| {
                                this.diff_wrap = !this.diff_wrap;
                                this.diff_visual.borrow_mut().take();
                                this.diff_scroll
                                    .set_offset(point(px(0.), px(0.)));
                                if let Err(err) =
                                    crate::settings::set_diff_wrap(this.diff_wrap)
                                {
                                    this.note_error(
                                        t().log_settings_save_failed(&err),
                                        cx,
                                    );
                                }
                                cx.notify();
                            })
                            .ok();
                    }),
            );
        }

        div()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex_col()
            .child(header)
            .child(body)
    }
}

fn diff_side_chip(selection: &Selection, cx: &Context<SpurShell>) -> gpui_kit::AnyElement {
    if selection.untracked {
        chip(t().diff_untracked, cx.theme().success)
    } else if selection.staged {
        // The staged side reads neutral: the section header and the
        // Commit button already say where staged changes go.
        chip(t().diff_staged, text_muted(cx))
    } else {
        chip(t().diff_unstaged, text_muted(cx))
    }
    .into_any_element()
}

fn diff_note(text: &str, cx: &Context<SpurShell>) -> gpui_kit::AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(TEXT_SM))
        .text_color(text_muted(cx))
        .child(text.to_string())
        .into_any_element()
}

fn diff_error_note(err: &str, cx: &Context<SpurShell>) -> gpui_kit::AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .px(px(16.))
        .child(
            div()
                .text_size(px(TEXT_SM))
                .text_color(cx.theme().danger)
                .child(t().diff_error),
        )
        .child(
            div()
                .max_w(px(520.))
                .font_family(MONO)
                .text_size(px(TEXT_XS))
                .text_color(text_faint(cx))
                .child(err.to_string()),
        )
        .into_any_element()
}

fn diff_list(
    diff: Rc<FileDiff>,
    selection: &Selection,
    shell: &SpurShell,
    cx: &mut Context<SpurShell>,
) -> gpui_kit::AnyElement {
    if shell.diff_wrap {
        diff_list_wrapped(diff, selection, shell, cx)
    } else {
        diff_list_plain(diff, selection, shell, cx)
    }
}

/// Hunk hover overlay (Stage/Unstage + Discard) at a precomputed geometry.
/// Shared by the plain and wrapped lists; only the row math differs.
fn hunk_overlay(
    top: gpui_kit::Pixels,
    height: gpui_kit::Pixels,
    hunk_ix: usize,
    selection: &Selection,
    this: WeakEntity<SpurShell>,
    cx: &mut Context<SpurShell>,
) -> Vec<gpui_kit::AnyElement> {
    let action_entity = this.clone();
    let copy_entity = this.clone();
    let discard_entity = this.clone();
    let (primary_label, primary_action) = if selection.staged {
        (t().context_unstage, HunkAction::Unstage)
    } else {
        (t().context_stage, HunkAction::Stage)
    };
    let outline = div()
        .absolute()
        .top(top)
        .left(px(0.))
        .right(px(0.))
        .h(height)
        .border_1()
        .border_color(violet(cx).alpha(0.55))
        .rounded(px(3.));
    // Small buttons (24px) plus the bar's padding and border.
    const BAR_H: f32 = 24.0 + 2.0 * 3.0 + 2.0;
    const BAR_INSET: f32 = 3.0;
    // Sticky: once the hunk's top scrolls out, the bar stays pinned to the
    // pane's top edge until it reaches the hunk's bottom.
    let natural = top + px(BAR_INSET);
    let last = (top + height - px(BAR_H + BAR_INSET)).max(natural);
    let bar_top = natural.max(px(BAR_INSET)).min(last);
    let bar = div()
        .absolute()
        .top(bar_top)
        .right(px(10.))
        .flex()
        .items_center()
        .gap(px(6.))
        // An opaque floating surface: the outline button's translucent
        // fill would otherwise let the diff text underneath show
        // through.
        .p(px(3.))
        .rounded(px(8.))
        .bg(cx.theme().popover)
        .border_1()
        .border_color(hairline(0.10))
        .child(
            Button::new("hunk-primary")
                .label(primary_label)
                .small()
                .outline()
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    action_entity
                        .update(cx, |this, cx| {
                            this.apply_hunk_action(hunk_ix, primary_action, cx);
                        })
                        .ok();
                }),
        )
        .child(
            Button::new("hunk-copy-patch")
                .label(t().context_copy_patch)
                .small()
                .outline()
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    copy_entity
                        .update(cx, |this, cx| this.copy_hunk_patch(hunk_ix, cx))
                        .ok();
                }),
        )
        .child(
            Button::new("hunk-discard")
                .label(t().discard_confirm)
                .small()
                .danger()
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    discard_entity
                        .update(cx, |this, cx| this.request_hunk_discard(hunk_ix, cx))
                        .ok();
                }),
        );
    vec![outline.into_any_element(), bar.into_any_element()]
}

fn diff_list_plain(
    diff: Rc<FileDiff>,
    selection: &Selection,
    shell: &SpurShell,
    cx: &mut Context<SpurShell>,
) -> gpui_kit::AnyElement {
    let count = diff.lines.len() + 1;
    // Width comes from the parse-time scan, not a per-frame rescan.
    let max_chars = diff.max_chars;
    let width = (96.0 + max_chars as f32 * MONO_CHAR_W).clamp(320.0, MAX_DIFF_WIDTH);
    let list_id: SharedString = format!(
        "diff-{}-{}",
        if selection.staged { "index" } else { "worktree" },
        selection.display
    )
    .into();
    let this = cx.entity().downgrade();

    // The list is vertical-only: without the restriction a shift+wheel
    // (horizontal) delta would also be remapped onto it, scrolling the diff
    // up/down while the wrapper scrolls it sideways. `UniformList` implements
    // only `InteractiveElement`, so set the style flag directly instead of the
    // `StatefulInteractiveElement::restrict_scroll_to_axis` helper.
    let rows = diff.clone();
    let row_entity = this.clone();
    let mut list = gpui_kit::uniform_list(list_id, count, move |range, _window, cx| {
        range
            .map(|ix| {
                if ix >= rows.lines.len() {
                    // Trailing spacer: see H_SCROLL_RESERVE_PX.
                    return div().h(px(H_SCROLL_RESERVE_PX)).into_any_element();
                }
                diff_row(
                    ix,
                    &rows.lines[ix],
                    None,
                    rows.lines[ix].hunk,
                    row_entity.clone(),
                    cx,
                )
            })
            .collect()
    })
    .track_scroll(&shell.diff_scroll)
    // Fill the pane when the content is narrower so change backgrounds span
    // the whole width; stay as wide as the longest line when it is wider
    // (the wrapper scrolls horizontally).
    .w_full()
    .min_w(px(width))
    .h_full();
    list.interactivity().base_style.restrict_scroll_to_axis = Some(true);

    // Hunk hover: a full-section outline plus Stage/Unstage + Discard at its
    // top-right (SourceGit's diff block actions), shown while the pointer is
    // on the hunk. Positioned from the row index and
    // the current scroll offset, so the overlay follows the rows.
    // Suppressed while a new diff loads: the visible rows are the previous
    // file's until the read finishes.
    let hovered = if diff.truncated || shell.diff_loading {
        None // a cut-off trailing hunk cannot be applied safely
    } else {
        shell.hovered_hunk.filter(|ix| *ix < diff.hunks.len())
    };
    // Read the offset the list actually lays out with. A wheel over a
    // non-scrollable list still records a transient delta that is clamped on
    // the next prepaint; reading the raw value moved the outline for a frame.
    let scroll_state = shell.diff_scroll.0.borrow();
    let scroll_y = scroll_state.base_handle.offset().y.clamp(
        -scroll_state.base_handle.max_offset().y,
        px(0.),
    );
    let overlay: Vec<gpui_kit::AnyElement> = hovered
        .map(|hunk_ix| {
            let range = diff.hunks[hunk_ix];
            let top = px(range.start as f32 * DIFF_ROW_H) + scroll_y;
            let height = px((range.end - range.start) as f32 * DIFF_ROW_H);
            hunk_overlay(top, height, hunk_ix, selection, this.clone(), cx)
        })
        .unwrap_or_default();

    diff_body_container(list, overlay, shell, cx)
}

/// Wrap-mode list: the diff laid out as visual rows (one per wrapped
/// chunk) at the same fixed row height, so virtualization, selection, and
/// the hunk overlay all keep working. Chunks fit by construction, so the
/// horizontal bar stays hidden on its own.
fn diff_list_wrapped(
    diff: Rc<FileDiff>,
    selection: &Selection,
    shell: &SpurShell,
    cx: &mut Context<SpurShell>,
) -> gpui_kit::AnyElement {
    let cols = wrap_cols_for(shell.diff_wrap_w);
    let cached = shell.diff_visual.borrow().as_ref().and_then(|(d, c, visual)| {
        (Rc::ptr_eq(d, &diff) && *c == cols).then(|| visual.clone())
    });
    let visual = cached.unwrap_or_else(|| {
        let visual = Rc::new(build_visual_rows(diff.as_ref(), cols));
        *shell.diff_visual.borrow_mut() = Some((diff.clone(), cols, visual.clone()));
        visual
    });
    // Same trailing spacer as the plain list (see H_SCROLL_RESERVE_PX).
    let count = visual.rows.len() + 1;
    let list_id: SharedString = format!(
        "diff-{}-{}-wrap",
        if selection.staged { "index" } else { "worktree" },
        selection.display
    )
    .into();
    let this = cx.entity().downgrade();
    let rows = visual.clone();
    let diff_ref = diff.clone();
    let row_entity = this.clone();
    let mut list = gpui_kit::uniform_list(list_id, count, move |range, _window, cx| {
        range
            .map(|ix| {
                if ix >= rows.rows.len() {
                    return div().h(px(H_SCROLL_RESERVE_PX)).into_any_element();
                }
                let row = &rows.rows[ix];
                let line = &diff_ref.lines[row.line_ix];
                diff_row(
                    ix,
                    line,
                    Some(row),
                    line.hunk,
                    row_entity.clone(),
                    cx,
                )
            })
            .collect()
    })
    .track_scroll(&shell.diff_scroll)
    .w_full()
    .min_w(px(320.0))
    .h_full();
    list.interactivity().base_style.restrict_scroll_to_axis = Some(true);

    let truncated = visual.truncated;
    let hovered = if truncated || shell.diff_loading {
        None // a cut-off trailing hunk cannot be applied safely
    } else {
        shell.hovered_hunk.filter(|ix| *ix < visual.hunks.len())
    };
    let scroll_state = shell.diff_scroll.0.borrow();
    let scroll_y = scroll_state.base_handle.offset().y.clamp(
        -scroll_state.base_handle.max_offset().y,
        px(0.),
    );
    let overlay: Vec<gpui_kit::AnyElement> = hovered
        .map(|hunk_ix| {
            let (start, end) = visual.hunks[hunk_ix];
            let top = px(start as f32 * DIFF_ROW_H) + scroll_y;
            let height = px((end - start) as f32 * DIFF_ROW_H);
            hunk_overlay(top, height, hunk_ix, selection, this.clone(), cx)
        })
        .unwrap_or_default();

    diff_body_container(list, overlay, shell, cx)
}

/// The scroll container shared by both list modes: hover clearing, the
/// horizontal scroller, both scrollbar overlays, and the hunk overlay. Also
/// captures the content width for wrap columns (change-guarded, so resizes
/// cannot loop).
fn diff_body_container(
    list: impl IntoElement,
    overlay: Vec<gpui_kit::AnyElement>,
    shell: &SpurShell,
    cx: &mut Context<SpurShell>,
) -> gpui_kit::AnyElement {
    let clear_entity = cx.entity().downgrade();
    let measure_entity = cx.entity().downgrade();
    div()
        .id("diff-body")
        .relative()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .on_hover(move |hovered, _, cx| {
            if !*hovered {
                clear_entity
                    .update(cx, |this, cx| {
                        if this.hovered_hunk.take().is_some() {
                            cx.notify();
                        }
                    })
                    .ok();
            }
        })
        .child(
            div()
                .id("diff-h")
                .size_full()
                .min_w_0()
                .overflow_x_scroll()
                // Single-axis area: without the lock GPUI remaps vertical
                // wheel/trackpad deltas onto the horizontal axis, so scrolling
                // the diff would move sideways too.
                .lock_scroll_axis()
                .track_scroll(&shell.diff_h_scroll)
                // The hunk outline is positioned from the scroll offset, so
                // repaint the pane while scrolling.
                .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
                .on_prepaint(move |bounds, _, cx| {
                    measure_entity
                        .update(cx, |this, cx| {
                            let w = f32::from(bounds.size.width);
                            if (this.diff_wrap_w - w).abs() > 0.5 {
                                this.diff_wrap_w = w;
                                this.diff_visual.borrow_mut().take();
                                cx.notify();
                            }
                        })
                        .ok();
                })
                .child(list),
        )
        .child(scrollbar_overlay("diff-v-scrollbar", &shell.diff_scroll))
        .child(h_scrollbar_overlay("diff-h-scrollbar", &shell.diff_h_scroll))
        .children(overlay)
        .into_any_element()
}

/// Text style for the selectable diff lines: the selectable element does not
/// inherit the row's style, so the mono face, size, and color are explicit.
fn diff_text_style(color: Hsla) -> gpui_kit::TextStyleRefinement {
    gpui_kit::TextStyleRefinement {
        font_family: Some(MONO.into()),
        font_size: Some(px(TEXT_SM).into()),
        color: Some(color),
        white_space: Some(gpui_kit::WhiteSpace::Nowrap),
        ..Default::default()
    }
}

fn diff_row(
    ix: usize,
    line: &DiffLine,
    visual: Option<&VisualRow>,
    hunk: Option<usize>,
    this: WeakEntity<SpurShell>,
    cx: &gpui_kit::App,
) -> gpui_kit::AnyElement {
    let content = visual.map(|row| row.text.as_str()).unwrap_or(&line.text);
    let (bg, fg) = match line.kind {
        DiffLineKind::Added => (cx.theme().success.alpha(0.08), cx.theme().success),
        DiffLineKind::Removed => (cx.theme().danger.alpha(0.08), cx.theme().danger),
        DiffLineKind::Hunk => (violet(cx).alpha(0.08), violet(cx)),
        DiffLineKind::Meta => (ink(0.0), text_faint(cx)),
        DiffLineKind::Context => (ink(0.0), text_muted(cx)),
    };
    // Wrapped continuations show no numbers (first visual line only) and
    // indent instead, so wrapped code keeps its shape. Plain rows always
    // carry numbers and no indent.
    let numbers = visual.is_none_or(|row| row.first);
    let indent = visual.is_some_and(|row| !row.first);
    let gutter = |value: Option<u32>| {
        div()
            .w(px(44.))
            .flex_none()
            .pr(px(8.))
            .text_right()
            .font_family(MONO)
            .text_size(px(TEXT_XS))
            .text_color(text_faint(cx))
            .child(
                value
                    .filter(|_| numbers)
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            )
    };
    let marker = match line.kind {
        DiffLineKind::Added => "+",
        DiffLineKind::Removed => "-",
        _ => " ",
    };
    let mut row = div()
        .id(("diff-row", ix))
        .w_full()
        .flex()
        .items_center()
        .h(px(DIFF_ROW_H))
        .bg(bg)
        .child(gutter(line.old))
        .child(gutter(line.new))
        .child(
            div()
                .w(px(10.))
                .flex_none()
                .font_family(MONO)
                .text_size(px(TEXT_XS))
                .text_color(fg)
                .child(marker),
        )
        .child(
            // Selectable line content (window text selection, Ctrl+C). The
            // selectable element carries its own text style; wrapping it keeps
            // the previous flex sizing.
            div()
                .flex_none()
                .cursor_text()
                .children(indent.then(|| div().flex_none().w(px(WRAP_INDENT_PX))))
                .child(
                    SelectableText::new(("diff-text", ix), content.to_string())
                        .text_style(diff_text_style(fg)),
                ),
        );
    if let Some(hunk) = hunk {
        // Rows report their hunk on hover; the pane clears it when the
        // pointer leaves (row leave alone must not clear, or moving between
        // two rows of one hunk would flicker the toolbar).
        row = row.on_hover(move |hovered, _, cx| {
            if *hovered {
                this.update(cx, |this, cx| {
                    if this.hovered_hunk != Some(hunk) {
                        this.hovered_hunk = Some(hunk);
                        cx.notify();
                    }
                })
                .ok();
            }
        });
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{DiffHunk, DiffLineKind};

    fn line(text: &str, hunk: Option<usize>) -> DiffLine {
        DiffLine {
            kind: DiffLineKind::Context,
            old: None,
            new: None,
            text: text.to_string(),
            raw: None,
            hunk,
        }
    }

    #[test]
    fn wrap_cols_clamp_to_a_sane_range() {
        assert_eq!(wrap_cols_for(0.0), WRAP_MIN_COLS);
        assert_eq!(wrap_cols_for(100.0), WRAP_MIN_COLS);
        // 600 px minus 98 gutter, 16 indent, 12 scrollbar = 474 px of text.
        assert_eq!(wrap_cols_for(600.0), 64, "474 px at 7.3 px/char");
        assert_eq!(wrap_cols_for(1e9), WRAP_MAX_COLS);
    }

    #[test]
    fn wrap_chunks_break_on_columns_and_expand_tabs() {
        assert_eq!(wrap_chunks("", 10), vec![String::new()]);
        assert_eq!(wrap_chunks("abc", 10), vec!["abc".to_string()]);
        assert_eq!(
            wrap_chunks("abcdefgh", 3),
            vec!["abc".to_string(), "def".to_string(), "gh".to_string()]
        );
        // Tabs count four columns; breaks stay on char boundaries.
        assert_eq!(
            wrap_chunks("a\tb", 4),
            vec!["a   ".to_string(), " b".to_string()]
        );
        assert_eq!(wrap_chunks("héllo!", 5), vec!["héllo".to_string(), "!".to_string()]);
    }

    #[test]
    fn visual_rows_carry_numbers_once_and_map_hunks() {
        let diff = FileDiff {
            lines: vec![
                line("short", None),
                line("a much longer line here", Some(0)),
                line("tail", Some(0)),
            ],
            hunks: vec![DiffHunk { start: 1, end: 3 }],
            ..Default::default()
        };
        let visual = build_visual_rows(&diff, 10);
        // "a much longer line here" (23 chars) wraps to 3 chunks at 10 cols.
        assert_eq!(visual.rows.len(), 1 + 3 + 1);
        assert!(visual.rows[0].first);
        assert!(visual.rows[1].first);
        assert!(!visual.rows[2].first);
        assert_eq!(visual.rows[1].line_ix, 1);
        assert_eq!(visual.hunks, vec![(1, 5)]);
        assert!(!visual.truncated);
    }
}
