//! Localization.
//!
//! English only for now, but all user-facing chrome text flows through here so
//! another language is: add a `Strings` value, extend `t()` to select it.
//! Composed strings (counts, paths) are methods so each language can format
//! and pluralize its own way.
//!
//! Not localized on purpose: repository/branch/author names (data), operation
//! log payloads written by Git/fixtures, and keyboard key names ("Ctrl K").

pub struct Strings {
    // Branding
    /// The product name. The title bar shows the outlined wordmark and the
    /// window title carries the full name, so nothing reads this yet; the
    /// rename and the About section will.
    #[allow(dead_code)]
    pub app_name: &'static str,
    pub window_title: &'static str,

    // Empty state
    pub no_repository_open: &'static str,
    pub press_ctrl_k: &'static str,
    pub open_repository: &'static str,
    /// First-run welcome: the one place the product
    /// allows the branch metaphor.
    pub first_run: &'static str,

    // Title bar / toolbar
    pub tooltip_refresh: &'static str,
    pub pull_label: &'static str,
    // Push
    pub push_label: &'static str,
    pub push_title: &'static str,
    pub push_local: &'static str,
    pub push_remote: &'static str,
    pub push_target: &'static str,
    pub push_confirm: &'static str,
    pub push_no_remotes: &'static str,
    // Fetch dialog
    pub fetch_title: &'static str,
    pub fetch_label: &'static str,
    pub fetch_force: &'static str,
    pub fetch_all: &'static str,
    pub fetch_no_tags: &'static str,
    pub fetch_confirm: &'static str,
    // Pull dialog
    pub pull_title: &'static str,
    pub pull_into: &'static str,
    pub pull_branch: &'static str,
    pub pull_changes: &'static str,
    pub pull_changes_nothing: &'static str,
    pub pull_changes_nothing_hint: &'static str,
    pub pull_changes_stash: &'static str,
    pub pull_changes_stash_hint: &'static str,
    pub pull_changes_discard: &'static str,
    pub pull_changes_discard_hint: &'static str,
    pub pull_rebase: &'static str,
    pub pull_confirm: &'static str,
    // Tabs
    pub tooltip_open_repo: &'static str,

    // Palette
    pub search_placeholder: &'static str,
    pub actions_section: &'static str,
    pub repositories_section: &'static str,
    pub manage_roots: &'static str,
    pub open_in_file_explorer: &'static str,
    pub no_matches: &'static str,
    pub hint_navigate: &'static str,
    pub hint_open: &'static str,
    pub hint_close: &'static str,
    pub hint_add: &'static str,
    pub hint_back: &'static str,
    pub detached: &'static str,
    pub collecting: &'static str,
    pub nothing_to_open: &'static str,

    // Roots dialog
    pub scan_roots: &'static str,
    pub roots_hint: &'static str,
    pub no_roots: &'static str,
    pub browse: &'static str,
    pub add: &'static str,
    pub tooltip_browse: &'static str,
    pub root_kind_wsl: &'static str,
    pub root_kind_windows: &'static str,

    // Repository view
    pub section_history: &'static str,
    pub section_local_changes: &'static str,    pub section_stashes: &'static str,
    pub section_local_branches: &'static str,
    pub section_remotes: &'static str,
    pub section_tags: &'static str,
    pub no_branches: &'static str,
    pub no_remotes: &'static str,
    pub no_tags: &'static str,
    pub default_branch: &'static str,
    pub no_stashes: &'static str,
    pub stash_select: &'static str,
    pub stash_error: &'static str,
    pub tooltip_create_branch: &'static str,
    // Create Branch dialog
    pub create_branch: &'static str,
    pub branch_name: &'static str,
    pub branch_name_placeholder: &'static str,
    pub branch_base: &'static str,
    pub branch_changes: &'static str,
    pub branch_changes_keep: &'static str,
    pub branch_changes_keep_hint: &'static str,
    pub branch_changes_stash: &'static str,
    pub branch_changes_stash_hint: &'static str,
    pub branch_changes_discard: &'static str,
    pub branch_changes_discard_hint: &'static str,
    pub branch_checkout: &'static str,
    pub branch_overwrite: &'static str,
    pub branch_create: &'static str,
    // Add Remote dialog
    pub tooltip_add_remote: &'static str,
    pub add_remote: &'static str,
    pub remote_name: &'static str,
    pub remote_name_placeholder: &'static str,
    pub remote_url: &'static str,
    pub remote_url_placeholder: &'static str,
    pub remote_add: &'static str,
    pub history_loading: &'static str,
    pub history_load_more: &'static str,
    pub history_empty: &'static str,
    pub history_error: &'static str,
    pub history_search_placeholder: &'static str,
    pub history_searching: &'static str,
    pub history_search_no_match: &'static str,
    pub retry: &'static str,
    // Commit detail panel (history selection)
    pub commit_tab_information: &'static str,
    pub commit_tab_changes: &'static str,
    pub commit_author: &'static str,
    pub commit_sha: &'static str,
    pub commit_parents: &'static str,
    pub commit_message: &'static str,
    pub commit_loading: &'static str,
    pub commit_error: &'static str,
    pub commit_no_files: &'static str,
    pub commit_diff_hint: &'static str,

    // Local Changes (Release 2)
    pub filter_changes_placeholder: &'static str,
    pub section_unstaged: &'static str,
    pub section_staged: &'static str,
    pub stage_all: &'static str,
    pub unstage_all: &'static str,
    /// The per-row checkbox stages or unstages that one file.
    pub check_stage_file: &'static str,
    pub check_unstage_file: &'static str,
    pub no_changes: &'static str,
    pub diff_select: &'static str,
    pub diff_error: &'static str,
    pub diff_binary: &'static str,
    pub diff_truncated: &'static str,
    pub diff_staged: &'static str,
    pub diff_unstaged: &'static str,
    pub diff_untracked: &'static str,
    // Merge-conflict resolver
    pub conflict_chip: &'static str,
    pub conflict_ours: &'static str,
    pub conflict_theirs: &'static str,
    pub conflict_both: &'static str,
    pub conflict_apply: &'static str,
    pub conflict_loading: &'static str,
    pub conflict_error: &'static str,
    pub conflict_no_markers: &'static str,
    // Local Changes file context menu
    pub context_reveal: &'static str,
    pub context_resolve: &'static str,
    pub context_checkout: &'static str,
    pub context_open_in_browser: &'static str,
    pub context_stage: &'static str,
    pub context_unstage: &'static str,
    pub context_discard: &'static str,
    pub context_stash: &'static str,
    pub context_copy_path: &'static str,
    pub context_copy_full_path: &'static str,
    // Commit context menu (History rows and panel)
    pub context_checkout_detached: &'static str,
    pub context_create_branch_here: &'static str,
    pub context_create_tag_here: &'static str,
    pub context_revert_commit: &'static str,
    pub context_copy_sha: &'static str,
    pub context_copy_short_sha: &'static str,
    pub context_copy_subject: &'static str,
    pub context_copy_patch: &'static str,
    pub context_rename_branch: &'static str,
    pub context_delete_branch: &'static str,
    pub context_pin_branch: &'static str,
    pub context_unpin_branch: &'static str,
    pub branch_delete_title: &'static str,
    pub branch_force_title: &'static str,
    pub branch_delete_anyway: &'static str,
    pub rename_title: &'static str,
    pub rename_name: &'static str,
    pub rename_placeholder: &'static str,
    pub rename_confirm: &'static str,
    pub branch_pinned: &'static str,
    // Rebase dialog
    pub context_rebase_onto: &'static str,
    pub rebase_onto: &'static str,
    pub rebase_changes: &'static str,
    pub rebase_autostash: &'static str,
    pub rebase_confirm: &'static str,
    // Create Tag dialog
    pub tag_title: &'static str,
    pub context_push_tag: &'static str,
    pub context_copy_tag_name: &'static str,
    pub context_ignore_file: &'static str,
    pub context_ignore_folder: &'static str,
    pub context_untrack_ignore: &'static str,
    pub context_fetch_remote: &'static str,
    pub context_prune_remote: &'static str,
    pub context_edit_url: &'static str,
    pub context_rename_remote: &'static str,
    pub context_remove_remote: &'static str,
    pub remote_edit_title: &'static str,
    pub remote_rename_title: &'static str,
    pub remote_save: &'static str,
    pub remote_remove_title: &'static str,
    pub remote_remove_confirm: &'static str,
    pub tag_push_title: &'static str,
    pub tag_push_remote: &'static str,
    pub tag_push_confirm: &'static str,
    pub tag_delete_title: &'static str,
    pub tag_delete_local: &'static str,
    pub tag_delete_confirm: &'static str,
    pub tag_filter_placeholder: &'static str,
    pub tag_name: &'static str,
    pub tag_name_placeholder: &'static str,
    pub tag_message: &'static str,
    pub tag_create: &'static str,
    pub tag_push_after: &'static str,
    // Stash dialog
    pub stash_title: &'static str,
    pub stash_drop_title: &'static str,
    pub context_apply: &'static str,
    pub context_drop: &'static str,
    pub context_pop: &'static str,
    pub context_stash_branch: &'static str,
    pub context_apply_file: &'static str,
    pub stash_branch_title: &'static str,
    pub stash_branch_name: &'static str,
    pub stash_branch_placeholder: &'static str,
    pub stash_branch_confirm: &'static str,
    pub stash_mode: &'static str,
    pub stash_mode_discard: &'static str,
    pub stash_mode_discard_hint: &'static str,
    pub stash_mode_keep_index: &'static str,
    pub stash_mode_keep_index_hint: &'static str,
    pub stash_mode_keep_all: &'static str,
    pub stash_mode_keep_all_hint: &'static str,
    pub stash_description: &'static str,
    pub stash_placeholder: &'static str,
    pub stash_confirm: &'static str,
    // Apply patch from clipboard
    pub apply_patch_action: &'static str,
    pub apply_patch_title: &'static str,
    pub clipboard_empty: &'static str,
    pub clipboard_not_patch: &'static str,
    pub discard_title: &'static str,
    pub discard_confirm: &'static str,
    pub commit_placeholder: &'static str,
    pub commit_subject_end: &'static str,
    pub commit_button: &'static str,
    /// Disabled-Commit hint and tooltip: why it cannot commit right now.
    pub commit_needs_staged: &'static str,
    /// Disabled-Commit tooltip when files are staged but the message is empty.
    pub commit_needs_message: &'static str,
    /// Drag-handle tooltip between the file list and the diff.
    pub col_resize_tooltip: &'static str,
    /// Soft-wrap toggle tooltip in the diff header.
    pub diff_wrap_tooltip: &'static str,

    // Operations
    pub cancel: &'static str,
    /// Tooltip of the commit detail panel's collapse button.
    pub collapse: &'static str,
    /// Accessible label of the detail panel's message expand toggle.
    pub expand: &'static str,

    // Operation log panel
    pub oplog_title: &'static str,
    pub oplog_empty: &'static str,
    pub oplog_cancel: &'static str,
    pub oplog_close: &'static str,
    pub oplog_details: &'static str,
    pub oplog_action: &'static str,
    pub oplog_truncated: &'static str,

    // Keyboard shortcuts
    pub shortcuts_title: &'static str,
    pub shortcuts_group_general: &'static str,
    pub shortcuts_group_repository: &'static str,
    pub shortcuts_group_commit: &'static str,
    pub shortcuts_group_palette: &'static str,
    pub shortcut_palette: &'static str,
    pub shortcut_settings: &'static str,
    pub shortcut_shortcuts: &'static str,
    pub shortcut_close: &'static str,
    pub shortcut_next_tab: &'static str,
    pub shortcut_prev_tab: &'static str,
    pub shortcut_close_tab: &'static str,
    pub shortcut_history: &'static str,
    pub shortcut_changes: &'static str,
    pub shortcut_stashes: &'static str,
    pub shortcut_commit: &'static str,
    pub shortcut_palette_next: &'static str,
    pub shortcut_palette_prev: &'static str,
    pub shortcut_undo_last: &'static str,
    pub shortcut_oplog: &'static str,
    pub settings_keyboard: &'static str,
    pub settings_keyboard_hint: &'static str,
    pub settings_tab_general: &'static str,
    pub settings_tab_shortcuts: &'static str,
    pub keymap_unbound: &'static str,

    // Settings page
    pub settings_title: &'static str,
    pub settings_close: &'static str,
    pub tooltip_settings: &'static str,
    pub settings_palette: &'static str,
    pub settings_appearance: &'static str,
    pub settings_appearance_hint: &'static str,
    pub settings_theme: &'static str,
    pub settings_theme_hint: &'static str,
    pub settings_theme_menu: &'static str,
    pub settings_library: &'static str,
    pub settings_library_hint: &'static str,
    pub settings_import: &'static str,
    pub settings_reset: &'static str,
    pub settings_reset_hint: &'static str,
    pub settings_reset_action: &'static str,
    pub settings_reset_one: &'static str,
    pub settings_colors: &'static str,
    pub settings_colors_hint: &'static str,
    // Notifications (error alert lifetime)
    pub settings_notifications: &'static str,
    pub settings_notifications_hint: &'static str,
    pub settings_alerts: &'static str,
    pub settings_alerts_hint: &'static str,
    pub settings_accounts: &'static str,
    pub settings_accounts_hint: &'static str,
    pub accounts_default: &'static str,
    pub accounts_default_hint: &'static str,
    pub accounts_none: &'static str,
    pub accounts_add: &'static str,
    pub accounts_add_hint: &'static str,
    pub accounts_edit: &'static str,
    pub accounts_remove: &'static str,
    pub profile_title_add: &'static str,
    pub profile_title_edit: &'static str,
    pub profile_label: &'static str,
    pub profile_label_placeholder: &'static str,
    pub profile_name: &'static str,
    pub profile_name_placeholder: &'static str,
    pub profile_email: &'static str,
    pub profile_email_placeholder: &'static str,
    pub profile_github: &'static str,
    pub profile_github_placeholder: &'static str,
    pub profile_hint: &'static str,
    pub profile_signed_in: &'static str,
    pub sidebar_account: &'static str,
    pub profile_save: &'static str,
    pub profile_issue_label: &'static str,
    pub profile_issue_name: &'static str,
    pub profile_issue_email: &'static str,
    pub profile_issue_github: &'static str,
    pub commit_as_auto: &'static str,
    pub commit_as_git_config: &'static str,
    pub commit_as_manage: &'static str,
    pub alert_timeout_3s: &'static str,
    pub alert_timeout_5s: &'static str,
    pub alert_timeout_10s: &'static str,
    pub alert_timeout_manual: &'static str,
    pub alert_dismiss: &'static str,
    pub settings_token_accent: &'static str,
    pub settings_token_accent_hint: &'static str,
    pub settings_token_background: &'static str,
    pub settings_token_background_hint: &'static str,
    pub settings_token_sidebar: &'static str,
    pub settings_token_sidebar_hint: &'static str,
    pub settings_token_popover: &'static str,
    pub settings_token_popover_hint: &'static str,
    pub settings_token_raised: &'static str,
    pub settings_token_raised_hint: &'static str,
    pub settings_token_text: &'static str,
    pub settings_token_text_hint: &'static str,
    pub settings_token_muted_text: &'static str,
    pub settings_token_muted_text_hint: &'static str,
    pub settings_token_faint_text: &'static str,
    pub settings_token_faint_text_hint: &'static str,
    pub settings_token_danger: &'static str,
    pub settings_token_danger_hint: &'static str,
    pub settings_token_warning: &'static str,
    pub settings_token_warning_hint: &'static str,
    pub settings_token_success: &'static str,
    pub settings_token_success_hint: &'static str,
    pub settings_save: &'static str,
    pub settings_save_hint: &'static str,
    pub settings_save_action: &'static str,
    pub settings_saving: &'static str,
    pub theme_name_placeholder: &'static str,
}

impl Strings {
    pub fn path_input_placeholder(&self) -> &'static str {
        if cfg!(windows) { "Linux or Windows path…" } else { "Folder path…" }
    }

    pub fn select_roots_prompt(&self) -> &'static str {
        "Select scan root folder(s)"
    }

    pub fn roots_count(&self, n: usize) -> String {
        if n == 1 {
            "1 root".to_string()
        } else {
            format!("{n} roots")
        }
    }

    pub fn results_count(&self, n: usize) -> String {
        if n == 1 {
            "1 result".to_string()
        } else {
            format!("{n} results")
        }
    }

    pub fn commits_count(&self, n: usize) -> String {
        if n == 1 {
            "1 commit".to_string()
        } else {
            format!("{n} commits")
        }
    }

    /// `CHANGES (5)` tab suffix in the commit detail panel.
    pub fn changed_files_count(&self, n: usize) -> String {
        if n == 1 {
            "1 changed file".to_string()
        } else {
            format!("{n} changed files")
        }
    }

    /// `UNSTAGED (7)` section header.
    pub fn section_count(&self, label: &str, n: usize) -> String {
        format!("{label} ({n})")
    }

    /// Operation-log line after a successful stage/unstage.
    pub fn log_change_action(&self, staged: bool, path: &str) -> String {
        format!("{} {path}", if staged { "staged" } else { "unstaged" })
    }

    /// `SUBJECT 12/50` counter beside the Commit button.
    pub fn commit_subject_count(&self, len: usize, limit: usize) -> String {
        format!("SUBJECT {len}/{limit}")
    }

    pub fn log_committed(&self, subject: &str) -> String {
        format!("committed: {subject}")
    }

    pub fn log_branch_created(&self, name: &str) -> String {
        format!("created branch {name}")
    }

    pub fn log_branch_checked_out(&self, name: &str) -> String {
        format!("checked out {name}")
    }

    pub fn log_remote_checked_out(&self, remote: &str, branch: &str) -> String {
        format!("checked out {remote}/{branch}")
    }

    pub fn accounts_summary(&self, name: &str, email: &str, github: Option<&str>) -> String {
        match github {
            Some(login) => format!("{name} <{email}> · GitHub {login}"),
            None => format!("{name} <{email}>"),
        }
    }

    pub fn commit_as(&self, label: &str) -> String {
        format!("Committing as {label}")
    }

    pub fn commit_as_source(&self, source: crate::accounts::Source) -> &'static str {
        match source {
            crate::accounts::Source::Chosen => "Picked for this repository",
            crate::accounts::Source::Remote => "Matches this repository's GitHub remote",
            crate::accounts::Source::Default => "Default profile",
        }
    }

    pub fn commit_as_warning(&self, label: &str, owners: &str) -> String {
        format!("This repository's remote belongs to {owners}, but commits use {label}. Pick another profile if that is wrong.")
    }

    pub fn log_remote_added(&self, name: &str) -> String {
        format!("added remote {name}")
    }

    /// Push result line: `pushed main → origin/main`.
    pub fn log_pushed(&self, local: &str, remote: &str, target: &str) -> String {
        format!("pushed {local} → {remote}/{target}")
    }

    pub fn log_fetched(&self, remote: &str) -> String {
        format!("fetched {remote}")
    }

    pub fn log_fetched_all(&self) -> String {
        "fetched all remotes".to_string()
    }

    pub fn log_pulled(&self, remote: &str, branch: &str, local: &str) -> String {
        format!("pulled {remote}/{branch} into {local}")
    }

    /// Progress caption under a running push modal.
    pub fn push_running(&self, local: &str, remote: &str, target: &str) -> String {
        format!("Pushing {local} → {remote}/{target}…")
    }

    /// Progress caption under a running fetch modal.
    pub fn fetch_running(&self, remote: &str) -> String {
        format!("Fetching {remote}…")
    }

    pub fn fetch_running_all(&self) -> String {
        "Fetching all remotes…".to_string()
    }

    /// Progress caption under a running pull modal.
    pub fn pull_running(&self, remote: &str, branch: &str) -> String {
        format!("Pulling {remote}/{branch}…")
    }

    /// Operation-log line after handing a URL to the default browser.
    pub fn log_opened_url(&self, url: &str) -> String {
        format!("opened {url}")
    }

    /// A remote whose URL is a local path or an SSH-only address has no page
    /// to open.
    pub fn log_no_web_url(&self, remote: &str) -> String {
        format!("remote {remote} has no browser URL")
    }

    // ---- Local Changes context menu ----

    pub fn log_discarded(&self, path: &str) -> String {
        format!("discarded changes to {path}")
    }

    pub fn log_conflict_resolved(&self, path: &str) -> String {
        format!("resolved conflicts in {path}")
    }

    // ---- Merge-conflict resolver ----

    /// Card title of one conflict (`Conflict 2 of 3`).
    pub fn conflict_section(&self, number: usize, total: usize) -> String {
        format!("Conflict {number} of {total}")
    }

    /// Choice progress in the resolver header.
    pub fn conflict_progress(&self, chosen: usize, total: usize) -> String {
        format!("{chosen} of {total} chosen")
    }

    pub fn conflict_ours_label(&self, label: &str) -> String {
        format!("Ours ({label})")
    }

    pub fn conflict_theirs_label(&self, label: &str) -> String {
        format!("Theirs ({label})")
    }

    pub fn log_stashed(&self, path: &str) -> String {
        format!("stashed changes to {path}")
    }

    pub fn log_stash_applied(&self, id: &str) -> String {
        format!("applied {id}")
    }

    pub fn log_stash_dropped(&self, id: &str) -> String {
        format!("dropped {id}")
    }

    // ---- Ignore ----

    /// "Ignore *.log".
    pub fn context_ignore_ext(&self, ext: &str) -> String {
        format!("Ignore *.{ext}")
    }

    pub fn log_ignored(&self, rule: &str, detail: &str) -> String {
        format!("ignored {rule} ({detail})")
    }

    // ---- Stash extras ----

    pub fn log_stash_popped(&self, id: &str) -> String {
        format!("popped {id}")
    }

    pub fn log_stash_branched(&self, id: &str, branch: &str) -> String {
        format!("created branch {branch} from {id}")
    }

    pub fn log_stash_file_applied(&self, id: &str, path: &str) -> String {
        format!("applied {path} from {id}")
    }

    pub fn log_no_stash_selected(&self) -> String {
        "no stash selected".to_string()
    }

    // ---- Commit context menu ----

    /// "Cherry-pick onto <current branch>".
    pub fn cherry_pick_onto(&self, branch: &str) -> String {
        format!("Cherry-pick onto {branch}")
    }

    pub fn log_detached_checked_out(&self, short: &str) -> String {
        format!("checked out {short} (detached)")
    }

    pub fn log_cherry_picked(&self, short: &str) -> String {
        format!("cherry-picked {short}")
    }

    pub fn log_reverted(&self, short: &str) -> String {
        format!("reverted {short}")
    }

    pub fn log_rebased(&self, branch: &str, onto: &str) -> String {
        format!("rebased {branch} onto {onto}")
    }

    pub fn log_no_rebase_target(&self) -> String {
        "there is no other branch to rebase onto".to_string()
    }

    pub fn rebase_title(&self, branch: &str) -> String {
        format!("Rebase {branch}")
    }

    pub fn rebase_body(&self, branch: &str) -> String {
        format!("Rebase's {branch} on top of the selected branch. Conflicts abort the rebase and leave {branch} unchanged.")
    }

    pub fn context_rename_branch_named(&self, branch: &str) -> String {
        format!("Rename branch {branch}…")
    }

    // ---- Undo safety net ----

    pub fn undo_action(&self) -> &'static str {
        "Undo"
    }

    pub fn undo_palette(&self, label: &str) -> String {
        format!("Undo: {label}")
    }

    pub fn redo_palette(&self, label: &str) -> String {
        format!("Redo: {label}")
    }

    pub fn log_undid(&self, label: &str) -> String {
        format!("undid {label}")
    }

    pub fn log_redid(&self, label: &str) -> String {
        format!("redid {label}")
    }

    pub fn undo_empty(&self) -> &'static str {
        "Nothing to undo"
    }

    pub fn redo_empty(&self) -> &'static str {
        "Nothing to redo"
    }

    /// Why an op ran without undo (backup failed, too large, not undoable).
    pub fn log_no_undo(&self, reason: &str) -> String {
        format!("no undo: {reason}")
    }

    pub fn undo_needs_clean_worktree(&self) -> &'static str {
        "worktree has changes; undo refused"
    }

    pub fn undo_target_moved(&self) -> &'static str {
        "target moved since; undo refused"
    }

    // ---- Reset dialog ----

    /// Commit menu + palette: "Reset main to here…".
    pub fn reset_here_action(&self, current: &str) -> String {
        format!("Reset {current} to here…")
    }

    pub fn reset_selected_action(&self) -> &'static str {
        "Reset current branch to selected commit…"
    }

    /// Dialog title: `Reset main to 3f2a1c9 "Refine sidebar"`.
    pub fn reset_title(&self, branch: &str, short: &str, subject: &str) -> String {
        if subject.is_empty() {
            format!("Reset {branch} to {short}")
        } else {
            format!("Reset {branch} to {short} \"{subject}\"")
        }
    }

    pub fn reset_mode_soft(&self) -> &'static str {
        "Soft"
    }

    pub fn reset_mode_soft_hint(&self) -> &'static str {
        "Move the branch; keep changes staged"
    }

    pub fn reset_mode_mixed(&self) -> &'static str {
        "Mixed"
    }

    pub fn reset_mode_mixed_hint(&self) -> &'static str {
        "Move the branch; keep changes unstaged"
    }

    pub fn reset_mode_hard(&self) -> &'static str {
        "Hard"
    }

    pub fn reset_mode_hard_hint(&self) -> &'static str {
        "Move the branch; discard all changes"
    }

    pub fn reset_undo_line(&self) -> &'static str {
        "Undo will be available."
    }

    pub fn reset_confirm(&self) -> &'static str {
        "Reset"
    }

    pub fn reset_running(&self, branch: &str, short: &str) -> String {
        format!("Resetting {branch} to {short}…")
    }

    /// Force-push warning: `{n}` commits on the upstream are dropped locally.
    pub fn reset_pushed(&self, n: u64) -> String {
        if n == 1 {
            "1 commit is already pushed; you will need to force-push".to_string()
        } else {
            format!("{n} commits are already pushed; you will need to force-push")
        }
    }

    pub fn reset_pushed_unknown(&self) -> &'static str {
        "Could not verify pushed commits."
    }

    pub fn log_reset_to(&self, branch: &str, short: &str) -> String {
        format!("reset {branch} to {short}")
    }

    // ---- Menu explainers (history context menu) ----

    pub fn explainer_checkout_detached(&self) -> &'static str {
        "Opens this commit without moving any branch. HEAD detaches; your branches stay where they are."
    }

    pub fn explainer_create_branch(&self) -> &'static str {
        "Starts a new branch at this commit. Nothing moves until you check it out."
    }

    pub fn explainer_create_tag(&self) -> &'static str {
        "Marks this commit with a tag name. Add a message to make it annotated."
    }

    pub fn explainer_cherry_pick(&self, branch: &str) -> String {
        format!(
            "Copies this commit onto {branch} as a new commit. Conflicts open the resolver; undo stays available."
        )
    }

    pub fn explainer_revert(&self) -> &'static str {
        "Creates a new commit that undoes this commit. History stays shared and linear; undo stays available."
    }

    pub fn explainer_reset(&self, branch: &str) -> String {
        format!(
            "Moves {branch} back to this commit. Soft, mixed, or hard — hard discards work but stays undoable. Warns about pushed commits."
        )
    }

    pub fn explainer_copy_sha(&self) -> &'static str {
        "Copies the full commit hash to the clipboard."
    }

    pub fn explainer_copy_short(&self) -> &'static str {
        "Copies the 7-character hash to the clipboard."
    }

    pub fn explainer_copy_subject(&self) -> &'static str {
        "Copies the commit subject line to the clipboard."
    }

    pub fn explainer_copy_patch(&self) -> &'static str {
        "Copies the full patch of this commit to the clipboard. Apply it later from the palette."
    }

    // ---- Recently discarded ----

    pub fn discarded_action(&self) -> &'static str {
        "Recently discarded…"
    }

    pub fn discarded_title(&self) -> &'static str {
        "Recently discarded"
    }

    pub fn discarded_empty(&self) -> &'static str {
        "No discarded changes."
    }

    pub fn discarded_restore(&self) -> &'static str {
        "Restore"
    }

    /// Meta line under a backup: `file.txt · +12 -5 · 2m ago`.
    pub fn discarded_meta(&self, files: &str, size: &str, ago: &str) -> String {
        format!("{files} · {size} · {ago}")
    }

    /// `3 files` label when a backup holds more than one.
    pub fn discarded_files(&self, n: usize) -> String {
        if n == 1 {
            "1 file".to_string()
        } else {
            format!("{n} files")
        }
    }

    pub fn log_tag_created(&self, name: &str) -> String {
        format!("created tag {name}")
    }

    // ---- Remote row menu ----

    pub fn log_pruned(&self, remote: &str) -> String {
        format!("pruned {remote}")
    }

    pub fn log_remote_url_saved(&self, remote: &str) -> String {
        format!("updated URL for {remote}")
    }

    pub fn log_remote_removed(&self, remote: &str) -> String {
        format!("removed remote {remote}")
    }

    pub fn remote_remove_body(&self, remote: &str) -> String {
        format!("Remove remote {remote}? Its branches stay local.")
    }

    pub fn tag_delete_body(&self, name: &str) -> String {
        format!("Delete tag {name} from:")
    }

    // ---- Tag row menu ----

    pub fn log_tag_created_pushed(&self, name: &str, remote: &str) -> String {
        format!("created tag {name} and pushed it to {remote}")
    }

    pub fn log_tag_pushed(&self, name: &str, remote: &str) -> String {
        format!("pushed tag {name} to {remote}")
    }

    pub fn log_tag_deleted(&self, name: &str, destinations: &[String]) -> String {
        if destinations.is_empty() {
            format!("deleted tag {name}")
        } else {
            format!("deleted tag {name} ({})", destinations.join(", "))
        }
    }

    // ---- Branch row menu ----

    pub fn log_branch_renamed(&self, old: &str, new: &str) -> String {
        format!("renamed {old} → {new}")
    }

    pub fn log_branch_deleted(&self, name: &str) -> String {
        format!("deleted branch {name}")
    }

    pub fn log_branch_delete_current(&self, name: &str) -> String {
        format!("cannot delete the checked-out branch ({name})")
    }

    pub fn branch_delete_body(&self, name: &str) -> String {
        format!("Delete local branch {name}? The remote branch is not touched. Undo will be available.")
    }

    pub fn branch_force_body(&self, name: &str) -> String {
        format!("{name} is not fully merged. Deleting removes it from the branch list. Undo will be available.")
    }

    pub fn log_branch_pinned(&self, name: &str) -> String {
        format!("pinned {name}")
    }

    pub fn log_branch_unpinned(&self, name: &str) -> String {
        format!("unpinned {name}")
    }

    pub fn log_branch_already_pinned(&self, name: &str) -> String {
        format!("{name} is already pinned")
    }

    pub fn log_no_current_branch(&self) -> String {
        "no checked-out branch".to_string()
    }

    pub fn rename_branch_action(&self) -> &'static str {
        "Rename current branch…"
    }

    pub fn sort_by_recent(&self) -> String {
        "Sort by recent activity".to_string()
    }

    pub fn sort_by_name(&self) -> String {
        "Sort alphabetically".to_string()
    }

    pub fn pin_branch_action(&self) -> &'static str {
        "Pin current branch"
    }

    pub fn log_copied_commit(&self, what: &str) -> String {
        format!("copied {what} to the clipboard")
    }

    pub fn log_copied_patch(&self, bytes: usize) -> String {
        format!("copied patch ({bytes} bytes) to the clipboard")
    }

    // ---- Apply patch from clipboard ----

    pub fn log_patch_applied(&self, files: &[String]) -> String {
        if files.len() == 1 {
            format!("applied patch ({})", files[0])
        } else {
            format!("applied patch ({} files)", files.len())
        }
    }

    /// Trailing preview line when the patch touches more files than shown.
    pub fn apply_patch_more(&self, n: usize) -> String {
        format!("+{n} more")
    }

    // ---- Keyboard shortcuts ----

    /// Cheat-sheet footer: where remapping lives.
    pub fn keymap_hint(&self, path: &str) -> String {
        format!("Edit keymap.json to remap: {path}")
    }

    /// Settings Keyboard card status line.
    pub fn keymap_custom(&self, n: usize) -> String {
        if n == 1 {
            "1 custom binding".to_string()
        } else {
            format!("{n} custom bindings")
        }
    }

    pub fn keymap_default(&self) -> String {
        "Default bindings".to_string()
    }

    /// Capture hint inside the binding button while listening.
    pub fn shortcut_capture_hint(&self) -> String {
        "Press keys…".to_string()
    }

    /// Refusal when the captured press has neither Ctrl nor Alt (nor F-key).
    pub fn keymap_needs_modifier(&self) -> String {
        if cfg!(target_os = "macos") {
            "Add Cmd, Ctrl or Alt — plain keys would hijack typing".to_string()
        } else {
            "Add Ctrl or Alt — plain keys would hijack typing".to_string()
        }
    }

    /// Collision message naming the current owner of the captured keys.
    pub fn keymap_conflict(&self, owner: &str, keys: &str) -> String {
        format!("{owner} already uses {keys} — reset it first")
    }

    pub fn keymap_saved(&self) -> String {
        "Saved".to_string()
    }

    // ---- Operation log ----

    /// Running-operation age in the panel ("running · 12s").
    pub fn oplog_elapsed(&self, secs: u64) -> String {
        format!("{secs}s")
    }

    /// Pump cancellation of queued work.
    pub fn log_cancel_queued(&self, n: usize) -> String {
        if n == 1 {
            "cancelled 1 queued operation".to_string()
        } else {
            format!("cancelled {n} queued operations")
        }
    }

    /// Pump cancellation of the running operation (no error toast).
    pub fn log_cancelled(&self, op: &str) -> String {
        format!("cancelled {op}")
    }

    pub fn log_no_commit_selected(&self) -> String {
        "no commit selected".to_string()
    }

    pub fn cherry_pick_action(&self) -> &'static str {
        "Cherry-pick selected commit"
    }

    pub fn revert_action(&self) -> &'static str {
        "Revert selected commit"
    }

    pub fn pop_stash_action(&self) -> &'static str {
        "Pop selected stash"
    }

    pub fn stash_branch_action(&self) -> &'static str {
        "Create branch from selected stash…"
    }

    pub fn checkout_detached_action(&self) -> &'static str {
        "Check out selected commit (detached)"
    }

    pub fn create_branch_here_action(&self) -> &'static str {
        "Create branch at selected commit…"
    }

    pub fn create_tag_here_action(&self) -> &'static str {
        "Create tag at selected commit…"
    }

    /// Body of the destructive stash Drop confirmation.
    pub fn stash_drop_body(&self, id: &str) -> String {
        format!("Drop {id}? Undo will be available.")
    }

    pub fn log_action_failed(&self, action: &str, err: &str) -> String {
        format!("could not {action}: {err}")
    }

    /// Stash entry message for a path-targeted stash.
    pub fn stash_message(&self, path: &str) -> String {
        format!("Spur: {path}")
    }

    /// Stash description for a multi-file selection.
    pub fn stash_message_files(&self, n: usize) -> String {
        if n == 1 {
            "Spur: 1 file".to_string()
        } else {
            format!("Spur: {n} files")
        }
    }

    /// Short "N files" label for multi-selection dialogs.
    pub fn file_count(&self, n: usize) -> String {
        if n == 1 {
            "1 file".to_string()
        } else {
            format!("{n} files")
        }
    }

    /// Body of the destructive Discard confirmation.
    pub fn discard_body(&self, path: &str, untracked: bool) -> String {
        if untracked {
            format!("Delete the untracked file {path}? Undo will be available.")
        } else {
            format!("Discard all changes to {path}? Undo will be available.")
        }
    }

    /// Multi-file variant of [`discard_body`](Self::discard_body); `untracked`
    /// means every target is untracked (the label says "Delete").
    pub fn discard_body_files(&self, n: usize, untracked: bool) -> String {
        if untracked {
            format!("Delete {n} untracked files? Undo will be available.")
        } else {
            format!("Discard changes to {n} files? Undo will be available.")
        }
    }

    // ---- Diff-pane hunk actions ----

    pub fn log_hunk_staged(&self, path: &str) -> String {
        format!("staged hunk in {path}")
    }

    pub fn log_hunk_unstaged(&self, path: &str) -> String {
        format!("unstaged hunk in {path}")
    }

    pub fn log_hunk_discarded(&self, path: &str) -> String {
        format!("discarded hunk in {path}")
    }

    pub fn hunk_discard_body(&self, path: &str) -> String {
        format!("Discard this hunk of {path}? Undo will be available.")
    }

    pub fn stashes_count(&self, n: usize) -> String {
        if n == 1 {
            "1 stash".to_string()
        } else {
            format!("{n} stashes")
        }
    }

    /// Roots hint; WSL and Windows paths only exist on Windows.
    pub fn roots_hint_native(&self) -> &'static str {
        if cfg!(windows) { self.roots_hint } else { "Repositories are discovered under these paths" }
    }

    pub fn tooltip_browse_native(&self) -> &'static str {
        if cfg!(windows) { self.tooltip_browse } else { "Pick folders with the system dialog" }
    }

    pub fn explorer_root_meta(&self, path: &str) -> String {
        format!("root: {path}")
    }

    // Operation-log lines written by the shell itself (Git/fixture payloads
    // stay as data).
    pub fn log_scan_root_added(&self, root: &str) -> String {
        format!("scan root added: {root}")
    }
    pub fn log_explorer(&self, target: &str) -> String {
        format!("explorer: {target}")
    }
    pub fn log_explorer_failed(&self, err: &str) -> String {
        format!("explorer failed: {err}")
    }
    pub fn log_repo_dropped(&self, path: &str) -> String {
        format!("repository dropped: {path}")
    }
    pub fn log_foreign_distro(&self, distro: &str) -> String {
        format!(
            "ignored path from WSL distribution '{distro}' — Spur runs Git in {}",
            crate::git::distro()
        )
    }
    pub fn log_nothing_to_reveal(&self) -> String {
        "nothing to reveal (no repository, no roots)".to_string()
    }
    pub fn log_settings_save_failed(&self, err: &str) -> String {
        format!("could not save settings: {err}")
    }
    pub fn log_root_diagnostic(&self, diagnostic: &str) -> String {
        format!("root: {diagnostic}")
    }
    pub fn log_discovered(&self, count: usize) -> String {
        format!("discovered {count} repositories")
    }
    pub fn log_scanning(&self, roots: usize) -> String {
        format!("scanning {roots} root(s)…")
    }
    pub fn log_rescan_queued(&self) -> String {
        "a scan is already running; rescan queued".to_string()
    }
    pub fn log_details_failed(&self, err: &str) -> String {
        format!("could not load repository details: {err}")
    }
    pub fn log_copied_path(&self, path: &str) -> String {
        format!("copied path: {path}")
    }
    pub fn log_external_not_configured(&self) -> String {
        "no external client configured (settings.json external_client)".to_string()
    }
    pub fn log_external_started(&self, program: &str) -> String {
        format!("opened externally with {program}")
    }
    pub fn log_external_failed(&self, err: &str) -> String {
        format!("external client failed: {err}")
    }
    pub fn log_auto_refresh(&self, enabled: bool) -> String {
        format!("auto-refresh {}", if enabled { "on" } else { "off" })
    }

    // ---- Workspace overview ----
    pub fn refresh_workspace(&self) -> &'static str {
        "Refresh workspace"
    }
    pub fn rescan_workspace(&self) -> &'static str {
        "Rescan workspace"
    }
    pub fn copy_path_action(&self) -> &'static str {
        "Copy repository path"
    }
    pub fn open_externally_action(&self) -> &'static str {
        "Open repository externally"
    }
    pub fn auto_refresh_action(&self, enabled: bool) -> String {
        format!("Auto-refresh: {}", if enabled { "on" } else { "off" })
    }
    /// Label of one alert-lifetime option (settings control and tooltip).
    pub fn alert_timeout_label(&self, mode: crate::settings::AlertTimeout) -> &'static str {
        match mode {
            crate::settings::AlertTimeout::ThreeSeconds => self.alert_timeout_3s,
            crate::settings::AlertTimeout::FiveSeconds => self.alert_timeout_5s,
            crate::settings::AlertTimeout::TenSeconds => self.alert_timeout_10s,
            crate::settings::AlertTimeout::Manual => self.alert_timeout_manual,
        }
    }
    pub fn filter_action(&self, current: &str) -> String {
        format!("Filter repositories: {current}")
    }
    pub fn sort_action(&self, current: &str) -> String {
        format!("Sort repositories: {current}")
    }
    pub fn stale_meta(&self) -> &'static str {
        "stale"
    }
    pub fn selected_meta(&self) -> &'static str {
        "selected"
    }
    pub fn no_status_collected(&self) -> &'static str {
        "No status collected yet"
    }
    pub fn palette_more_hidden(&self, hidden: usize) -> String {
        format!("{hidden} more repositories — refine the search")
    }
    pub fn exclude_action(&self) -> &'static str {
        "Exclude selected repositories"
    }
    pub fn clear_excludes_action(&self) -> &'static str {
        "Clear workspace excludes"
    }
    pub fn log_excluded(&self, count: usize) -> String {
        format!("excluded {count} path(s) from the workspace")
    }
    pub fn log_excludes_cleared(&self) -> String {
        "workspace excludes cleared".to_string()
    }
    pub fn exclude_count(&self, count: usize) -> String {
        format!("{count} excluded")
    }

    pub fn theme_applied(&self, name: &str) -> String {
        format!("Using \"{name}\"")
    }
    pub fn theme_saved(&self, name: &str) -> String {
        format!("Saved \"{name}\"")
    }
    pub fn theme_updated(&self, name: &str) -> String {
        format!("Updated \"{name}\"")
    }
    pub fn theme_deleted(&self, name: &str) -> String {
        format!("Deleted \"{name}\"")
    }
    /// Name auto-created when the built-in preset is edited in place.
    pub fn theme_auto_name(&self, preset: &str) -> String {
        format!("{preset} Custom")
    }
    pub fn themes_imported(&self, names: &str) -> String {
        if names.is_empty() {
            "No themes imported".to_string()
        } else {
            format!("Imported {names}")
        }
    }
    pub fn theme_reverted(&self) -> String {
        "Reverted to built-in colors".to_string()
    }
    pub fn settings_import_prompt(&self) -> &'static str {
        "Select theme JSON file(s)"
    }

    // ---- Title-bar remote actions: the tooltip names the scope ----

    pub fn fetch_scope_tip(&self, repo: &str) -> String {
        format!("Fetch remote changes for {repo}…")
    }
    pub fn push_scope_tip(&self, repo: &str) -> String {
        format!("Push the current branch of {repo} to its remote…")
    }
    pub fn pull_scope_tip(&self, repo: &str) -> String {
        format!("Pull the upstream into {repo}'s checked-out branch…")
    }
    pub fn pull_diverged_tip(&self, repo: &str) -> String {
        format!(
            "{repo} is ahead of and behind its upstream — a fast-forward pull is not possible; choose merge or rebase in the dialog"
        )
    }

    // ---- Repository tab status: what the marker cannot say ----

    pub fn tab_local_changes(&self, n: usize) -> String {
        if n == 1 {
            "1 local change".to_string()
        } else {
            format!("{n} local changes")
        }
    }
    pub fn tab_behind(&self, n: u32) -> String {
        format!("{n} behind upstream")
    }
    pub fn tab_ahead(&self, n: u32) -> String {
        format!("{n} ahead of upstream")
    }
    /// Tab tooltip: "3 local changes · 2 behind upstream". Empty when the
    /// tab has nothing to report, so no tooltip is shown at all.
    pub fn tab_status_tooltip(&self, local: Option<usize>, behind: u32, ahead: u32) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(n) = local {
            parts.push(self.tab_local_changes(n));
        }
        if behind > 0 {
            parts.push(self.tab_behind(behind));
        }
        if ahead > 0 {
            parts.push(self.tab_ahead(ahead));
        }
        parts.join(" · ")
    }
}

pub static ENGLISH: Strings = Strings {
    app_name: "spur",
    window_title: "Spur — Git workspace client",

    no_repository_open: "No repository open",
    press_ctrl_k: "Press Ctrl K to open a repository",
    open_repository: "Open repository…",
    first_run: "Room to branch. Add a folder and Spur lists the repositories inside it.",

    tooltip_refresh: "Re-scan and refresh status",
    pull_label: "Pull",
    push_label: "Push",
    push_title: "Push Branch",
    push_local: "Branch",
    push_remote: "Remote",
    push_target: "Target branch",
    push_confirm: "Push",
    push_no_remotes: "No remotes configured",
    fetch_title: "Fetch Remote Changes",
    fetch_label: "Fetch",
    fetch_force: "Force override local refs",
    fetch_all: "Fetch all remotes",
    fetch_no_tags: "Fetch without tags",
    fetch_confirm: "Fetch",
    pull_title: "Pull Remote Changes",
    pull_into: "Into checked-out branch",
    pull_branch: "Remote branch",
    pull_changes: "Local changes",
    pull_changes_nothing: "Do nothing",
    pull_changes_nothing_hint: "Pull over the local changes; Git refuses when they would be overwritten",
    pull_changes_stash: "Stash & reapply",
    pull_changes_stash_hint: "Stash all changes (untracked included), pull, then apply them back",
    pull_changes_discard: "Discard",
    pull_changes_discard_hint: "Move all changes into a stash and pull without reapplying",
    pull_rebase: "Use rebase instead of merge",
    pull_confirm: "Pull",

    tooltip_open_repo: "Open repository (Ctrl K)",

    search_placeholder: "Search repositories…",
    actions_section: "Actions",
    repositories_section: "Repositories",
    manage_roots: "Manage roots…",
    open_in_file_explorer: "Open in File Explorer",
    no_matches: "No matches",
    hint_navigate: "navigate",
    hint_open: "open",
    hint_close: "close",
    hint_add: "add",
    hint_back: "back",
    detached: "(detached)",
    collecting: "collecting…",
    nothing_to_open: "nothing to open",

    scan_roots: "Scan roots",
    roots_hint: "Repositories are discovered under these paths — Linux (WSL) and Windows paths are supported",
    no_roots: "No roots — add one below",
    browse: "Browse…",
    add: "Add",
    tooltip_browse: "Pick folders with the system dialog (\\\\wsl.localhost paths become Linux paths)",
    root_kind_wsl: "WSL",
    root_kind_windows: "Windows",

    section_history: "History",
    section_local_changes: "Local Changes",
    section_stashes: "Stashes",
    section_local_branches: "Local Branches",
    section_remotes: "Remotes",
    section_tags: "Tags",
    no_branches: "No branches",
    no_remotes: "No remotes",
    no_tags: "No tags",
    default_branch: "default",
    no_stashes: "No stashes",
    stash_select: "Select a stash to see its files",
    stash_error: "Could not read the stash",
    tooltip_create_branch: "Create branch…",
    create_branch: "Create Branch",
    branch_name: "Name",
    branch_name_placeholder: "new-branch-name",
    branch_base: "Based on",
    branch_changes: "What to do with the local changes",
    branch_changes_keep: "Keep Changes",
    branch_changes_keep_hint: "Changes stay in the working tree",
    branch_changes_stash: "Stash Changes",
    branch_changes_stash_hint: "Changes are stashed and reapplied after checkout",
    branch_changes_discard: "Discard Changes",
    branch_changes_discard_hint: "All changes will be discarded",
    branch_checkout: "Checkout after create",
    branch_overwrite: "Overwrite existing branch",
    branch_create: "Create",
    tooltip_add_remote: "Add remote…",
    add_remote: "Add Remote",
    remote_name: "Name",
    remote_name_placeholder: "origin",
    remote_url: "Repository URL",
    remote_url_placeholder: "git@github.com:owner/repo.git",
    remote_add: "Add",
    history_loading: "Reading history…",
    history_load_more: "Load more…",
    history_empty: "No commits",
    history_error: "Could not read history",
    history_search_placeholder: "Find branch…",
    history_searching: "searching…",
    history_search_no_match: "no match",
    retry: "Retry",
    commit_tab_information: "INFORMATION",
    commit_tab_changes: "CHANGES",
    commit_author: "AUTHOR",
    commit_sha: "SHA",
    commit_parents: "PARENTS",
    commit_message: "MESSAGE",
    commit_loading: "Reading commit…",
    commit_error: "Could not read the commit",
    commit_no_files: "No changed files",
    commit_diff_hint: "Double-click a file to see its diff",

    filter_changes_placeholder: "Filter changes…",
    section_unstaged: "UNSTAGED",
    section_staged: "STAGED",
    stage_all: "Stage all",
    unstage_all: "Unstage all",
    check_stage_file: "Stage this file",
    check_unstage_file: "Unstage this file",
    no_changes: "No changes",
    diff_select: "Select a file to see its diff",
    diff_error: "Could not read the diff",
    diff_binary: "Binary file — no text preview",
    diff_truncated: "Diff truncated",
    diff_staged: "staged",
    diff_unstaged: "unstaged",
    diff_untracked: "untracked",
    conflict_chip: "conflicted",
    conflict_ours: "Ours",
    conflict_theirs: "Theirs",
    conflict_both: "Both",
    conflict_apply: "Resolve & stage",
    conflict_loading: "Reading the conflicted file…",
    conflict_error: "Could not read the conflicted file",
    conflict_no_markers: "No conflict markers found in this file",
    context_reveal: "Reveal in File Explorer",
    context_resolve: "Resolve Conflicts…",
    context_checkout: "Checkout",
    context_open_in_browser: "Open in browser",
    context_stage: "Stage",
    context_unstage: "Unstage",
    context_discard: "Discard Changes",
    context_stash: "Stash Changes",
    context_copy_path: "Copy Path",
    context_copy_full_path: "Copy Full Path",
    context_checkout_detached: "Checkout (detached)…",
    context_create_branch_here: "Create branch here…",
    context_create_tag_here: "Create tag here…",
    context_revert_commit: "Revert commit",
    context_copy_sha: "Copy SHA",
    context_copy_short_sha: "Copy short SHA",
    context_copy_subject: "Copy subject",
    context_copy_patch: "Copy patch",
    context_rename_branch: "Rename…",
    context_delete_branch: "Delete…",
    context_pin_branch: "Pin to top",
    context_unpin_branch: "Unpin",
    branch_delete_title: "Delete branch?",
    branch_force_title: "Delete unmerged branch?",
    branch_delete_anyway: "Delete anyway",
    rename_title: "Rename branch",
    rename_name: "New name",
    rename_placeholder: "feature/new-name",
    rename_confirm: "Rename",
    branch_pinned: "Pinned",
    context_rebase_onto: "Rebase onto…",
    rebase_onto: "Onto",
    rebase_changes: "Local changes",
    rebase_autostash: "Stash local changes and reapply them afterwards",
    rebase_confirm: "Rebase",
    tag_title: "Create tag",
    context_push_tag: "Push to remote…",
    context_copy_tag_name: "Copy name",
    context_ignore_file: "Ignore this file",
    context_ignore_folder: "Ignore this folder",
    context_untrack_ignore: "Untrack and ignore this file",
    context_fetch_remote: "Fetch this remote",
    context_prune_remote: "Prune",
    context_edit_url: "Edit URL…",
    context_rename_remote: "Rename…",
    context_remove_remote: "Remove…",
    remote_edit_title: "Edit remote URL",
    remote_rename_title: "Rename remote",
    remote_save: "Save",
    remote_remove_title: "Remove remote?",
    remote_remove_confirm: "Remove",
    tag_push_title: "Push tag",
    tag_push_remote: "Remote",
    tag_push_confirm: "Push",
    tag_delete_title: "Delete tag?",
    tag_delete_local: "Local",
    tag_delete_confirm: "Delete",
    tag_filter_placeholder: "Filter tags…",
    tag_name: "Name",
    tag_name_placeholder: "v1.0.0",
    tag_message: "Message (annotated when not empty)",
    tag_create: "Create",
    tag_push_after: "Push to remote",
    stash_title: "Stash Changes",
    stash_drop_title: "Drop this stash?",
    context_apply: "Apply",
    context_drop: "Drop",
    context_pop: "Pop",
    context_stash_branch: "Create branch from stash…",
    context_apply_file: "Apply this file only",
    stash_branch_title: "Create branch from stash",
    stash_branch_name: "New branch",
    stash_branch_placeholder: "feature/from-stash",
    stash_branch_confirm: "Create and checkout",
    stash_mode: "Changes after stashing",
    stash_mode_discard: "Discard",
    stash_mode_discard_hint: "All changes will be discarded",
    stash_mode_keep_index: "Keep Index",
    stash_mode_keep_index_hint: "Staged changes are left intact",
    stash_mode_keep_all: "Keep All",
    stash_mode_keep_all_hint: "All changes are left intact",
    stash_description: "Description",
    stash_placeholder: "Stash description (optional)",
    stash_confirm: "Stash",
    apply_patch_action: "Apply patch from clipboard",
    apply_patch_title: "Apply patch",
    clipboard_empty: "Clipboard is empty",
    clipboard_not_patch: "Clipboard does not contain a patch",
    discard_title: "Discard changes?",
    discard_confirm: "Discard",
    commit_placeholder: "Subject, blank line, description",
    commit_subject_end: "SUBJECT END",
    commit_button: "Commit",
    commit_needs_staged: "Stage files to commit",
    commit_needs_message: "Write a commit message",
    col_resize_tooltip: "Drag to resize · double-click to reset",
    diff_wrap_tooltip: "Soft wrap long lines",

    cancel: "Cancel",
    collapse: "Collapse",
    expand: "Expand",

    oplog_title: "Operation log",
    oplog_empty: "No operations yet.",
    oplog_cancel: "Cancel",
    oplog_close: "Close operation log",
    oplog_details: "Details",
    oplog_action: "Operation log",
    oplog_truncated: "(output truncated)",

    shortcuts_title: "Keyboard shortcuts",
    shortcuts_group_general: "General",
    shortcuts_group_repository: "Repository",
    shortcuts_group_commit: "Commit message",
    shortcuts_group_palette: "Palette",
    shortcut_palette: "Command palette",
    shortcut_settings: "Settings",
    shortcut_shortcuts: "Keyboard shortcuts",
    shortcut_close: "Close dialog",
    shortcut_next_tab: "Next repository tab",
    shortcut_prev_tab: "Previous repository tab",
    shortcut_close_tab: "Close repository tab",
    shortcut_history: "Go to History",
    shortcut_changes: "Go to Local Changes",
    shortcut_stashes: "Go to Stashes",
    shortcut_commit: "Commit",
    shortcut_palette_next: "Next item",
    shortcut_palette_prev: "Previous item",
    shortcut_undo_last: "Undo last operation",
    shortcut_oplog: "Operation log",
    settings_keyboard: "Keyboard",
    settings_keyboard_hint: "Shortcut remapping",
    settings_tab_general: "General",
    settings_tab_shortcuts: "Shortcuts",
    keymap_unbound: "Not bound",

    settings_title: "Settings",
    settings_close: "Close settings",
    tooltip_settings: "Settings (Ctrl ,)",
    settings_palette: "Settings…",
    settings_appearance: "Appearance",
    settings_appearance_hint: "Choose how Spur looks. These settings stay on this device.",
    settings_theme: "Theme",
    settings_theme_hint: "Used across the whole app",
    settings_theme_menu: "Themes",
    settings_library: "Theme library",
    settings_library_hint: "Import a theme from a JSON file",
    settings_import: "Import theme",
    settings_reset: "Reset to built-in",
    settings_reset_hint: "Restore every color of this theme to its built-in values",
    settings_reset_action: "Reset",
    settings_reset_one: "Reset this color to built-in",
    settings_colors: "Colors",
    settings_colors_hint: "Fine-tune every surface and text role — edits save automatically.",
    settings_notifications: "Notifications",
    settings_notifications_hint: "How transient messages behave",
    settings_alerts: "Error alerts",
    settings_alerts_hint: "How long a failed or refused action stays visible",
    settings_accounts: "Accounts",
    settings_accounts_hint: "Commit identities and the GitHub login each repository uses. Logins stay in Git Credential Manager.",
    accounts_default: "Default profile",
    accounts_default_hint: "Used when a repository has no pick and no remote that matches a profile",
    accounts_none: "None (Git config)",
    accounts_add: "Add profile",
    accounts_add_hint: "A name and email for commits, and optionally a GitHub account",
    accounts_edit: "Edit",
    accounts_remove: "Remove",
    profile_title_add: "Add Profile",
    profile_title_edit: "Edit Profile",
    profile_label: "Label",
    profile_label_placeholder: "Work",
    profile_name: "Commit name",
    profile_name_placeholder: "Ada Lovelace",
    profile_email: "Commit email",
    profile_email_placeholder: "ada@example.com",
    profile_github: "GitHub account (optional)",
    profile_github_placeholder: "username",
    profile_hint: "Commits and pushes in repositories that use this profile use these details. Signing in is still done by Git Credential Manager.",
    profile_signed_in: "Signed in:",
    sidebar_account: "Account",
    profile_save: "Save",
    profile_issue_label: "Enter a label that no other profile uses",
    profile_issue_name: "Enter a name without < or >",
    profile_issue_email: "Enter a valid email address",
    profile_issue_github: "A GitHub username has letters, digits and hyphens only",
    commit_as_auto: "Automatic",
    commit_as_git_config: "Git config",
    commit_as_manage: "Manage profiles…",
    alert_timeout_3s: "3 seconds",
    alert_timeout_5s: "5 seconds",
    alert_timeout_10s: "10 seconds",
    alert_timeout_manual: "Click to remove",
    alert_dismiss: "Dismiss",
    settings_token_accent: "Accent",
    settings_token_accent_hint: "Buttons, links, and selection",
    settings_token_background: "Background",
    settings_token_background_hint: "Window and tables",
    settings_token_sidebar: "Sidebar",
    settings_token_sidebar_hint: "Repository sections panel",
    settings_token_popover: "Popover",
    settings_token_popover_hint: "Dialogs, menus, and the palette",
    settings_token_raised: "Raised surface",
    settings_token_raised_hint: "Inputs and hovered rows",
    settings_token_text: "Text",
    settings_token_text_hint: "Names and commit subjects",
    settings_token_muted_text: "Muted text",
    settings_token_muted_text_hint: "Paths, branches, and dates",
    settings_token_faint_text: "Faint text",
    settings_token_faint_text_hint: "Hashes, counts, and section labels",
    settings_token_danger: "Danger",
    settings_token_danger_hint: "Errors and conflicts",
    settings_token_warning: "Warning",
    settings_token_warning_hint: "Diverged and caution states",
    settings_token_success: "Success",
    settings_token_success_hint: "Remote refs and successful results",
    settings_save: "Save as theme",
    settings_save_hint: "Store the current colors under a new name",
    settings_save_action: "Save",
    settings_saving: "Saving…",
    theme_name_placeholder: "Theme name",

};

/// Current language. Extend with e.g. `if locale == "de" { &GERMAN }`.
/// "File Explorer" is "Finder" on macOS.
pub fn file_manager(text: &str) -> String {
    if cfg!(target_os = "macos") {
        text.replace("File Explorer", "Finder")
    } else {
        text.to_string()
    }
}

/// Shortcut hints in the strings are written for Windows ("Ctrl K"); macOS
/// shows its Cmd symbol instead.
pub fn native_keys(text: &str) -> String {
    if cfg!(target_os = "macos") {
        text.replace("Ctrl", "⌘")
    } else {
        text.to_string()
    }
}

pub fn t() -> &'static Strings {
    &ENGLISH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_tooltip_only_names_what_is_true() {
        let s = t();
        assert_eq!(
            s.tab_status_tooltip(Some(3), 2, 0),
            "3 local changes · 2 behind upstream"
        );
        assert_eq!(
            s.tab_status_tooltip(None, 0, 0),
            "",
            "a clean tab says nothing and gets no tooltip"
        );
        assert_eq!(
            s.tab_status_tooltip(Some(1), 0, 1),
            "1 local change · 1 ahead of upstream"
        );
        assert_eq!(
            s.tab_status_tooltip(None, 4, 2),
            "4 behind upstream · 2 ahead of upstream"
        );
    }
}
