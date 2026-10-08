//! History section: a virtualized commit list with a painted lane graph.
//!
//! Data comes from `git::log_history` (all refs, date order), paged in
//! `HISTORY_PAGE` chunks. Lane placement is computed by `graph::layout` over
//! the accumulated list so lanes stay continuous across pages; each row paints
//! its own slice of the graph on a small canvas.

use super::*;

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::ElementExt as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme, Sizable as _};
use gpui_kit::{
    canvas, fill, hsla, point, px, quad, size, uniform_list, AnyElement, App, BorderStyle,
    Corners, FontWeight, Hsla, IntoElement, PathBuilder, Pixels, Point, ScrollStrategy,
    SharedString, StatefulInteractiveElement as _, WeakEntity,
};

use crate::git::HISTORY_PAGE;
use crate::graph::GraphRow;
use crate::i18n::t;
use crate::model::{HistoryCommit, RefKind};

/// Row height and node geometry (logical pixels). Lane spacing comes from
/// `graph::column_metrics` so paging keeps the column width stable.
const ROW_H: f32 = 34.;
const NODE_R: f32 = 4.;
const MERGE_R: f32 = 6.;
const LINE_W: f32 = 2.2;
const RING_W: f32 = 2.0;
const MERGE_PLUS_W: f32 = 2.2;

/// Commit history in append-only pages. Renderers and the per-tab cache hold
/// cheap `Rc` clones of this container, so appending a page never deep-copies
/// the accumulated commits — the previous `Rc<Vec<_>>::make_mut` did exactly
/// that whenever a rendered element still held a reference (23–75 ms at
/// 30k–100k rows).
#[derive(Debug, Clone, Default)]
pub(super) struct HistoryPages {
    pages: Vec<Rc<Vec<HistoryCommit>>>,
    /// Start index of each page (parallel to `pages`).
    starts: Vec<usize>,
    len: usize,
}

impl HistoryPages {
    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append one laid-out page; only the (small) page-index vectors are
    /// cloned when this container is still shared.
    pub(super) fn extend(&mut self, page: Vec<HistoryCommit>) {
        if page.is_empty() {
            return;
        }
        self.starts.push(self.len);
        self.len += page.len();
        self.pages.push(Rc::new(page));
    }

    pub(super) fn get(&self, ix: usize) -> Option<&HistoryCommit> {
        let start = self.starts.binary_search(&ix).unwrap_or_else(|at| at.wrapping_sub(1));
        let page = self.pages.get(start)?;
        page.get(ix - self.starts[start])
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &HistoryCommit> {
        self.pages.iter().flat_map(|page| page.iter())
    }

    /// Global index of a commit hash (ref search).
    pub(super) fn position_hash(&self, hash: &str) -> Option<usize> {
        let mut base = 0;
        for page in &self.pages {
            if let Some(ix) = page.iter().position(|commit| commit.hash == hash) {
                return Some(base + ix);
            }
            base += page.len();
        }
        None
    }
}

impl std::ops::Index<usize> for HistoryPages {
    type Output = HistoryCommit;

    fn index(&self, ix: usize) -> &HistoryCommit {
        self.get(ix).expect("history index within bounds")
    }
}

/// History session of one tab, kept across tab switches so returning to a tab
/// does not re-run `git log` and re-page from the start. `revision` is the
/// HEAD+refs signature the session was loaded from; a restore whose signature
/// no longer matches reloads instead of paging stale tips.
pub(super) struct CachedHistory {
    history: Rc<HistoryPages>,
    tips: Option<Vec<String>>,
    remotes: Option<std::collections::HashSet<String>>,
    exhausted: bool,
    graph_cols: usize,
    layout: crate::graph::LayoutState,
    selected_commit: Option<String>,
    refs: Vec<(String, String)>,
    refs_loaded: bool,
    revision: Option<(Option<String>, u64)>,
    scroll: UniformListScrollHandle,
}

impl SpurShell {
    /// Start (or restore) history for the active repository tab.
    pub(super) fn open_history(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.active_repo().map(|e| PathKey::new(&e.path)) else {
            return;
        };
        if self.history_for.as_ref() == Some(&key) {
            return;
        }
        // Keep the tab we are leaving; returning to it is instant.
        if let Some(previous) = self.history_for.clone() {
            self.store_history_cache(&previous);
        }
        if let Some(cached) = self.take_cached_history(&key.0) {
            // A session whose refs moved while the tab was inactive must not
            // keep paging its old fixed tips: drop it and start fresh.
            let stale = match (&cached.revision, self.current_history_signature()) {
                (Some(loaded), Some(current)) => *loaded != current,
                _ => false,
            };
            if stale {
                crate::logging::log!("history: {}: refs moved while inactive; reloading", key.0);
                self.reset_history(Some(key));
                self.load_history_refs(cx);
                self.load_history_page(cx);
                self.history_revision = self.current_history_signature();
                return;
            }
            self.restore_history(key, cached, cx);
            self.history_revision = self.current_history_signature();
            self.load_history_refs(cx);
            // Resume an unfinished session only while it has no data (its
            // first page was still in flight when cached). A partially paged
            // session waits for the "load more" row instead of fetching a
            // page on every return visit.
            if self.history.is_empty() && !self.history_exhausted {
                self.load_history_page(cx);
            }
            return;
        }
        self.reset_history(Some(key));
        self.load_history_refs(cx);
        self.load_history_page(cx);
        self.history_revision = self.current_history_signature();
    }

    /// Force a fresh history session for the active tab. Used when refs or
    /// HEAD moved (in-app commit, external commit/fetch, manual refresh).
    pub(super) fn reload_history(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.active_repo().map(|e| PathKey::new(&e.path)) else {
            return;
        };
        self.history_revision = self.current_history_signature();
        // The cache holds the pre-reload session; drop it so it cannot come
        // back on the next tab switch.
        self.history_cache.remove(&key.0);
        self.reset_history(Some(key));
        self.load_history_refs(cx);
        self.load_history_page(cx);
        cx.notify();
    }

    /// Reload the active history when a status round shows that HEAD or any
    /// ref moved. The first observation only records the signature: the
    /// session was just loaded from the repository itself.
    pub(super) fn maybe_reload_history(&mut self, cx: &mut Context<Self>) {
        if self.history_for.is_none() {
            return;
        }
        let Some(signature) = self.current_history_signature() else {
            return;
        };
        match &self.history_revision {
            Some(previous) if previous == &signature => {}
            None => self.history_revision = Some(signature),
            Some(_) => {
                crate::logging::log!("history: refs changed; reloading the active tab");
                self.reload_history(cx);
            }
        }
    }

    fn store_history_cache(&mut self, key: &PathKey) {
        self.history_cache.insert(
            key.0.clone(),
            CachedHistory {
                history: self.history.clone(),
                tips: self.history_tips.clone(),
                remotes: self.history_remotes.clone(),
                exhausted: self.history_exhausted,
                graph_cols: self.history_graph_cols,
                layout: self.history_layout.clone(),
                selected_commit: self.selected_commit.clone(),
                refs: self.history_refs.clone(),
                refs_loaded: self.history_refs_loaded,
                revision: self.history_revision.clone(),
                // Returning to the tab lands where it was left.
                scroll: self.history_scroll.clone(),
            },
        );
        // LRU budget: inactive sessions beyond the cap are dropped oldest
        // first, so browsing many repositories cannot retain unbounded
        // history.
        self.history_cache_order.retain(|cached| cached != &key.0);
        self.history_cache_order.push_back(key.0.clone());
        while self.history_cache_order.len() > HISTORY_CACHE_MAX {
            if let Some(oldest) = self.history_cache_order.pop_front() {
                self.history_cache.remove(&oldest);
            }
        }
    }

    /// Drop one tab's cached session (tab closed).
    pub(super) fn drop_cached_history(&mut self, key: &str) {
        self.history_cache.remove(key);
        self.history_cache_order.retain(|cached| cached != key);
    }

    /// Take one cached session out (opening the tab): the entry and its LRU
    /// order slot are both removed, so the budget cannot evict live entries
    /// for stale order slots.
    fn take_cached_history(&mut self, key: &str) -> Option<CachedHistory> {
        self.history_cache_order.retain(|cached| cached != key);
        self.history_cache.remove(key)
    }

    fn restore_history(&mut self, key: PathKey, cached: CachedHistory, cx: &mut Context<Self>) {
        // Bump the generation so replies from the tab we left are dropped.
        self.history_gen = self.history_gen.wrapping_add(1);
        self.history_for = Some(key);
        self.history = cached.history;
        self.history_tips = cached.tips;
        self.history_remotes = cached.remotes;
        self.history_exhausted = cached.exhausted;
        self.history_graph_cols = cached.graph_cols;
        self.history_layout = cached.layout;
        self.selected_commit = cached.selected_commit;
        self.history_revision = cached.revision;
        // The detail panel is not part of the cached session; reload it for
        // the selection this tab comes back with.
        if let Some(hash) = self.selected_commit.clone() {
            self.load_commit_detail(hash, cx);
        } else {
            self.clear_commit_detail();
        }
        self.history_refs = cached.refs;
        self.history_refs_loaded = cached.refs_loaded;
        self.history_loading = false;
        self.history_error = None;
        self.history_target = None;
        self.history_searching = false;
        self.history_no_match = false;
        self.history_jump_pending = false;
        self.history_scroll = cached.scroll;
        self.history_refs_loading = false;
    }

    /// Drop the history session (no active repository); pending async results
    /// are invalidated by the generation bump.
    pub(super) fn clear_history(&mut self) {
        self.history_cache.clear();
        self.history_cache_order.clear();
        self.history_revision = None;
        self.reset_history(None);
    }

    fn reset_history(&mut self, key: Option<PathKey>) {
        self.history_gen = self.history_gen.wrapping_add(1);
        self.history_for = key;
        self.history_tips = None;
        self.history_graph_cols = 0;
        self.history_layout = crate::graph::LayoutState::default();
        self.history_remotes = None;
        self.history = Rc::new(HistoryPages::default());
        self.history_loading = false;
        self.history_exhausted = false;
        self.history_error = None;
        self.selected_commit = None;
        self.clear_commit_detail();
        self.history_target = None;
        self.history_searching = false;
        self.history_no_match = false;
        self.history_jump_pending = false;
        self.history_scroll = UniformListScrollHandle::new();
        self.history_refs.clear();
        self.history_refs_loaded = false;
        self.history_refs_loading = false;
    }

    /// Fetch the next page; no-op while a page is in flight or at the end.
    pub(super) fn load_history_page(&mut self, cx: &mut Context<Self>) {
        if self.history_loading || self.history_exhausted {
            return;
        }
        let Some(key) = self.active_repo().map(|e| PathKey::new(&e.path)) else {
            return;
        };
        let path = key.0.clone();
        let log_path = path.clone();
        let started = Instant::now();
        let skip = self.history.len();
        let known_tips = self.history_tips.clone();
        let known_remotes = self.history_remotes.clone();
        // Incremental lane state: the page is laid out against the session's
        // accumulated state on the background thread, so the UI update only
        // appends finished rows (no whole-history rebuild, no deep clone).
        let mut layout_state = self.history_layout.clone();
        let generation = self.history_gen;
        self.history_loading = true;
        let exec = cx.background_executor().spawn(async move {
            // A session paginates a fixed revision set: `--all` would resolve
            // refs anew for every page, so a fetch mid-scroll could duplicate
            // or drop commits. Resolve once, then reuse the hashes.
            let tips = match known_tips {
                Some(tips) => tips,
                None => crate::git::revision_tips(&path),
            };
            // Remote names classify `%D` decorations; also resolved once per
            // session instead of per page.
            let remotes = match known_remotes {
                Some(remotes) => remotes,
                None => crate::git::remote_names(&path),
            };
            let mut page =
                crate::git::log_history(&path, &tips, &remotes, skip, HISTORY_PAGE)?;
            layout_state.apply(&mut page);
            Ok::<_, String>((tips, remotes, page, layout_state))
        });
        cx.spawn(async move |this, cx| {
            let result = exec.await;
            this.update(cx, |this, cx| {
                // Drop pages from a session the user has since left or restarted.
                if this.history_gen != generation || this.history_for.as_ref() != Some(&key) {
                    return;
                }
                this.history_loading = false;
                match result {
                    Ok((tips, remotes, page, layout_state)) => {
                        if this.history_tips.is_none() {
                            this.history_tips = Some(tips);
                        }
                        if this.history_remotes.is_none() {
                            this.history_remotes = Some(remotes);
                        }
                        if page.len() < HISTORY_PAGE {
                            this.history_exhausted = true;
                        }
                        let all = Rc::make_mut(&mut this.history);
                        let added = page.len();
                        all.extend(page);
                        // Reserve the graph column from the first page (0 =
                        // unset): later wider pages compress lane spacing
                        // instead of moving every row's text.
                        if this.history_graph_cols == 0 {
                            this.history_graph_cols = layout_state.cols();
                        }
                        this.history_layout = layout_state;
                        crate::logging::log!(
                            "history: {log_path}: +{added} commits ({} total) in {:.0}ms",
                            all.len(),
                            started.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                    Err(err) => {
                        crate::logging::log!("history: {log_path}: {err}");
                        this.history_error = Some(err);
                        this.history_exhausted = true;
                        this.history_searching = false;
                    }
                }
                this.continue_history_search(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn retry_history(&mut self, cx: &mut Context<Self>) {
        self.history_error = None;
        self.history_exhausted = false;
        self.load_history_page(cx);
    }

    // ---- ref search: find a branch's location, jump and highlight ----

    /// Load all branch/remote/tag names with their commit hashes (once per tab).
    fn load_history_refs(&mut self, cx: &mut Context<Self>) {
        if self.history_refs_loading || self.history_refs_loaded {
            return;
        }
        let Some(key) = self.active_repo().map(|e| PathKey::new(&e.path)) else {
            return;
        };
        let path = key.0.clone();
        let generation = self.history_gen;
        self.history_refs_loading = true;
        let exec = cx
            .background_executor()
            .spawn(async move { crate::git::ref_heads(&path) });
        cx.spawn(async move |this, cx| {
            let refs = exec.await;
            this.update(cx, |this, cx| {
                if this.history_gen != generation || this.history_for.as_ref() != Some(&key) {
                    return;
                }
                this.history_refs_loading = false;
                this.history_refs_loaded = true;
                this.history_refs = refs;
                if !this.history_query.is_empty() {
                    let jump = this.history_jump_pending;
                    this.update_history_search(jump, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Live search while typing: jump when the ref's commit is already loaded.
    pub(super) fn preview_history_search(&mut self, cx: &mut Context<Self>) {
        self.history_jump_pending = false;
        self.update_history_search(false, cx);
    }

    /// Enter: resolve the ref, load pages until its commit is present, jump.
    pub(super) fn jump_history_search(&mut self, cx: &mut Context<Self>) {
        self.history_jump_pending = true;
        self.update_history_search(true, cx);
    }

    fn update_history_search(&mut self, jump: bool, cx: &mut Context<Self>) {
        self.history_query = self
            .history_search_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        self.history_target = None;
        self.history_no_match = false;
        self.history_searching = false;
        if self.history_query.is_empty() {
            cx.notify();
            return;
        }
        if !self.history_refs_loaded {
            self.history_searching = jump;
            self.load_history_refs(cx);
            cx.notify();
            return;
        }
        let Some((_, hash)) = best_match(&self.history_refs, &self.history_query) else {
            self.history_no_match = true;
            cx.notify();
            return;
        };
        let hash = hash.clone();
        self.history_target = Some(hash.clone());
        if let Some(ix) = self.history.position_hash(&hash) {
            self.history_scroll.scroll_to_item(ix, ScrollStrategy::Center);
        } else if jump && !self.history_exhausted {
            self.history_searching = true;
            self.load_history_page(cx);
        }
        cx.notify();
    }

    /// Called after every page: keep paging until the searched commit shows up.
    fn continue_history_search(&mut self, cx: &mut Context<Self>) {
        if !self.history_searching {
            return;
        }
        let Some(hash) = self.history_target.clone() else {
            self.history_searching = false;
            return;
        };
        if let Some(ix) = self.history.position_hash(&hash) {
            self.history_searching = false;
            self.history_scroll.scroll_to_item(ix, ScrollStrategy::Center);
        } else if !self.history_exhausted {
            self.load_history_page(cx);
        } else {
            self.history_searching = false;
            self.history_no_match = true;
        }
    }

    pub(super) fn section_history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let commits = self.history.clone();
        let count = commits.len();
        let loading = self.history_loading;
        let error = self.history_error.clone();
        let exhausted = self.history_exhausted;
        let selected = self.selected_commit.clone();
        let highlight = self.history_target.clone();
        let menu_target = self
            .row_menu_open
            .as_ref()
            .map(|open| open.key.clone());
        // The checked-out branch names the ref chip that gets emphasized.
        let current_branch: Option<SharedString> = self
            .active_snapshot()
            .and_then(|collected| collected.snapshot.branch.clone())
            .map(SharedString::from);
        let this = cx.entity().downgrade();

        // The lane count is tracked incrementally with the layout; rendering
        // never rescans every loaded row.
        let cols = self.history_layout.cols();
        // Session-stable column: lane spacing tightens as wider pages arrive;
        // the width only grows past the lane-spacing floor (graph::column_metrics).
        let (lane_w, graph_w) =
            crate::graph::column_metrics(cols, self.history_graph_cols);
        let metrics = GraphMetrics {
            width: px(graph_w),
            lane_w: px(lane_w),
        };

        let list_id: SharedString = match &self.history_for {
            Some(key) => format!("history-{}", key.0).into(),
            None => "history-none".into(),
        };

        let body: AnyElement = if count == 0 {
            if let Some(err) = &error {
                self.history_error_state(err.clone(), this.clone(), cx)
            } else if loading || !exhausted {
                empty_note(t().history_loading, cx).into_any_element()
            } else {
                empty_note(t().history_empty, cx).into_any_element()
            }
        } else {
            let has_tail = error.is_some() || !exhausted;
            let total = count + usize::from(has_tail);
            uniform_list(list_id, total, move |range, _window, cx| {
                range
                    .map(|ix| {
                        if ix < count {
                            commit_row(
                                ix,
                                &commits[ix],
                                RowContext {
                                    selected: selected.as_deref()
                                        == Some(commits[ix].hash.as_str()),
                                    highlighted: highlight.as_deref()
                                        == Some(commits[ix].hash.as_str()),
                                    menu_target: menu_target.as_deref()
                                        == Some(commits[ix].hash.as_str()),
                                    current_branch: current_branch.clone(),
                                    graph: metrics,
                                },
                                this.clone(),
                                cx,
                            )
                        } else if let Some(err) = &error {
                            retry_row(err.clone(), this.clone(), cx)
                        } else {
                            load_more_row(loading, this.clone(), cx)
                        }
                    })
                    .collect()
            })
            .track_scroll(&self.history_scroll)
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        // The repository's gis status symbols sit left of the commit count:
        // local changes and sync state (ahead/behind/diverged), each with its
        // textual tooltip.
        let flags: Vec<crate::model::Flag> = self
            .active_repo()
            .map(|repo| repo.flags.clone())
            .unwrap_or_default();
        let (ahead, behind) = self
            .active_repo()
            .map(|repo| (repo.ahead, repo.behind))
            .unwrap_or_default();
        let mut strip_left = div().flex().items_center().gap(px(6.));
        for flag in flags {
            strip_left =
                strip_left.child(flag_chip(flag, flag.symbol_with_counts(ahead, behind), cx));
        }
        strip_left = strip_left.child(t().commits_count(count));
        let measure = cx.entity().downgrade();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .on_prepaint(move |bounds, _, cx| {
                measure
                    .update(cx, |this, cx| {
                        let h = f32::from(bounds.size.height);
                        if (this.history_col_h - h).abs() > 0.5 {
                            this.history_col_h = h;
                            cx.notify();
                        }
                    })
                    .ok();
            })
            .child(self.section_strip(
                strip_left,
                self.history_search_strip(cx),
                cx,
            ))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .px(px(6.))
                    .py(px(4.))
                    .child(scrollbar_overlay(
                        "history-scrollbar",
                        &self.history_scroll,
                    ))
                    .child(body),
            )
            .children(
                (self.selected_commit.is_some() && self.commit_panel_open)
                    .then(|| self.render_commit_panel(cx)),
            )
    }

    /// Section-strip right side: jump-to-branch search plus its status.
    fn history_search_strip(&self, cx: &Context<Self>) -> impl IntoElement {
        let status: Option<(SharedString, Hsla)> = if self.history_error.is_some() {
            Some((t().history_error.into(), cx.theme().danger))
        } else if self.history_searching {
            Some((t().history_searching.into(), text_muted(cx)))
        } else if self.history_no_match {
            Some((t().history_search_no_match.into(), cx.theme().danger))
        } else {
            None
        };
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .children(status.map(|(text, color)| {
                div()
                    .flex_none()
                    .text_size(px(TEXT_XS))
                    .text_color(color)
                    .child(text)
            }))
            .child(
                gpui_kit::component::Icon::new(IconName::Search)
                    .size(px(13.))
                    .flex_none()
                    .text_color(text_muted(cx)),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(230.))
                    .child(Input::new(&self.history_search_input).small().w_full()),
            )
    }

    fn history_error_state(
        &self,
        err: String,
        this: WeakEntity<SpurShell>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(TEXT_SM))
                    .text_color(cx.theme().danger)
                    .child(t().history_error),
            )
            .child(
                div()
                    .max_w(px(560.))
                    .font_family(MONO)
                    .text_size(px(TEXT_XS))
                    .text_color(text_muted(cx))
                    .overflow_hidden()
                    .child(err),
            )
            .child(
                div()
                    .id("history-error-retry")
                    .cursor_pointer()
                    .text_size(px(TEXT_SM))
                    .text_color(violet(cx))
                    .child(t().retry)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| this.retry_history(cx)).ok();
                    }),
            )
            .into_any_element()
    }
}

/// Per-frame graph geometry shared by every row (session-stable column).
#[derive(Clone, Copy)]
struct GraphMetrics {
    width: Pixels,
    lane_w: Pixels,
}

/// Per-row history render inputs; keeps the row builder's argument list short.
struct RowContext {
    selected: bool,
    highlighted: bool,
    /// The row whose context menu is open; it sits on a raised card.
    menu_target: bool,
    /// Checked-out branch, so its ref chip can be emphasized.
    current_branch: Option<SharedString>,
    graph: GraphMetrics,
}

/// One commit row: graph canvas, subject, ref chips, meta columns.
fn commit_row(
    ix: usize,
    commit: &HistoryCommit,
    ctx: RowContext,
    this: WeakEntity<SpurShell>,
    cx: &mut App,
) -> AnyElement {
    let RowContext {
        selected,
        highlighted,
        menu_target,
        current_branch,
        graph,
    } = ctx;
    let graph_row = commit.graph.clone();
    let hash = commit.hash.clone();
    // The checked-out commit carries the HEAD decoration (`git log %D`), even
    // in a detached state; it gets a violet wash and a semibold subject so
    // "where am I" reads before the ref chips.
    let is_head = commit.refs.iter().any(|(_, kind)| *kind == RefKind::Head);
    let fade_key = format!("history-row-{ix}:{}", commit.hash);

    let (rest_bg, hover_bg) = if is_head {
        (violet(cx).alpha(0.10), violet(cx).alpha(0.16))
    } else {
        (ink(0.0), ink(0.035))
    };
    let mut row = div()
        .id(("history-row", ix))
        .cursor_pointer()
        .w_full()
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(ROW_H))
        .px(px(8.))
        .rounded(px(CONTROL_RADIUS))
        .bg(hover_blend(&fade_key, rest_bg, hover_bg))
        .on_hover(hover_listener(fade_key));
    row = if highlighted {
        row.bg(cx.theme().warning.alpha(0.14))
    } else if selected {
        row.bg(ink(0.08))
    } else {
        row
    };
    // The checked-out commit keeps its violet wash under the shadow.
    if menu_target {
        let (card_bg, card_shadow) = raised_surface(1.0);
        row = row.shadow(card_shadow);
        if !is_head {
            row = row.bg(card_bg);
        }
    }

    // The graph canvas spans the full row height, so its bounds give the
    // row's top edge for placing the context menu below the row.
    let row_top = Rc::new(std::cell::Cell::new(px(0.)));
    let canvas_top = row_top.clone();
    row = row.child(
        canvas(
            move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
                canvas_top.set(bounds.origin.y)
            },
            move |bounds, _, window, cx| paint_graph(bounds, &graph_row, graph.lane_w, window, cx),
        )
        .w(graph.width)
        .h(px(ROW_H))
        .flex_none(),
    );

    for (label, kind) in &commit.refs {
        let (icon, color) = match kind {
            RefKind::Head => (IconName::GitCommitHorizontal, violet(cx)),
            RefKind::Branch => (IconName::GitBranch, violet(cx)),
            RefKind::Remote => (IconName::Cloud, cx.theme().success),
            RefKind::Tag => (IconName::Tag, cx.theme().warning),
        };
        let is_current = matches!(kind, RefKind::Branch)
            && current_branch.as_deref() == Some(label.as_str());
        row = row.child(ref_chip(icon, &truncate_label(label, 28), color, is_current));
    }

    row.child(
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(TEXT_MD))
            .text_color(text_primary(cx))
            .font_weight(if is_head {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            })
            .child(commit.subject.clone()),
    )
    .child(
        div()
            .flex_none()
            .max_w(px(150.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(TEXT_XS))
            .text_color(text_muted(cx))
            .child(commit.author.clone()),
    )
    .child(
        div()
            .flex_none()
            .w(px(58.))
            .font_family(MONO)
            .text_size(px(TEXT_XS))
            .text_color(text_faint(cx))
            .child(short_hash(&hash)),
    )
    .child(
        div()
            .flex_none()
            .w(px(96.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(TEXT_XS))
            .text_color(text_muted(cx))
            .child(commit.date.clone()),
    )
    .on_click({
        let entity = this.clone();
        move |_, _, cx| {
            entity
                .update(cx, |this, cx| {
                    this.select_commit(hash.clone(), cx);
                })
                .ok();
        }
    })
    .on_mouse_down(gpui_kit::MouseButton::Right, {
        let target = super::commit_menu::CommitTarget::of(commit, current_branch.clone());
        let entity = this.clone();
        move |event, window, cx| {
            let position = point(event.position.x, row_top.get() + px(ROW_H));
            entity
                .update(cx, |this, cx| {
                    this.open_commit_menu(target.clone(), position, window, cx)
                })
                .ok();
        }
    })
    .into_any_element()
}

fn load_more_row(loading: bool, this: WeakEntity<SpurShell>, cx: &mut App) -> AnyElement {
    if !loading {
        // Rendered only when scrolled into view; fetch the next page afterwards.
        let this = this.clone();
        cx.defer(move |cx| {
            this.update(cx, |this, cx| this.load_history_page(cx)).ok();
        });
    }
    let this = this.clone();
    div()
        .id("history-more")
        .flex()
        .items_center()
        .justify_center()
        .h(px(ROW_H + 6.))
        .cursor_pointer()
        .text_size(px(TEXT_SM))
        .text_color(text_muted(cx))
        .child(if loading {
            t().history_loading.to_string()
        } else {
            t().history_load_more.to_string()
        })
        .on_click(move |_, _, cx| {
            this.update(cx, |this, cx| this.load_history_page(cx)).ok();
        })
        .into_any_element()
}

fn retry_row(err: String, this: WeakEntity<SpurShell>, cx: &mut App) -> AnyElement {
    div()
        .id("history-retry")
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(ROW_H + 6.))
        .px(px(10.))
        .text_size(px(TEXT_SM))
        .child(
            div()
                .flex_none()
                .text_color(cx.theme().danger)
                .child(t().history_error),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .font_family(MONO)
                .text_size(px(TEXT_XS))
                .text_color(text_muted(cx))
                .child(err),
        )
        .child(
            div()
                .id("history-retry-action")
                .flex_none()
                .cursor_pointer()
                .text_color(violet(cx))
                .child(t().retry)
                .on_click(move |_, _, cx| {
                    this.update(cx, |this, cx| this.retry_history(cx)).ok();
                }),
        )
        .into_any_element()
}

fn short_hash(hash: &str) -> String {
    hash.get(..7).unwrap_or(hash).to_string()
}

// ---- lane graph painting ----

/// Lane colors: a small curated palette. A light theme keeps the same hues at
/// a lower lightness so the lines stay readable on pale surfaces.
fn lane_color(lane: usize) -> Hsla {
    let (h, s, l): (f32, f32, f32) = match lane % 8 {
        0 => (0.688, 0.87, 0.79),
        1 => (0.44, 0.55, 0.62),
        2 => (0.09, 0.72, 0.64),
        3 => (0.96, 0.60, 0.66),
        4 => (0.57, 0.78, 0.68),
        5 => (0.36, 0.50, 0.62),
        6 => (0.85, 0.55, 0.70),
        _ => (0.03, 0.70, 0.64),
    };
    if widgets::light_mode() {
        hsla(h, (s + 0.1).min(1.0), (l - 0.3).max(0.35), 1.0)
    } else {
        hsla(h, s, l, 1.0)
    }
}

fn paint_graph(
    bounds: Bounds<Pixels>,
    row: &GraphRow,
    lane_w: Pixels,
    window: &mut Window,
    cx: &App,
) {
    let x = |lane: usize| bounds.origin.x + lane_w * (lane as f32 + 0.5);
    let top = bounds.origin.y;
    let mid = top + px(ROW_H / 2.);
    let bottom = top + px(ROW_H);
    // Overdraw past the row edges: fractional row heights (display scaling)
    // otherwise leave a hairline seam between adjacent rows. Lanes that pass
    // through the whole row are drawn as one stroke so the half-row split at
    // `mid` cannot show either. A lane that only appears in `bottom` is either
    // this commit's own lane (tip: draw down from the node) or a lane an edge
    // just branched into — there the curve covers the connection, so no
    // straight stub is drawn above the join.
    let over = px(1.);
    let passes = |lane: usize| row.top.contains(&lane) && row.bottom.contains(&lane);

    for &lane in &row.top {
        if passes(lane) {
            stroke(
                window,
                point(x(lane), top - over),
                point(x(lane), bottom + over),
                px(LINE_W),
                lane_color(lane),
            );
        } else if lane == row.node {
            stroke(
                window,
                point(x(lane), top - over),
                point(x(lane), mid),
                px(LINE_W),
                lane_color(lane),
            );
        } else {
            // Converging: a lane waiting for this commit ends here, bending
            // once from its own vertical into the node. Upper half only, so
            // it never overlaps the merge lines below.
            curve(
                window,
                point(x(lane), top - over),
                point(x(lane), mid),
                point(x(row.node), mid),
                lane_color(lane),
            );
        }
    }
    for &lane in &row.bottom {
        if passes(lane) || lane != row.node {
            continue;
        }
        stroke(
            window,
            point(x(lane), mid),
            point(x(lane), bottom + over),
            px(LINE_W),
            lane_color(lane),
        );
    }
    for &to in &row.edges {
        // Merge line: from the node down into the target lane with one bend,
        // overdrawing into the next row so no seam shows at the junction.
        curve(
            window,
            point(x(row.node), mid),
            point(x(to), mid),
            point(x(to), bottom + over),
            lane_color(to),
        );
    }

    paint_node(window, cx, row, x(row.node), mid);
}

/// SourceGit-style nodes: commits are hollow rings, merges a filled dot with
/// a punched-out plus; the opaque center keeps the lane line out of the hole.
fn paint_node(window: &mut Window, cx: &App, row: &GraphRow, x: Pixels, y: Pixels) {
    let color = lane_color(row.node);
    let bg = cx.theme().background;
    let r = px(if row.is_merge { MERGE_R } else { NODE_R });
    let dot = Bounds {
        origin: point(x - r, y - r),
        size: size(r * 2., r * 2.),
    };
    if row.is_merge {
        window.paint_quad(fill(dot, color).corner_radii(Corners::all(r)));
        let arm = px(2.8);
        stroke(
            window,
            point(x - arm, y),
            point(x + arm, y),
            px(MERGE_PLUS_W),
            bg,
        );
        stroke(
            window,
            point(x, y - arm),
            point(x, y + arm),
            px(MERGE_PLUS_W),
            bg,
        );
    } else {
        window.paint_quad(fill(dot, bg).corner_radii(Corners::all(r)));
        window.paint_quad(quad(
            dot,
            Corners::all(r),
            hsla(0., 0., 0., 0.),
            px(RING_W),
            color,
            BorderStyle::default(),
        ));
    }
}

fn stroke(
    window: &mut Window,
    from: Point<Pixels>,
    to: Point<Pixels>,
    width: Pixels,
    color: Hsla,
) {
    let mut path = PathBuilder::stroke(width);
    path.move_to(from);
    path.line_to(to);
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// Single-bend connector: leaves `from` toward `ctrl`, arrives at `to`.
/// One bend per half-row keeps slopes steep enough that MSAA has nothing
/// visible to step on (the old S-curve fit a whole lane jump into half a row
/// with a kink at each end).
fn curve(
    window: &mut Window,
    from: Point<Pixels>,
    ctrl: Point<Pixels>,
    to: Point<Pixels>,
    color: Hsla,
) {
    let mut path = PathBuilder::stroke(px(LINE_W));
    path.move_to(from);
    path.curve_to(to, ctrl);
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// Best ref for a query: exact > prefix > substring, then shortest name,
/// then name order (deterministic across runs).
fn best_match<'a>(refs: &'a [(String, String)], query: &str) -> Option<&'a (String, String)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    refs.iter()
        .filter(|(name, _)| name.to_lowercase().contains(&q))
        .min_by_key(|(name, _)| {
            let lower = name.to_lowercase();
            let rank = if lower == q {
                0
            } else if lower.starts_with(&q) {
                1
            } else {
                2
            };
            (rank, name.len(), name.clone())
        })
}

#[cfg(test)]
mod tests {
    use super::best_match;
    use super::HistoryPages;
    use crate::model::HistoryCommit;

    #[test]
    fn history_pages_append_and_index_across_page_boundaries() {
        let mut pages = HistoryPages::default();
        assert!(pages.is_empty());
        pages.extend(Vec::new()); // empty page is a no-op
        assert_eq!(pages.len(), 0);
        let commits = |range: std::ops::Range<usize>| {
            range
                .map(|ix| HistoryCommit {
                    hash: format!("h{ix}"),
                    ..Default::default()
                })
                .collect::<Vec<_>>()
        };
        pages.extend(commits(0..5));
        pages.extend(commits(5..9));
        assert_eq!(pages.len(), 9);
        assert_eq!(pages[0].hash, "h0");
        assert_eq!(pages[4].hash, "h4");
        assert_eq!(pages[5].hash, "h5");
        assert_eq!(pages[8].hash, "h8");
        assert_eq!(pages.position_hash("h6"), Some(6));
        assert_eq!(pages.position_hash("nope"), None);

        // Appending while a snapshot (the renderer's cheap clone) is alive
        // leaves that snapshot intact and does not copy commit data.
        let snapshot = pages.clone();
        pages.extend(commits(9..12));
        assert_eq!(snapshot.len(), 9);
        assert_eq!(snapshot[8].hash, "h8");
        assert_eq!(pages.len(), 12);
        assert_eq!(pages[10].hash, "h10");
    }

    fn refs() -> Vec<(String, String)> {
        vec![
            ("main".into(), "a".into()),
            ("origin/main".into(), "b".into()),
            ("feature/maintenance".into(), "c".into()),
            ("release".into(), "d".into()),
        ]
    }

    #[test]
    fn best_match_prefers_exact_then_prefix() {
        assert_eq!(best_match(&refs(), "main").unwrap().0, "main");
        assert_eq!(best_match(&refs(), "MAIN").unwrap().0, "main");
        assert_eq!(best_match(&refs(), "ori").unwrap().0, "origin/main");
        assert_eq!(best_match(&refs(), "tenance").unwrap().0, "feature/maintenance");
        assert!(best_match(&refs(), "zzz").is_none());
        assert!(best_match(&refs(), "  ").is_none());
    }
}
