//! Repository detail view: 200px section sidebar (History / Local Changes /
//! Stashes as pages; Local Branches / Remotes / Tags as collapsible lists)
//! plus the section content.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon, Sizable as _};
use gpui_kit::{FontWeight, IntoElement, SharedString, StatefulInteractiveElement as _};

use crate::i18n::t;
use crate::model::RowState;

/// Height cap for one expanded sidebar list before it scrolls on its own.
const SIDEBAR_LIST_MAX_H: f32 = 200.0;
/// One row in a sidebar list, matching the virtualized 32px layout.
const SIDEBAR_ROW_H: f32 = 32.0;
/// Above this many flattened rows, the branch/remote trees render
/// virtualized instead of animated: the 200px viewport never shows more than
/// a handful of rows, so building thousands per frame would dominate.
const VIRTUAL_TREE_THRESHOLD: usize = 240;

// ---- one grid for the sidebar's six rows ----
/// Height of every nav page and group header row.
const SIDE_ROW_H: f32 = 30.0;
/// The single gap used between label, count and action slot.
const SIDE_ROW_GAP: f32 = 6.0;
/// Trailing slot each row reserves for its action (empty when it has none),
/// so an action appearing never moves the count.
const SIDE_ACTION_W: f32 = 20.0;
/// Right-aligned count column width (three mono digits).
const SIDE_COUNT_W: f32 = 24.0;

/// Gap between a header label and its chips.
const HEADER_GAP: f32 = 6.0;
/// Branch chip beside the repository name, capped in characters so the name
/// keeps most of the line; the full branch shows on hover.
const SIDE_CHIP_MAX: usize = 10;

/// Pages of the repository sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepoSection {
    History,
    LocalChanges,
    Stashes,
}

impl RepoSection {
    pub(super) fn label(self) -> &'static str {
        match self {
            RepoSection::History => t().section_history,
            RepoSection::LocalChanges => t().section_local_changes,
            RepoSection::Stashes => t().section_stashes,
        }
    }

    const TOP: [RepoSection; 3] = [
        RepoSection::History,
        RepoSection::LocalChanges,
        RepoSection::Stashes,
    ];
}

/// The sidebar's collapsible lists: their content lives in
/// the sidebar instead of separate pages. `side_open` in [`SpurShell`] is
/// indexed by [`SideList::index`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SideList {
    Branches,
    Remotes,
    Tags,
}

impl SideList {
    pub(super) const ALL: [SideList; 3] = [
        SideList::Branches,
        SideList::Remotes,
        SideList::Tags,
    ];
    /// Stable key for element ids.
    pub(super) fn key(self) -> &'static str {
        match self {
            SideList::Branches => "branches",
            SideList::Remotes => "remotes",
            SideList::Tags => "tags",
        }
    }

    fn label(self) -> &'static str {
        match self {
            SideList::Branches => t().section_local_branches,
            SideList::Remotes => t().section_remotes,
            SideList::Tags => t().section_tags,
        }
    }

    fn index(self) -> usize {
        match self {
            SideList::Branches => 0,
            SideList::Remotes => 1,
            SideList::Tags => 2,
        }
    }
}

impl SpurShell {
    pub(super) fn render_repo_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Settled folder animations drop their rows (started in render; the
        // per-frame value read below never mutates).
        self.settle_folder_fades();
        self.sync_detail_lists(cx);
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(self.render_repo_sidebar(cx))
            .child(self.render_repo_content(cx))
    }

    /// Sidebar header and section rows align with the content header: same
    /// 44px strip height and same 12px text inset on both sides of the seam.
    fn render_repo_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entry = self.active_repo();
        let (name, path, branch): (String, String, Option<String>) = match entry {
            Some(e) => (
                e.name.clone(),
                e.path.to_string_lossy().into_owned(),
                e.branch.clone(),
            ),
            None => ("—".into(), String::new(), None),
        };
        // The name line carries the branch chip and the path line carries the
        // status chips. The labels take whatever the chips leave and are
        // truncated by measured text layout: the path from the start, so
        // the repository folder stays, the name at the end. Nothing clips
        // mid-glyph, whatever the characters, font metrics or chips.
        let branch_chip = branch.as_deref().map(|b| {
            let shown = truncate_label(b, SIDE_CHIP_MAX);
            let mut el = div()
                .id("repo-sidebar-branch")
                .flex_none()
                .child(chip(shown.clone(), text_muted(cx)));
            if shown != b {
                // A shortened branch name keeps its full value on hover.
                let full: SharedString = b.to_string().into();
                el = el.tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx));
            }
            el
        });
        // (label, danger) — the collecting chip is quiet, the stale one warns.
        let mut status_chips: Vec<(&'static str, bool)> = Vec::new();
        if entry
            .map(|e| matches!(e.state, RowState::Loading))
            .unwrap_or(false)
        {
            status_chips.push((t().collecting, false));
        }
        if entry.and_then(|e| e.stale_error()).is_some() {
            status_chips.push((t().stale_meta(), true));
        }
        // The full path rides on the truncated one as a tooltip.
        let mut path_el = div()
            .id("repo-sidebar-path")
            .flex_1()
            .min_w_0()
            .font_family(MONO)
            .text_size(px(10.5))
            .text_color(text_muted(cx))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis_start()
            .child(path.clone());
        if !path.is_empty() {
            let full: SharedString = path.clone().into();
            path_el =
                path_el.tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx));
        }
        let snapshot = self.active_snapshot();
        let details = self.active_details();
        let local_changes = self.change_lists.unstaged.len() + self.change_lists.staged.len();
        let stashes = snapshot.map(|c| c.snapshot.stash as usize).unwrap_or(0);
        let commits = self.history.len();
        let branches = self.branches.len();
        let remotes = details.map(|d| d.remotes.len()).unwrap_or(0);
        let tags = details.map(|d| d.tags.len()).unwrap_or(0);

        let count_for = move |s: RepoSection| match s {
            RepoSection::History => commits,
            RepoSection::LocalChanges => local_changes,
            RepoSection::Stashes => stashes,
        };
        let now = Instant::now();
        let side_count = |list: SideList| match list {
            SideList::Branches => branches,
            SideList::Remotes => remotes,
            SideList::Tags => tags,
        };
        let side_body = |list: SideList, open_f: f32, this: &Self, cx: &mut Context<Self>| match list {
            SideList::Branches => this.sidebar_branches(open_f, cx),
            SideList::Remotes => this.sidebar_remotes(open_f, cx),
            SideList::Tags => this.sidebar_tags(open_f, cx),
        };

        div()
            .relative()
            .w(px(self.sidebar_w))
            .flex_none()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(hairline(0.05))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(px(HEADER_H))
                    .px(px(12.))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(1.))
                    .border_b_1()
                    .border_color(hairline(0.05))
                    // Line 1: the repository name and the neutral branch chip,
                    // which lives here so it can never crowd the path.
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(HEADER_GAP))
                            .min_w_0()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(TEXT_MD))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(text_primary(cx))
                                    .child(name),
                            )
                            .children(branch_chip),
                    )
                    // Line 2: the path (full value in its tooltip) plus the
                    // status chips.
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(HEADER_GAP))
                            .min_w_0()
                            .child(path_el)
                            .children(
                                status_chips
                                    .into_iter()
                                    .map(|(label, danger)| {
                                        chip(
                                            label,
                                            if danger {
                                                cx.theme().danger
                                            } else {
                                                text_muted(cx)
                                            },
                                        )
                                    })
                                    .collect::<Vec<_>>(),
                            ),
                    ),
            )
            .child(
                div()
                    .id("repo-sidebar-nav")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(px(2.))
                    .p(px(8.))
                    // Only the collapsible lists scroll; the
                    // navigation column itself never shows a scrollbar.
                    .overflow_hidden()
                    .children(RepoSection::TOP.iter().map(|s| {
                        let s = *s;
                        nav_item(s, count_for(s), self.section == s, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.section = s;
                                cx.notify();
                            }))
                            .into_any_element()
                    }))
                    .child(
                        div()
                            .h(px(1.))
                            .bg(hairline(0.05))
                            .mx(px(8.))
                            .my(px(6.)),
                    )
                    .children(SideList::ALL.iter().map(|list| {
                        let list = *list;
                        let ix = list.index();
                        let open = self.side_open[ix];
                        let open_f = self.side_fade[ix].value_at(now);
                        // Every group action is an "add" and carries a
                        // tooltip; Tags has none, so its slot stays
                        // empty instead of shifting the count.
                        let action = match list {
                            SideList::Branches => Some(
                                wash_icon_button(
                                    "create-branch",
                                    IconName::Plus,
                                    t().tooltip_create_branch,
                                    cx.listener(|this, _, window, cx| {
                                        // The header toggles its list on click; the
                                        // button must not also collapse/expand it.
                                        cx.stop_propagation();
                                        this.request_create_branch(window, cx)
                                    }),
                                    cx,
                                )
                                .into_any_element(),
                            ),
                            SideList::Remotes => Some(
                                wash_icon_button(
                                    "add-remote",
                                    IconName::Plus,
                                    t().tooltip_add_remote,
                                    cx.listener(|this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.request_add_remote(window, cx)
                                    }),
                                    cx,
                                )
                                .into_any_element(),
                            ),
                            SideList::Tags => None,
                        };
                        let header = side_header(list, side_count(list), open, open_f, action, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let opening = !this.side_open[ix];
                                this.side_open[ix] = opening;
                                this.side_fade[ix].retarget_with(
                                    motion::Curve::Menu,
                                    if opening { 1.0 } else { 0.0 },
                                    Duration::from_millis(150),
                                    cx.reduce_motion(),
                                    Instant::now(),
                                );
                                cx.notify();
                            }));
                        // An open group sits on a raised card that fades in
                        // with the body.
                        let (card_bg, card_shadow) = raised_surface(open_f);
                        let mut group = div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .my(px(3. * open_f))
                            .pb(px(4. * open_f))
                            .rounded(px(CONTROL_RADIUS + 2.))
                            .bg(card_bg)
                            .shadow(card_shadow)
                            .child(header);
                        // While closing the body stays mounted until the fade
                        // reaches zero, then it is dropped.
                        if open || open_f > 0.0 {
                            group = group.child(side_body(list, open_f, self, cx));
                        }
                        group
                    })),
            )
            .children(self.sidebar_account(cx))
            // Grab strip over the right border, last so it paints on top.
            .child(
                super::panes::pane_handle(super::panes::Pane::Sidebar, cx.entity().downgrade())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .w(px(5.)),
            )
    }

    fn render_repo_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let content: gpui_kit::AnyElement = match self.section {
            RepoSection::History => self.section_history(cx).into_any_element(),
            RepoSection::LocalChanges => self.render_changes_section(cx).into_any_element(),
            RepoSection::Stashes => self.section_stashes(cx).into_any_element(),
        };
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(content)
    }

    pub(super) fn section_strip(
        &self,
        left: impl IntoElement,
        right: impl IntoElement,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(HEADER_H))
            .px(px(12.))
            .border_b_1()
            .border_color(hairline(0.05))
            .child(left)
            .child(div().flex_1())
            .child(right)
            .text_size(px(TEXT_SM))
            .text_color(text_muted(cx))
    }

    /// Right-hand note for the section strip: the live stale error, if any.
    pub(super) fn live_note(&self, cx: &Context<Self>) -> impl IntoElement {
        let (text, color) = match self.active_repo().and_then(|entry| entry.stale_error()) {
            Some(error) => (
                format!("{}: {error}", t().stale_meta()),
                cx.theme().danger,
            ),
            None => (String::new(), text_faint(cx)),
        };
        div()
            .max_w(px(460.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(TEXT_XS))
            .text_color(color)
            .child(text)
    }

    /// Expanded Local Branches: SourceGit-style folder tree (branches sharing
    /// a `/` prefix group under collapsible folders), capped height, thin
    /// scrollbar. Very long trees fall back to virtualized rows (no
    /// per-folder height animation): the viewport is only 200px tall, so
    /// building every row per frame dominates for ref-heavy repositories.
    fn sidebar_branches(&self, open_f: f32, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let rows = self.branch_rows.clone();
        let count = rows.len();
        let pinned_infos = self.pinned_branch_rows.clone();
        if count == 0 && pinned_infos.is_empty() {
            return sidebar_empty(t().no_branches, open_f, cx);
        }
        let branches = self.branch_tree_branches.clone();
        let default: Option<SharedString> = self
            .active_snapshot()
            .and_then(|collected| collected.refs.default_branch.clone())
            .map(SharedString::from);
        let pinned_names: std::collections::HashSet<String> = pinned_infos
            .iter()
            .map(|branch| branch.name.clone())
            .collect();
        let row_ctx = BranchRowCtx {
            default,
            pinned_names,
            sort: self.branch_sort,
            this: cx.entity().downgrade(),
        };
        // Pinned branches render above the tree in pin order, with the same
        // row treatment and menu. Their element ids live past any tree index.
        let pinned_ctx = row_ctx.clone();
        let pinned_section: Vec<gpui_kit::AnyElement> = pinned_infos
            .iter()
            .enumerate()
            .map(|(position, info)| {
                const PINNED_ID_OFFSET: usize = 1_000_000;
                let ix = PINNED_ID_OFFSET + position;
                let entity = pinned_ctx.this.clone();
                let name = info.name.clone();
                let current = info.current;
                let leaf = branch_tree::branch_leaf(&info.name);
                sidebar_branch_row(ix, info, &leaf, 0, pinned_ctx.default.as_deref(), cx)
                    .context_menu(move |menu, _, _| {
                        super::branch_menu::branch_menu(
                            menu,
                            name.clone(),
                            current,
                            true,
                            pinned_ctx.sort,
                            entity.clone(),
                        )
                    })
                    .into_any_element()
            })
            .collect();
        let pinned_header: Option<gpui_kit::AnyElement> = (!pinned_infos.is_empty()).then(|| {
            div()
                .flex_none()
                .pl(px(20.))
                .pr(px(8.))
                .pt(px(4.))
                .text_size(px(TEXT_XS))
                .text_color(text_faint(cx))
                .child(t().branch_pinned)
                .into_any_element()
        });
        if count > VIRTUAL_TREE_THRESHOLD {
            return div()
                .flex()
                .flex_col()
                .children(pinned_header)
                .children(pinned_section)
                .child({
                    let row_ctx = row_ctx.clone();
                    sidebar_virtual_list(
                        "sidebar-branches".into(),
                        "sidebar-branches-scrollbar",
                        &self.branches_vscroll,
                        open_f,
                        count,
                        move |ix, cx| branch_row_at(ix, &rows, &branches, &row_ctx, cx),
                    )
                })
                .into_any_element();
        }
        // Folder animations scale each row by the smallest open fraction of
        // its ancestor folders, so a collapsing folder clips its whole
        // subtree like the collapsible above it.
        let now = Instant::now();
        let factors: Rc<Vec<f32>> = Rc::new(
            rows.iter()
                .map(|row| {
                    let name = match row {
                        branch_tree::BranchRow::Folder { path, .. } => path.as_str(),
                        branch_tree::BranchRow::Branch { index, .. } => {
                            branches[*index].name.as_str()
                        }
                    };
                    name.match_indices('/')
                        .map(|(slash, _)| self.folder_fade(&format!("b/{}", &name[..slash]), now))
                        .fold(1.0f32, f32::min)
                })
                .collect(),
        );
        let row_ctx = row_ctx.clone();
        let body = sidebar_list(
            "sidebar-branches".into(),
            "sidebar-branches-scrollbar",
            &self.branches_scroll,
            open_f,
            factors,
            cx,
            move |ix, cx| branch_row_at(ix, &rows, &branches, &row_ctx, cx),
        );
        div()
            .flex()
            .flex_col()
            .children(pinned_header)
            .children(pinned_section)
            .child(body)
            .into_any_element()
    }

    /// Expanded Remotes: remote folders with their remote-tracking branches
    /// (each branch carries the checkout context menu), capped height, thin
    /// scrollbar. The flattened rows are cached in the shell.
    fn sidebar_remotes(&self, open_f: f32, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let remotes = self
            .active_details()
            .map(|details| details.remotes.len())
            .unwrap_or(0);
        let branches = self.remote_branches.clone();
        let rows = self.remote_rows.clone();
        let count = rows.len();
        if remotes == 0 && branches.is_empty() {
            return sidebar_empty(t().no_remotes, open_f, cx);
        }
        let this = cx.entity().downgrade();
        if count > VIRTUAL_TREE_THRESHOLD {
            return sidebar_virtual_list(
                "sidebar-remotes".into(),
                "sidebar-remotes-scrollbar",
                &self.remotes_vscroll,
                open_f,
                count,
                move |ix, cx| remote_row_at(ix, &rows, &branches, this.clone(), cx),
            );
        }
        let now = Instant::now();
        let factors: Rc<Vec<f32>> = Rc::new(
            rows.iter()
                .map(|row| match row {
                    branch_tree::RemoteRow::Remote { .. } => 1.0,
                    branch_tree::RemoteRow::Branch { index, .. } => {
                        self.folder_fade(&format!("r/{}", branches[*index].remote), now)
                    }
                })
                .collect(),
        );
        sidebar_list(
            "sidebar-remotes".into(),
            "sidebar-remotes-scrollbar",
            &self.remotes_scroll,
            open_f,
            factors,
            cx,
            move |ix, cx| remote_row_at(ix, &rows, &branches, this.clone(), cx),
        )
    }

    /// Expanded Tags: compact virtualized rows, capped height, own scrollbar.
    /// Past 20 tags a filter box appears above the list; the
    /// rows carry the tag context menu (checkout, copy, push, delete).
    fn sidebar_tags(&self, open_f: f32, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let all_tags: Vec<(String, String)> = self
            .active_details()
            .map(|details| details.tags.clone())
            .unwrap_or_default();
        if all_tags.is_empty() {
            return sidebar_empty(t().no_tags, open_f, cx);
        }
        let query = self.tag_filter.trim().to_lowercase();
        let tags: Vec<(String, String)> = if query.is_empty() {
            all_tags.clone()
        } else {
            all_tags
                .iter()
                .filter(|(name, _)| name.to_lowercase().contains(&query))
                .cloned()
                .collect()
        };
        let count = tags.len();
        let this = cx.entity().downgrade();
        let list = sidebar_virtual_list(
            "sidebar-tags".into(),
            "sidebar-tags-scrollbar",
            &self.tags_scroll,
            open_f,
            count,
            move |ix, cx| {
                let (name, hash) = &tags[ix];
                let entity = this.clone();
                let tag = name.clone();
                sidebar_row(format!("tag-{ix}"), ("tag", ix), 0, cx)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(MONO)
                            .text_size(px(TEXT_SM))
                            .text_color(cx.theme().warning)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(MONO)
                            .text_size(px(TEXT_XS))
                            .text_color(text_faint(cx))
                            .child(hash.clone()),
                    )
                    .context_menu(move |menu, _, _| {
                        super::tag_dialog::tag_menu(menu, tag.clone(), entity.clone())
                    })
                    .into_any_element()
            },
        );
        if all_tags.len() <= 20 {
            return list;
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div().px(px(8.)).pb(px(4.)).child(
                    gpui_kit::component::input::Input::new(&self.tag_filter_input)
                        .small()
                        .w_full(),
                ),
            )
            .child(list)
            .into_any_element()
    }

    /// Queue a checkout of an existing branch (branch context menu) on the
    /// shared sequential queue.
    pub(super) fn checkout_branch(&mut self, name: String, cx: &mut Context<Self>) {
        if let Some(repo_id) = self.active_repo_id() {
            self.change_ops
                .push_back(changes::ChangeOp::Checkout(changes::CheckoutOp {
                    repo_id,
                    name,
                }));
            self.pump_change_ops(cx);
        }
    }

    /// Queue a checkout of a remote-tracking branch (Remotes context menu);
    /// the Git layer creates a tracking local branch when none exists.
    pub(super) fn checkout_remote_branch(
        &mut self,
        remote: String,
        branch: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(repo_id) = self.active_repo_id() {
            self.change_ops.push_back(changes::ChangeOp::CheckoutRemote(
                changes::CheckoutRemoteOp {
                    repo_id,
                    remote,
                    branch,
                },
            ));
            self.pump_change_ops(cx);
        }
    }

    /// Open a remote's web page in the default browser (Remotes context menu).
    /// Local-path and SSH-only remotes have no page; the log says so.
    pub(super) fn open_remote_in_browser(
        &mut self,
        remote: &str,
        url: &str,
        cx: &mut Context<Self>,
    ) {
        match web_url(url) {
            Some(web) => {
                match std::process::Command::new("explorer.exe").arg(&web).spawn() {
                    Ok(_) => {
                        crate::logging::log!("browser: {web}");
                        self.ops.push_info(t().log_opened_url(&web));
                    }
                    Err(err) => {
                        self.ops
                            .push_info(t().log_action_failed("open in browser", &err.to_string()));
                    }
                }
            }
            None => self.note_error(t().log_no_web_url(remote), cx),
        }
        cx.notify();
    }
}

/// Compact local-branch row for the sidebar: the leaf name truncates, then
/// the state chips follow. The current branch carries SourceGit's green check
/// and a semibold name. The caller attaches the checkout context menu.
fn sidebar_branch_row(
    ix: usize,
    branch: &crate::status::BranchInfo,
    leaf: &str,
    depth: usize,
    default_branch: Option<&str>,
    cx: &mut gpui_kit::App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let is_default = default_branch == Some(branch.name.as_str());
    let mut name = div()
        .flex_1()
        .min_w_0()
        .font_family(MONO)
        .text_size(px(TEXT_SM))
        .text_color(text_primary(cx))
        .overflow_hidden()
        .whitespace_nowrap()
        .child(leaf.to_string());
    if branch.current {
        name = name.font_weight(FontWeight::SEMIBOLD);
    }
    // The checked-out branch carries a light success wash so it reads at a
    // glance even before the green check icon.
    let key = format!("branch-{ix}");
    let (base, hover) = if branch.current {
        (
            cx.theme().success.alpha(0.12),
            cx.theme().success.alpha(0.20),
        )
    } else {
        (ink(0.0), ink(0.04))
    };
    let mut row = sidebar_row(key.clone(), ("branch", ix), depth, cx)
        .bg(hover_blend(&key, base, hover))
        .child(
            Icon::new(if branch.current {
                IconName::CircleCheck
            } else {
                IconName::GitBranch
            })
            .size(px(13.))
            .flex_none()
            .text_color(if branch.current {
                cx.theme().success
            } else {
                text_faint(cx)
            }),
        )
        .child(name);
    // Sync counts against the upstream, in faint mono. Color never
    // carries the meaning alone: the arrows are always explicit.
    if branch.upstream.is_some() && !branch.upstream_gone {
        let mut counts = String::new();
        if branch.ahead > 0 {
            counts.push_str(&format!("⇡{}", branch.ahead));
        }
        if branch.behind > 0 {
            if !counts.is_empty() {
                counts.push(' ');
            }
            counts.push_str(&format!("⇣{}", branch.behind));
        }
        if !counts.is_empty() {
            row = row.child(
                div()
                    .flex_none()
                    .font_family(MONO)
                    .text_size(px(TEXT_XS))
                    .text_color(text_faint(cx))
                    .child(counts),
            );
        }
    }
    if is_default && !branch.current {
        row = row.child(chip(t().default_branch, text_muted(cx)));
    }
    if branch.upstream_gone {
        row = row.child(chip(t().stale_meta(), cx.theme().danger));
    }
    row
}

/// One folder row of the branch tree: chevron, folder icon, name, subtree
/// count. Clicking toggles the folder through the shell's closed set.
fn sidebar_folder_row(
    ix: usize,
    row: &branch_tree::BranchRow,
    this: gpui_kit::WeakEntity<SpurShell>,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    let branch_tree::BranchRow::Folder {
        path,
        name,
        count,
        open,
        depth,
    } = row
    else {
        return div().into_any_element();
    };
    let path = path.clone();
    sidebar_row(
        format!("branch-folder-{path}"),
        ("branch-folder", ix),
        *depth,
        cx,
    )
    .on_click(move |_, _, cx| {
        this.update(cx, |shell, cx| {
            // `remove` reports whether it was closed: that is the opening
            // direction of this toggle.
            let opening = shell.branch_folders_closed.remove(&path);
            if !opening {
                shell.branch_folders_closed.insert(path.clone());
            }
            shell.set_folder_fade(&format!("b/{path}"), opening, cx);
            shell.rebuild_branch_rows();
            cx.notify();
        })
        .ok();
    })
    .child(
        Icon::new(if *open {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        })
        .size(px(12.))
        .flex_none()
        .text_color(text_faint(cx)),
    )
    .child(
            Icon::new(if *open {
                IconName::FolderOpen
            } else {
                IconName::Folder
            })
            .size(px(13.))
            .flex_none()
            // Decorative group icons stay neutral.
            .text_color(text_muted(cx)),
    )
    .child(
        div()
            .flex_1()
            .min_w_0()
            .text_size(px(TEXT_SM))
            .text_color(text_primary(cx))
            .overflow_hidden()
            .whitespace_nowrap()
            .child(truncate_label(name, 16)),
    )
    .child(
        div()
            .font_family(MONO)
            .text_size(px(TEXT_XS))
            .text_color(text_faint(cx))
            .child(format!("({count})")),
    )
    .into_any_element()
}

/// One remote row: chevron, cloud icon, name, branch count. The remote's URL
/// is shown on hover as a tooltip, like the shell's buttons.
/// Clicking toggles its branch list through the shell's closed set.
fn sidebar_remote_row(
    ix: usize,
    row: &branch_tree::RemoteRow,
    this: gpui_kit::WeakEntity<SpurShell>,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    let branch_tree::RemoteRow::Remote {
        name,
        url,
        count,
        open,
    } = row
    else {
        return div().into_any_element();
    };
    let toggle_key = name.clone();
    let menu_this = this.clone();
    let menu_remote = name.clone();
    let menu_url = url.clone();
    let hover_url = url.clone();
    sidebar_row(format!("remote-{name}"), ("remote", ix), 0, cx)
        .on_click(move |_, _, cx| {
            this.update(cx, |shell, cx| {
                let opening = shell.remote_folders_closed.remove(&toggle_key);
                if !opening {
                    shell.remote_folders_closed.insert(toggle_key.clone());
                }
                shell.set_folder_fade(&format!("r/{toggle_key}"), opening, cx);
                shell.rebuild_remote_rows();
                cx.notify();
            })
            .ok();
        })
        .tooltip(move |window, cx| {
            Tooltip::new(hover_url.clone()).build(window, cx)
        })
        .child(
            Icon::new(if *open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .size(px(12.))
            .flex_none()
            .text_color(text_faint(cx)),
        )
        .child(
            Icon::new(IconName::Cloud)
                .size(px(13.))
                .flex_none()
                // Decorative group icons stay neutral.
                .text_color(text_muted(cx)),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(TEXT_SM))
                .text_color(text_primary(cx))
                .child(truncate_label(name, 14)),
        )
        .child(
            div()
                .flex_none()
                .font_family(MONO)
                .text_size(px(TEXT_XS))
                .text_color(text_faint(cx))
                .child(format!("({count})")),
        )
        .context_menu(move |menu, _, _| {
            super::remote_dialog::remote_menu(
                menu,
                menu_remote.clone(),
                menu_url.clone(),
                menu_this.clone(),
            )
        })
        .into_any_element()
}

/// One remote-tracking-branch row (indented under its remote). The caller
/// attaches the checkout context menu.
fn sidebar_remote_branch_row(
    ix: usize,
    leaf: &str,
    cx: &mut gpui_kit::App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    sidebar_row(format!("remote-branch-{ix}"), ("remote-branch", ix), 1, cx)
        .child(
            Icon::new(IconName::GitBranch)
                .size(px(13.))
                .flex_none()
                .text_color(text_faint(cx)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(MONO)
                .text_size(px(TEXT_SM))
                .text_color(text_primary(cx))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(leaf.to_string()),
        )
}

/// Shared compact sidebar-row shell (32px, hover wash, rounded). `depth`
/// indents the row inside the branch tree; every row sits one level below
/// its group header and draws one guide line per ancestor level, each
/// running down from that ancestor's chevron.
fn sidebar_row(
    key: String,
    id: (&'static str, usize),
    depth: usize,
    _cx: &mut gpui_kit::App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    const STEP: f32 = 12.0;
    // Center of a 12px chevron at an 8px row inset.
    const GUIDE_X: f32 = 14.0;
    let indent = 8.0 + (depth + 1) as f32 * STEP;
    div()
        .id(id)
        .relative()
        .w_full()
        .cursor_pointer()
        .children((0..=depth).map(|level| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(GUIDE_X + level as f32 * STEP))
                .w(px(1.))
                .bg(hairline(0.08))
        }))
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(SIDEBAR_ROW_H))
        .pl(px(indent))
        .pr(px(8.))
        .rounded(px(CONTROL_RADIUS))
        .bg(hover_blend(&key, ink(0.0), ink(0.04)))
        .on_hover(hover_listener(key))
}

/// One expanded sidebar list: plain rows (not virtualized) so a folder can
/// animate its subtree's height, capped at [`SIDEBAR_LIST_MAX_H`] so a long
/// list scrolls in place. `open_f` is the collapsible's open fraction;
/// `factors` scales each row for its folders' expand/collapse animations.
/// Used below [`VIRTUAL_TREE_THRESHOLD`] rows.
fn sidebar_list(
    id: SharedString,
    scrollbar_id: &'static str,
    scroll: &ScrollHandle,
    open_f: f32,
    factors: Rc<Vec<f32>>,
    cx: &mut gpui_kit::App,
    row: impl Fn(usize, &mut gpui_kit::App) -> gpui_kit::AnyElement + 'static,
) -> gpui_kit::AnyElement {
    let count = factors.len();
    let total: f32 = (0..count).map(|ix| SIDEBAR_ROW_H * factors[ix]).sum();
    let mut rows = Vec::with_capacity(count);
    for ix in 0..count {
        let f = factors[ix];
        rows.push(
            div()
                .w_full()
                .flex_none()
                .h(px(SIDEBAR_ROW_H * f))
                .opacity(f)
                .overflow_hidden()
                .child(row(ix, cx))
                .into_any_element(),
        );
    }
    div()
        .relative()
        .w_full()
        .h(px(total.min(SIDEBAR_LIST_MAX_H) * open_f))
        .opacity(open_f)
        .overflow_hidden()
        .child(thin_scrollbar_overlay(scrollbar_id, scroll))
        .child(
            div()
                .id(id)
                .absolute()
                .inset_0()
                .overflow_y_scroll()
                .track_scroll(scroll)
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .children(rows),
                ),
        )
        .into_any_element()
}

/// Virtualized sidebar list (tags always; branch/remote trees above
/// [`VIRTUAL_TREE_THRESHOLD`]): only the visible rows are built.
fn sidebar_virtual_list(
    id: SharedString,
    scrollbar_id: &'static str,
    scroll: &UniformListScrollHandle,
    open_f: f32,
    count: usize,
    row: impl Fn(usize, &mut gpui_kit::App) -> gpui_kit::AnyElement + 'static,
) -> gpui_kit::AnyElement {
    let height = (count as f32 * SIDEBAR_ROW_H).min(SIDEBAR_LIST_MAX_H);
    div()
        .relative()
        .w_full()
        .h(px(height * open_f))
        .opacity(open_f)
        .overflow_hidden()
        .child(thin_scrollbar_overlay(scrollbar_id, scroll))
        .child(
            gpui_kit::uniform_list(id, count, move |range, _window, cx| {
                range.map(|ix| row(ix, cx)).collect()
            })
            .w_full()
            .h(px(height))
            .track_scroll(scroll),
        )
        .into_any_element()
}

/// Shared inputs for one branch-tree row (both renders). Owned so the
/// row closures stay `'static`.
#[derive(Clone)]
struct BranchRowCtx {
    default: Option<SharedString>,
    /// This repository's pinned branch names, for the Pin/Unpin menu label.
    pinned_names: std::collections::HashSet<String>,
    sort: crate::settings::BranchSort,
    this: gpui_kit::WeakEntity<SpurShell>,
}

/// One branch-tree row (shared by the animated and virtualized renders).
fn branch_row_at(
    ix: usize,
    rows: &[branch_tree::BranchRow],
    branches: &[crate::status::BranchInfo],
    ctx: &BranchRowCtx,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    match &rows[ix] {
        branch_tree::BranchRow::Folder { .. } => {
            sidebar_folder_row(ix, &rows[ix], ctx.this.clone(), cx)
        }
        branch_tree::BranchRow::Branch { index, leaf, depth } => {
            let entity = ctx.this.clone();
            let branch_name = branches[*index].name.clone();
            let pinned = ctx.pinned_names.contains(&branch_name);
            let current = branches[*index].current;
            let sort = ctx.sort;
            sidebar_branch_row(ix, &branches[*index], leaf, *depth, ctx.default.as_deref(), cx)
                .context_menu(move |menu, _, _| {
                    super::branch_menu::branch_menu(
                        menu,
                        branch_name.clone(),
                        current,
                        pinned,
                        sort,
                        entity.clone(),
                    )
                })
                .into_any_element()
        }
    }
}

/// One remote-tree row (shared by the animated and virtualized renders).
fn remote_row_at(
    ix: usize,
    rows: &[branch_tree::RemoteRow],
    branches: &[crate::status::RemoteBranch],
    this: gpui_kit::WeakEntity<SpurShell>,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    match &rows[ix] {
        branch_tree::RemoteRow::Remote { .. } => sidebar_remote_row(ix, &rows[ix], this, cx),
        branch_tree::RemoteRow::Branch { index, leaf } => {
            let entity = this.clone();
            let remote = branches[*index].remote.clone();
            let branch_name = branches[*index].name.clone();
            sidebar_remote_branch_row(ix, leaf, cx)
                .context_menu(move |menu, _, _| {
                    let entity = entity.clone();
                    let remote = remote.clone();
                    let name = branch_name.clone();
                    menu.item(context_menu_item(
                        t().context_checkout.into(),
                        IconName::GitBranch,
                        move |_, _, cx| {
                            let remote = remote.clone();
                            let name = name.clone();
                            entity
                                .update(cx, |shell, cx| {
                                    shell.checkout_remote_branch(remote, name, cx)
                                })
                                .ok();
                        },
                    ))
                })
                .into_any_element()
        }
    }
}

/// Shared chrome for the sidebar's six rows: one height, padding, gap,
/// hover wash and selected treatment for the nav pages and the group
/// headers alike. `selected` is the active page; open groups are not
/// selected (see `side_header`).
fn side_row_shell(
    key: String,
    selected: bool,
    cx: &Context<SpurShell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let mut row = div()
        .id(SharedString::from(key.clone()))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(SIDE_ROW_GAP))
        .h(px(SIDE_ROW_H))
        .px(px(8.))
        .rounded(px(CONTROL_RADIUS))
        .text_size(px(TEXT_MD))
        .bg(if selected {
            ink(0.07)
        } else {
            hover_blend(&key, ink(0.0), ink(0.04))
        })
        .on_hover(hover_listener(key.clone()));
    row = if selected {
        row.text_color(text_primary(cx))
    } else {
        row.text_color(hover_blend(&key, text_muted(cx), text_primary(cx)))
    };
    row
}

/// The count column every row shares: faint mono digits in a fixed-width
/// right-aligned cell, so all six counts line up in one column.
fn count_column(count: usize, cx: &Context<SpurShell>) -> gpui_kit::AnyElement {
    div()
        .flex_none()
        .min_w(px(SIDE_COUNT_W))
        .text_right()
        .font_family(MONO)
        .text_size(px(TEXT_XS))
        .text_color(text_faint(cx))
        .child(format!("{count}"))
        .into_any_element()
}

/// The trailing slot every row reserves for its action; empty when the row
/// has none, so an action never shifts the count.
fn action_slot(action: Option<gpui_kit::AnyElement>) -> gpui_kit::AnyElement {
    div()
        .flex_none()
        .w(px(SIDE_ACTION_W))
        .h(px(SIDE_ACTION_W))
        .children(action)
        .into_any_element()
}

/// A collapsible group header: chevron, label, count, action slot. The
/// action is mounted only while the row is hovered — an invisible button
/// would still swallow clicks, and its slot is reserved either way.
fn side_header(
    list: SideList,
    count: usize,
    open: bool,
    open_f: f32,
    action: Option<gpui_kit::AnyElement>,
    cx: &Context<SpurShell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let key = format!("side-{}", list.key());
    let hover = motion::hover_progress(&key);
    let action = (hover > 0.0).then(|| {
        div()
            .size(px(SIDE_ACTION_W))
            .opacity(hover)
            .children(action)
            .into_any_element()
    });
    // An open group is expanded, not selected: it keeps primary text and a
    // header wash marking the top of its card, lighter than the selected one.
    let row = side_row_shell(key.clone(), false, cx).bg(hover_blend(
        &key,
        ink(0.05 * open_f),
        ink(0.04 + 0.03 * open_f),
    ));
    let row = if open {
        row.text_color(text_primary(cx))
    } else {
        row
    };
    row.child(
        Icon::new(if open {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        })
        .size(px(12.))
        .flex_none()
        .text_color(text_faint(cx)),
    )
    .child(
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .child(list.label()),
    )
    .child(count_column(count, cx))
    .child(action_slot(action))
}

/// Faint note for an expanded but empty sidebar list, revealed with the
/// collapsible's open fraction.
fn sidebar_empty(text: &str, open_f: f32, cx: &Context<SpurShell>) -> gpui_kit::AnyElement {
    div()
        .h(px(30. * open_f))
        .opacity(open_f)
        .overflow_hidden()
        .child(
            div()
                .px(px(10.))
                .py(px(6.))
                .text_size(px(TEXT_XS))
                .text_color(text_faint(cx))
                .child(text.to_string()),
        )
        .into_any_element()
}

/// A nav page row: the same shell, count column and action slot as the
/// group headers.
fn nav_item(
    s: RepoSection,
    count: usize,
    active: bool,
    cx: &Context<SpurShell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let key = format!("nav-{}", s.label());
    side_row_shell(key, active, cx)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(s.label()),
        )
        .child(count_column(count, cx))
        .child(action_slot(None))
}
