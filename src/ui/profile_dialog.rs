//! Account profiles: the add/edit dialog, which profile a repository uses,
//! and saving the choices. The profile list itself is shown in Settings and
//! the per-repository pick in the commit box.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _};
use gpui_kit::{IntoElement, SharedString};

use crate::accounts::{self, Profile, ProfileIssue, Source};
use crate::i18n::t;

use super::remote_dialog::{dialog_input, modal_card};

/// An open add/edit dialog. The fields live in the `profile_*_input` entities.
#[derive(Clone, Debug)]
pub(super) struct ProfileRequest {
    /// Label of the profile being edited; `None` while adding.
    pub editing: Option<String>,
}

/// The profile in effect for one repository and why.
pub(super) struct ActiveProfile {
    pub profile: Profile,
    pub source: Source,
    /// See [`accounts::Resolved::other_owners`].
    pub other_owners: Option<String>,
}

impl SpurShell {
    /// Profile for a repository: its pick, a matching remote, or the default.
    pub(super) fn resolve_profile(&self, repo_path: &str) -> Option<ActiveProfile> {
        let remote_urls: Vec<String> = self
            .details
            .get(repo_path)
            .map(|details| details.remotes.iter().map(|(_, url)| url.clone()).collect())
            .unwrap_or_default();
        let chosen = self.repo_profiles.get(repo_path).map(String::as_str);
        accounts::resolve(&self.profiles, self.default_profile.as_deref(), chosen, &remote_urls)
            .map(|resolved| ActiveProfile {
                profile: resolved.profile.clone(),
                source: resolved.source,
                other_owners: resolved.other_owners,
            })
    }

    /// Hand the repository's profile to the Git runner. Called right before
    /// an operation starts, so the choice is always the current one.
    pub(super) fn apply_profile(&self, worktree: &str) {
        let active = self.resolve_profile(worktree);
        accounts::set_for(worktree, active.as_ref().map(|active| &active.profile));
    }

    /// Pick a profile for one repository: `Some("")` is Git's own config,
    /// `None` goes back to the automatic choice.
    pub(super) fn pick_repo_profile(
        &mut self,
        repo_path: &str,
        pick: Option<String>,
        cx: &mut Context<Self>,
    ) {
        match pick {
            Some(label) => self.repo_profiles.insert(repo_path.to_string(), label),
            None => self.repo_profiles.remove(repo_path),
        };
        self.persist_accounts(cx);
    }

    pub(super) fn set_default_profile(&mut self, label: Option<String>, cx: &mut Context<Self>) {
        self.default_profile = label;
        self.persist_accounts(cx);
    }

    /// Remove a profile. Repositories that used it fall back to the
    /// automatic choice.
    pub(super) fn remove_profile(&mut self, label: &str, cx: &mut Context<Self>) {
        self.profiles.retain(|profile| profile.label != label);
        if self.default_profile.as_deref() == Some(label) {
            self.default_profile = None;
        }
        self.repo_profiles.retain(|_, picked| picked != label);
        self.persist_accounts(cx);
    }

    fn persist_accounts(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = crate::settings::set_accounts(
            &self.profiles,
            self.default_profile.as_deref(),
            &self.repo_profiles,
        ) {
            self.note_error(t().log_settings_save_failed(&err), cx);
        }
        cx.notify();
    }

    /// Open the dialog, prefilled when editing, and look up which GitHub
    /// accounts Git Credential Manager already has. A new profile is prefilled
    /// with Git's own `user.name` and `user.email`.
    pub(super) fn request_profile(
        &mut self,
        editing: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = editing
            .as_deref()
            .and_then(|label| self.profiles.iter().find(|profile| profile.label == label))
            .cloned();
        let (label, name, email, github) = match &current {
            Some(profile) => (
                profile.label.as_str(),
                profile.name.as_str(),
                profile.email.as_str(),
                profile.github.as_deref().unwrap_or(""),
            ),
            None => ("", "", "", ""),
        };
        for (input, value) in [
            (&self.profile_label_input, label),
            (&self.profile_name_input, name),
            (&self.profile_email_input, email),
            (&self.profile_github_input, github),
        ] {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        self.profile_request = Some(ProfileRequest { editing });
        self.open_modal(cx);
        let adding = current.is_none();
        cx.spawn_in(window, async move |this, cx| {
            let (stored, identity) = cx
                .background_executor()
                .spawn(async {
                    (accounts::stored_github_accounts(), accounts::git_identity())
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.stored_accounts = stored;
                // A new profile starts from Git's own identity, unless the
                // user already started typing.
                if adding && this.profile_request.is_some() {
                    for (input, value) in [
                        (&this.profile_name_input, identity.0),
                        (&this.profile_email_input, identity.1),
                    ] {
                        if let Some(value) = value
                            && input.read(cx).value().is_empty()
                        {
                            input.update(cx, |state, cx| state.set_value(value, window, cx));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn cancel_profile(&mut self, cx: &mut Context<Self>) {
        if self.profile_request.is_some() {
            self.close_modal(cx);
        }
    }

    fn profile_from_inputs(&self, cx: &App) -> Profile {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        Profile {
            label: text(&self.profile_label_input),
            name: text(&self.profile_name_input),
            email: text(&self.profile_email_input),
            github: Some(text(&self.profile_github_input)).filter(|login| !login.is_empty()),
        }
    }

    /// What stops the dialog's profile from being saved, if anything. A label
    /// another profile already uses counts as a label problem.
    fn profile_issue(&self, profile: &Profile, editing: Option<&str>) -> Option<ProfileIssue> {
        let taken = self
            .profiles
            .iter()
            .any(|known| known.label == profile.label && Some(known.label.as_str()) != editing);
        if taken {
            return Some(ProfileIssue::Label);
        }
        profile.validate().err()
    }

    pub(super) fn confirm_profile(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.profile_request.clone() else {
            return;
        };
        let profile = self.profile_from_inputs(cx);
        if self.profile_issue(&profile, request.editing.as_deref()).is_some() {
            return;
        }
        match request.editing {
            Some(old) => {
                if let Some(slot) = self.profiles.iter_mut().find(|known| known.label == old) {
                    *slot = profile.clone();
                }
                if old != profile.label {
                    if self.default_profile.as_deref() == Some(old.as_str()) {
                        self.default_profile = Some(profile.label.clone());
                    }
                    for picked in self.repo_profiles.values_mut().filter(|picked| **picked == old) {
                        picked.clone_from(&profile.label);
                    }
                }
            }
            None => self.profiles.push(profile),
        }
        self.persist_accounts(cx);
        self.close_modal(cx);
    }

    /// Footer at the bottom of the repository sidebar: which profile the
    /// open repository commits as. Clicking the name picks another one.
    /// Hidden until a profile exists.
    pub(super) fn sidebar_account(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        if self.profiles.is_empty() {
            return None;
        }
        let repo_path = self.active_repo()?.path.to_string_lossy().into_owned();
        let active = self.resolve_profile(&repo_path);
        let email = active.as_ref().map(|active| active.profile.email.clone());
        let chip = self.account_chip(&repo_path, active.as_ref(), cx);
        Some(
            div()
                .flex_none()
                .border_t_1()
                .border_color(hairline(0.05))
                .px(px(12.))
                .py(px(8.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(10.5))
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_color(text_faint(cx))
                        .child(t().sidebar_account),
                )
                .child(div().flex().items_center().ml(px(-5.)).child(chip))
                .children(email.map(|email| {
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(TEXT_XS))
                        .text_color(text_muted(cx))
                        .child(email)
                }))
                .into_any_element(),
        )
    }

    /// Full-window scrim + card for the add/edit dialog.
    pub(super) fn render_profile_dialog(
        &self,
        request: ProfileRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let profile = self.profile_from_inputs(cx);
        let issue = self.profile_issue(&profile, request.editing.as_deref());
        let touched = !(profile.label.is_empty()
            && profile.name.is_empty()
            && profile.email.is_empty()
            && profile.github.is_none());
        let body = match issue {
            Some(issue) if touched => match issue {
                ProfileIssue::Label => t().profile_issue_label,
                ProfileIssue::Name => t().profile_issue_name,
                ProfileIssue::Email => t().profile_issue_email,
                ProfileIssue::Github => t().profile_issue_github,
            },
            _ => t().profile_hint,
        };
        let title = if request.editing.is_some() {
            t().profile_title_edit
        } else {
            t().profile_title_add
        };

        let mut github_row = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(dialog_input(&self.profile_github_input));
        if !self.stored_accounts.is_empty() {
            github_row = github_row.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(TEXT_XS))
                            .text_color(text_faint(cx))
                            .child(t().profile_signed_in),
                    )
                    .children(self.stored_accounts.iter().enumerate().map(|(ix, login)| {
                        let id: SharedString = format!("profile-account-{ix}").into();
                        let login = login.clone();
                        Button::new(id)
                            .label(login.clone())
                            .ghost()
                            .xsmall()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let login = login.clone();
                                this.profile_github_input
                                    .update(cx, |state, cx| state.set_value(&login, window, cx));
                            }))
                    })),
            );
        }
        modal_card(
            cx,
            "profile",
            IconName::Cloud,
            violet(cx),
            title.to_string(),
            Some(body.to_string()),
            vec![
                (t().profile_label, dialog_input(&self.profile_label_input)),
                (t().profile_name, dialog_input(&self.profile_name_input)),
                (t().profile_email, dialog_input(&self.profile_email_input)),
                (t().profile_github, github_row.into_any_element()),
            ],
            t().cancel,
            Self::cancel_profile,
            t().profile_save,
            Self::confirm_profile,
            issue.is_none(),
            false,
        )
    }
}
