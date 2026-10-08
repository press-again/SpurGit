//! Resizable panes: the repository sidebar, the Local Changes file list and
//! the commit details panel share one drag. Moves resize live, release
//! persists, double-click resets to the default.

use super::*;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{Div, MouseButton, MouseDownEvent, MouseMoveEvent, Stateful, WeakEntity};

use crate::settings::{
    clamp_commit_panel_ratio, clamp_sidebar_w, CHANGES_LIST_W_DEFAULT,
    COMMIT_PANEL_RATIO_DEFAULT, SIDEBAR_W_DEFAULT,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pane {
    Sidebar,
    ChangesList,
    /// Resized vertically, as a share of the history column.
    CommitPanel,
}

impl Pane {
    fn id(self) -> &'static str {
        match self {
            Pane::Sidebar => "sidebar-handle",
            Pane::ChangesList => "changes-col-handle",
            Pane::CommitPanel => "commit-panel-handle",
        }
    }
}

/// A resize in progress: the pointer coordinate along the drag axis and the
/// pane's size when it started.
#[derive(Clone, Copy, Debug)]
pub(super) struct PaneDrag {
    pane: Pane,
    start: f32,
    start_size: f32,
}

/// Grab strip for `pane`; the caller sizes and places it. A plain div takes
/// no keyboard focus, so tab order is unchanged.
pub(super) fn pane_handle(pane: Pane, entity: WeakEntity<SpurShell>) -> Stateful<Div> {
    let id = pane.id();
    let handle = div()
        .id(id)
        .bg(hover_blend(id, ink(0.0), ink(0.10)))
        .on_hover(hover_listener(id))
        .tooltip(|window, cx| Tooltip::new(t().col_resize_tooltip).build(window, cx))
        .on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, _: &mut Window, cx: &mut App| {
                // Pixels are logical in gpui: no scale-factor division.
                let at = match pane {
                    Pane::CommitPanel => event.position.y,
                    _ => event.position.x,
                };
                let reset = event.click_count >= 2;
                entity
                    .update(cx, |this, cx| {
                        this.start_pane_drag(pane, f32::from(at), reset, cx)
                    })
                    .ok();
            },
        );
    match pane {
        Pane::CommitPanel => handle.cursor_ns_resize(),
        _ => handle.cursor_ew_resize(),
    }
}

impl SpurShell {
    fn start_pane_drag(&mut self, pane: Pane, at: f32, reset: bool, cx: &mut Context<Self>) {
        if reset {
            self.pane_drag = None;
            match pane {
                Pane::Sidebar => self.sidebar_w = SIDEBAR_W_DEFAULT,
                Pane::ChangesList => self.changes_list_w = CHANGES_LIST_W_DEFAULT,
                Pane::CommitPanel => self.commit_panel_ratio = COMMIT_PANEL_RATIO_DEFAULT,
            }
            self.save_pane_size(pane, cx);
        } else {
            let start_size = match pane {
                Pane::Sidebar => self.sidebar_w,
                Pane::ChangesList => self.list_col_w(),
                Pane::CommitPanel => self.commit_panel_ratio,
            };
            self.pane_drag = Some(PaneDrag {
                pane,
                start: at,
                start_size,
            });
        }
        cx.notify();
    }

    /// Resize the dragged pane live. A move without the button held ends a
    /// drag whose release happened outside the window.
    pub(super) fn drag_pane(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.pane_drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_pane_drag(cx);
            return;
        }
        let dx = f32::from(event.position.x) - drag.start;
        let dy = f32::from(event.position.y) - drag.start;
        match drag.pane {
            Pane::Sidebar => self.sidebar_w = clamp_sidebar_w(drag.start_size + dx),
            Pane::ChangesList => {
                let viewport_w = f32::from(window.viewport_size().width);
                self.changes_list_w =
                    changes::clamp_col_width(drag.start_size + dx, viewport_w);
            }
            // The handle sits on the panel's top edge: dragging up grows it.
            Pane::CommitPanel if self.history_col_h > 0.0 => {
                self.commit_panel_ratio =
                    clamp_commit_panel_ratio(drag.start_size - dy / self.history_col_h);
            }
            Pane::CommitPanel => {}
        }
        cx.notify();
    }

    /// End a pane drag, persisting the size. No-op when not dragging.
    pub(super) fn end_pane_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.pane_drag.take() {
            self.save_pane_size(drag.pane, cx);
            cx.notify();
        }
    }

    fn save_pane_size(&mut self, pane: Pane, cx: &mut Context<Self>) {
        let saved = match pane {
            Pane::Sidebar => crate::settings::set_sidebar_w(self.sidebar_w),
            Pane::ChangesList => crate::settings::set_changes_list_w(self.changes_list_w),
            Pane::CommitPanel => crate::settings::set_commit_panel_ratio(self.commit_panel_ratio),
        };
        if let Err(err) = saved {
            self.note_error(t().log_settings_save_failed(&err), cx);
        }
    }
}
