//! Commit context menu: per-commit actions for history rows.
//!
//! The safe subset ships first: detached checkout, branch/tag creation,
//! cherry-pick, revert, and clipboard copies.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{DismissEvent, Focusable as _, Pixels, Point, Subscription, WeakEntity};

use crate::i18n::t;
use crate::model::RefKind;

/// What the menu acts on: one loaded commit plus the checked-out branch name
/// (for the cherry-pick label) and the local branches pointing at it (for
/// the rename items).
#[derive(Clone)]
pub(super) struct CommitTarget {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub current_branch: Option<String>,
    pub local_branches: Vec<String>,
}

/// Local branch names decorating one commit.
fn local_branches(commit: &crate::model::HistoryCommit) -> Vec<String> {
    commit
        .refs
        .iter()
        .filter(|(_, kind)| *kind == RefKind::Branch)
        .map(|(name, _)| name.clone())
        .collect()
}

/// The open history-row menu. The shell draws it at window level so it can
/// open at the pointer's x just below the clicked row, which the generic
/// context menu (always at the pointer) cannot do.
pub(super) struct OpenCommitMenu {
    /// The right-clicked commit, raised in the history list while open.
    pub hash: String,
    menu: Entity<PopupMenu>,
    position: Point<Pixels>,
    _dismiss: Subscription,
}

impl CommitTarget {
    pub(super) fn of(
        commit: &crate::model::HistoryCommit,
        current_branch: Option<SharedString>,
    ) -> Self {
        let short = commit.hash.get(..7).unwrap_or(&commit.hash).to_string();
        CommitTarget {
            hash: commit.hash.clone(),
            short,
            subject: commit.subject.clone(),
            current_branch: current_branch.map(|branch| branch.to_string()),
            local_branches: local_branches(commit),
        }
    }
}

/// The full commit menu for a row right-click. Every item clones what it
/// needs; handlers resolve the repository through the shell like the stash
/// menu does.
pub(super) fn commit_menu(
    menu: PopupMenu,
    target: CommitTarget,
    this: WeakEntity<SpurShell>,
) -> PopupMenu {
    let pick = target.current_branch.clone().unwrap_or_else(|| "HEAD".to_string());
    let mut menu = menu.item(commit_item(
        t().context_checkout_detached.into(),
        IconName::GitCommitHorizontal,
        ItemExplainer {
            kind: explainer::ExplainerKind::CheckoutDetached,
            ctx_a: String::new(),
        },
        this.clone(),
        target.hash.clone(),
        target.short.clone(),
        |shell, hash, short, _, cx| shell.checkout_commit_detached(hash, short, cx),
    ))
    .item(commit_item(
        t().context_create_branch_here.into(),
        IconName::GitBranchPlus,
        ItemExplainer {
            kind: explainer::ExplainerKind::CreateBranch,
            ctx_a: String::new(),
        },
        this.clone(),
        target.hash.clone(),
        target.short.clone(),
        |shell, hash, _, window, cx| shell.request_create_branch_at(hash, window, cx),
    ))
    .item(commit_item(
        t().context_create_tag_here.into(),
        IconName::Tag,
        ItemExplainer {
            kind: explainer::ExplainerKind::CreateTag,
            ctx_a: String::new(),
        },
        this.clone(),
        target.hash.clone(),
        target.short.clone(),
        |shell, hash, _, window, cx| shell.request_create_tag(hash, window, cx),
    ));
    for branch in &target.local_branches {
        let entity = this.clone();
        let branch = branch.clone();
        menu = menu.item(context_menu_item(
            t().context_rename_branch_named(&branch),
            IconName::Pencil,
            move |_, window, cx| {
                let branch = branch.clone();
                entity
                    .update(cx, |shell, cx| shell.request_rename_branch(branch, window, cx))
                    .ok();
            },
        ));
    }
    menu.item(commit_item(
        t().cherry_pick_onto(&pick),
        IconName::Plus,
        ItemExplainer {
            kind: explainer::ExplainerKind::CherryPick,
            ctx_a: pick.clone(),
        },
        this.clone(),
        target.hash.clone(),
        target.short.clone(),
        |shell, hash, short, _, cx| shell.cherry_pick_commit(hash, short, cx),
    ))
    .item(commit_item(
        t().context_revert_commit.into(),
        IconName::RotateCcw,
        ItemExplainer {
            kind: explainer::ExplainerKind::Revert,
            ctx_a: String::new(),
        },
        this.clone(),
        target.hash.clone(),
        target.short.clone(),
        |shell, hash, short, _, cx| shell.revert_commit(hash, short, cx),
    ))
    .item({
        let entity = this.clone();
        let target = target.clone();
        let label = t().reset_here_action(target.current_branch.as_deref().unwrap_or("HEAD"));
        explainer::explained_menu_item(
            label.clone(),
            IconName::Rewind,
            explainer::ExplainerKind::Reset,
            label,
            target
                .current_branch
                .clone()
                .unwrap_or_else(|| "HEAD".to_string()),
            entity.clone(),
            move |_, _, cx| {
                entity
                    .update(cx, |shell, cx| {
                        shell.clear_explainer(cx);
                        shell.request_reset_to(
                            target.hash.clone(),
                            target.short.clone(),
                            target.subject.clone(),
                            cx,
                        )
                    })
                    .ok();
            },
        )
    })
    .item(copy_item(
        t().context_copy_sha.into(),
        explainer::ExplainerKind::CopySha,
        this.clone(),
        target.hash.clone(),
        "SHA",
    ))
    .item(copy_item(
        t().context_copy_short_sha.into(),
        explainer::ExplainerKind::CopyShortSha,
        this.clone(),
        target.short.clone(),
        "short SHA",
    ))
    .item(copy_item(
        t().context_copy_subject.into(),
        explainer::ExplainerKind::CopySubject,
        this.clone(),
        target.subject.clone(),
        "subject",
    ))
    .item({
        let entity = this.clone();
        let hash = target.hash.clone();
        let label: String = t().context_copy_patch.into();
        explainer::explained_menu_item(
            label.clone(),
            IconName::ClipboardCopy,
            explainer::ExplainerKind::CopyPatch,
            label,
            String::new(),
            entity.clone(),
            move |_, _, cx| {
                entity
                    .update(cx, |shell, cx| {
                        shell.clear_explainer(cx);
                        shell.copy_commit_patch(hash.clone(), cx)
                    })
                    .ok();
            },
        )
    })
}

/// Hover explainer for one menu item: which card plus its context.
#[derive(Clone)]
struct ItemExplainer {
    kind: explainer::ExplainerKind,
    ctx_a: String,
}

/// One menu item running a shell operation against the active repository,
/// with a hover explainer.
fn commit_item(
    label: String,
    icon: IconName,
    explainer: ItemExplainer,
    this: WeakEntity<SpurShell>,
    hash: String,
    short: String,
    run: impl Fn(&mut SpurShell, String, String, &mut Window, &mut Context<SpurShell>) + 'static,
) -> PopupMenuItem {
    let title = label.clone();
    explainer::explained_menu_item(
        label,
        icon,
        explainer.kind,
        title,
        explainer.ctx_a,
        this.clone(),
        move |_, window, cx| {
            let hash = hash.clone();
            let short = short.clone();
            this.update(cx, |shell, cx| {
                shell.clear_explainer(cx);
                run(shell, hash, short, &mut *window, cx)
            })
            .ok();
        },
    )
}

/// One clipboard copy: silent except for the operation-log line.
fn copy_item(
    label: String,
    kind: explainer::ExplainerKind,
    this: WeakEntity<SpurShell>,
    text: String,
    what: &'static str,
) -> PopupMenuItem {
    let title = label.clone();
    explainer::explained_menu_item(
        label,
        IconName::ClipboardCopy,
        kind,
        title,
        String::new(),
        this.clone(),
        move |_, _, cx| {
            let text = text.clone();
            this.update(cx, |shell, cx| {
                shell.clear_explainer(cx);
                shell.copy_commit_text(what, text, cx)
            })
            .ok();
        },
    )
}

impl SpurShell {
    /// Open the commit menu for a history row at `position` (window space).
    /// Focus returns to wherever it was once the menu is dismissed.
    pub(super) fn open_commit_menu(
        &mut self,
        target: CommitTarget,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        let hash = target.hash.clone();
        let previous_focus = window.focused(cx);
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            let menu = match previous_focus {
                Some(handle) => menu.action_context(handle),
                None => menu,
            };
            commit_menu(menu, target, this)
        });
        let dismiss = cx.subscribe_in(&menu, window, |this, menu, _: &DismissEvent, _, cx| {
            // A newer menu may already have replaced this one.
            if this
                .commit_menu_open
                .as_ref()
                .is_some_and(|open| open.menu.entity_id() == menu.entity_id())
            {
                this.commit_menu_open = None;
                cx.notify();
            }
        });
        menu.focus_handle(cx).focus(window, cx);
        self.commit_menu_open = Some(OpenCommitMenu {
            hash,
            menu,
            position,
            _dismiss: dismiss,
        });
        cx.notify();
    }

    /// Close the commit menu even when focus has left it; it hands focus back
    /// only if it still held it.
    pub(super) fn close_commit_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.commit_menu_open.take() {
            let cancel = gpui_kit::base::actions::Cancel;
            open.menu.focus_handle(cx).dispatch_action(&cancel, window, cx);
            cx.notify();
        }
    }

    /// The open commit menu, deferred above the window like the generic
    /// context menu; the full-window layer keeps the history list from
    /// scrolling out from under it.
    pub(super) fn render_commit_menu(&self, window: &Window) -> Option<gpui_kit::AnyElement> {
        let open = self.commit_menu_open.as_ref()?;
        let size = window.bounds().size;
        Some(
            gpui_kit::deferred(
                gpui_kit::anchored().child(
                    div()
                        .w(size.width)
                        .h(size.height)
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .child(
                            gpui_kit::anchored()
                                .position(open.position)
                                .snap_to_window_with_margin(px(8.))
                                .child(open.menu.clone()),
                        ),
                ),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }

    /// Queue `git checkout --detach` for one commit.
    pub(super) fn checkout_commit_detached(
        &mut self,
        hash: String,
        short: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(repo_id) = self.active_repo_id() {
            self.change_ops.push_back(changes::ChangeOp::CheckoutDetached(
                changes::CheckoutDetachedOp {
                    repo_id,
                    hash,
                    short,
                },
            ));
            self.pump_change_ops(cx);
        }
    }

    /// Queue `git cherry-pick` onto the checked-out branch.
    pub(super) fn cherry_pick_commit(
        &mut self,
        hash: String,
        short: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(repo_id) = self.active_repo_id() {
            self.change_ops.push_back(changes::ChangeOp::CherryPick(
                changes::CherryPickOp {
                    repo_id,
                    hash,
                    short,
                },
            ));
            self.pump_change_ops(cx);
        }
    }

    /// Queue `git revert` of one commit.
    pub(super) fn revert_commit(&mut self, hash: String, short: String, cx: &mut Context<Self>) {
        if let Some(repo_id) = self.active_repo_id() {
            self.change_ops.push_back(changes::ChangeOp::Revert(changes::RevertOp {
                repo_id,
                hash,
                short,
            }));
            self.pump_change_ops(cx);
        }
    }

    /// Copy a commit field to the clipboard (SHA, short SHA, subject).
    pub(super) fn copy_commit_text(
        &mut self,
        what: &'static str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        self.ops.push_info(t().log_copied_commit(what));
        cx.notify();
    }

    /// Copy an apply-able patch of one commit: the patch is read on the
    /// background thread, then lands on the clipboard like a plain copy.
    pub(super) fn copy_commit_patch(&mut self, hash: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let worktree = repo.path.to_string_lossy().into_owned();
        cx.spawn(async move |this, cx| {
            let patch = cx
                .background_executor()
                .spawn(async move { crate::git::commit_patch(&worktree, &hash) })
                .await;
            this.update(cx, |this, cx| match patch {
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

    /// The selected commit as a menu target (palette entries).
    pub(super) fn selected_commit_target(&self) -> Option<CommitTarget> {
        let hash = self.selected_commit.clone()?;
        let short = hash.get(..7).unwrap_or(&hash).to_string();
        let commit = self.history.iter().find(|commit| commit.hash == hash);
        let subject = commit
            .map(|commit| commit.subject.clone())
            .unwrap_or_default();
        let local_branches = commit.map(local_branches).unwrap_or_default();
        let current_branch = self
            .active_snapshot()
            .and_then(|collected| collected.snapshot.branch.clone());
        Some(CommitTarget {
            hash,
            short,
            subject,
            current_branch,
            local_branches,
        })
    }
}
