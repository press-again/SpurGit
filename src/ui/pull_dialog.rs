//! Pull dialog: explicit remote and
//! remote-branch selection into the checked-out branch, local-changes
//! handling, and an explicit merge/rebase choice. The integration mode and
//! local-changes policy never follow user Git configuration; the backend
//! rechecks the checked-out branch before it runs.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _, DropdownButton};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, Sizable as _};
use gpui_kit::IntoElement;

use crate::git::PullChanges;
use crate::i18n::t;

use super::push_dialog::split_upstream;

/// A pull request while the dialog is open.
#[derive(Clone, Debug)]
pub(super) struct PullRequest {
    pub repo_id: String,
    /// Checked-out branch the pull lands in (read-only).
    pub local: String,
    /// Selected remote.
    pub remote: String,
    /// Selected branch on that remote.
    pub branch: String,
    /// Configured upstream, when any (default selection).
    pub upstream: Option<String>,
    pub changes: PullChanges,
    pub rebase: bool,
    /// True while the queued pull runs; the dialog stays open with a
    /// progress bar until the operation completes.
    pub busy: bool,
}

impl SpurShell {
    /// Open the pull dialog for the active repository. Refuses with a log
    /// line when there is no branch, no commit, or no remote.
    pub(super) fn request_pull(&mut self, cx: &mut Context<Self>) {
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        let Some(snapshot) = self.active_snapshot().map(|collected| collected.snapshot.clone())
        else {
            self.ops
                .push_info(t().log_action_failed("pull", "status not collected yet"));
            cx.notify();
            return;
        };
        let Some(local) = snapshot.branch.clone() else {
            self.ops
                .push_info(t().log_action_failed("pull", "no branch checked out"));
            cx.notify();
            return;
        };
        if snapshot.unborn {
            self.ops
                .push_info(t().log_action_failed("pull", "no commits yet"));
            cx.notify();
            return;
        }
        let upstream = snapshot.upstream.clone();
        let mut remotes = self.remote_names();
        if remotes.is_empty()
            && let Some((upstream_remote, _)) = upstream.as_deref().and_then(split_upstream)
        {
            remotes.push(upstream_remote.to_string());
        }
        if remotes.is_empty() {
            self.note_error(t().push_no_remotes.to_string(), cx);
            cx.notify();
            return;
        }
        // Prefer the remembered remote, then the upstream's remote, then the
        // first configured one.
        let remote = self
            .pull_remote
            .clone()
            .filter(|name| remotes.contains(name))
            .or_else(|| {
                upstream
                    .as_deref()
                    .and_then(split_upstream)
                    .map(|(remote, _)| remote.to_string())
            })
            .or_else(|| remotes.iter().any(|name| name == "origin").then(|| "origin".to_string()))
            .unwrap_or_else(|| remotes[0].clone());
        // Default branch: the upstream branch when it lives on the chosen
        // remote, otherwise the same name as the local branch (always one of
        // the options, so a pull never defaults to an unrelated branch).
        let options = self.remote_branch_options(&remote, &local, upstream.as_deref());
        let branch = upstream
            .as_deref()
            .and_then(split_upstream)
            .filter(|(upstream_remote, _)| *upstream_remote == remote.as_str())
            .map(|(_, branch)| branch.to_string())
            .filter(|branch| options.contains(branch))
            .unwrap_or_else(|| local.clone());
        self.pull_request = Some(PullRequest {
            repo_id,
            local,
            remote,
            branch,
            upstream,
            changes: self.pull_changes,
            rebase: self.pull_rebase,
            busy: false,
        });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_pull(&mut self, cx: &mut Context<Self>) {
        if self.pull_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: remember the choices and queue the explicit pull. The
    /// dialog stays open (busy) until the operation completes.
    pub(super) fn confirm_pull(&mut self, cx: &mut Context<Self>) {
        let Some(mut request) = self.pull_request.clone() else {
            return;
        };
        if request.busy || request.branch.is_empty() {
            return;
        }
        self.pull_remote = Some(request.remote.clone());
        self.pull_changes = request.changes;
        self.pull_rebase = request.rebase;
        request.busy = true;
        self.pull_request = Some(request.clone());
        self.change_ops
            .push_back(changes::ChangeOp::Pull(changes::PullOp {
                repo_id: request.repo_id,
                remote: request.remote,
                branch: request.branch,
                local: request.local,
                rebase: request.rebase,
                changes: request.changes,
            }));
        self.pump_change_ops(cx);
    }

    /// True when the selected remote branch is neither the checked-out
    /// branch's counterpart nor present locally: checking it out is then the
    /// likelier intent than merging it into the checked-out branch.
    fn pull_offers_checkout(&self, request: &PullRequest) -> bool {
        let upstream = request.upstream.as_deref().and_then(split_upstream);
        request.branch != request.local
            && upstream != Some((request.remote.as_str(), request.branch.as_str()))
            && !self.branches.iter().any(|branch| branch.name == request.branch)
    }

    /// Alternative to the pull: create a tracking local branch for the
    /// selected remote branch and switch to it.
    pub(super) fn checkout_from_pull(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.pull_request.clone() else {
            return;
        };
        if request.busy {
            return;
        }
        self.close_modal(cx);
        self.checkout_remote_branch(request.remote, request.branch, cx);
    }

    /// Full-window scrim + card shown while the dialog is open.
    pub(super) fn render_pull_dialog(
        &self,
        request: PullRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let remotes = self.remote_names();
        let menu_entity = cx.entity().downgrade();
        let menu_remotes = remotes.clone();
        let selected = request.remote.clone();
        let remote_menu = DropdownButton::new("pull-remote")
            .button(
                Button::new("pull-remote-btn")
                    .label(truncate_label(&selected, 28))
                    .ghost()
                    .xsmall()
                    .cursor_pointer()
                    .disabled(request.busy),
            )
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for name in &menu_remotes {
                    let entity = menu_entity.clone();
                    let label = name.clone();
                    let picked = name.clone();
                    let is_selected = name == &selected;
                    menu = menu.item(
                        PopupMenuItem::element(move |_window, _cx| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .self_stretch()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(truncate_label(&label, 32))
                        })
                        .checked(is_selected)
                        .on_click(move |_, _, cx| {
                            let picked = picked.clone();
                            entity
                                .update(cx, |this, cx| {
                                    let (local, upstream) = this
                                        .pull_request
                                        .as_ref()
                                        .map(|request| {
                                            (request.local.clone(), request.upstream.clone())
                                        })
                                        .unwrap_or_default();
                                    let options = this.remote_branch_options(
                                        &picked,
                                        &local,
                                        upstream.as_deref(),
                                    );
                                    if let Some(request) = this.pull_request.as_mut() {
                                        request.remote = picked.clone();
                                        // A branch that is not offered by the
                                        // new remote falls back to the local
                                        // branch's name.
                                        if !options.contains(&request.branch) {
                                            request.branch = local.clone();
                                        }
                                    }
                                    cx.notify();
                                })
                                .ok();
                        }),
                    );
                }
                menu
            });

        let branch_options =
            self.remote_branch_options(&request.remote, &request.local, request.upstream.as_deref());
        let branch_label = request.branch.clone();
        let branch_entity = cx.entity().downgrade();
        let branch_menu = DropdownButton::new("pull-branch")
            .button(
                Button::new("pull-branch-btn")
                    .label(truncate_label(&branch_label, 28))
                    .ghost()
                    .xsmall()
                    .cursor_pointer()
                    .disabled(request.busy),
            )
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for name in &branch_options {
                    let entity = branch_entity.clone();
                    let label = name.clone();
                    let picked = name.clone();
                    let is_selected = name == &branch_label;
                    menu = menu.item(
                        PopupMenuItem::element(move |_window, _cx| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .self_stretch()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(truncate_label(&label, 32))
                        })
                        .checked(is_selected)
                        .on_click(move |_, _, cx| {
                            let picked = picked.clone();
                            entity
                                .update(cx, |this, cx| {
                                    if let Some(request) = this.pull_request.as_mut() {
                                        request.branch = picked.clone();
                                    }
                                    cx.notify();
                                })
                                .ok();
                        }),
                    );
                }
                menu
            });

        // Local-changes handling: the same selection-row vocabulary as the
        // stash dialog.
        let options = [
            (
                PullChanges::Nothing,
                t().pull_changes_nothing,
                t().pull_changes_nothing_hint,
            ),
            (
                PullChanges::StashReapply,
                t().pull_changes_stash,
                t().pull_changes_stash_hint,
            ),
            (
                PullChanges::Discard,
                t().pull_changes_discard,
                t().pull_changes_discard_hint,
            ),
        ];
        let mut modes: Vec<gpui_kit::AnyElement> = Vec::new();
        for (ix, (mode, label, hint)) in options.into_iter().enumerate() {
            let selected = request.changes == mode;
            let entity = cx.entity().downgrade();
            modes.push(
                div()
                    .id(("pull-changes", ix))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(CONTROL_RADIUS))
                    .bg(if selected { ink(0.08) } else { ink(0.0) })
                    .hover(|style| style.bg(ink(0.06)))
                    .child(
                        Icon::new(if selected {
                            IconName::Check
                        } else {
                            IconName::Circle
                        })
                        .size(px(14.))
                        .text_color(if selected { violet(cx) } else { text_faint(cx) }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(TEXT_MD))
                                    .text_color(text_primary(cx))
                                    .child(label),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_XS))
                                    .text_color(text_faint(cx))
                                    .child(hint),
                            ),
                    )
                    .on_click(move |_, _, cx| {
                        entity
                            .update(cx, |this, cx| {
                                if let Some(request) = this.pull_request.as_mut()
                                    && !request.busy
                                {
                                    request.changes = mode;
                                }
                                cx.notify();
                            })
                            .ok();
                    })
                    .into_any_element(),
            );
        }

        let rebase_entity = cx.entity().downgrade();
        let can_pull = !request.branch.is_empty() && !remotes.is_empty() && !request.busy;
        let offers_checkout = self.pull_offers_checkout(&request);

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
                cx.listener(|this, _, _, cx| this.cancel_pull(cx)),
            )
            .child(
                div()
                    .w(px(480.))
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
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                Icon::new(IconName::ArrowDownToLine)
                                    .size(px(16.))
                                    .text_color(violet(cx)),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_LG))
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(text_primary(cx))
                                    .child(t().pull_title),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().pull_into),
                    )
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(TEXT_SM))
                            .text_color(text_primary(cx))
                            .child(request.local.clone()),
                    )
                    .child(
                        div()
                            .pt(px(2.))
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().push_remote),
                    )
                    .child(select_shell(remote_menu))
                    .child(
                        div()
                            .pt(px(2.))
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().pull_branch),
                    )
                    .child(select_shell(branch_menu))
                    .child(
                        div()
                            .pt(px(2.))
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().pull_changes),
                    )
                    .children(modes)
                    .child(
                        Checkbox::new("pull-rebase")
                            .checked(request.rebase)
                            .label(t().pull_rebase)
                            .on_change(move |checked, _, cx| {
                                let checked = *checked;
                                rebase_entity
                                    .update(cx, |this, cx| {
                                        if let Some(request) = this.pull_request.as_mut()
                                            && !request.busy
                                        {
                                            request.rebase = checked;
                                        }
                                        cx.notify();
                                    })
                                    .ok();
                            }),
                    )
                    .children(request.busy.then(|| {
                        busy_bar(t().pull_running(&request.remote, &request.branch), true, cx)
                    }))
                    .children(offers_checkout.then(|| {
                        div()
                            .text_size(px(TEXT_XS))
                            .text_color(text_muted(cx))
                            .child(t().pull_checkout_hint(
                                &request.remote,
                                &request.branch,
                                &request.local,
                            ))
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("pull-cancel")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .disabled(request.busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_pull(cx))),
                            )
                            .child({
                                let pull = Button::new("pull-confirm")
                                    .label(t().pull_confirm)
                                    .small()
                                    .disabled(!can_pull)
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| this.confirm_pull(cx)));
                                if offers_checkout {
                                    pull.outline()
                                } else {
                                    pull.primary()
                                }
                            })
                            .children(offers_checkout.then(|| {
                                Button::new("pull-checkout")
                                    .label(t().pull_checkout)
                                    .small()
                                    .primary()
                                    .disabled(!can_pull)
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.checkout_from_pull(cx)),
                                    )
                            })),
                    ),
            )
            .into_any_element()
    }
}
