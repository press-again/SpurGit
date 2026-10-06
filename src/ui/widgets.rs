//! Small reusable building blocks: color tones, text roles, and the tiny
//! visual elements shared across the shell (chips, key hints, icon buttons).

use super::*;

use crate::git::{DiffLine, DiffLineKind, FileDiff};
use crate::model::{Flag, FlagColor};
use gpui_kit::assets::IconName;
use gpui_kit::base::InteractiveElementExt as _;
use gpui_kit::base::SelectableText;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon, Sizable as _};
use gpui_kit::{App, FontWeight, Hsla, IntoElement, StatefulInteractiveElement as _};

use std::sync::atomic::{AtomicBool, Ordering};

/// True while a light theme is applied (kept in sync by `theme::apply_*`).
/// Washes and hairlines flip from white to black so hover states, raised
/// surfaces, and separators stay visible on pale backgrounds.
static LIGHT_WASH: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_light_wash(light: bool) {
    LIGHT_WASH.store(light, Ordering::Relaxed);
}

pub(crate) fn light_mode() -> bool {
    LIGHT_WASH.load(Ordering::Relaxed)
}

/// Mode-aware wash: white on the dark themes, black on a light one.
fn wash(alpha: f32) -> Hsla {
    if light_mode() {
        hsla(0.0, 0.0, 0.0, alpha)
    } else {
        hsla(0.0, 0.0, 1.0, alpha)
    }
}

/// Hover/active/raised washes, matching Zeron's helpers (theme.rs ink_for).
pub(super) fn ink(alpha: f32) -> Hsla {
    wash(alpha)
}

/// The low-contrast separators, same mode-aware wash.
pub(super) fn hairline(alpha: f32) -> Hsla {
    wash(alpha)
}

/// Raised-card surface at strength `f` (0 = flat, 1 = fully raised): a
/// wash plus a soft shadow. On a light theme a dark wash reads as a sunken
/// grey, so the card is white paper with a fainter shadow instead.
pub(super) fn raised_surface(f: f32) -> (Hsla, Vec<gpui_kit::BoxShadow>) {
    let (bg, shadow_alpha) = if light_mode() {
        (hsla(0., 0., 1., f), 0.12)
    } else {
        (wash(0.05 * f), 0.24)
    };
    let shadow = gpui_kit::BoxShadow {
        color: hsla(0., 0., 0., shadow_alpha * f),
        offset: gpui_kit::point(px(0.), px(2.)),
        blur_radius: px(6.),
        spread_radius: px(0.),
        inset: false,
    };
    (bg, vec![shadow])
}

/// The one small type scale (logical px).
pub(super) const TEXT_XS: f32 = 11.0; // section labels, counts, kbd hints
pub(super) const TEXT_SM: f32 = 12.0; // dense metadata, logs
pub(super) const TEXT_MD: f32 = 13.0; // body, rows, navigation
pub(super) const TEXT_LG: f32 = 15.0; // dialog titles, empty state

/// Estimated character width of Geist Mono at `TEXT_SM`, for horizontal
/// extents (diff content width, the commit editor's 80-column guide).
pub(super) const MONO_CHAR_W: f32 = 7.3;

/// Primary text (#e8e8ea) — subjects and labels.
pub(super) fn text_primary(cx: &App) -> Hsla {
    cx.theme().foreground
}

/// Muted text (#a9a9ae) — paths, branches, dates, hints. Subordinate to
/// [`text_primary`] but still readable on the dark shell.
pub(super) fn text_muted(cx: &App) -> Hsla {
    cx.theme().muted_foreground
}

/// Faint text (#85858a) — hashes, counts, tertiary hints.
pub(super) fn text_faint(cx: &App) -> Hsla {
    cx.theme().table_head_foreground
}

/// Solid, readable violet for text and icons.
///
/// Accent rule: the accent marks primary actions and selection only —
/// primary buttons, the active tab, selected rows/options, interactive links,
/// and activity (spinners, busy bars). Everything else (status letters,
/// environment chips, decorative icons, counts) stays neutral, so no screen
/// reads as a warning when nothing is wrong.
///
/// NOTE: `ThemeColor::accent` maps from the theme JSON token `accent.background`
/// — a low-alpha hover wash — so it is NOT usable as a foreground color.
/// `link` is our solid `#a89bf8` violet and stays readable on dark surfaces.
pub(super) fn violet(cx: &App) -> Hsla {
    cx.theme().link
}

/// Inverse of `to_root_string` for "open in Explorer": Linux paths become
/// `\\wsl.localhost\<distro>\…` so Windows Explorer can open them, except
/// `/mnt/<drive>/…` mounts, which are real Windows folders and open as
/// `X:\…`; Windows paths pass through.
pub(super) fn explorer_target(path: &str) -> String {
    let p = path.trim();
    if cfg!(not(windows)) {
        return p.to_string();
    }
    if let Some(rest) = p.strip_prefix("/mnt/") {
        let (drive, tail) = match rest.split_once('/') {
            Some((drive, tail)) => (drive, tail),
            None => (rest, ""),
        };
        if drive.len() == 1 && drive.chars().all(|c| c.is_ascii_lowercase()) {
            let drive = drive.to_ascii_uppercase();
            return if tail.is_empty() {
                format!("{drive}:\\")
            } else {
                format!("{drive}:\\{}", tail.replace('/', "\\"))
            };
        }
    }
    if p.starts_with('/') || p.starts_with('~') {
        let rel = p.trim_start_matches('~').trim_start_matches('/');
        format!("\\\\wsl.localhost\\{}\\{}", crate::git::distro(), rel.replace('/', "\\"))
    } else {
        p.to_string()
    }
}

/// Truncate a display label and append an ellipsis when it is too long.
pub(super) fn truncate_label(name: &str, max_chars: usize) -> String {
    if name.chars().count() <= max_chars {
        return name.to_string();
    }
    let mut out: String = name.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Browser URL for a Git remote, when one exists. `https://`/`http://` pass
/// through, `git@host:path`, `ssh://[user@]host[:port]/path`, and
/// `git://host/path` become `https://host/path`, and a trailing `.git` is
/// trimmed for a friendlier page. Local paths and `file://` URLs have no web
/// form.
pub(super) fn web_url(remote: &str) -> Option<String> {
    let url = remote.trim();
    if url.is_empty() {
        return None;
    }
    fn strip_git(path: &str) -> &str {
        path.strip_suffix(".git").unwrap_or(path)
    }
    if let Some(rest) = url.strip_prefix("https://") {
        return Some(format!("https://{}", strip_git(rest)));
    }
    if let Some(rest) = url.strip_prefix("http://") {
        return Some(format!("http://{}", strip_git(rest)));
    }
    if let Some(rest) = url.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        return Some(format!(
            "https://{host}/{}",
            strip_git(path.trim_start_matches('/'))
        ));
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next().unwrap_or(authority);
        let host = host.split(':').next().unwrap_or(host);
        return Some(format!("https://{host}/{}", strip_git(path)));
    }
    if let Some(rest) = url.strip_prefix("git://") {
        let (host, path) = rest.split_once('/')?;
        return Some(format!("https://{host}/{}", strip_git(path)));
    }
    None
}

/// Truncate a path keeping its informative tail: `…/services/api/gateway`.
pub(super) fn truncate_path(path: &str, max_chars: usize) -> String {
    let count = path.chars().count();
    if count <= max_chars {
        return path.to_string();
    }
    let tail: String = path.chars().skip(count - max_chars.saturating_sub(1)).collect();
    format!("…{tail}")
}

/// Humanized time since a collection or fetch (rows and inspector meta).
pub(super) fn ago_label(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 5 {
        "just now".to_string()
    } else if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

/// Quiet tag chip: tinted background, solid-color label, no border (saturated
/// outlined chips overpower the repository data around them).
pub(super) fn chip(text: impl Into<gpui_kit::SharedString>, color: Hsla) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .items_center()
        .h(px(18.))
        .rounded(px(5.))
        .bg(color.alpha(0.12))
        .px(px(6.))
        .text_size(px(10.5))
        .text_color(color)
        .child(text.into())
}

/// One gis status symbol on a quiet tinted wash with its textual tooltip.
/// The visible text is the symbol (plus the count for the sync flags); color
/// reinforces it, never replaces it (CONCEPT table).
pub(super) fn flag_chip(
    flag: Flag,
    text: impl Into<gpui_kit::SharedString>,
    cx: &App,
) -> impl IntoElement {
    let color = match flag.color_role() {
        FlagColor::Neutral => text_muted(cx),
        FlagColor::Success => cx.theme().success,
        FlagColor::Warning => cx.theme().warning,
        FlagColor::Danger => cx.theme().danger,
        FlagColor::Accent => violet(cx),
    };
    let text = text.into();
    let tooltip: gpui_kit::SharedString = flag.label().into();
    div()
        .id(gpui_kit::SharedString::from(format!(
            "flag-{}",
            flag.label()
        )))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(18.))
        .h(px(18.))
        .px(px(4.))
        .rounded(px(5.))
        .bg(color.alpha(0.10))
        .text_size(px(TEXT_XS))
        .text_color(color)
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .child(text)
}

/// Reference chip (HEAD/branch/remote/tag): icon + label on a quiet tinted
/// wash — meaning first, color second. The checked-out branch chip gets a
/// border and bold text so it stands out from the other refs.
pub(super) fn ref_chip(
    icon: IconName,
    text: &str,
    color: Hsla,
    current: bool,
) -> impl IntoElement {
    let mut chip = div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.))
        .h(px(18.))
        .px(px(6.))
        .rounded(px(5.))
        .bg(color.alpha(0.12))
        .text_size(px(10.5))
        .text_color(color)
        .child(Icon::new(icon).size(px(11.)).text_color(color));
    if current {
        chip = chip
            .border_1()
            .border_color(color.alpha(0.65))
            .font_weight(FontWeight::BOLD);
    }
    chip.child(text.to_string())
}

/// A vertical scrollbar overlay for a `.relative()` container that scrolls
/// `handle`. Unlike the kit's `vertical_scrollbar`, it stays visible while the
/// content overflows instead of fading out after scrolling (SourceGit-style),
/// and renders nothing when everything fits.
pub(super) fn scrollbar_overlay<H>(id: &'static str, handle: &H) -> impl IntoElement
where
    H: gpui_kit::component::scroll::ScrollbarHandle + Clone + 'static,
{
    scrollbar_overlay_axis(
        id,
        handle,
        gpui_kit::component::scroll::ScrollbarAxis::Vertical,
        false,
    )
}

/// [`scrollbar_overlay`] with a narrower track, for the sidebar collapsibles.
pub(super) fn thin_scrollbar_overlay<H>(id: &'static str, handle: &H) -> impl IntoElement
where
    H: gpui_kit::component::scroll::ScrollbarHandle + Clone + 'static,
{
    scrollbar_overlay_axis(
        id,
        handle,
        gpui_kit::component::scroll::ScrollbarAxis::Vertical,
        true,
    )
}

/// Same, horizontal (bottom edge).
pub(super) fn h_scrollbar_overlay<H>(id: &'static str, handle: &H) -> impl IntoElement
where
    H: gpui_kit::component::scroll::ScrollbarHandle + Clone + 'static,
{
    scrollbar_overlay_axis(
        id,
        handle,
        gpui_kit::component::scroll::ScrollbarAxis::Horizontal,
        false,
    )
}

fn scrollbar_overlay_axis<H>(
    id: &'static str,
    handle: &H,
    axis: gpui_kit::component::scroll::ScrollbarAxis,
    thin: bool,
) -> impl IntoElement
where
    H: gpui_kit::component::scroll::ScrollbarHandle + Clone + 'static,
{
    use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
    let bar = Scrollbar::new(handle)
        .id(id)
        .axis(axis)
        .mode(ScrollbarMode::Always)
        .viewport_from_layout();
    // Half-width track (8 instead of 16) with a centered 4px thumb.
    let bar = if thin {
        bar.styles(|styles| {
            styles
                .track(|track| track.width(px(8.)))
                .thumb(|thumb| thumb.width(px(4.)).inset(px(2.)))
                .thumb_hover(|thumb| thumb.width(px(5.)).inset(px(2.)))
                .thumb_active(|thumb| thumb.width(px(6.)).inset(px(2.)))
        })
    } else {
        bar
    };
    div().absolute().inset_0().child(bar)
}

/// Bordered control shell around a select/dropdown trigger, so modal selects
/// read as inputs instead of plain text.
pub(super) fn select_shell(trigger: impl IntoElement) -> impl IntoElement {
    div()
        .rounded(px(CONTROL_RADIUS))
        .border_1()
        .border_color(hairline(0.10))
        .bg(ink(0.04))
        .px(px(6.))
        .py(px(3.))
        .flex()
        .items_center()
        .child(trigger)
}

/// Lines of Git's live progress shown under a network operation's busy bar.
const PROGRESS_LINES: usize = 3;
const PROGRESS_LINE_H: f32 = 16.0;

/// Indeterminate progress bar with a caption, shown inside a modal while its
/// operation runs. With `progress`, Git's live output follows below, redrawn
/// every frame while the bar animates.
pub(super) fn busy_bar(label: String, progress: bool, cx: &Context<SpurShell>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .relative()
                .w_full()
                .h(px(3.))
                .rounded(px(2.))
                .bg(ink(0.08))
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .top(px(0.))
                        .bottom(px(0.))
                        .w(gpui_kit::relative(0.32))
                        .rounded(px(2.))
                        .bg(violet(cx))
                        .with_animation(
                            "modal-busy-bar",
                            gpui_kit::Animation::new(Duration::from_millis(1100)).repeat(),
                            |el, t| el.left(gpui_kit::relative(t * 1.32 - 0.32)),
                        ),
                ),
        )
        .child(
            div()
                .text_size(px(TEXT_XS))
                .text_color(text_muted(cx))
                .child(label),
        )
        .children(progress.then(|| {
            // The block keeps its height, so the dialog does not grow as
            // lines arrive.
            div()
                .flex()
                .flex_col()
                .h(px(PROGRESS_LINES as f32 * PROGRESS_LINE_H))
                .font_family(MONO)
                .text_size(px(TEXT_XS))
                .line_height(px(PROGRESS_LINE_H))
                .text_color(text_faint(cx))
                .children(
                    crate::process::progress_lines(PROGRESS_LINES)
                        .into_iter()
                        .map(|line| {
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(line)
                        }),
                )
        }))
}

/// Element ids of one [`readonly_diff_list`] instance (two can exist: the
/// commit detail and the stash view).
pub(super) struct ReadonlyDiffIds {
    pub list: &'static str,
    pub v_scrollbar: &'static str,
    pub h_wrapper: &'static str,
    pub h_scrollbar: &'static str,
}

/// Read-only virtualized unified diff (commit detail, stash view): old/new
/// gutters, add/remove washes, and horizontal scrolling — no hunk actions.
pub(super) fn readonly_diff_list(
    ids: ReadonlyDiffIds,
    diff: Rc<FileDiff>,
    v_scroll: &UniformListScrollHandle,
    h_scroll: &ScrollHandle,
) -> gpui_kit::AnyElement {
    /// Never build a horizontally scrollable strip wider than this.
    const MAX_DIFF_WIDTH: f32 = 6000.0;
    /// Read-only diff rows (same density as the Local Changes diff).
    const ROW_H: f32 = 19.0;

    let count = diff.lines.len();
    // Width comes from the parse-time scan, not a per-frame rescan.
    let max_chars = diff.max_chars;
    let width = (96.0 + max_chars as f32 * MONO_CHAR_W).clamp(320.0, MAX_DIFF_WIDTH);
    let rows = diff.clone();
    let list_id = ids.list;
    let mut list = gpui_kit::uniform_list(list_id, count, move |range, _window, cx| {
        range
            .map(|ix| readonly_diff_row(list_id, ROW_H, ix, &rows.lines[ix], cx))
            .collect()
    })
    .track_scroll(v_scroll)
    .w_full()
    .min_w(px(width))
    .h_full();
    list.interactivity().base_style.restrict_scroll_to_axis = Some(true);
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .child(
            div()
                .id(ids.h_wrapper)
                .size_full()
                .min_w_0()
                .overflow_x_scroll()
                .lock_scroll_axis()
                .track_scroll(h_scroll)
                .child(list),
        )
        .child(scrollbar_overlay(ids.v_scrollbar, v_scroll))
        .child(h_scrollbar_overlay(ids.h_scrollbar, h_scroll))
        .into_any_element()
}

fn readonly_diff_row(
    prefix: &'static str,
    row_h: f32,
    ix: usize,
    line: &DiffLine,
    cx: &gpui_kit::App,
) -> gpui_kit::AnyElement {
    let (bg, fg) = match line.kind {
        DiffLineKind::Added => (cx.theme().success.alpha(0.08), cx.theme().success),
        DiffLineKind::Removed => (cx.theme().danger.alpha(0.08), cx.theme().danger),
        DiffLineKind::Hunk => (violet(cx).alpha(0.08), violet(cx)),
        DiffLineKind::Meta => (ink(0.0), text_faint(cx)),
        DiffLineKind::Context => (ink(0.0), text_muted(cx)),
    };
    let gutter = |value: Option<u32>| {
        div()
            .w(px(44.))
            .flex_none()
            .pr(px(8.))
            .text_right()
            .font_family(MONO)
            .text_size(px(TEXT_XS))
            .text_color(text_faint(cx))
            .child(value.map(|v| v.to_string()).unwrap_or_default())
    };
    let marker = match line.kind {
        DiffLineKind::Added => "+",
        DiffLineKind::Removed => "-",
        _ => " ",
    };
    div()
        .id(gpui_kit::SharedString::from(format!("{prefix}-row-{ix}")))
        .w_full()
        .flex()
        .items_center()
        .h(px(row_h))
        .bg(bg)
        .child(gutter(line.old))
        .child(gutter(line.new))
        .child(
            div()
                .w(px(10.))
                .flex_none()
                .font_family(MONO)
                .text_size(px(TEXT_XS))
                .text_color(fg)
                .child(marker),
        )
        .child(
            // Selectable line content (window text selection, Ctrl+C), same
            // treatment as the Local Changes diff.
            div()
                .flex_none()
                .cursor_text()
                .child(
                    SelectableText::new(
                        gpui_kit::SharedString::from(format!("{prefix}-text-{ix}")),
                        line.text.clone(),
                    )
                    .text_style(readonly_diff_text_style(fg)),
                ),
        )
        .into_any_element()
}

/// Text style for the selectable read-only diff lines: the selectable element
/// does not inherit row styles, so face, size, and color are explicit.
fn readonly_diff_text_style(color: Hsla) -> gpui_kit::TextStyleRefinement {
    gpui_kit::TextStyleRefinement {
        font_family: Some(MONO.into()),
        font_size: Some(px(TEXT_SM).into()),
        color: Some(color),
        white_space: Some(gpui_kit::WhiteSpace::Nowrap),
        ..Default::default()
    }
}

/// Ghost icon button with tooltip (kit text-variant button).
pub(super) fn icon_button(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    icon_button_element(id, Icon::new(icon).size(px(14.)), tooltip, on_click)
}

/// Small square icon button with the tab-close hover wash — the same rounded
/// tint the tab "x" uses, for header actions that sit on quiet surfaces.
pub(super) fn wash_icon_button(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let key = format!("{id}-wash");
    div()
        .id(id)
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .w(px(20.))
        .h(px(20.))
        .rounded(px(4.))
        .bg(hover_blend(&key, ink(0.0), ink(0.10)))
        .on_hover(hover_listener(key))
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .on_click(on_click)
        .child(Icon::new(icon).size(px(13.)).text_color(text_muted(cx)))
}

/// Same button with a caller-built icon element (custom app icon slots).
pub(super) fn icon_button_element(
    id: &'static str,
    icon: impl IntoElement + 'static,
    tooltip: impl Into<gpui_kit::SharedString>,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    Button::new(id)
        .child(icon)
        .text()
        .small()
        .tooltip(tooltip)
        .on_click(on_click)
}

/// A context-menu item with a pointer cursor. The kit's standard items do not
/// set one, so the icon and label live in custom content that covers the row.
/// Shared by the Local Changes context menu and the branch list.
pub(super) fn context_menu_item(
    label: String,
    icon: IconName,
    handler: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> PopupMenuItem {
    PopupMenuItem::element(move |_window, cx| {
        div()
            .flex_1()
            .min_w_0()
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(Icon::new(icon).size(px(14.)).text_color(text_muted(cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(label.clone()),
            )
    })
    .on_click(handler)
}

/// Centered muted note used by empty sections.
pub(super) fn empty_note(text: &str, cx: &Context<SpurShell>) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(TEXT_SM))
        .text_color(text_muted(cx))
        .child(text.to_string())
}

/// Keyboard key chip ("↑↓", "esc", …). Key names are not localized.
pub(super) fn kbd_chip(keys: &str, cx: &Context<SpurShell>) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .h(px(20.))
        .rounded(px(5.))
        .bg(ink(0.05))
        .px(px(5.))
        .font_family(MONO)
        .text_size(px(10.5))
        .text_color(text_muted(cx))
        .child(keys.to_string())
}

/// Key chip plus its localized label.
pub(super) fn key_hint(keys: &str, label: &'static str, cx: &Context<SpurShell>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .child(kbd_chip(keys, cx))
        .child(
            div()
                .text_size(px(10.5))
                .text_color(text_muted(cx))
                .child(label),
        )
}

/// Name with the matched query substring in accent (Zeron highlights matches).
pub(super) fn highlighted(name: &str, query: &str, cx: &Context<SpurShell>) -> gpui_kit::AnyElement {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return div().child(name.to_string()).into_any_element();
    }
    let lower = name.to_lowercase();
    match lower.find(&q) {
        Some(start) => {
            let end = start + q.len();
            let (before, rest) = name.split_at(start);
            let (mid, after) = rest.split_at(end - start);
            div()
                .flex()
                .child(before.to_string())
                .child(
                    div()
                        .text_color(violet(cx))
                        .font_weight(FontWeight::MEDIUM)
                        .child(mid.to_string()),
                )
                .child(after.to_string())
                .into_any_element()
        }
        None => div().child(name.to_string()).into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn explorer_target_maps_wsl_mounts_and_native_paths() {
        assert_eq!(explorer_target("/mnt/c/Users/me/dev"), r"C:\Users\me\dev");
        assert_eq!(explorer_target("/mnt/d"), r"D:\");
        assert_eq!(
            explorer_target("/home/me/dev"),
            format!("\\\\wsl.localhost\\{}\\home\\me\\dev", crate::git::distro())
        );
        assert_eq!(explorer_target(r"C:\src"), r"C:\src");
    }

    #[test]
    fn web_url_covers_the_common_remote_shapes_and_rejects_local_paths() {
        assert_eq!(
            web_url("git@github.com:owner/repo.git").as_deref(),
            Some("https://github.com/owner/repo")
        );
        assert_eq!(
            web_url("ssh://git@github.com:22/owner/repo.git").as_deref(),
            Some("https://github.com/owner/repo")
        );
        assert_eq!(
            web_url("https://github.com/owner/repo.git").as_deref(),
            Some("https://github.com/owner/repo")
        );
        assert_eq!(
            web_url("http://git.example.com/team/app").as_deref(),
            Some("http://git.example.com/team/app")
        );
        assert_eq!(
            web_url("git://example.com/team/app.git").as_deref(),
            Some("https://example.com/team/app")
        );
        assert_eq!(web_url("/tmp/bare.git"), None);
        assert_eq!(web_url(r"C:\repos\bare.git"), None);
        assert_eq!(web_url("file:///srv/git/app.git"), None);
        assert_eq!(web_url(""), None);
    }
}
