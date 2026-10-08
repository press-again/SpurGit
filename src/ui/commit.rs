//! Commit box: commits exactly the staged snapshot. The message travels
//! on stdin, hooks and signing stay enabled, and a failure shows Git's output
//! with the message retained.

use super::*;

use gpui_kit::component::button::{Button, ButtonVariants as _, DropdownButton};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::assets::IconName;

use super::profile_dialog::ActiveProfile;
use gpui_kit::component::{ActiveTheme, Disableable as _, Sizable as _};
use gpui_kit::{Entity, IntoElement};

use crate::i18n::t;

/// SourceGit-style subject length indicator (informational, never blocking).
const SUBJECT_LIMIT: usize = 50;
/// SourceGit's subject guide: the 80-column mark inside the message editor.
const SUBJECT_GUIDE_COL: usize = 80;
/// The kit's multi-line editor lays the text out inside medium paddings
/// (`Size::Medium`); overlay guides add this inset so they line up with the
/// glyphs.
const EDITOR_PAD_LEFT: f32 = 10.0;

/// Byte range of the SourceGit-style subject: everything up to the first
/// blank line that follows the first non-blank line. The blank line and
/// everything after it are the description. The full text is what Git
/// receives — this is only used for the counter, the log, and the guides.
pub(super) fn commit_subject_range(message: &str) -> std::ops::Range<usize> {
    let start = message.len() - message.trim_start().len();
    let trimmed = &message[start..];
    let mut subject_end = trimmed.len();
    let mut line_start = 0;
    for line in trimmed.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']).trim().is_empty() {
            subject_end = line_start;
            break;
        }
        line_start += line.len();
    }
    let end = start + trimmed[..subject_end].trim_end().len();
    start..end
}

pub(super) fn commit_subject(message: &str) -> &str {
    &message[commit_subject_range(message)]
}

/// Description part of a commit message: everything after the subject,
/// trimmed. Empty for a subject-only message.
pub(super) fn commit_body(message: &str) -> &str {
    let range = commit_subject_range(message);
    message[range.end..].trim()
}

/// SourceGit's subject/description visualization: a dashed line at the end of
/// the subject block with a `SUBJECT END` label, plus the 80-column mark.
/// Returns `None` while the editor has no subject or has not laid out yet.
fn commit_guides(
    state: &Entity<TextareaState>,
    cx: &App,
) -> Option<impl IntoElement> {
    let editor = state.read(cx);
    if commit_subject_range(&editor.value()).is_empty() || editor.text_bounds().is_none() {
        return None;
    }
    let guide_color = hairline(0.14);
    let label_color = text_faint(cx);
    let state = state.clone();

    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(
                // 80-column mark for the subject.
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(EDITOR_PAD_LEFT + SUBJECT_GUIDE_COL as f32 * MONO_CHAR_W))
                    .w(px(1.))
                    .border_1()
                    .border_dashed()
                    .border_color(guide_color),
            )
            .child(
                // The rule paints after the editor in the same frame, so it
                // reads the layout being drawn instead of the previous one.
                gpui_kit::canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        paint_subject_rule(&state, bounds, guide_color, label_color, window, cx)
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            ),
    )
}

/// Paint the subject-end rule with its right-aligned label. The width comes
/// from the editor's own text bounds, so it tracks wrapped subjects.
fn paint_subject_rule(
    state: &Entity<TextareaState>,
    bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
    line_color: gpui_kit::Hsla,
    label_color: gpui_kit::Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    const LABEL_H: f32 = 12.0;
    let editor = state.read(cx);
    let message = editor.value();
    let range = commit_subject_range(&message);
    if range.is_empty() {
        return;
    }
    // The editor only lays out the lines in view; outside them
    // `range_to_bounds` snaps to the first visible line, so the rule is
    // skipped while the subject's last line is scrolled out.
    let end_line = message[..range.end].matches('\n').count();
    if !editor
        .visible_row_range()
        .is_some_and(|visible| visible.contains(&end_line))
    {
        return;
    }
    let (Some(text_bounds), Some(end)) = (editor.text_bounds(), editor.range_to_bounds(&range))
    else {
        return;
    };
    let rule = gpui_kit::Bounds::new(
        gpui_kit::point(bounds.left() + px(EDITOR_PAD_LEFT), end.bottom()),
        gpui_kit::size(text_bounds.size.width, px(LABEL_H)),
    );
    window.paint_quad(gpui_kit::quad(
        rule,
        px(0.),
        gpui_kit::transparent_black(),
        gpui_kit::Edges {
            top: px(1.),
            ..Default::default()
        },
        line_color,
        gpui_kit::BorderStyle::Dashed,
    ));
    let label: SharedString = t().commit_subject_end.into();
    let run = gpui_kit::TextRun {
        len: label.len(),
        font: gpui_kit::Font {
            style: gpui_kit::FontStyle::Italic,
            ..gpui_kit::font(MONO)
        },
        color: label_color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = window
        .text_system()
        .shape_line(label, px(10.), &[run], None);
    shaped
        .paint(
            rule.origin,
            px(LABEL_H),
            gpui_kit::TextAlign::Right,
            Some(rule.size.width),
            window,
            cx,
        )
        .ok();
}

impl SpurShell {
    pub(super) fn render_commit_box(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let message = self.commit_message.read(cx).value();
        let subject_len = commit_subject(&message).chars().count();
        let staged_empty = self.change_lists.staged.is_empty();
        let can_commit =
            !self.commit_in_flight && !staged_empty && !message.trim().is_empty();

        let mut panel = div()
            .flex_none()
            .min_w_0()
            .overflow_hidden()
            .border_t_1()
            .border_color(hairline(0.05))
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(6.));
        if let Some(err) = &self.commit_error {
            panel = panel.child(
                div()
                    .max_h(px(72.))
                    .overflow_hidden()
                    .rounded(px(CONTROL_RADIUS))
                    .bg(cx.theme().danger.alpha(0.10))
                    .px(px(8.))
                    .py(px(6.))
                    .font_family(MONO)
                    .text_size(px(TEXT_XS))
                    .text_color(cx.theme().danger)
                    .child(err.clone()),
            );
        }
        let submit = Button::new("commit-submit")
            .label(t().commit_button)
            .primary()
            .small()
            .flex_none()
            .rounded(px(999.))
            .cursor_pointer()
            .disabled(!can_commit)
            .on_click(cx.listener(|this, _, _, cx| this.submit_commit(cx)));
        // The tooltip explains the disabled state. It names the actual
        // blocker: nothing staged, then an empty message. A commit in flight
        // needs no explanation.
        let blocker = if self.commit_in_flight {
            None
        } else if staged_empty {
            Some(t().commit_needs_staged)
        } else if message.trim().is_empty() {
            Some(t().commit_needs_message)
        } else {
            None
        };
        let submit = match blocker {
            Some(reason) => submit.tooltip(reason),
            None => submit,
        };
        let repo_path = self.active_repo().map(|repo| repo.path.to_string_lossy().into_owned());
        let active_profile = repo_path.as_deref().and_then(|path| self.resolve_profile(path));
        let warning = active_profile.as_ref().and_then(|active| {
            active
                .other_owners
                .as_deref()
                .map(|owners| t().commit_as_warning(&active.profile.label, owners))
        });
        panel
            .child(
                div()
                    .rounded(px(CONTROL_RADIUS))
                    .border_1()
                    .border_color(hairline(0.10))
                    .bg(ink(0.03))
                    .px(px(6.))
                    .py(px(4.))
                    // The guides are positioned from the editor's own text
                    // bounds, so they share a relative box exactly the size
                    // of the textarea.
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .overflow_hidden()
                            .child(
                                Textarea::new(&self.commit_message)
                                    .appearance(false)
                                    .bordered(false)
                                    .font_family(MONO)
                                    .text_size(px(TEXT_SM))
                                    .h(px(88.)),
                            )
                            .children(commit_guides(&self.commit_message, cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .font_family(MONO)
                            .text_size(px(TEXT_XS))
                            .text_color(if subject_len > SUBJECT_LIMIT {
                                cx.theme().warning
                            } else {
                                text_faint(cx)
                            })
                            .child(t().commit_subject_count(subject_len, SUBJECT_LIMIT)),
                    )
                    .child(submit),
            )
            .children(warning.map(|line| {
                div()
                    .text_size(px(TEXT_XS))
                    .text_color(cx.theme().warning)
                    .child(line)
            }))
    }

    /// "Committing as" chip: shows the repository's profile and lets you pick
    /// another for this repository, or fall back to Git's own config.
    pub(super) fn account_chip(
        &self,
        repo_path: &str,
        active: Option<&ActiveProfile>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let pick = self.repo_profiles.get(repo_path).cloned();
        let shown = active
            .map(|active| active.profile.label.clone())
            .unwrap_or_else(|| t().commit_as_git_config.to_string());
        let tooltip = match active {
            Some(active) => format!(
                "{} · {}",
                t().commit_as(&active.profile.label),
                t().commit_as_source(active.source)
            ),
            None => t().commit_as_git_config.to_string(),
        };
        let labels: Vec<String> = self.profiles.iter().map(|profile| profile.label.clone()).collect();
        let shell = cx.entity().downgrade();
        let repo = repo_path.to_string();
        DropdownButton::new("commit-profile")
            .button(
                Button::new("commit-profile-btn")
                    .label(truncate_label(&shown, 24))
                    .ghost()
                    .xsmall()
                    .tooltip(tooltip),
            )
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                // `None` is the automatic choice, `Some("")` Git's own config.
                let mut options: Vec<(Option<String>, String)> =
                    vec![(None, t().commit_as_auto.to_string())];
                options.extend(labels.iter().map(|label| (Some(label.clone()), label.clone())));
                options.push((Some(String::new()), t().commit_as_git_config.to_string()));
                for (value, shown) in options {
                    let shell = shell.clone();
                    let repo = repo.clone();
                    let checked = value == pick;
                    menu = menu.item(
                        PopupMenuItem::element(move |_window, _cx| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .self_stretch()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .whitespace_nowrap()
                                .child(truncate_label(&shown, 32))
                        })
                        .checked(checked)
                        .on_click(move |_, _, cx| {
                            let value = value.clone();
                            let repo = repo.clone();
                            shell
                                .update(cx, |this, cx| this.pick_repo_profile(&repo, value, cx))
                                .ok();
                        }),
                    );
                }
                let manage = shell.clone();
                menu.separator().item(context_menu_item(
                    t().commit_as_manage.to_string(),
                    IconName::Settings,
                    move |_, window, cx| {
                        manage
                            .update(cx, |this, cx| this.open_settings(window, cx))
                            .ok();
                    },
                ))
            })
            .into_any_element()
    }

    /// Commit the staged snapshot. The in-flight flag plus Git's own empty-index
    /// failure make a double submit impossible; the message is only cleared
    /// after a confirmed success.
    pub(super) fn submit_commit(&mut self, cx: &mut Context<Self>) {
        if self.commit_in_flight {
            return;
        }
        let message = self.commit_message.read(cx).value().to_string();
        if message.trim().is_empty() || self.change_lists.staged.is_empty() {
            return;
        }
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        let Some(row) = self.overview.row(&repo_id) else {
            return;
        };
        let worktree = row.path.to_string_lossy().into_owned();
        self.apply_profile(&worktree);
        // The log line shows the whole subject (before the blank line) on one
        // line; Git still receives the full subject + description.
        let subject = commit_subject(&message).replace('\n', " ");
        self.commit_in_flight = true;
        self.commit_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let bytes = message.into_bytes();
            let result = cx
                .background_executor()
                .spawn(async move { crate::git::commit_staged(&worktree, &bytes) })
                .await;
            this.update(cx, |this, cx| {
                this.commit_in_flight = false;
                match result {
                    Ok(hash) => {
                        crate::logging::log!("commit: {repo_id}: {hash} {subject}");
                        this.ops.push_info(t().log_committed(&subject));
                        this.commit_error = None;
                        this.commit_clear_pending = true;
                        this.refresh_repo(repo_id.clone(), true, cx);
                    }
                    Err(err) => {
                        crate::logging::log!("commit: {repo_id}: failed: {err}");
                        // Hook output stays visible and the message is kept.
                        this.commit_error = Some(err);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{commit_body, commit_subject};

    #[test]
    fn commit_subject_stops_at_the_first_blank_line() {
        assert_eq!(commit_subject("Subject\n\nDescription"), "Subject");
        assert_eq!(
            commit_subject("Subject\nsecond line\n\nDescription\nmore"),
            "Subject\nsecond line"
        );
        assert_eq!(commit_subject("Only a subject"), "Only a subject");
        assert_eq!(commit_subject("\n\nSubject\n\nDescription"), "Subject");
        assert_eq!(commit_subject("  Subject  \n \nDescription"), "Subject");
        assert_eq!(commit_subject(""), "");
        // Leading blank lines are skipped: the first non-blank line starts
        // the subject, like SourceGit.
        assert_eq!(commit_subject("   \n\nFirst"), "First");
    }

    #[test]
    fn commit_body_is_what_follows_the_blank_line() {
        assert_eq!(commit_body("Subject\n\nDescription"), "Description");
        assert_eq!(
            commit_body("Subject\nsecond line\n\nDescription\nmore"),
            "Description\nmore"
        );
        assert_eq!(commit_body("Only a subject"), "");
        assert_eq!(commit_body("Subject\n\n"), "");
        assert_eq!(commit_body(""), "");
    }
}
