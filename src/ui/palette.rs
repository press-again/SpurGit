//! Ctrl+K repository palette: actions + searchable repository list.
//! Structure mirrors Zeron's command palette (crates/ui/src/shell/
//! command_palette.rs @ 1fdcfe19): 560px card, radius 16, 44px search header,
//! a real input with the caret and selection owned by the component library,
//! sectioned rows, accent match highlighting, kbd-hint footer, 0.35 scrim,
//! pointer-motion highlight, and Enter-repeat latching.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::{
    App, Focusable as _, InteractiveElement as _, KeyDownEvent, KeyUpEvent, SharedString,
    StatefulInteractiveElement as _,
};

use crate::i18n::t;
use crate::model::{Flag, RowState};

use super::app_icon::{self, IconSlot};

/// Repositories rendered in the palette at most; typing narrows the rest.
/// Rendering hundreds of rows every frame is what made the palette lag.
const PALETTE_REPO_LIMIT: usize = 60;

gpui_kit::actions!(
    spur,
    [TogglePalette, ClosePalette, PaletteNext, PalettePrev, SubmitCommit]
);

/// One physical Enter press activates once; synthetic repeat releases are
/// unreliable (Zeron's EnterPress), so latch until the key is released.
#[derive(Default)]
pub(super) struct EnterPress {
    down: bool,
}

impl EnterPress {
    fn press(&mut self, is_held: bool) -> bool {
        let was_down = std::mem::replace(&mut self.down, true);
        !was_down && !is_held
    }

    fn release(&mut self) {
        self.down = false;
    }
}

#[derive(Clone, Copy)]
pub(super) enum PaletteItem {
    /// "Manage roots…" action row.
    ManageRoots,
    /// "Open in File Explorer" action row for the active repository.
    OpenInExplorer,
    /// Manual workspace status refresh.
    RefreshNow,
    /// Full rescan of the scan roots (discovery + status).
    RescanWorkspace,
    /// Toggle and persist the bounded auto-refresh preference.
    ToggleAutoRefresh,
    /// Cycle the workspace status filter.
    CycleFilter,
    /// Cycle the repository sort key.
    CycleSort,
    /// Copy the active/first repository path.
    CopyPath,
    /// Preview the clipboard patch and apply it with `git apply`.
    ApplyPatch,
    /// Open the operation log panel.
    OperationLog,
    /// Undo the newest entry of the active repository.
    UndoLast,
    /// Replay the banked redo step (ref moves only).
    RedoLast,
    /// Recently-discarded backups of the active repository.
    Discarded,
    /// Switch the repository section (Ctrl+1, 2, 3).
    GoHistory,
    GoChanges,
    GoStashes,
    /// Check out the selected history commit detached.
    CheckoutDetachedSelected,
    /// Rename the checked-out branch.
    RenameCurrentBranch,
    /// Pin the checked-out branch above the branch tree.
    PinCurrentBranch,
    /// Open the Create Branch dialog based at the selected commit.
    BranchHereSelected,
    /// Open the Create Tag dialog at the selected commit.
    TagHereSelected,
    /// Cherry-pick the selected commit onto the checked-out branch.
    CherryPickSelected,
    /// Revert the selected commit.
    RevertSelected,
    /// Reset the checked-out branch to the selected commit.
    ResetSelected,
    /// Copy the selected commit's SHA / short SHA / subject / patch.
    CopyShaSelected,
    CopyShortShaSelected,
    CopySubjectSelected,
    CopyPatchSelected,
    /// Pop the selected stash entry.
    PopSelectedStash,
    /// Open the branch-from-stash dialog for the selected stash.
    StashBranchSelected,
    /// Hide the selected (or active) repositories from the workspace.
    ExcludeSelected,
    /// Drop every workspace exclusion.
    ClearExcludes,
    /// Invoke the configured WSL-aware external client.
    OpenExternally,
    /// "Settings…" action row opening the settings page.
    Settings,
    Repo(usize), // index into SpurShell::repos
}

impl SpurShell {
    pub(super) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.roots_open {
            // Ctrl+K inside the roots dialog steps back to the palette, the
            // same destination as Escape.
            self.close_roots_to_palette(cx);
            return;
        }
        if self.palette_open {
            if self.palette_closing {
                // Reopen mid-exit: fade upward from the displayed value.
                self.palette_closing = false;
                self.dialog_fade
                    .retarget(1.0, motion::MENU_IN, cx.reduce_motion(), Instant::now());
                self.palette_focus_pending = true;
                cx.notify();
            } else {
                self.close_palette(window, cx);
            }
            return;
        }
        // Close any dialog first so Escape closes only the palette, not
        // something it left open beneath. The dialog's focus goes with it.
        if self.modal_fade.target() > 0.0 {
            self.close_modal(cx);
            window.focus(&self.root_focus, cx);
        }
        self.close_oplog(cx);
        self.refresh_for_palette(cx);
        self.remember_dialog_focus(window, cx);
        self.palette_open = true;
        self.palette_closing = false;
        self.set_dialog_width(PALETTE_W, cx);
        self.palette_active = 0;
        self.palette_enter.release();
        // `set_value` emits no change event, so the repository search the
        // input normally drives has to be cleared alongside it.
        self.palette_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.overview.set_search("");
        self.palette_scroll.set_offset(point(px(0.), px(0.)));
        self.dialog_fade
            .retarget(1.0, motion::MENU_IN, cx.reduce_motion(), Instant::now());
        self.palette_focus_pending = true;
        crate::logging::log!("palette: open");
        cx.notify();
    }

    /// Animated close: the overlay keeps rendering until the shared fade
    /// reaches zero, then the render pass drops it. Every dismissal path
    /// (Escape, scrim click, activating a result) routes through here.
    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.palette_open || self.palette_closing {
            return;
        }
        self.palette_closing = true;
        // The exit is a fade only; the card keeps whatever width the last
        // morph reached and cannot replay it.
        self.dialog_fade
            .retarget(0.0, motion::MENU_OUT, cx.reduce_motion(), Instant::now());
        self.restore_dialog_focus(window, cx);
        crate::logging::log!("palette: close");
        cx.notify();
    }

    fn palette_query(&self, cx: &App) -> String {
        self.palette_input.read(cx).value().to_string()
    }

    fn palette_items(&self, cx: &App) -> Vec<PaletteItem> {
        let q = self.palette_query(cx).trim().to_lowercase();
        let mut items = Vec::new();
        if q.is_empty() || matches_query(&q, t().manage_roots) {
            items.push(PaletteItem::ManageRoots);
        }
        if q.is_empty() || matches_query(&q, &crate::i18n::file_manager(t().open_in_file_explorer)) {
            items.push(PaletteItem::OpenInExplorer);
        }
        if q.is_empty() || matches_query(&q, t().refresh_workspace()) {
            items.push(PaletteItem::RefreshNow);
        }
        if q.is_empty() || matches_query(&q, t().rescan_workspace()) {
            items.push(PaletteItem::RescanWorkspace);
        }
        if q.is_empty() || matches_query(&q, &t().auto_refresh_action(self.auto_refresh)) {
            items.push(PaletteItem::ToggleAutoRefresh);
        }
        if q.is_empty() || matches_query(&q, &t().filter_action(self.overview.filter().label())) {
            items.push(PaletteItem::CycleFilter);
        }
        if q.is_empty() || matches_query(&q, &t().sort_action(self.overview.sort().label())) {
            items.push(PaletteItem::CycleSort);
        }
        if q.is_empty() || matches_query(&q, t().copy_path_action()) {
            items.push(PaletteItem::CopyPath);
        }
        if q.is_empty() || matches_query(&q, t().oplog_action) {
            items.push(PaletteItem::OperationLog);
        }
        // Undo/redo offer the active repository's stack top.
        if let Some(repo_id) = self.active_repo_id() {
            if self
                .undo_top_label(&repo_id)
                .is_some_and(|label| q.is_empty() || matches_query(&q, &t().undo_palette(&label)))
            {
                items.push(PaletteItem::UndoLast);
            }
            if self
                .redo_label(&repo_id)
                .is_some_and(|label| q.is_empty() || matches_query(&q, &t().redo_palette(&label)))
            {
                items.push(PaletteItem::RedoLast);
            }
            // Recently-discarded: only while backups exist.
            if !self.discarded_for(&repo_id).is_empty()
                && (q.is_empty() || matches_query(&q, t().discarded_action()))
            {
                items.push(PaletteItem::Discarded);
            }
        }
        if self.active_repo().is_some()
            && (q.is_empty() || matches_query(&q, t().apply_patch_action))
        {
            items.push(PaletteItem::ApplyPatch);
        }
        // Section navigation: mirrors the sidebar plus Ctrl+1/2/3.
        if self.active_repo().is_some() {
            if q.is_empty() || matches_query(&q, t().shortcut_history) {
                items.push(PaletteItem::GoHistory);
            }
            if q.is_empty() || matches_query(&q, t().shortcut_changes) {
                items.push(PaletteItem::GoChanges);
            }
            if q.is_empty() || matches_query(&q, t().shortcut_stashes) {
                items.push(PaletteItem::GoStashes);
            }
        }
        if self.selected_stash.is_some() {
            if q.is_empty() || matches_query(&q, t().pop_stash_action()) {
                items.push(PaletteItem::PopSelectedStash);
            }
            if q.is_empty() || matches_query(&q, t().stash_branch_action()) {
                items.push(PaletteItem::StashBranchSelected);
            }
        }
        if self.current_branch_name().is_some() {
            if q.is_empty() || matches_query(&q, t().rename_branch_action()) {
                items.push(PaletteItem::RenameCurrentBranch);
            }
            // Unpinning stays in the row menu; the palette only pins.
            let pin_offered = self
                .current_branch_name()
                .is_some_and(|branch| !self.is_branch_pinned(&branch));
            if pin_offered && (q.is_empty() || matches_query(&q, t().pin_branch_action())) {
                items.push(PaletteItem::PinCurrentBranch);
            }
        }
        // Commit actions operate on the selected history commit.
        if self.selected_commit.is_some() {
            if q.is_empty() || matches_query(&q, t().checkout_detached_action()) {
                items.push(PaletteItem::CheckoutDetachedSelected);
            }
            if q.is_empty() || matches_query(&q, t().create_branch_here_action()) {
                items.push(PaletteItem::BranchHereSelected);
            }
            if q.is_empty() || matches_query(&q, t().create_tag_here_action()) {
                items.push(PaletteItem::TagHereSelected);
            }
            if q.is_empty() || matches_query(&q, t().cherry_pick_action()) {
                items.push(PaletteItem::CherryPickSelected);
            }
            if q.is_empty() || matches_query(&q, t().revert_action()) {
                items.push(PaletteItem::RevertSelected);
            }
            if q.is_empty() || matches_query(&q, t().reset_selected_action()) {
                items.push(PaletteItem::ResetSelected);
            }
            if q.is_empty() || matches_query(&q, t().context_copy_sha) {
                items.push(PaletteItem::CopyShaSelected);
            }
            if q.is_empty() || matches_query(&q, t().context_copy_short_sha) {
                items.push(PaletteItem::CopyShortShaSelected);
            }
            if q.is_empty() || matches_query(&q, t().context_copy_subject) {
                items.push(PaletteItem::CopySubjectSelected);
            }
            if q.is_empty() || matches_query(&q, t().context_copy_patch) {
                items.push(PaletteItem::CopyPatchSelected);
            }
        }
        if q.is_empty() || matches_query(&q, t().exclude_action()) {
            items.push(PaletteItem::ExcludeSelected);
        }
        if q.is_empty() || matches_query(&q, t().clear_excludes_action()) {
            items.push(PaletteItem::ClearExcludes);
        }
        // Offered only when an external client is configured; without one the
        // action could only fail with "no external client configured".
        if self.external_client.is_some()
            && (q.is_empty() || matches_query(&q, t().open_externally_action()))
        {
            items.push(PaletteItem::OpenExternally);
        }
        if q.is_empty() || matches_query(&q, t().settings_palette) {
            items.push(PaletteItem::Settings);
        }
        // The controller already applies filter + search + sort; the palette
        // only maps visible identities back to row indices.
        let index_of: std::collections::HashMap<String, usize> = self
            .repos
            .iter()
            .enumerate()
            .map(|(ix, repo)| (repo.path.to_string_lossy().into_owned(), ix))
            .collect();
        let mut repos_shown = 0usize;
        for id in self.overview.visible_ids() {
            if repos_shown >= PALETTE_REPO_LIMIT {
                break;
            }
            if let Some(&ix) = index_of.get(&id) {
                items.push(PaletteItem::Repo(ix));
                repos_shown += 1;
            }
        }
        items
    }

    /// Repositories hidden by the render cap (shown as a hint row).
    fn hidden_repo_count(&self) -> usize {
        self.overview.visible_rows().len().saturating_sub(PALETTE_REPO_LIMIT)
    }

    /// Short hash of the selected history commit, shown as row metadata on
    /// the commit actions.
    fn selected_commit_meta(&self) -> Option<SharedString> {
        self.selected_commit
            .as_deref()
            .map(|hash| hash.get(..7).unwrap_or(hash).into())
    }

    /// The first visible repository when no tab is open (action fallback).
    fn action_repo_path(&self) -> Option<String> {
        self.active_repo()
            .map(|repo| repo.path.to_string_lossy().into_owned())
            .or_else(|| {
                self.overview
                    .visible_rows()
                    .first()
                    .map(|row| row.path.to_string_lossy().into_owned())
            })
    }

    fn activate_palette_item(
        &mut self,
        item: PaletteItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match item {
            PaletteItem::ManageRoots => {
                self.open_roots(window, cx);
                return;
            }
            PaletteItem::OpenInExplorer => {
                // Fall back to the first scan root when no repository is open,
                // so the action always reveals something.
                let target = self
                    .active_repo()
                    .map(|r| r.path.to_string_lossy().into_owned())
                    .or_else(|| self.roots.first().cloned());
                match target {
                    Some(path) => self.open_in_explorer(&path, cx),
                    None => self.note_error(t().log_nothing_to_reveal(), cx),
                }
            }
            PaletteItem::Settings => {
                self.show_settings(window, cx);
                self.close_palette(window, cx);
                return;
            }
            PaletteItem::RefreshNow => {
                self.refresh_workspace(cx);
            }
            PaletteItem::RescanWorkspace => {
                self.rescan_workspace(cx);
            }
            PaletteItem::ToggleAutoRefresh => {
                self.toggle_auto_refresh(cx);
            }
            PaletteItem::CycleFilter => {
                // Keep the palette open so the label can be cycled repeatedly.
                self.cycle_filter(cx);
                return;
            }
            PaletteItem::CycleSort => {
                self.cycle_sort(cx);
                return;
            }
            PaletteItem::CopyPath => match self.action_repo_path() {
                Some(path) => self.copy_path(&path, cx),
                None => self.note_error(t().log_nothing_to_reveal(), cx),
            },
            PaletteItem::OperationLog => {
                self.toggle_oplog(cx);
            }
            PaletteItem::UndoLast => {
                if let Some(repo_id) = self.active_repo_id() {
                    self.queue_undo_for(repo_id, cx);
                }
            }
            PaletteItem::RedoLast => {
                if let Some(repo_id) = self.active_repo_id() {
                    self.queue_redo_for(repo_id, cx);
                }
            }
            PaletteItem::Discarded => {
                self.open_discarded(cx);
            }
            PaletteItem::ApplyPatch => {
                self.request_apply_patch(cx);
            }
            PaletteItem::GoHistory => {
                self.show_section(repo::RepoSection::History, cx);
            }
            PaletteItem::GoChanges => {
                self.show_section(repo::RepoSection::LocalChanges, cx);
            }
            PaletteItem::GoStashes => {
                self.show_section(repo::RepoSection::Stashes, cx);
            }
            PaletteItem::CheckoutDetachedSelected => {
                match self.selected_commit_target() {
                    Some(target) => {
                        self.checkout_commit_detached(target.hash, target.short, cx)
                    }
                    None => self.note_error(t().log_no_commit_selected(), cx),
                }
            }
            PaletteItem::BranchHereSelected => {
                match self.selected_commit_target() {
                    Some(target) => {
                        self.request_create_branch_at(target.hash, window, cx)
                    }
                    None => self.note_error(t().log_no_commit_selected(), cx),
                }
            }
            PaletteItem::TagHereSelected => {
                match self.selected_commit_target() {
                    Some(target) => self.request_create_tag(target.hash, window, cx),
                    None => self.note_error(t().log_no_commit_selected(), cx),
                }
            }
            PaletteItem::CherryPickSelected => {
                match self.selected_commit_target() {
                    Some(target) => self.cherry_pick_commit(target.hash, target.short, cx),
                    None => self.note_error(t().log_no_commit_selected(), cx),
                }
            }
            PaletteItem::RevertSelected => match self.selected_commit_target() {
                Some(target) => self.revert_commit(target.hash, target.short, cx),
                None => self.note_error(t().log_no_commit_selected(), cx),
            },
            PaletteItem::ResetSelected => match self.selected_commit_target() {
                Some(target) => {
                    self.request_reset_to(target.hash, target.short, target.subject, cx)
                }
                None => self.note_error(t().log_no_commit_selected(), cx),
            },
            PaletteItem::CopyShaSelected => match self.selected_commit_target() {
                Some(target) => self.copy_commit_text("SHA", target.hash, cx),
                None => self.note_error(t().log_no_commit_selected(), cx),
            },
            PaletteItem::CopyShortShaSelected => match self.selected_commit_target() {
                Some(target) => self.copy_commit_text("short SHA", target.short, cx),
                None => self.note_error(t().log_no_commit_selected(), cx),
            },
            PaletteItem::CopySubjectSelected => match self.selected_commit_target() {
                Some(target) => self.copy_commit_text("subject", target.subject, cx),
                None => self.note_error(t().log_no_commit_selected(), cx),
            },
            PaletteItem::CopyPatchSelected => match self.selected_commit_target() {
                Some(target) => self.copy_commit_patch(target.hash, cx),
                None => self.note_error(t().log_no_commit_selected(), cx),
            },
            PaletteItem::RenameCurrentBranch => {
                self.rename_current_branch(window, cx)
            }
            PaletteItem::PopSelectedStash => match self.selected_stash.clone() {
                Some(id) => self.pop_stash(id, cx),
                None => self.note_error(t().log_no_stash_selected(), cx),
            },
            PaletteItem::StashBranchSelected => {
                match self.selected_stash.clone() {
                    Some(id) => self.request_stash_branch(id, window, cx),
                    None => self.note_error(t().log_no_stash_selected(), cx),
                }
            }
            PaletteItem::PinCurrentBranch => self.pin_current_branch(cx),
            PaletteItem::ExcludeSelected => {
                let mut targets = self.overview.action_targets(crate::overview::TargetScope::Selected);
                if targets.is_empty()
                    && let Some(path) = self.action_repo_path()
                {
                    targets.push(path);
                }
                self.exclude_paths(targets, cx);
            }
            PaletteItem::ClearExcludes => {
                self.clear_excludes(cx);
            }
            PaletteItem::OpenExternally => match self.action_repo_path() {
                Some(path) => self.open_externally(&path, cx),
                None => self.note_error(t().log_nothing_to_reveal(), cx),
            },
            PaletteItem::Repo(ix) => {
                let key = PathKey::new(&self.repos[ix].path);
                self.open_repo(key, cx);
            }
        }
        self.close_palette(window, cx);
    }

    /// Pointer motion moves the highlight, so hover and keyboard never light
    /// two rows. Rows scrolling under a resting pointer do not steal the
    /// keyboard's place.
    fn hover_palette_item(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.palette_active != ix {
            self.palette_active = ix;
            cx.notify();
        }
    }

    /// Move the palette highlight by `delta`, wrapping around, and keep the
    /// selected row visible.
    fn palette_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.palette_items(cx).len();
        if len == 0 {
            return;
        }
        let cur = self.palette_active as isize;
        self.palette_active = (cur + delta).rem_euclid(len as isize) as usize;
        self.palette_scroll.scroll_to_item(self.palette_active);
        cx.notify();
    }

    pub(super) fn handle_palette_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev.keystroke.key.as_str() {
            "up" => self.palette_move(-1, cx),
            "down" => self.palette_move(1, cx),
            "tab" => {
                let delta = if ev.keystroke.modifiers.shift { -1 } else { 1 };
                self.palette_move(delta, cx);
            }
            "enter" => {
                // Held Enter must not fire twice; the latch releases on key-up.
                if !self.palette_enter.press(ev.is_held) {
                    cx.stop_propagation();
                    return;
                }
                if let Some(item) = self
                    .palette_items(cx)
                    .into_iter()
                    .nth(self.palette_active)
                {
                    self.activate_palette_item(item, window, cx);
                }
            }
            "escape" => self.close_palette(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    // ---- rendering ----

    pub(super) fn render_palette_card(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        if std::mem::take(&mut self.palette_focus_pending) {
            let handle = self.palette_input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        let items = self.palette_items(cx);
        self.palette_active = self.palette_active.min(items.len().saturating_sub(1));
        let query = self.palette_query(cx);

        let mut list: Vec<gpui_kit::AnyElement> = Vec::new();
        let mut current_section: Option<&'static str> = None;
        for (ix, item) in items.iter().enumerate() {
            let active = ix == self.palette_active;
            let section = match item {
                PaletteItem::ManageRoots
                | PaletteItem::OpenInExplorer
                | PaletteItem::RefreshNow
                | PaletteItem::RescanWorkspace
                | PaletteItem::ToggleAutoRefresh
                | PaletteItem::CycleFilter
                | PaletteItem::CycleSort
                | PaletteItem::CopyPath
                | PaletteItem::OperationLog
                | PaletteItem::ApplyPatch
                | PaletteItem::GoHistory
                | PaletteItem::GoChanges
                | PaletteItem::GoStashes
                | PaletteItem::CheckoutDetachedSelected
                | PaletteItem::BranchHereSelected
                | PaletteItem::TagHereSelected
                | PaletteItem::CherryPickSelected
                | PaletteItem::RevertSelected
                | PaletteItem::ResetSelected
                | PaletteItem::CopyShaSelected
                | PaletteItem::CopyShortShaSelected
                | PaletteItem::CopySubjectSelected
                | PaletteItem::CopyPatchSelected
                | PaletteItem::PopSelectedStash
                | PaletteItem::StashBranchSelected
                | PaletteItem::RenameCurrentBranch
                | PaletteItem::PinCurrentBranch
                | PaletteItem::ExcludeSelected
                | PaletteItem::ClearExcludes
                | PaletteItem::OpenExternally
                | PaletteItem::UndoLast
                | PaletteItem::RedoLast
                | PaletteItem::Discarded
                | PaletteItem::Settings => Some(t().actions_section),
                PaletteItem::Repo(_) => Some(t().repositories_section),
            };
            let mut section_new = None;
            if section != current_section {
                section_new = section;
                current_section = section;
            }
            let key = format!("palette-{ix}");
            let row = match item {
                PaletteItem::ManageRoots => {
                    let root_count = self.roots.len();
                    palette_row(ix, active, &key, cx)
                        .child(
                            div().flex_none().child(app_icon::render(
                                IconSlot::PaletteRoots,
                                text_muted(cx),
                                cx,
                            )),
                        )
                        .child(div().flex_1().min_w_0().child(t().manage_roots))
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(TEXT_XS))
                                .text_color(text_faint(cx))
                                .child(t().roots_count(root_count)),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_roots(window, cx);
                        }))
                }
                PaletteItem::OpenInExplorer => {
                    let meta: SharedString = if let Some(r) = self.active_repo() {
                        r.name.clone().into()
                    } else if let Some(root) = self.roots.first() {
                        t().explorer_root_meta(&truncate_path(root, 28)).into()
                    } else {
                        t().nothing_to_open.into()
                    };
                    palette_row(ix, active, &key, cx)
                        .child(
                            div().flex_none().child(app_icon::render(
                                IconSlot::PaletteExplorer,
                                text_muted(cx),
                                cx,
                            )),
                        )
                        .child(div().flex_1().min_w_0().child(crate::i18n::file_manager(t().open_in_file_explorer)))
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(180.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(TEXT_XS))
                                .text_color(text_faint(cx))
                                .child(meta),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.activate_palette_item(PaletteItem::OpenInExplorer, window, cx)
                        }))
                }
                PaletteItem::Settings => {
                    palette_row(ix, active, &key, cx)
                        .child(
                            div().flex_none().child(app_icon::render(
                                IconSlot::PaletteSettings,
                                text_muted(cx),
                                cx,
                            )),
                        )
                        .child(div().flex_1().min_w_0().child(t().settings_palette))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.activate_palette_item(PaletteItem::Settings, window, cx)
                        }))
                }
                PaletteItem::RefreshNow => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::RefreshCw,
                        t().refresh_workspace().into(),
                        None,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::RefreshNow, window, cx)
                    }))
                }
                PaletteItem::RescanWorkspace => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::FolderSync,
                        t().rescan_workspace().into(),
                        Some(t().roots_count(self.roots.len()).into()),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::RescanWorkspace, window, cx)
                    }))
                }
                PaletteItem::ToggleAutoRefresh => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::ListChecks,
                        t().auto_refresh_action(self.auto_refresh).into(),
                        None,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::ToggleAutoRefresh, window, cx)
                    }))
                }
                PaletteItem::CycleFilter => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::ListFilter,
                        t().filter_action(self.overview.filter().label()).into(),
                        None,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::CycleFilter, window, cx)
                    }))
                }
                PaletteItem::CycleSort => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::ArrowUpDown,
                        t().sort_action(self.overview.sort().label()).into(),
                        None,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::CycleSort, window, cx)
                    }))
                }
                PaletteItem::CopyPath => {
                    let meta = self
                        .action_repo_path()
                        .map(|path| truncate_path(&path, 30).into());
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::ClipboardCopy,
                        t().copy_path_action().into(),
                        meta,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::CopyPath, window, cx)
                    }))
                }
                PaletteItem::OperationLog => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::ScrollText,
                        t().oplog_action.into(),
                        None,
                        cx,
                    )
                    .child(
                        div().flex_none().child(kbd_chip(
                            &shortcuts::keys_display(cx, "operation-log").unwrap_or_default(),
                            cx,
                        )),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::OperationLog, window, cx)
                    }))
                }
                PaletteItem::Discarded => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::ArchiveRestore,
                        t().discarded_action().into(),
                        None,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::Discarded, window, cx)
                    }))
                }
                PaletteItem::UndoLast => {
                    let label = self
                        .active_repo_id()
                        .and_then(|id| self.undo_top_label(&id))
                        .unwrap_or_default();
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::Undo,
                        t().undo_palette(&label).into(),
                        None,
                        cx,
                    )
                    .child(
                        div().flex_none().child(kbd_chip(
                            &shortcuts::keys_display(cx, "undo-last").unwrap_or_default(),
                            cx,
                        )),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::UndoLast, window, cx)
                    }))
                }
                PaletteItem::RedoLast => {
                    let label = self
                        .active_repo_id()
                        .and_then(|id| self.redo_label(&id))
                        .unwrap_or_default();
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::Redo,
                        t().redo_palette(&label).into(),
                        None,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::RedoLast, window, cx)
                    }))
                }
                PaletteItem::ApplyPatch => {
                    let meta = self.active_repo().map(|repo| repo.name.clone().into());
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::ClipboardPaste,
                        t().apply_patch_action.into(),
                        meta,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::ApplyPatch, window, cx)
                    }))
                }
                PaletteItem::GoHistory => {
                    section_row(ix, active, &key, repo::RepoSection::History, cx)
                }
                PaletteItem::GoChanges => {
                    section_row(ix, active, &key, repo::RepoSection::LocalChanges, cx)
                }
                PaletteItem::GoStashes => {
                    section_row(ix, active, &key, repo::RepoSection::Stashes, cx)
                }
                PaletteItem::CheckoutDetachedSelected => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::GitCommitHorizontal,
                        t().checkout_detached_action().into(),
                        self.selected_commit_meta(),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(
                            PaletteItem::CheckoutDetachedSelected,
                            window,
                            cx,
                        )
                    }))
                }
                PaletteItem::BranchHereSelected => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::GitBranchPlus,
                        t().create_branch_here_action().into(),
                        self.selected_commit_meta(),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::BranchHereSelected, window, cx)
                    }))
                }
                PaletteItem::TagHereSelected => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::Tag,
                        t().create_tag_here_action().into(),
                        self.selected_commit_meta(),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::TagHereSelected, window, cx)
                    }))
                }
                PaletteItem::CherryPickSelected => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::Plus,
                        t().cherry_pick_action().into(),
                        self.selected_commit_meta(),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::CherryPickSelected, window, cx)
                    }))
                }
                PaletteItem::RevertSelected => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::RotateCcw,
                        t().revert_action().into(),
                        self.selected_commit_meta(),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::RevertSelected, window, cx)
                    }))
                }
                PaletteItem::ResetSelected => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::Rewind,
                        t().reset_selected_action().into(),
                        self.selected_commit_meta(),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::ResetSelected, window, cx)
                    }))
                }
                PaletteItem::CopyShaSelected => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::ClipboardCopy,
                        t().context_copy_sha.into(),
                        self.selected_commit_meta(),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::CopyShaSelected, window, cx)
                    }))
                }
                PaletteItem::CopyShortShaSelected => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::ClipboardCopy,
                        t().context_copy_short_sha.into(),
                        self.selected_commit_meta(),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::CopyShortShaSelected, window, cx)
                    }))
                }
                PaletteItem::CopySubjectSelected => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::ClipboardCopy,
                        t().context_copy_subject.into(),
                        self.selected_commit_meta(),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::CopySubjectSelected, window, cx)
                    }))
                }
                PaletteItem::CopyPatchSelected => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::ClipboardCopy,
                        t().context_copy_patch.into(),
                        self.selected_commit_meta(),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::CopyPatchSelected, window, cx)
                    }))
                }
                PaletteItem::RenameCurrentBranch => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::Circle,
                        t().rename_branch_action().into(),
                        self.current_branch_name().map(SharedString::from),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::RenameCurrentBranch, window, cx)
                    }))
                }
                PaletteItem::PopSelectedStash => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::Archive,
                        t().pop_stash_action().into(),
                        self.selected_stash.clone().map(SharedString::from),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::PopSelectedStash, window, cx)
                    }))
                }
                PaletteItem::StashBranchSelected => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::GitBranchPlus,
                        t().stash_branch_action().into(),
                        self.selected_stash.clone().map(SharedString::from),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::StashBranchSelected, window, cx)
                    }))
                }
                PaletteItem::PinCurrentBranch => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::Check,
                        t().pin_branch_action().into(),
                        self.current_branch_name().map(SharedString::from),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::PinCurrentBranch, window, cx)
                    }))
                }
                PaletteItem::OpenExternally => {
                    let meta: Option<SharedString> = self
                        .external_client
                        .as_ref()
                        .map(|client| client.program.clone().into());
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::ExternalLink,
                        t().open_externally_action().into(),
                        meta,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::OpenExternally, window, cx)
                    }))
                }
                PaletteItem::ExcludeSelected => {
                    let meta: SharedString = t().exclude_count(self.exclude.len()).into();
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::EyeOff,
                        t().exclude_action().into(),
                        Some(meta),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::ExcludeSelected, window, cx)
                    }))
                }
                PaletteItem::ClearExcludes => {
                    palette_action_row(
                        ix,
                        active,
                        &key,
                        IconName::Eye,
                        t().clear_excludes_action().into(),
                        None,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_palette_item(PaletteItem::ClearExcludes, window, cx)
                    }))
                }
                PaletteItem::Repo(rix) => {
                    let r = &self.repos[*rix];
                    let rix = *rix;
                    let id = r.path.to_string_lossy().into_owned();
                    let selected = self.overview.is_selected(&id);
                    let stale = r.stale_error().is_some();
                    let sync_color = if r.flags.contains(&Flag::UpstreamMissing) {
                        cx.theme().danger
                    } else if r.ahead > 0 && r.behind > 0 {
                        cx.theme().warning
                    } else if r.ahead > 0 || r.behind > 0 {
                        violet(cx)
                    } else {
                        text_faint(cx)
                    };
                    let branch: SharedString = if matches!(r.state, RowState::Loading) {
                        t().collecting.into()
                    } else {
                        truncate_label(
                            &r.branch.clone().unwrap_or_else(|| t().detached.into()),
                            22,
                        )
                        .into()
                    };
                    let name_display = truncate_label(&r.name, 36);
                    // WSL repositories show their Linux path; Windows-mounted
                    // repositories show the real Windows path they live on.
                    let path_str = r.path.to_string_lossy();
                    let is_wsl = crate::model::is_wsl_worktree(&path_str);
                    let path_display = match crate::model::to_windows_path(&path_str) {
                        Some(windows) => truncate_path(&windows, 52),
                        None => truncate_path(&path_str, 52),
                    };
                    palette_repo_row(ix, active, &key, cx)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    Icon::new(IconName::FolderClosed)
                                        .size(px(15.))
                                        .flex_none()
                                        .text_color(text_muted(cx)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_color(text_primary(cx))
                                        .child(highlighted(&name_display, &query, cx)),
                                )
                                .children(is_wsl.then(|| chip(t().root_kind_wsl, text_muted(cx))))
                                .child(
                                    div()
                                        .flex_none()
                                        .font_family(MONO)
                                        .text_size(px(TEXT_XS))
                                        .text_color(sync_color)
                                        .child(r.sync_label()),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .pl(px(23.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .font_family(MONO)
                                        .text_size(px(TEXT_XS))
                                        .text_color(text_muted(cx))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .child(path_display),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .max_w(px(150.))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .font_family(MONO)
                                        .text_size(px(TEXT_XS))
                                        .text_color(text_faint(cx))
                                        .child(branch),
                                )
                                .children(selected.then(|| chip(t().selected_meta(), violet(cx))))
                                .children(stale.then(|| chip(t().stale_meta(), cx.theme().danger))),
                        )
                        .on_click(cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                            let modifiers = event.modifiers();
                            if modifiers.control || modifiers.platform {
                                this.select_repo(&id, cx);
                            } else {
                                this.activate_palette_item(PaletteItem::Repo(rix), window, cx)
                            }
                        }))
                }
            };
            list.push(palette_entry(ix, section_new, &key, row, cx));
        }
        if self.hidden_repo_count() > 0 {
            list.push(
                div()
                    .px(px(10.))
                    .py(px(6.))
                    .text_size(px(TEXT_XS))
                    .text_color(text_faint(cx))
                    .child(t().palette_more_hidden(self.hidden_repo_count()))
                    .into_any_element(),
            );
        }
        if items.is_empty() {
            list.push(
                div()
                    .py(px(18.))
                    .flex()
                    .justify_center()
                    .text_size(px(TEXT_SM))
                    .text_color(text_muted(cx))
                    .child(t().no_matches)
                    .into_any_element(),
            );
        }

        let count = items.len();
        div()
            .id("palette")
            .track_focus(&self.palette_focus)
            .key_context("palette")
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                this.handle_palette_key(ev, window, cx)
            }))
            .on_key_up(cx.listener(|this, ev: &KeyUpEvent, _, cx| {
                if ev.keystroke.key == "enter" {
                    this.palette_enter.release();
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &PaletteNext, _, cx| this.palette_move(1, cx)))
            .on_action(cx.listener(|this, _: &PalettePrev, _, cx| this.palette_move(-1, cx)))
            .w(px(PALETTE_W))
            .max_h(px(520.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(PALETTE_RADIUS))
            .border_1()
            .border_color(hairline(0.10))
            .bg(cx.theme().popover)
            .shadow_lg()
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(
                // Header: query + esc hint (Zeron: 44px min, hairline below)
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .min_h(px(44.))
                    .px(px(16.))
                    .py(px(6.))
                    .border_b_1()
                    .border_color(hairline(0.06))
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(16.))
                            .flex_none()
                            .text_color(text_muted(cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                Input::new(&self.palette_input)
                                    .appearance(false)
                                    .focus_bordered(false)
                                    .w_full(),
                            ),
                    )
                    .child(kbd_chip("esc", cx)),
            )
            .child(
                div()
                    .id("palette-list")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .p(px(6.))
                    .overflow_y_scroll()
                    .track_scroll(&self.palette_scroll)
                    .children(list),
            )
            .child(
                // Footer: key hints (Zeron: hairline above, hint chips)
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .h(px(36.))
                    .px(px(12.))
                    .border_t_1()
                    .border_color(hairline(0.06))
                    .child(key_hint("↑↓", t().hint_navigate, cx))
                    .child(key_hint("↵", t().hint_open, cx))
                    .child(key_hint("esc", t().hint_close, cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(text_faint(cx))
                            .child(t().results_count(count)),
                    ),
            )
    }
}

// ---- palette building blocks ----

/// Every whitespace-separated word must appear somewhere in the label
/// (Zeron's command search matches words, not a literal phrase).
fn matches_query(query: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    query.split_whitespace().all(|word| text.contains(word))
}

/// One palette item container. The section label lives inside the item, so the
/// item's child index in the scroll list is exactly the item index that
/// [`ScrollHandle::scroll_to_item`] targets — and the label scrolls away with
/// its section.
fn palette_entry(
    ix: usize,
    section: Option<&'static str>,
    key: &str,
    row: impl IntoElement,
    cx: &Context<SpurShell>,
) -> gpui_kit::AnyElement {
    let mut entry = div()
        .id(("palette-item", ix))
        .flex()
        .flex_col()
        .flex_none()
        .on_mouse_move(cx.listener(move |this, _: &gpui_kit::MouseMoveEvent, _, cx| {
            this.hover_palette_item(ix, cx)
        }))
        .on_hover(hover_listener(key.to_string()));
    if let Some(section) = section {
        entry = entry.child(palette_section(section, cx));
    }
    entry.child(row).into_any_element()
}

/// `active` rows carry the stronger wash; inactive rows blend the wash in on
/// hover. The row owns an element id so click handlers can attach; the
/// container (see [`palette_entry`]) owns the scroll item index.
fn palette_row(
    ix: usize,
    active: bool,
    key: &str,
    cx: &Context<SpurShell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let mut row = div()
        .id(("palette-row", ix))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(PALETTE_ITEM_H))
        .overflow_hidden()
        .px(px(10.))
        .rounded(px(PALETTE_ITEM_RADIUS))
        .text_size(px(TEXT_MD));
    if active {
        row = row.bg(ink(0.08)).text_color(text_primary(cx));
    } else {
        row = row
            .bg(hover_blend(key, ink(0.0), ink(0.06)))
            .text_color(hover_blend(key, text_muted(cx), text_primary(cx)));
    }
    row
}

/// Repository rows are two-line (name + sync, then path + branch) like Zeron's
/// session rows: long names and paths truncate in their own columns instead of
/// colliding on one line.
fn palette_repo_row(
    ix: usize,
    active: bool,
    key: &str,
    cx: &Context<SpurShell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let mut row = div()
        .id(("palette-row", ix))
        .cursor_pointer()
        .flex()
        .flex_col()
        .gap(px(2.))
        .overflow_hidden()
        .px(px(10.))
        .py(px(7.))
        .rounded(px(PALETTE_ITEM_RADIUS))
        .text_size(px(TEXT_MD));
    if active {
        row = row.bg(ink(0.08)).text_color(text_primary(cx));
    } else {
        row = row
            .bg(hover_blend(key, ink(0.0), ink(0.06)))
            .text_color(hover_blend(key, text_muted(cx), text_primary(cx)));
    }
    row
}

fn palette_section(text: &str, cx: &Context<SpurShell>) -> gpui_kit::AnyElement {
    div()
        .px(px(10.))
        .pt(px(8.))
        .pb(px(4.))
        .text_size(px(TEXT_XS))
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .text_color(text_faint(cx))
        .child(text.to_string())
        .into_any_element()
}

/// Section-navigation rows: icon, label, and the shortcut chip on the
/// right, read from the effective keymap like the cheat sheet.
fn section_row(
    ix: usize,
    active: bool,
    key: &str,
    section: repo::RepoSection,
    cx: &Context<SpurShell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let (icon, label, item, action) = match section {
        repo::RepoSection::History => (
            IconName::Clock,
            t().shortcut_history,
            PaletteItem::GoHistory,
            "show-history",
        ),
        repo::RepoSection::LocalChanges => (
            IconName::Diff,
            t().shortcut_changes,
            PaletteItem::GoChanges,
            "show-changes",
        ),
        repo::RepoSection::Stashes => (
            IconName::Archive,
            t().shortcut_stashes,
            PaletteItem::GoStashes,
            "show-stashes",
        ),
    };
    let row = palette_action_row(ix, active, key, icon, label.into(), None, cx).child(
        div()
            .flex_none()
            .child(kbd_chip(
                &shortcuts::keys_display(cx, action).unwrap_or_default(),
                cx,
            )),
    );
    row.on_click(cx.listener(move |this, _, window, cx| {
        this.activate_palette_item(item, window, cx)
    }))
}

/// One action row: icon, label, and optional right-aligned meta.
fn palette_action_row(
    ix: usize,
    active: bool,
    key: &str,
    icon: IconName,
    label: SharedString,
    meta: Option<SharedString>,
    cx: &Context<SpurShell>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let row = palette_row(ix, active, key, cx)
        .child(
            div()
                .flex_none()
                .child(Icon::new(icon).size(px(15.)).text_color(text_muted(cx))),
        )
        .child(div().flex_1().min_w_0().child(label));
    match meta {
        Some(meta) => row.child(
            div()
                .flex_none()
                .max_w(px(180.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(TEXT_XS))
                .text_color(text_faint(cx))
                .child(meta),
        ),
        None => row,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_repeats_activate_once_until_release() {
        let mut enter = EnterPress::default();
        // Synthetic repeat releases are dropped on some backends: every repeat
        // arrives as a fresh keydown. Only the first may activate.
        assert!(enter.press(false));
        for _ in 0..35 {
            assert!(!enter.press(false));
        }
        enter.release();
        assert!(enter.press(false));
    }

    #[test]
    fn flagged_enter_repeats_do_not_activate() {
        let mut enter = EnterPress::default();
        assert!(!enter.press(true));
        assert!(!enter.press(false));
        enter.release();
        assert!(enter.press(false));
        assert!(!enter.press(true));
    }

    #[test]
    fn query_matches_words_in_any_order() {
        assert!(matches_query("roots manage", "Manage roots…"));
        assert!(matches_query("  ", "anything"));
        assert!(!matches_query("manage files", "Manage roots…"));
    }
}
