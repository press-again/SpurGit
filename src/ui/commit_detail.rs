//! Commit detail panel below the history list: INFORMATION (author, date,
//! SHA, parents, message) and CHANGES (changed files with their status). A
//! double-click on a file opens a read-only diff beside the list.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{
    FontWeight, InteractiveElement as _, IntoElement, StatefulInteractiveElement as _, WeakEntity,
};

use crate::git::{CommitDetail, CommitFile, FileDiff};
use crate::i18n::t;

use super::commit::{commit_body, commit_subject};

/// Commit panel tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CommitTab {
    Information,
    Changes,
}

/// Detail load state.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum DetailState {
    Empty,
    Loading,
    Ready(Rc<CommitDetail>),
    Error(String),
}

/// Per-file diff state (read-only).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum FileDiffState {
    Empty,
    Loading,
    Ready(Rc<FileDiff>),
    Error(String),
}

/// One changed-file row (28px, half the diff's density).
const COMMIT_FILE_ROW_H: f32 = 28.0;
/// Changed-file list column width.
const COMMIT_FILES_W: f32 = 320.0;
/// Vertical cap of the commit-message block; longer messages scroll in place.
const MESSAGE_MAX_H: f32 = 200.0;
/// The INFORMATION tab's file list never shrinks below this height.
const COMMIT_FILES_MIN_H: f32 = 72.0;

impl SpurShell {
    /// Select a commit in the history list and load its detail. The current
    /// tab is kept (switching commits on the CHANGES tab stays there).
    pub(super) fn select_commit(&mut self, hash: String, cx: &mut Context<Self>) {
        self.selected_commit = Some(hash.clone());
        self.commit_panel_open = true;
        self.commit_file_selected = None;
        self.commit_file_diff = FileDiffState::Empty;
        self.load_commit_detail(hash, cx);
    }

    /// Hide the detail panel (the history selection stays highlighted);
    /// clicking a commit again brings it back.
    pub(super) fn collapse_commit_detail(&mut self, cx: &mut Context<Self>) {
        self.commit_panel_open = false;
        cx.notify();
    }

    /// Load metadata + changed files for `hash`; late replies are dropped.
    /// Commit objects are immutable, so a cached detail is served without a
    /// Git read.
    pub(super) fn load_commit_detail(&mut self, hash: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let worktree = repo.path.to_string_lossy().into_owned();
        self.commit_detail_gen = self.commit_detail_gen.wrapping_add(1);
        let generation = self.commit_detail_gen;
        self.commit_detail_for = Some(hash.clone());
        // A new commit's message starts collapsed and scrolled to the top.
        self.commit_message_scroll = ScrollHandle::new();
        self.commit_message_open = false;
        let key = (worktree.clone(), hash.clone());
        if let Some(cached) = self.commit_detail_cache.get(&key).cloned() {
            self.commit_detail = DetailState::Ready(cached);
            cx.notify();
            return;
        }
        self.commit_detail = DetailState::Loading;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let load_hash = hash.clone();
            let result = cx
                .background_executor()
                .spawn(async move { crate::git::commit_detail(&worktree, &load_hash) })
                .await;
            this.update(cx, |this, cx| {
                if this.commit_detail_gen != generation {
                    return; // a newer selection owns the panel
                }
                this.commit_detail = match result {
                    Ok(detail) => {
                        let detail = Rc::new(detail);
                        this.cache_commit_detail(key, detail.clone());
                        DetailState::Ready(detail)
                    }
                    Err(err) => DetailState::Error(err),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// FIFO-capped bookkeeping for immutable commit details.
    fn cache_commit_detail(&mut self, key: (String, String), detail: Rc<CommitDetail>) {
        self.commit_detail_cache.insert(key.clone(), detail);
        self.commit_detail_order.retain(|cached| cached != &key);
        self.commit_detail_order.push_back(key);
        while self.commit_detail_order.len() > COMMIT_DETAIL_CACHE_MAX {
            if let Some(oldest) = self.commit_detail_order.pop_front() {
                self.commit_detail_cache.remove(&oldest);
            }
        }
    }

    /// Drop the detail state (history reset / tab switch).
    pub(super) fn clear_commit_detail(&mut self) {
        self.commit_detail_gen = self.commit_detail_gen.wrapping_add(1);
        self.commit_detail_for = None;
        self.commit_detail = DetailState::Empty;
        self.commit_file_selected = None;
        self.commit_file_diff = FileDiffState::Empty;
    }

    pub(super) fn set_commit_tab(&mut self, tab: CommitTab, cx: &mut Context<Self>) {
        if self.commit_tab != tab {
            self.commit_tab = tab;
            // Switching to CHANGES shows the first changed file right away:
            // the selected file when one was picked on the
            // INFORMATION tab, otherwise the first. A diff that is already
            // loaded (or loading) is left alone.
            if tab == CommitTab::Changes
                && matches!(self.commit_file_diff, FileDiffState::Empty)
            {
                let file_count = match &self.commit_detail {
                    DetailState::Ready(detail) => detail.files.len(),
                    _ => 0,
                };
                if file_count > 0 {
                    let ix = self.commit_file_selected.unwrap_or(0).min(file_count - 1);
                    self.open_commit_file(ix, cx);
                }
            }
            cx.notify();
        }
    }

    /// Single click on a changed file selects it.
    pub(super) fn select_commit_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.commit_file_selected != Some(ix) {
            self.commit_file_selected = Some(ix);
            cx.notify();
        }
    }

    /// Double click on a changed file opens it in the CHANGES tab (also used
    /// by the Changes tab's single click).
    pub(super) fn show_commit_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.commit_tab = CommitTab::Changes;
        self.open_commit_file(ix, cx);
    }

    /// Double click on a changed file loads its read-only diff.
    pub(super) fn open_commit_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        let DetailState::Ready(detail) = &self.commit_detail else {
            return;
        };
        let Some(file) = detail.files.get(ix).cloned() else {
            return;
        };
        let Some(repo) = self.active_repo() else {
            return;
        };
        let worktree = repo.path.to_string_lossy().into_owned();
        let parent = detail.parents.first().cloned();
        let hash = detail.hash.clone();
        self.commit_file_selected = Some(ix);
        self.commit_file_diff_gen = self.commit_file_diff_gen.wrapping_add(1);
        let generation = self.commit_file_diff_gen;
        // Immutable object pair: a cached diff (and every truncated/binary
        // state) is served without re-reading.
        let key = super::diff::DiffKey::Object {
            worktree: worktree.clone(),
            object: hash.clone(),
            path: file.path.clone(),
        };
        if let Some(cached) = self.diff_cache.get(&key).cloned() {
            self.commit_file_diff = FileDiffState::Ready(cached);
            self.commit_diff_scroll = UniformListScrollHandle::new();
            self.commit_diff_h_scroll = ScrollHandle::new();
            cx.notify();
            return;
        }
        self.commit_file_diff = FileDiffState::Loading;
        // Another file starts at the top, with no stale handle geometry.
        self.commit_diff_scroll = UniformListScrollHandle::new();
        self.commit_diff_h_scroll = ScrollHandle::new();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    crate::git::commit_file_diff(&worktree, parent.as_deref(), &hash, &file.path)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.commit_file_diff_gen != generation {
                    return;
                }
                this.commit_file_diff = match result {
                    Ok(diff) => {
                        let diff = Rc::new(diff);
                        this.cache_diff(key, diff.clone());
                        FileDiffState::Ready(diff)
                    }
                    Err(err) => FileDiffState::Error(err),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The panel below the history list (rendered only when a commit is
    /// selected).
    pub(super) fn render_commit_panel(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let body = match self.commit_tab {
            CommitTab::Information => self.commit_information(cx),
            CommitTab::Changes => self.commit_changes(cx),
        };
        // Until the history column is measured the panel keeps the even
        // split it had before it became resizable.
        let panel = if self.history_col_h > 0.0 {
            div()
                .flex_none()
                .h(px(self.history_col_h * self.commit_panel_ratio))
        } else {
            div().flex_1()
        };
        panel
            .relative()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(hairline(0.08))
            .child(self.commit_tab_strip(cx))
            .child(body)
            // Grab strip over the top border, last so it paints on top.
            .child(
                super::panes::pane_handle(
                    super::panes::Pane::CommitPanel,
                    cx.entity().downgrade(),
                )
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(5.)),
            )
            .into_any_element()
    }

    fn commit_tab_strip(&self, cx: &Context<Self>) -> impl IntoElement {
        let tabs = [
            (CommitTab::Information, t().commit_tab_information),
            (CommitTab::Changes, t().commit_tab_changes),
        ];
        let mut row = div()
            .flex_none()
            .h(px(30.))
            .min_w_0()
            .overflow_hidden()
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(16.));
        for (ix, (tab, label)) in tabs.into_iter().enumerate() {
            let active = self.commit_tab == tab;
            let mut button = div()
                .id(("commit-tab", ix))
                .flex_none()
                .cursor_pointer()
                .h_full()
                .flex()
                .items_center()
                .text_size(px(TEXT_XS))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if active { text_primary(cx) } else { text_faint(cx) });
            if active {
                // The active tab is underlined in the accent like SourceGit's.
                button = button.border_b_2().border_color(violet(cx));
            }
            row = row.child(button.child(label).on_click(cx.listener(
                move |this, _, _, cx| this.set_commit_tab(tab, cx),
            )));
        }
        row = row.child(div().flex_1());
        if let DetailState::Ready(detail) = &self.commit_detail {
            if self.commit_tab == CommitTab::Changes {
                // Shrinks and truncates instead of pushing the collapse button
                // out of the panel.
                row = row.child(
                    div()
                        .min_w_0()
                        .pr(px(6.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(TEXT_XS))
                        .text_color(text_faint(cx))
                        .child(t().changed_files_count(detail.files.len())),
                );
            }
            // Total line changes of the commit, upper right of the panel and
            // visible on both tabs.
            row = row.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pr(px(6.))
                    .font_family(MONO)
                    .text_size(px(TEXT_XS))
                    .child(
                        div()
                            .text_color(cx.theme().success)
                            .child(format!("+{}", detail.additions)),
                    )
                    .child(
                        div()
                            .text_color(cx.theme().danger)
                            .child(format!("−{}", detail.deletions)),
                    ),
            );
        }
        row.child(wash_icon_button(
            "collapse-commit-detail",
            IconName::ChevronDown,
            t().collapse,
            cx.listener(|this, _, _, cx| this.collapse_commit_detail(cx)),
            cx,
        ))
    }

    fn commit_information(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let detail = match &self.commit_detail {
            DetailState::Ready(detail) => detail.clone(),
            DetailState::Error(err) => return commit_error_note(err, cx),
            _ => return commit_note(t().commit_loading, cx),
        };
        // The column itself does not scroll: the message and
        // the changed-file list are two scroll areas that each own their wheel
        // events, and the file list always keeps visible space instead of
        // being pushed under the fold by a long message.
        let mut column = div()
            .id("commit-info")
            .flex_1()
            .min_h_0()
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .overflow_hidden();
        column = column.child(commit_info_row(
            t().commit_author,
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(px(TEXT_MD))
                        .text_color(text_primary(cx))
                        .child(detail.author.clone()),
                )
                .child(
                    div()
                        .font_family(MONO)
                        .text_size(px(TEXT_XS))
                        .text_color(text_muted(cx))
                        .child(detail.email.clone()),
                )
                .child(
                    div()
                        .font_family(MONO)
                        .text_size(px(TEXT_XS))
                        .text_color(text_faint(cx))
                        .child(detail.date.clone()),
                ),
            cx,
        ));
        column = column.child(commit_info_row(
            t().commit_sha,
            div()
                .font_family(MONO)
                .text_size(px(TEXT_SM))
                .text_color(text_primary(cx))
                .child(detail.hash.clone()),
            cx,
        ));
        if !detail.parents.is_empty() {
            let parents = detail
                .parents
                .iter()
                .map(|parent| short_hash(parent))
                .collect::<Vec<_>>()
                .join("  ");
            column = column.child(commit_info_row(
                t().commit_parents,
                div()
                    .font_family(MONO)
                    .text_size(px(TEXT_SM))
                    .text_color(text_muted(cx))
                    .child(parents),
                cx,
            ));
        }
        // The message shows its subject only; a message with a description
        // gets a chevron that expands the description in a capped scroll area.
        // Collapsed, the changed files (the more important content) keep the
        // panel space.
        let subject = commit_subject(&detail.message);
        let body = commit_body(&detail.message);
        let has_body = !body.is_empty();
        let open = self.commit_message_open && has_body;

        let mut message = div().flex().flex_col().gap(px(6.)).min_h_0();
        message = if open {
            message.flex_1()
        } else {
            message.flex_none()
        };
        let mut header = div()
            .id("commit-message-toggle")
            .flex()
            .items_start()
            .gap(px(10.))
            .child(
                div()
                    .w(px(78.))
                    .flex_none()
                    .text_size(px(TEXT_XS))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_faint(cx))
                    .child(t().commit_message),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(TEXT_MD))
                    .text_color(text_primary(cx))
                    .child(subject.to_string()),
            );
        if has_body {
            let hint = if open { t().collapse } else { t().expand };
            header = header
                .cursor_pointer()
                .aria_label(hint)
                .tooltip(move |window, cx| Tooltip::new(hint).build(window, cx))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.commit_message_open = !this.commit_message_open;
                    cx.notify();
                }))
                .child(
                    Icon::new(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(px(13.))
                    .flex_none()
                    .text_color(text_muted(cx)),
                );
        }
        message = message.child(header);
        if open {
            message = message.child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .max_h(px(MESSAGE_MAX_H))
                    .overflow_hidden()
                    .pl(px(88.))
                    .child(scrollbar_overlay(
                        "commit-message-scrollbar",
                        &self.commit_message_scroll,
                    ))
                    .child(
                        div()
                            .id("commit-message")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.commit_message_scroll)
                            .text_size(px(TEXT_MD))
                            .text_color(text_muted(cx))
                            .child(body.to_string()),
                    ),
            );
        }
        column
            .child(message)
            .child(self.commit_files_block(&detail, cx))
            .into_any_element()
    }

    /// INFORMATION tab: changed files in a virtualized list that keeps its own
    /// scrollbar and a guaranteed slice of the panel height, so the files stay
    /// visible next to a long commit message.
    fn commit_files_block(
        &self,
        detail: &Rc<CommitDetail>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let count = detail.files.len();
        let selected = self.commit_file_selected;
        let this = cx.entity().downgrade();
        let files = detail.clone();
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(COMMIT_FILES_MIN_H))
            .min_w_0()
            .overflow_hidden()
            .border_t_1()
            .border_color(hairline(0.06))
            .pt(px(4.))
            .child(scrollbar_overlay(
                "commit-files-info-scrollbar",
                &self.commit_files_scroll,
            ))
            .child(
                gpui_kit::uniform_list("commit-files-info", count, move |range, _window, cx| {
                    range
                        .map(|ix| {
                            commit_file_row(
                                ix,
                                &files.files[ix],
                                selected == Some(ix),
                                false,
                                this.clone(),
                                cx,
                            )
                        })
                        .collect()
                })
                .track_scroll(&self.commit_files_scroll)
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }

    fn commit_changes(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let detail = match &self.commit_detail {
            DetailState::Ready(detail) => detail.clone(),
            DetailState::Error(err) => return commit_error_note(err, cx),
            _ => return commit_note(t().commit_loading, cx),
        };
        if detail.files.is_empty() {
            return commit_note(t().commit_no_files, cx);
        }
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(self.commit_file_list(detail.clone(), cx))
            .child(self.commit_file_diff_pane(&detail, cx))
            .into_any_element()
    }

    fn commit_file_list(
        &self,
        detail: Rc<CommitDetail>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let count = detail.files.len();
        let selected = self.commit_file_selected;
        let this = cx.entity().downgrade();
        div()
            .w(px(COMMIT_FILES_W))
            .flex_none()
            .flex()
            .flex_col()
            .min_h_0()
            .border_r_1()
            .border_color(hairline(0.05))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .px(px(6.))
                    .py(px(4.))
                    .child(scrollbar_overlay(
                        "commit-files-scrollbar",
                        &self.commit_files_scroll,
                    ))
                    .child(
                        gpui_kit::uniform_list("commit-files", count, move |range, _window, cx| {
                            range
                                .map(|ix| {
                                    commit_file_row(
                                        ix,
                                        &detail.files[ix],
                                        selected == Some(ix),
                                        true,
                                        this.clone(),
                                        cx,
                                    )
                                })
                                .collect()
                        })
                        .track_scroll(&self.commit_files_scroll)
                        .flex_1()
                        .min_h_0(),
                    ),
            )
            .into_any_element()
    }

    fn commit_file_diff_pane(
        &self,
        detail: &Rc<CommitDetail>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let selection = self
            .commit_file_selected
            .and_then(|ix| detail.files.get(ix).cloned());
        let stats = match &self.commit_file_diff {
            FileDiffState::Ready(diff) => Some((diff.additions, diff.deletions)),
            _ => None,
        };
        let mut header = div()
            .flex_none()
            .h(px(32.))
            .min_w_0()
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(hairline(0.05));
        if let Some(file) = &selection {
            header = header.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(MONO)
                    .text_size(px(TEXT_SM))
                    .text_color(text_primary(cx))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(truncate_path(&file.display_path(), 64)),
            );
        } else {
            header = header.child(div().flex_1());
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
        let body: gpui_kit::AnyElement = match (&self.commit_file_diff, &selection) {
            (_, None) => commit_note(t().commit_diff_hint, cx),
            (FileDiffState::Loading, _) => div().flex_1().min_h_0().into_any_element(),
            (FileDiffState::Empty, Some(_)) => commit_note(t().commit_diff_hint, cx),
            (FileDiffState::Error(err), _) => commit_error_note(err, cx),
            (FileDiffState::Ready(diff), Some(_)) if diff.binary => {
                commit_note(t().diff_binary, cx)
            }
            (FileDiffState::Ready(diff), Some(_)) if diff.lines.is_empty() => {
                commit_note(t().no_changes, cx)
            }
            (FileDiffState::Ready(diff), Some(_)) => readonly_diff_list(
                ReadonlyDiffIds {
                    list: "commit-diff",
                    v_scrollbar: "commit-diff-v-scrollbar",
                    h_wrapper: "commit-diff-h",
                    h_scrollbar: "commit-diff-h-scrollbar",
                },
                diff.clone(),
                &self.commit_diff_scroll,
                &self.commit_diff_h_scroll,
            ),
        };
        div()
            // Definite basis beside the fixed-width file list: without
            // `w(px(0.))` the diff content's min-content width (the list is
            // `min_w(px(width))` for horizontal scrolling) sized this item and
            // pushed the panel past the window. Same containment as the Local
            // Changes diff column.
            .flex()
            .flex_1()
            .w(px(0.))
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }
}

/// `LABEL  value` row of the information tab.
fn commit_info_row(
    label: &str,
    value: impl IntoElement,
    cx: &Context<SpurShell>,
) -> gpui_kit::AnyElement {
    div()
        .flex()
        .items_start()
        .gap(px(10.))
        .child(
            div()
                .w(px(78.))
                .flex_none()
                .text_size(px(TEXT_XS))
                .font_weight(FontWeight::MEDIUM)
                .text_color(text_faint(cx))
                .child(label.to_string()),
        )
        .child(div().flex_1().min_w_0().child(value))
        .into_any_element()
}

fn commit_note(text: &str, cx: &Context<SpurShell>) -> gpui_kit::AnyElement {
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

fn commit_error_note(err: &str, cx: &Context<SpurShell>) -> gpui_kit::AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(TEXT_SM))
                .text_color(cx.theme().danger)
                .child(t().commit_error),
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

fn commit_file_row(
    ix: usize,
    file: &CommitFile,
    selected: bool,
    open_on_single_click: bool,
    this: WeakEntity<SpurShell>,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    let key = format!("commit-file-{ix}");
    // Modified stays neutral: the letter carries the meaning.
    let color = match file.status {
        'A' => cx.theme().success,
        'D' => cx.theme().danger,
        'R' | 'C' => cx.theme().warning,
        _ => text_muted(cx),
    };
    div()
        .id(("commit-file", ix))
        .w_full()
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(COMMIT_FILE_ROW_H))
        .px(px(8.))
        .rounded(px(CONTROL_RADIUS))
        .bg(if selected {
            ink(0.08)
        } else {
            hover_blend(&key, ink(0.0), ink(0.055))
        })
        .on_hover(hover_listener(key))
        .child(
            div()
                .w(px(12.))
                .flex_none()
                .font_family(MONO)
                .text_size(px(TEXT_SM))
                .text_color(color)
                .child(file.status.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(MONO)
                .text_size(px(TEXT_SM))
                .text_color(if selected {
                    text_primary(cx)
                } else {
                    text_muted(cx)
                })
                .overflow_hidden()
                .whitespace_nowrap()
                .child(file.display_path()),
        )
        .on_click(move |event, _, cx| {
            // Changes tab: any click shows the diff. Information tab: only a
            // double click switches to Changes with the file highlighted.
            let open = open_on_single_click || event.click_count() >= 2;
            this.update(cx, |this, cx| {
                this.select_commit_file(ix, cx);
                if open {
                    this.show_commit_file(ix, cx);
                }
            })
            .ok();
        })
        .into_any_element()
}

/// Changed-file rows as a plain column (the Information tab lists them under
/// the message; the Changes tab uses a virtualized list instead).
/// `abc1234` display form of a full hash (also used for parent lists).
fn short_hash(hash: &str) -> &str {
    if hash.len() > 7 { &hash[..7] } else { hash }
}
