//! Branch row menu: checkout, rebase (checked-out branch), rename, delete,
//! and pin.
//!
//! Merge, compare, and push/upstream actions are deliberately absent: the
//! menu grows with those features instead of offering dead rows.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _, DropdownButton};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, Sizable as _};
use gpui_kit::{IntoElement, WeakEntity};
use gpui_kit::SharedString;

use crate::i18n::t;

/// Rename dialog state: the branch being renamed. The new name lives in
/// [`SpurShell::branch_rename_input`].
#[derive(Clone, Debug)]
pub(super) struct RenameRequest {
    pub repo_id: String,
    pub old: String,
}

/// Rebase dialog state: the checked-out branch, the chosen target, and
/// whether local changes are stashed around the rebase.
#[derive(Clone, Debug)]
pub(super) struct RebaseRequest {
    pub repo_id: String,
    pub branch: String,
    pub onto: String,
    pub autostash: bool,
}

/// First delete confirm: the branch plus its repo identity.
#[derive(Clone, Debug)]
pub(super) struct BranchDeleteRequest {
    pub repo_id: String,
    pub name: String,
}

/// Second confirm, shown only after git refused the safe delete: the branch
/// is not fully merged and deleting loses commits.
#[derive(Clone, Debug)]
pub(super) struct RefusedDelete {
    pub repo_id: String,
    pub name: String,
}

/// The branch row menu: checkout plus rename/delete/pin and the tree sort
/// toggle; the checked-out branch (`current`) also offers a rebase. `pinned`
/// selects the Pin/Unpin label; `sort` selects which sort direction the
/// toggle offers.
pub(super) fn branch_menu(
    menu: PopupMenu,
    name: String,
    current: bool,
    pinned: bool,
    sort: crate::settings::BranchSort,
    this: WeakEntity<SpurShell>,
) -> PopupMenu {
    let mut menu = menu.item({
        let entity = this.clone();
        let branch = name.clone();
        context_menu_item(
            t().context_checkout.into(),
            IconName::GitBranch,
            move |_, _, cx| {
                let branch = branch.clone();
                entity
                    .update(cx, |shell, cx| shell.checkout_branch(branch, cx))
                    .ok();
            },
        )
    });
    if current {
        let entity = this.clone();
        menu = menu.item(context_menu_item(
            t().context_rebase_onto.into(),
            IconName::GitCompare,
            move |_, _, cx| {
                entity
                    .update(cx, |shell, cx| shell.request_rebase(cx))
                    .ok();
            },
        ));
    }
    menu.item({
        let entity = this.clone();
        let branch = name.clone();
        context_menu_item(
            t().context_rename_branch.into(),
            IconName::Circle,
            move |_, window, cx| {
                let branch = branch.clone();
                entity
                    .update(cx, |shell, cx| {
                        shell.request_rename_branch(branch, window, cx)
                    })
                    .ok();
            },
        )
    })
    .item({
        let entity = this.clone();
        let branch = name.clone();
        context_menu_item(
            t().context_delete_branch.into(),
            IconName::Trash,
            move |_, _, cx| {
                let branch = branch.clone();
                entity
                    .update(cx, |shell, cx| shell.request_branch_delete(branch, cx))
                    .ok();
            },
        )
    })
    .item({
        let entity = this.clone();
        let branch = name.clone();
        let label = if pinned {
            t().context_unpin_branch.into()
        } else {
            t().context_pin_branch.into()
        };
        let icon = if pinned {
            IconName::Close
        } else {
            IconName::Check
        };
        context_menu_item(label, icon, move |_, _, cx| {
            let branch = branch.clone();
            entity
                .update(cx, |shell, cx| shell.toggle_pin_branch(branch, cx))
                .ok();
        })
    })
    .item({
        let entity = this.clone();
        let (label, icon) = match sort {
            crate::settings::BranchSort::Name => {
                (t().sort_by_recent(), IconName::ArrowUpDown)
            }
            crate::settings::BranchSort::Recent => {
                (t().sort_by_name(), IconName::ArrowUpDown)
            }
        };
        context_menu_item(label, icon, move |_, _, cx| {
            entity
                .update(cx, |shell, cx| shell.cycle_branch_sort(cx))
                .ok();
        })
    })
}

impl SpurShell {
    /// Checked-out branch name of the active repository, if any.
    pub(super) fn current_branch_name(&self) -> Option<String> {
        self.active_snapshot()
            .and_then(|collected| collected.snapshot.branch.clone())
    }

    /// Whether `name` in the active repository is pinned.
    pub(super) fn is_branch_pinned(&self, name: &str) -> bool {
        let Some(repo) = self.active_repo() else {
            return false;
        };
        let repo = repo.path.to_string_lossy().into_owned();
        self.pinned_branches
            .contains(&crate::settings::join_pin(&repo, name))
    }

    /// Toggle the pin for one branch and persist the list.
    pub(super) fn toggle_pin_branch(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo().map(|repo| repo.path.to_string_lossy().into_owned()) else {
            return;
        };
        let key = crate::settings::join_pin(&repo, &name);
        if self.pinned_branches.contains(&key) {
            self.pinned_branches.retain(|pinned| pinned != &key);
            self.ops.push_info(t().log_branch_unpinned(&name));
        } else {
            self.pinned_branches.push(key);
            self.ops.push_info(t().log_branch_pinned(&name));
        }
        if let Err(err) = crate::settings::set_pinned_branches(&self.pinned_branches) {
            self.note_error(err, cx);
            return;
        }
        self.rebuild_branch_rows();
        cx.notify();
    }

    /// After a rename (`Some(new)`) or delete (`None`): move or drop the pin
    /// so it never lingers on a gone name (and re-pins a recreated one).
    pub(super) fn retarget_pin(
        &mut self,
        repo: &str,
        old: &str,
        new: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let key = crate::settings::join_pin(repo, old);
        let Some(ix) = self.pinned_branches.iter().position(|pinned| pinned == &key) else {
            return;
        };
        match new {
            Some(new) => self.pinned_branches[ix] = crate::settings::join_pin(repo, new),
            None => {
                self.pinned_branches.remove(ix);
            }
        }
        if let Err(err) = crate::settings::set_pinned_branches(&self.pinned_branches) {
            self.note_error(err, cx);
        }
        self.rebuild_branch_rows();
    }

    /// Cycle the Local Branches sort order and persist it.
    pub(super) fn cycle_branch_sort(&mut self, cx: &mut Context<Self>) {
        self.branch_sort = match self.branch_sort {
            crate::settings::BranchSort::Name => crate::settings::BranchSort::Recent,
            crate::settings::BranchSort::Recent => crate::settings::BranchSort::Name,
        };
        if let Err(err) = crate::settings::set_branch_sort(self.branch_sort) {
            self.note_error(err, cx);
            return;
        }
        self.rebuild_branch_rows();
        cx.notify();
    }

    /// Open the rename dialog prefilled with the row's branch.
    pub(super) fn request_rename_branch(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let repo_id = repo.path.to_string_lossy().into_owned();
        self.branch_rename_input.update(cx, |state, cx| {
            state.set_value(&name, window, cx)
        });
        self.rename_request = Some(RenameRequest {
            repo_id,
            old: name,
        });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_rename_branch(&mut self, cx: &mut Context<Self>) {
        if self.rename_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: queue the rename on the shared sequential change queue.
    pub(super) fn confirm_rename_branch(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.rename_request.clone() else {
            return;
        };
        let new = self.branch_rename_input.read(cx).value().trim().to_string();
        if new.is_empty() {
            return;
        }
        self.change_ops.push_back(changes::ChangeOp::RenameBranch(
            changes::RenameBranchOp {
                repo_id: request.repo_id,
                old: request.old,
                new,
            },
        ));
        self.pump_change_ops(cx);
        self.close_modal(cx);
    }

    /// Delete asks first, always: queue the safe delete and let git decide
    /// whether the branch is fully merged. The checked-out branch is refused
    /// up front with a reason instead of a git error.
    pub(super) fn request_branch_delete(&mut self, name: String, cx: &mut Context<Self>) {
        let current = self
            .active_snapshot()
            .and_then(|collected| collected.snapshot.branch.clone());
        if Some(name.as_str()) == current.as_deref() {
            self.note_error(t().log_branch_delete_current(&name), cx);
            return;
        }
        let Some(repo) = self.active_repo() else {
            return;
        };
        let repo_id = repo.path.to_string_lossy().into_owned();
        self.branch_delete_request = Some(BranchDeleteRequest { repo_id, name });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_branch_delete(&mut self, cx: &mut Context<Self>) {
        if self.branch_delete_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// First confirm: queue the safe (`-d`) delete. An unmerged branch comes
    /// back through the pump as a second, explicit force confirm.
    pub(super) fn confirm_branch_delete(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.branch_delete_request.clone() else {
            return;
        };
        self.change_ops.push_back(changes::ChangeOp::DeleteBranch(
            changes::DeleteBranchOp {
                repo_id: request.repo_id,
                name: request.name,
                force: false,
            },
        ));
        self.pump_change_ops(cx);
        self.close_modal(cx);
    }

    pub(super) fn cancel_branch_force_delete(&mut self, cx: &mut Context<Self>) {
        if self.branch_delete_refused.is_some() {
            self.close_modal(cx);
        }
    }

    /// Second confirm: the branch is not fully merged; delete anyway (`-D`).
    pub(super) fn confirm_branch_force_delete(&mut self, cx: &mut Context<Self>) {
        let Some(refused) = self.branch_delete_refused.clone() else {
            return;
        };
        self.change_ops.push_back(changes::ChangeOp::DeleteBranch(
            changes::DeleteBranchOp {
                repo_id: refused.repo_id,
                name: refused.name,
                force: true,
            },
        ));
        self.pump_change_ops(cx);
        self.close_modal(cx);
    }

    /// Branches the checked-out `branch` can be rebased onto: the other local
    /// branches, then the remote-tracking ones.
    fn rebase_targets(&self, branch: &str) -> Vec<String> {
        self.branches
            .iter()
            .map(|info| info.name.clone())
            .filter(|name| name != branch)
            .chain(
                self.remote_branches
                    .iter()
                    .filter(|remote| remote.name != "HEAD")
                    .map(|remote| format!("{}/{}", remote.remote, remote.name)),
            )
            .collect()
    }

    /// Open the rebase dialog for the checked-out branch, preselecting the
    /// repository's default branch and stashing only when the tree is dirty.
    pub(super) fn request_rebase(&mut self, cx: &mut Context<Self>) {
        let Some(branch) = self.current_branch_name() else {
            self.note_error(t().log_no_current_branch(), cx);
            return;
        };
        let Some(repo) = self.active_repo() else {
            return;
        };
        let repo_id = repo.path.to_string_lossy().into_owned();
        let targets = self.rebase_targets(&branch);
        let default = self
            .active_snapshot()
            .and_then(|collected| collected.refs.default_branch.clone());
        let Some(onto) = default
            .filter(|name| targets.contains(name))
            .or_else(|| targets.first().cloned())
        else {
            self.note_error(t().log_no_rebase_target(), cx);
            return;
        };
        let autostash = self
            .active_snapshot()
            .is_some_and(|collected| !collected.snapshot.is_clean());
        self.rebase_request = Some(RebaseRequest {
            repo_id,
            branch,
            onto,
            autostash,
        });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_rebase(&mut self, cx: &mut Context<Self>) {
        if self.rebase_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: queue the rebase on the shared sequential change queue.
    pub(super) fn confirm_rebase(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.rebase_request.clone() else {
            return;
        };
        self.change_ops
            .push_back(changes::ChangeOp::Rebase(changes::RebaseOp {
                repo_id: request.repo_id,
                branch: request.branch,
                onto: request.onto,
                autostash: request.autostash,
            }));
        self.pump_change_ops(cx);
        self.close_modal(cx);
    }

    /// Palette: rename the checked-out branch.
    pub(super) fn rename_current_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(branch) = self
            .active_snapshot()
            .and_then(|collected| collected.snapshot.branch.clone())
        else {
            self.note_error(t().log_no_current_branch(), cx);
            return;
        };
        self.request_rename_branch(branch, window, cx);
    }

    /// Palette: pin the checked-out branch (reports when already pinned).
    pub(super) fn pin_current_branch(&mut self, cx: &mut Context<Self>) {
        let Some(branch) = self
            .active_snapshot()
            .and_then(|collected| collected.snapshot.branch.clone())
        else {
            self.note_error(t().log_no_current_branch(), cx);
            return;
        };
        if self.is_branch_pinned(&branch) {
            self.note_error(t().log_branch_already_pinned(&branch), cx);
            return;
        }
        self.toggle_pin_branch(branch, cx);
    }

    /// Full-window scrim + card for the rename dialog.
    pub(super) fn render_rename_dialog(
        &self,
        request: RenameRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let can_rename = !self.branch_rename_input.read(cx).value().trim().is_empty();
        confirm_card(
            cx,
            "rename-branch",
            IconName::Circle,
            text_primary(cx),
            format!("{} {}", t().rename_title, request.old),
            None,
            vec![(
                t().rename_name,
                dialog_input(&self.branch_rename_input),
            )],
            t().cancel,
            Self::cancel_rename_branch,
            t().rename_confirm,
            Self::confirm_rename_branch,
            can_rename,
            false,
        )
    }

    /// Full-window scrim + card for the rebase dialog: target dropdown and
    /// the autostash toggle.
    pub(super) fn render_rebase_dialog(
        &self,
        request: RebaseRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let this = cx.entity().downgrade();
        let targets = self.rebase_targets(&request.branch);
        let onto = request.onto.clone();
        let menu_entity = this.clone();
        let onto_menu = DropdownButton::new("rebase-onto")
            .button(
                Button::new("rebase-onto-btn")
                    .label(truncate_label(&request.onto, 32))
                    .ghost()
                    .xsmall(),
            )
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu.scrollable(true).max_h(px(320.));
                for name in &targets {
                    let entity = menu_entity.clone();
                    let picked = name.clone();
                    let label = name.clone();
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
                                .child(truncate_label(&label, 42))
                        })
                        .checked(name == &onto)
                        .on_click(move |_, _, cx| {
                            let picked = picked.clone();
                            entity
                                .update(cx, |this, cx| {
                                    if let Some(request) = this.rebase_request.as_mut() {
                                        request.onto = picked;
                                    }
                                    cx.notify();
                                })
                                .ok();
                        }),
                    );
                }
                menu
            });
        let autostash = Checkbox::new("rebase-autostash")
            .checked(request.autostash)
            .label(t().rebase_autostash)
            .on_change(move |checked, _, cx| {
                let checked = *checked;
                this.update(cx, |this, cx| {
                    if let Some(request) = this.rebase_request.as_mut() {
                        request.autostash = checked;
                    }
                    cx.notify();
                })
                .ok();
            });
        confirm_card(
            cx,
            "rebase",
            IconName::GitCompare,
            violet(cx),
            t().rebase_title(&request.branch),
            Some(t().rebase_body(&request.branch)),
            vec![
                (t().rebase_onto, select_shell(onto_menu).into_any_element()),
                (t().rebase_changes, autostash.into_any_element()),
            ],
            t().cancel,
            Self::cancel_rebase,
            t().rebase_confirm,
            Self::confirm_rebase,
            true,
            false,
        )
    }

    /// Full-window scrim + card for the first delete confirm.
    pub(super) fn render_branch_delete_confirm(
        &self,
        request: BranchDeleteRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        confirm_card(
            cx,
            "branch-delete",
            IconName::Trash,
            cx.theme().danger,
            t().branch_delete_title.to_string(),
            Some(t().branch_delete_body(&request.name)),
            vec![],
            t().cancel,
            Self::cancel_branch_delete,
            t().context_delete_branch,
            Self::confirm_branch_delete,
            true,
            true,
        )
    }

    /// Full-window scrim + card for the force delete after git refused `-d`.
    pub(super) fn render_branch_force_confirm(
        &self,
        refused: RefusedDelete,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        confirm_card(
            cx,
            "branch-force-delete",
            IconName::Trash,
            cx.theme().danger,
            t().branch_force_title.to_string(),
            Some(t().branch_force_body(&refused.name)),
            vec![],
            t().cancel,
            Self::cancel_branch_force_delete,
            t().branch_delete_anyway,
            Self::confirm_branch_force_delete,
            true,
            true,
        )
    }
}

/// One labeled input row inside a confirm card.
fn dialog_input(input: &Entity<InputState>) -> gpui_kit::AnyElement {
    use gpui_kit::component::input::Input;
    div()
        .rounded(px(CONTROL_RADIUS))
        .border_1()
        .border_color(hairline(0.10))
        .bg(ink(0.03))
        .px(px(6.))
        .py(px(2.))
        .child(
            Input::new(input)
                .appearance(false)
                .focus_bordered(false)
                .w_full(),
        )
        .into_any_element()
}

/// Shared scrim + card for the small branch confirms: title row, optional
/// body, labeled rows, and Cancel + action buttons.
#[allow(clippy::too_many_arguments)]
fn confirm_card(
    cx: &mut Context<SpurShell>,
    id_prefix: &'static str,
    icon: IconName,
    icon_color: gpui_kit::Hsla,
    title: String,
    body: Option<String>,
    rows: Vec<(&'static str, gpui_kit::AnyElement)>,
    cancel_label: &'static str,
    cancel_fn: fn(&mut SpurShell, &mut Context<SpurShell>),
    confirm_label: &'static str,
    confirm_fn: fn(&mut SpurShell, &mut Context<SpurShell>),
    confirm_enabled: bool,
    confirm_danger: bool,
) -> gpui_kit::AnyElement {
    let cancel_id: SharedString = format!("{id_prefix}-cancel").into();
    let confirm_id: SharedString = format!("{id_prefix}-confirm").into();
    let mut card = div()
        .w(px(440.))
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
                .child(Icon::new(icon).size(px(16.)).text_color(icon_color))
                .child(
                    div()
                        .text_size(px(TEXT_LG))
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_color(text_primary(cx))
                        .child(title),
                ),
        );
    if let Some(body) = body {
        card = card.child(
            div()
                .font_family(MONO)
                .text_size(px(TEXT_MD))
                .text_color(text_muted(cx))
                .child(body),
        );
    }
    for (label, row) in rows {
        card = card
            .child(
                div()
                    .text_size(px(TEXT_XS))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(text_faint(cx))
                    .child(label),
            )
            .child(row);
    }
    let mut confirm_button = Button::new(confirm_id)
        .label(confirm_label)
        .small()
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| confirm_fn(this, cx)));
    confirm_button = if confirm_danger {
        confirm_button.danger()
    } else {
        confirm_button.primary()
    };
    if !confirm_enabled {
        confirm_button = confirm_button.disabled(true);
    }
    let card = card.child(
        div()
            .flex()
            .justify_end()
            .gap(px(8.))
            .child(
                Button::new(cancel_id)
                    .label(cancel_label)
                    .small()
                    .text()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| cancel_fn(this, cx))),
            )
            .child(confirm_button),
    );
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
            cx.listener(move |this, _, _, cx| cancel_fn(this, cx)),
        )
        .child(card)
        .into_any_element()
}
