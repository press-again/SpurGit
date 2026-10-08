//! Settings page: appearance, colors, notifications, and keyboard. It follows
//! Zeron's Appearance layout — section title + description, grouped rows with an icon,
//! title and subtitle, and a theme dropdown with previews — and hosts the
//! theme store: select/import/delete themes and edit every exposed color with
//! the kit color picker. Color edits auto-save.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::button::DropdownButton;
use gpui_kit::component::color_picker::ColorPicker;
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, Selectable, Sizable as _};
use gpui_kit::StatefulInteractiveElement as _;
use gpui_kit::{App, ElementId, Hsla, InteractiveElement as _, KeyDownEvent, Rgba, Window};

use crate::i18n::t;
use crate::theme;
use crate::theme::ThemeConfig;

gpui_kit::actions!(spur, [OpenSettings]);

/// Settings page tab: the existing appearance/colors/notifications page or
/// the interactive shortcut editor.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsTab {
    General,
    Shortcuts,
}

impl SpurShell {
    pub(super) fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            self.close_settings(cx);
        } else {
            self.open_settings(window, cx);
        }
    }

    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Shortcut/gear path: an open palette or roots dialog yields instantly
        // so the page underneath is visible.
        self.palette_open = false;
        self.palette_closing = false;
        self.roots_open = false;
        self.show_settings(window, cx);
    }

    /// Reveal the settings page without touching the dialog overlay (the
    /// palette action closes the palette with its exit fade on top).
    pub(super) fn show_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = true;
        self.theme_menu_open = false;
        self.theme_status = None;
        self.theme_save_gen = self.theme_save_gen.wrapping_add(1);
        self.sync_theme_draft(window, cx);
        crate::logging::log!("settings: open");
        cx.notify();
    }

    /// Leaving the page flushes a pending auto-save immediately.
    pub(super) fn close_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_open {
            if self.theme_dirty {
                self.theme_save_gen = self.theme_save_gen.wrapping_add(1);
                self.persist_draft(cx);
            }
            self.settings_open = false;
            self.theme_menu_open = false;
            // A capture cannot continue off-page; the saved status survives
            // for the Keyboard card.
            self.shortcut_capture = None;
            crate::logging::log!("settings: close");
            cx.notify();
        }
    }

    /// Load the editor from the selected theme: the draft config plus every
    /// picker swatch. `set_value` does not emit, so no preview fires here.
    pub(super) fn sync_theme_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = theme::selected(cx);
        self.theme_draft = theme::config(cx, &selected)
            .or_else(|| theme::config(cx, theme::PRESET_NAME))
            .expect("the built-in preset always exists");
        self.theme_dirty = false;
        for (token, picker) in &self.theme_pickers {
            let color = hsla_of(&theme::token_hex(&self.theme_draft, *token));
            picker.update(cx, |state, cx| state.set_value(color, window, cx));
        }
        self.theme_name_input
            .update(cx, |state, cx| state.set_value("", window, cx));
    }

    /// One picker committed a color: write it into the draft, preview, and
    /// schedule the auto-save (debounced so a slider drag writes once).
    pub(super) fn apply_theme_token(&mut self, token: theme::Token, color: Hsla, cx: &mut Context<Self>) {
        match theme::set_token(&mut self.theme_draft, token, &hex_of(color)) {
            Ok(()) => {
                self.theme_dirty = true;
                theme::preview_config(cx, &self.theme_draft);
                self.theme_status = None;
                self.schedule_auto_save(cx);
            }
            Err(err) => self.theme_status = Some((err, true)),
        }
        cx.notify();
    }

    /// Same as [`apply_theme_token`](Self::apply_theme_token) for a hex string.
    pub(super) fn edit_theme_token(&mut self, token: theme::Token, hex: &str, cx: &mut Context<Self>) {
        self.apply_theme_token(token, hsla_of(hex), cx);
    }

    /// Coalesce rapid edits: only the last scheduled save within the window
    /// runs, and only while the draft is still the current selection.
    fn schedule_auto_save(&mut self, cx: &mut Context<Self>) {
        self.theme_save_gen = self.theme_save_gen.wrapping_add(1);
        let generation = self.theme_save_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            this.update(cx, |this, cx| {
                if this.theme_save_gen == generation {
                    this.persist_draft(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Write the draft to disk under the selected theme. Editing a built-in
    /// auto-creates a custom copy the first time.
    fn persist_draft(&mut self, cx: &mut Context<Self>) {
        let selected = theme::selected(cx);
        let name = if theme::is_builtin(cx, &selected) {
            theme::unique_name(cx, &t().theme_auto_name(&selected))
        } else {
            selected
        };
        self.theme_draft.name = name.as_str().into();
        match theme::save(cx, self.theme_draft.clone()) {
            Ok(()) => {
                self.theme_dirty = false;
                self.theme_status = Some((t().theme_saved(&name), false));
                crate::logging::log!("theme: saved {name}");
            }
            Err(err) => self.theme_status = Some((err, true)),
        }
        cx.notify();
    }

    fn select_theme(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        // Leaving the current theme flushes its pending auto-save first, so
        // edits are never lost by switching.
        if self.theme_dirty {
            self.persist_draft(cx);
        }
        self.theme_save_gen = self.theme_save_gen.wrapping_add(1);
        if theme::apply(cx, &name) {
            self.sync_theme_draft(window, cx);
            self.theme_status = Some((t().theme_applied(&name), false));
            crate::logging::log!("theme: applied {name}");
        }
        self.theme_menu_open = false;
        cx.notify();
    }

    fn delete_theme(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.theme_save_gen = self.theme_save_gen.wrapping_add(1);
        match theme::delete(cx, &name) {
            Ok(()) => {
                self.sync_theme_draft(window, cx);
                self.theme_status = Some((t().theme_deleted(&name), false));
                crate::logging::log!("theme: deleted {name}");
            }
            Err(err) => self.theme_status = Some((err, true)),
        }
        cx.notify();
    }

    /// Import theme JSON files picked in the native dialog into the library.
    fn import_themes(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t().settings_import_prompt().into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |this, cx| {
                    let mut imported = Vec::new();
                    let mut error = None;
                    for path in paths {
                        match theme::import(cx, &path) {
                            Ok(name) => imported.push(name),
                            Err(err) => {
                                error = Some(format!(
                                    "{}: {err}",
                                    path.file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_default()
                                ));
                                break;
                            }
                        }
                    }
                    this.theme_status = Some(match error {
                        Some(err) => (err, true),
                        None => (t().themes_imported(&imported.join(", ")), false),
                    });
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub(super) fn save_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.theme_save_gen = self.theme_save_gen.wrapping_add(1);
        let name = self.theme_name_input.read(cx).value().trim().to_string();
        let updating = theme::config(cx, &name).is_some();
        self.theme_draft.name = name.as_str().into();
        match theme::save(cx, self.theme_draft.clone()) {
            Ok(()) => {
                self.sync_theme_draft(window, cx);
                self.theme_status = Some((
                    if updating {
                        t().theme_updated(&name)
                    } else {
                        t().theme_saved(&name)
                    },
                    false,
                ));
            }
            Err(err) => self.theme_status = Some((err, true)),
        }
        cx.notify();
    }

    /// Revert every color of the current theme to the built-in preset values
    /// (auto-saved like any other edit).
    fn reset_theme(&mut self, cx: &mut Context<Self>) {
        let preset = theme::baseline(cx);
        for token in theme::TOKENS {
            let hex = theme::token_hex(&preset, token);
            let _ = theme::set_token(&mut self.theme_draft, token, &hex);
        }
        self.theme_dirty = true;
        theme::preview_config(cx, &self.theme_draft);
        self.theme_status = Some((t().theme_reverted(), false));
        self.schedule_auto_save(cx);
        cx.notify();
    }

    // ---- rendering ----

    pub(super) fn render_settings(&self, cx: &Context<Self>) -> impl IntoElement {

        div()
            .id("settings")
            .flex()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                // Capture runs in a keystroke interceptor (bindings fire
                // before key-down listeners, so it could not happen here).
                if this.shortcut_capture.is_some() {
                    return;
                }
                if ev.keystroke.key.as_str() == "escape" {
                    if this.theme_menu_open {
                        this.theme_menu_open = false;
                        cx.notify();
                    } else {
                        this.close_settings(cx);
                    }
                }
            }))
            .child(self.settings_header(cx))
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_focus(&self.shortcut_focus)
                    .track_scroll(&self.settings_scroll)
                    .pt(px(24.))
                    .pb(px(24.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .w(px(680.))
                            .px(px(20.))
                            .flex()
                            .flex_col()
                            .gap(px(22.))
                            .children(self.settings_body(cx)),
                    ),
            )
    }

    /// Tab content: the existing page under General, the interactive
    /// shortcut editor under Shortcuts.
    fn settings_body(&self, cx: &Context<Self>) -> Vec<gpui_kit::AnyElement> {
        match self.settings_tab {
            SettingsTab::General => {
                let selected = theme::selected(cx);
                let selected_config = theme::config(cx, &selected).unwrap_or_else(|| {
                    theme::config(cx, theme::PRESET_NAME).expect("preset exists")
                });
                let names = theme::names(cx);
                vec![
                    section_heading(
                        t().settings_appearance,
                        t().settings_appearance_hint,
                        cx,
                    )
                    .into_any_element(),
                    self.appearance_card(&selected_config, &names, cx),
                    self.status_line(cx),
                    section_heading(t().settings_colors, t().settings_colors_hint, cx)
                        .into_any_element(),
                    self.colors_card(cx),
                    section_heading(t().settings_accounts, t().settings_accounts_hint, cx)
                        .into_any_element(),
                    self.accounts_card(cx),
                    section_heading(
                        t().settings_notifications,
                        t().settings_notifications_hint,
                        cx,
                    )
                    .into_any_element(),
                    self.notifications_card(cx),
                    section_heading(t().settings_updates, t().settings_updates_hint, cx)
                        .into_any_element(),
                    self.updates_card(cx),
                    section_heading(t().settings_keyboard, t().settings_keyboard_hint, cx)
                        .into_any_element(),
                    self.keyboard_card(cx),
                ]
            }
            SettingsTab::Shortcuts => self.shortcuts_tab(cx),
        }
    }

    /// General/Shortcuts tab switch in the Settings header.
    fn settings_tab_button(
        &self,
        tab: SettingsTab,
        id: &'static str,
        label: &'static str,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let active = self.settings_tab == tab;
        div()
            .id(id)
            .cursor_pointer()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(if active { ink(0.08) } else { ink(0.0) })
            .text_size(px(TEXT_SM))
            .text_color(if active {
                text_primary(cx)
            } else {
                text_muted(cx)
            })
            .child(label.to_string())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.settings_tab = tab;
                cx.notify();
            }))
    }

    fn settings_header(&self, cx: &Context<Self>) -> impl IntoElement {        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(HEADER_H))
            .px(px(12.))
            .border_b_1()
            .border_color(hairline(0.05))
            .child(
                div()
                    .text_size(px(TEXT_MD))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(text_primary(cx))
                    .child(t().settings_title),
            )
            .child(self.settings_tab_button(
                SettingsTab::General,
                "settings-tab-general",
                t().settings_tab_general,
                cx,
            ))
            .child(self.settings_tab_button(
                SettingsTab::Shortcuts,
                "settings-tab-shortcuts",
                t().settings_tab_shortcuts,
                cx,
            ))
            .child(div().flex_1())
            .child(kbd_chip("esc", cx))
            .child(icon_button(
                "settings-close",
                IconName::Close,
                t().settings_close,
                cx.listener(|this, _, _, cx| this.close_settings(cx)),
            ))
    }

    fn status_line(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let (text, color) = if let Some((text, error)) = &self.theme_status {
            (
                text.clone(),
                if *error {
                    cx.theme().danger
                } else {
                    cx.theme().success
                },
            )
        } else if self.theme_dirty {
            (t().settings_saving.to_string(), violet(cx))
        } else {
            (String::new(), text_faint(cx))
        };
        div()
            .min_h(px(16.))
            .px(px(2.))
            .text_size(px(TEXT_XS))
            .text_color(color)
            .child(text)
            .into_any_element()
    }

    /// Zeron's Appearance card: theme dropdown, theme library import, save-as.
    fn appearance_card(
        &self,
        selected_config: &ThemeConfig,
        names: &[String],
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        let trigger = ThemePickerTrigger {
            id: "theme-picker-trigger".into(),
            config: selected_config.clone(),
            selected: self.theme_menu_open,
        };
        let menu = self.theme_menu(names, selected_config, cx);
        let picker = Popover::new("theme-picker")
            .open(self.theme_menu_open)
            .appearance(false)
            .anchor(gpui_kit::Anchor::TopRight)
            .trigger(trigger)
            .on_open_change({
                let this = cx.entity().downgrade();
                move |open, _, cx| {
                    this.update(cx, |this, cx| {
                        this.theme_menu_open = *open;
                        cx.notify();
                    })
                    .ok();
                }
            })
            .child(menu);

        let preset = theme::baseline(cx);
        let all_builtin = theme::TOKENS.iter().all(|token| {
            theme::token_hex(&self.theme_draft, *token) == theme::token_hex(&preset, *token)
        });

        settings_card(
            vec![
                picker.into_any_element(),
                settings_separator(),
                settings_row(
                    IconName::Download,
                    t().settings_library,
                    t().settings_library_hint,
                    Button::new("settings-import")
                        .label(t().settings_import)
                        .icon(Icon::new(IconName::Download).size(px(14.)))
                        .ghost()
                        .small()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.import_themes(cx)))
                        .into_any_element(),
                    cx,
                ),
                settings_separator(),
                settings_row(
                    IconName::RotateCcw,
                    t().settings_reset,
                    t().settings_reset_hint,
                    Button::new("settings-reset")
                        .label(t().settings_reset_action)
                        .icon(Icon::new(IconName::RotateCcw).size(px(14.)))
                        .ghost()
                        .small()
                        .disabled(all_builtin)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.reset_theme(cx)))
                        .into_any_element(),
                    cx,
                ),
                settings_separator(),
                settings_row(
                    IconName::Plus,
                    t().settings_save,
                    t().settings_save_hint,
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .w(px(200.))
                                .child(Input::new(&self.theme_name_input).small().w_full()),
                        )
                        .child(
                            Button::new("settings-save-theme")
                                .label(t().settings_save_action)
                                .primary()
                                .small()
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_theme(window, cx)
                                })),
                        )
                        .into_any_element(),
                    cx,
                ),
            ],
        )
    }

    /// The dropdown panel: theme rows with a two-tone preview, a check for the
    /// selected one, and a delete affordance for saved themes only.
    fn theme_menu(
        &self,
        names: &[String],
        selected_config: &ThemeConfig,
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        let selected = theme::selected(cx);
        let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
        for (ix, name) in names.iter().enumerate() {
            let Some(config) = theme::config(cx, name) else {
                continue;
            };
            let is_selected = *name == selected;
            let is_custom = !theme::is_builtin(cx, name);
            let row_key = format!("settings-theme-{ix}");
            let click_name = name.clone();
            let mut row = div()
                .id(("settings-theme", ix))
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(38.))
                .px(px(8.))
                .rounded(px(CONTROL_RADIUS))
                .bg(hover_blend(&row_key, ink(0.0), ink(0.06)))
                .on_hover(hover_listener(row_key))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_theme(click_name.clone(), window, cx)
                }))
                .child(theme_preview(&config))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(TEXT_MD))
                        .text_color(if is_selected {
                            text_primary(cx)
                        } else {
                            text_muted(cx)
                        })
                        .child(name.clone()),
                );
            if is_selected {
                row = row.child(
                    Icon::new(IconName::Check)
                        .size(px(14.))
                        .flex_none()
                        .text_color(violet(cx)),
                );
            }
            if is_custom {
                let delete_key = format!("settings-theme-delete-{ix}");
                let delete_name = name.clone();
                row = row.child(
                    div()
                        .id(("settings-theme-delete", ix))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(22.))
                        .h(px(22.))
                        .rounded(px(5.))
                        .bg(hover_blend(&delete_key, ink(0.0), ink(0.10)))
                        .hover(|s| s.bg(cx.theme().danger.alpha(0.18)))
                        .on_hover(hover_listener(delete_key))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.delete_theme(delete_name.clone(), window, cx);
                        }))
                        .child(
                            Icon::new(IconName::Trash)
                                .size(px(13.))
                                .text_color(text_muted(cx)),
                        ),
                );
            }
            rows.push(row.into_any_element());
        }

        let _ = selected_config;
        div()
            .w(px(300.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .p(px(4.))
            .rounded(px(PANEL_RADIUS))
            .border_1()
            .border_color(hairline(0.10))
            .bg(cx.theme().popover)
            .shadow_lg()
            .child(
                div()
                    .px(px(8.))
                    .pt(px(6.))
                    .pb(px(4.))
                    .text_size(px(10.5))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(text_faint(cx))
                    .child(t().settings_theme_menu),
            )
            .children(rows)
            .into_any_element()
    }

    /// One picker per exposed color token; the hex readout shows the current
    /// value, and the picker owns palette/HSLA/hex editing.
    fn colors_card(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let preset = theme::baseline(cx);
        let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
        for (ix, (token, picker)) in self.theme_pickers.iter().enumerate() {
            if ix > 0 {
                rows.push(settings_separator());
            }
            let current = theme::token_hex(&self.theme_draft, *token);
            let builtin = theme::token_hex(&preset, *token);
            let changed = current != builtin;
            // Keep the slot reserved so rows do not shift when the reset
            // affordance appears.
            let reset: gpui_kit::AnyElement = if changed {
                let token = *token;
                Button::new(("settings-token-reset", ix))
                    .icon(Icon::new(IconName::RotateCcw).size(px(12.)))
                    .ghost()
                    .xsmall()
                    .tooltip(t().settings_reset_one)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit_theme_token(token, &builtin, cx)
                    }))
                    .into_any_element()
            } else {
                div().w(px(20.)).into_any_element()
            };
            rows.push(settings_row(
                token_icon(*token),
                token_label(*token),
                token_hint(*token),
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(6.))
                    .child(reset)
                    .child(ColorPicker::new(picker).small())
                    .child(
                        div()
                            .w(px(68.))
                            .font_family(MONO)
                            .text_size(px(TEXT_XS))
                            .text_color(text_faint(cx))
                            .child(current),
                    )
                    .into_any_element(),
                cx,
            ));
        }
        settings_card(rows)
    }

    /// Accounts card: one row per profile, the default profile, and add.
    fn accounts_card(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
        for (ix, profile) in self.profiles.iter().enumerate() {
            let edit_label = profile.label.clone();
            let remove_label = profile.label.clone();
            let controls = div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(
                    Button::new(SharedString::from(format!("profile-edit-{ix}")))
                        .label(t().accounts_edit)
                        .ghost()
                        .small()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.request_profile(Some(edit_label.clone()), window, cx)
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("profile-remove-{ix}")))
                        .label(t().accounts_remove)
                        .ghost()
                        .small()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remove_profile(&remove_label, cx)
                        })),
                );
            rows.push(settings_row(
                IconName::Cloud,
                &profile.label,
                &t().accounts_summary(&profile.name, &profile.email, profile.github.as_deref()),
                controls.into_any_element(),
                cx,
            ));
            rows.push(settings_separator());
        }
        if !self.profiles.is_empty() {
            let current = self.default_profile.clone();
            let labels: Vec<String> = self.profiles.iter().map(|p| p.label.clone()).collect();
            let shell = cx.entity().downgrade();
            let menu = DropdownButton::new("default-profile")
                .button(
                    Button::new("default-profile-btn")
                        .label(current.clone().unwrap_or_else(|| t().accounts_none.to_string()))
                        .ghost()
                        .small()
                        .cursor_pointer(),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    let options = std::iter::once(None).chain(labels.iter().cloned().map(Some));
                    for option in options {
                        let shell = shell.clone();
                        let shown = option.clone().unwrap_or_else(|| t().accounts_none.to_string());
                        let checked = option == current;
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
                                    .child(shown.clone())
                            })
                            .checked(checked)
                            .on_click(move |_, _, cx| {
                                let option = option.clone();
                                shell
                                    .update(cx, |this, cx| this.set_default_profile(option, cx))
                                    .ok();
                            }),
                        );
                    }
                    menu
                });
            rows.push(settings_row(
                IconName::Settings,
                t().accounts_default,
                t().accounts_default_hint,
                menu.into_any_element(),
                cx,
            ));
            rows.push(settings_separator());
        }
        rows.push(settings_row(
            IconName::Plus,
            t().accounts_add,
            t().accounts_add_hint,
            Button::new("profile-add")
                .label(t().profile_title_add)
                .small()
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.request_profile(None, window, cx)))
                .into_any_element(),
            cx,
        ));
        settings_card(rows)
    }

    /// Notifications card: how long the bottom-left error alert stays.
    fn notifications_card(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let current = self.alert_timeout;
        let shell = cx.entity().downgrade();
        let menu = DropdownButton::new("alert-timeout")
            .button(
                Button::new("alert-timeout-btn")
                    .label(t().alert_timeout_label(current))
                    .ghost()
                    .small()
                    .cursor_pointer(),
            )
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for mode in crate::settings::AlertTimeout::ALL {
                    let shell = shell.clone();
                    let label = t().alert_timeout_label(mode);
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
                                .child(label)
                        })
                        .checked(mode == current)
                        .on_click(move |_, _, cx| {
                            shell
                                .update(cx, |this, cx| this.set_alert_timeout(mode, cx))
                                .ok();
                        }),
                    );
                }
                menu
            });
        settings_card(vec![settings_row(
            IconName::Bell,
            t().settings_alerts,
            t().settings_alerts_hint,
            menu.into_any_element(),
            cx,
        )])
    }

    /// Updates card: current version, and the install button once the
    /// background check found a newer release.
    fn updates_card(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let subtitle = match &self.update {
            Some(release) => t().update_available(&release.version),
            None => t().update_none.to_string(),
        };
        let control = match self.update {
            Some(_) => Button::new("install-update")
                .label(t().update_install)
                .small()
                .cursor_pointer()
                .loading(self.updating)
                .on_click(cx.listener(|this, _, _, cx| this.install_update(cx)))
                .into_any_element(),
            None => div().into_any_element(),
        };
        settings_card(vec![settings_row(
            IconName::Download,
            &t().update_current(env!("CARGO_PKG_VERSION")),
            &subtitle,
            control,
            cx,
        )])
    }

    /// Persist the alert-lifetime choice. Changing it closes any visible alert
    /// so a manual one cannot outlive its close button.
    fn set_alert_timeout(&mut self, mode: crate::settings::AlertTimeout, cx: &mut Context<Self>) {
        self.alert_timeout = mode;
        self.dismiss_alert(cx);
        if let Err(err) = crate::settings::set_alert_timeout(mode) {
            self.note_error(t().log_settings_save_failed(&err), cx);
        }
        cx.notify();
    }

    /// Keyboard card: remap status from `keymap.json` plus a shortcut
    /// into the cheat sheet. Load problems (unknown actions, bad
    /// keystrokes, collisions) surface here instead of hiding in the log.
    fn keyboard_card(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let diagnostics = shortcuts::diagnostics(cx);
        let customized = shortcuts::customized(cx);
        let subtitle = if let Some(first) = diagnostics.first() {
            first.clone()
        } else if customized > 0 {
            t().keymap_custom(customized)
        } else {
            t().keymap_default()
        };
        let control = Button::new("keyboard-shortcuts")
            .label(t().settings_tab_shortcuts)
            .ghost()
            .small()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.settings_tab = SettingsTab::Shortcuts;
                cx.notify();
            }));
        settings_card(vec![settings_row(
            IconName::Keyboard,
            t().settings_keyboard,
            &subtitle,
            control.into_any_element(),
            cx,
        )])
    }

    /// Shortcuts tab: every action grouped by scope, its live binding
    /// as a capture button, and a Reset button for remapped rows. Clicking a
    /// binding listens for the next key press (Escape cancels).
    fn shortcuts_tab(&self, cx: &Context<Self>) -> Vec<gpui_kit::AnyElement> {
        let mut out = vec![self.shortcut_status_line(cx)];
        let bindings = shortcuts::bindings(cx);
        for group in shortcuts::ShortcutGroup::ORDER {
            let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
            let mut actions = 0usize;
            for (ix, binding) in bindings.iter().enumerate() {
                let Some(entry) = shortcuts::def(&binding.action) else {
                    continue;
                };
                if entry.group != group {
                    continue;
                }
                if actions > 0 {
                    rows.push(settings_separator());
                }
                actions += 1;
                rows.push(self.shortcut_row(ix, &binding.action, &binding.keys, cx));
            }
            if actions == 0 {
                continue;
            }
            out.push(
                section_heading(group.label(), "", cx).into_any_element(),
            );
            out.push(settings_card(rows));
        }
        out
    }

    /// Capture/save status line above the editor, mirroring the icons card.
    fn shortcut_status_line(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let (text, error) = self.shortcut_status.clone().unwrap_or_default();
        div()
            .min_h(px(18.))
            .px(px(2.))
            .text_size(px(TEXT_XS))
            .text_color(if error {
                cx.theme().danger
            } else {
                text_muted(cx)
            })
            .child(text)
            .into_any_element()
    }

    /// One editor row: the action label, its binding as a capture button,
    /// and Reset when the row differs from the compiled default.
    fn shortcut_row(
        &self,
        ix: usize,
        action: &str,
        keys: &[String],
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        let capturing = self.shortcut_capture.as_deref() == Some(action);
        let display = if capturing {
            t().shortcut_capture_hint()
        } else {
            shortcuts::display_for(action, keys)
        };
        let action_owned = action.to_string();
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .py(px(6.))
            .px(px(10.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(TEXT_MD))
                    .text_color(text_primary(cx))
                    .child(
                        shortcuts::def(action)
                            .map(|entry| (entry.label)(t()).to_string())
                            .unwrap_or(action_owned.clone()),
                    ),
            )
            .child(
                div()
                    .id(("shortcut-cap", ix))
                    .cursor_pointer()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .min_w(px(64.))
                    .h(px(22.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(if capturing {
                        violet(cx)
                    } else {
                        hairline(0.10)
                    })
                    .bg(ink(0.05))
                    .px(px(6.))
                    .font_family(MONO)
                    .text_size(px(10.5))
                    .text_color(if capturing {
                        text_primary(cx)
                    } else {
                        text_muted(cx)
                    })
                    .child(display)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_capture(action_owned.clone(), window, cx)
                    })),
            );
        if shortcuts::is_customized(action, keys) {
            let reset_action = action.to_string();
            row = row.child(
                Button::new(("shortcut-reset", ix))
                    .label(t().settings_reset_action)
                    .text()
                    .xsmall()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.reset_binding(reset_action.clone(), cx)
                    })),
            );
        }
        row.into_any_element()
    }
}

// ---- picker trigger ----

/// The theme dropdown trigger: the whole Appearance "Theme" row. `Popover`
/// calls [`Selectable::selected`] with the open state, which highlights the
/// chip like Zeron's open trigger.
#[derive(IntoElement)]
struct ThemePickerTrigger {
    id: ElementId,
    config: ThemeConfig,
    selected: bool,
}

impl Selectable for ThemePickerTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl gpui_kit::RenderOnce for ThemePickerTrigger {
    fn render(self, _: &mut Window, cx: &mut gpui_kit::App) -> impl IntoElement {
        let key = "settings-theme-trigger";
        div()
            .id(self.id)
            .flex()
            .items_center()
            .gap(px(12.))
            .min_h(px(64.))
            .mx(px(4.))
            .px(px(10.))
            .rounded(px(CONTROL_RADIUS))
            .bg(hover_blend(key, ink(0.0), ink(0.04)))
            .on_hover(hover_listener(key))
            .child(row_icon(IconName::Palette, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .text_size(px(TEXT_MD))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_primary(cx))
                            .child(t().settings_theme),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(text_muted(cx))
                            .child(t().settings_theme_hint),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .bg(ink(0.05))
                    .border_1()
                    .border_color(if self.selected {
                        violet(cx)
                    } else {
                        hairline(0.10)
                    })
                    .child(theme_preview(&self.config))
                    .child(
                        div()
                            .max_w(px(140.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(TEXT_MD))
                            .text_color(text_primary(cx))
                            .child(self.config.name.clone()),
                    )
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(px(12.))
                            .text_color(text_muted(cx)),
                    ),
            )
    }
}

// ---- page building blocks ----

fn section_heading(title: &str, hint: &str, cx: &Context<SpurShell>) -> gpui_kit::AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .text_size(px(17.))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(text_primary(cx))
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(TEXT_MD))
                .text_color(text_muted(cx))
                .child(hint.to_string()),
        )
        .into_any_element()
}

fn settings_card(rows: Vec<gpui_kit::AnyElement>) -> gpui_kit::AnyElement {
    div()
        .rounded(px(PANEL_RADIUS))
        .border_1()
        .border_color(hairline(0.06))
        .bg(ink(0.02))
        .flex()
        .flex_col()
        .overflow_hidden()
        .children(rows)
        .into_any_element()
}

fn settings_separator() -> gpui_kit::AnyElement {
    div().h(px(1.)).mx(px(4.)).bg(hairline(0.04)).into_any_element()
}

fn row_icon(icon: IconName, cx: &App) -> gpui_kit::AnyElement {
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(30.))
        .rounded(px(7.))
        .bg(ink(0.05))
        .child(Icon::new(icon).size(px(15.)).text_color(text_muted(cx)))
        .into_any_element()
}

fn settings_row(
    icon: IconName,
    title: &str,
    subtitle: &str,
    control: gpui_kit::AnyElement,
    cx: &Context<SpurShell>,
) -> gpui_kit::AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .min_h(px(64.))
        .mx(px(4.))
        .px(px(10.))
        .py(px(8.))
        .child(row_icon(icon, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(px(TEXT_MD))
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_color(text_primary(cx))
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(text_muted(cx))
                        .child(subtitle.to_string()),
                ),
        )
        .child(control)
        .into_any_element()
}

/// A two-tone theme preview with the accent as a bottom strip.
fn theme_preview(config: &ThemeConfig) -> gpui_kit::AnyElement {
    let sidebar = hex_rgb(&theme::token_hex(config, theme::Token::Sidebar));
    let background = hex_rgb(&theme::token_hex(config, theme::Token::Background));
    let accent = hex_rgb(&theme::token_hex(config, theme::Token::Accent));
    div()
        .flex_none()
        .flex()
        .flex_col()
        .w(px(26.))
        .h(px(18.))
        .rounded(px(4.))
        .overflow_hidden()
        .border_1()
        .border_color(hairline(0.14))
        .child(
            div()
                .flex_1()
                .flex()
                .child(div().w(px(9.)).h_full().bg(sidebar))
                .child(div().flex_1().h_full().bg(background)),
        )
        .child(div().h(px(3.)).w_full().bg(accent))
        .into_any_element()
}

fn token_icon(token: theme::Token) -> IconName {
    match token {
        theme::Token::Accent => IconName::Palette,
        theme::Token::Background => IconName::Square,
        theme::Token::Sidebar => IconName::PanelLeft,
        theme::Token::Popover => IconName::Layers,
        theme::Token::Raised => IconName::Layers2,
        theme::Token::Text => IconName::Type,
        theme::Token::MutedText => IconName::TypeOutline,
        theme::Token::FaintText => IconName::TypeOutline,
        theme::Token::Danger => IconName::TriangleAlert,
        theme::Token::Warning => IconName::CircleAlert,
        theme::Token::Success => IconName::CircleCheck,
    }
}

fn token_label(token: theme::Token) -> &'static str {
    match token {
        theme::Token::Accent => t().settings_token_accent,
        theme::Token::Background => t().settings_token_background,
        theme::Token::Sidebar => t().settings_token_sidebar,
        theme::Token::Popover => t().settings_token_popover,
        theme::Token::Raised => t().settings_token_raised,
        theme::Token::Text => t().settings_token_text,
        theme::Token::MutedText => t().settings_token_muted_text,
        theme::Token::FaintText => t().settings_token_faint_text,
        theme::Token::Danger => t().settings_token_danger,
        theme::Token::Warning => t().settings_token_warning,
        theme::Token::Success => t().settings_token_success,
    }
}

fn token_hint(token: theme::Token) -> &'static str {
    match token {
        theme::Token::Accent => t().settings_token_accent_hint,
        theme::Token::Background => t().settings_token_background_hint,
        theme::Token::Sidebar => t().settings_token_sidebar_hint,
        theme::Token::Popover => t().settings_token_popover_hint,
        theme::Token::Raised => t().settings_token_raised_hint,
        theme::Token::Text => t().settings_token_text_hint,
        theme::Token::MutedText => t().settings_token_muted_text_hint,
        theme::Token::FaintText => t().settings_token_faint_text_hint,
        theme::Token::Danger => t().settings_token_danger_hint,
        theme::Token::Warning => t().settings_token_warning_hint,
        theme::Token::Success => t().settings_token_success_hint,
    }
}

fn hex_of(color: Hsla) -> String {
    let rgba = Rgba::from(color);
    let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(rgba.r),
        channel(rgba.g),
        channel(rgba.b)
    )
}

fn hex_rgb(hex: &str) -> Rgba {
    let channel = |range: std::ops::Range<usize>| {
        u32::from_str_radix(hex.get(range).unwrap_or("00"), 16).unwrap_or(0)
    };
    gpui_kit::rgb((channel(1..3) << 16) | (channel(3..5) << 8) | channel(5..7))
}

fn hsla_of(hex: &str) -> Hsla {
    Hsla::from(hex_rgb(hex))
}
