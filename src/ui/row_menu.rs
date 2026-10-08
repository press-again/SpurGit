//! Row context menus (history and sidebar rows). The shell draws the open
//! menu at window level so it opens at the pointer's x just below the
//! clicked row, which the generic context menu (always at the pointer)
//! cannot do.

use super::*;

use std::cell::Cell;

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::{
    DismissEvent, Div, Focusable as _, MouseButton, Pixels, Point, Stateful, Subscription,
    WeakEntity,
};

/// The open row menu.
pub(super) struct OpenRowMenu {
    /// Identifies the right-clicked row (a commit hash in the history list),
    /// so the row can stay raised while its menu is open.
    pub key: String,
    menu: Entity<PopupMenu>,
    position: Point<Pixels>,
    _dismiss: Subscription,
}

/// Open `build`'s menu when `row` is right-clicked, below the row at the
/// pointer's x. `row` must be `relative()` so its bounds can be measured.
pub(super) fn row_menu_trigger(
    row: Stateful<Div>,
    key: String,
    this: WeakEntity<SpurShell>,
    build: impl Fn(PopupMenu) -> PopupMenu + 'static,
) -> Stateful<Div> {
    let bottom = Rc::new(Cell::new(px(0.)));
    let measured = bottom.clone();
    let build = Rc::new(build);
    row.on_mouse_down(MouseButton::Right, move |event, window, cx| {
        let position = gpui_kit::point(event.position.x, bottom.get());
        let build = build.clone();
        this.update(cx, |shell, cx| {
            shell.open_row_menu(key.clone(), position, move |menu| build(menu), window, cx)
        })
        .ok();
    })
    .on_prepaint(move |bounds, _, _| measured.set(bounds.bottom()))
}

impl SpurShell {
    /// Open a row menu at `position` (window space), replacing any open one.
    /// Focus returns to wherever it was once the menu is dismissed.
    pub(super) fn open_row_menu(
        &mut self,
        key: String,
        position: Point<Pixels>,
        build: impl FnOnce(PopupMenu) -> PopupMenu + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_focus = window.focused(cx);
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            build(match previous_focus {
                Some(handle) => menu.action_context(handle),
                None => menu,
            })
        });
        let dismiss = cx.subscribe_in(&menu, window, |this, menu, _: &DismissEvent, _, cx| {
            // A newer menu may already have replaced this one.
            if this
                .row_menu_open
                .as_ref()
                .is_some_and(|open| open.menu.entity_id() == menu.entity_id())
            {
                this.row_menu_open = None;
                cx.notify();
            }
        });
        menu.focus_handle(cx).focus(window, cx);
        self.row_menu_open = Some(OpenRowMenu {
            key,
            menu,
            position,
            _dismiss: dismiss,
        });
        cx.notify();
    }

    /// Close the row menu even when focus has left it; it hands focus back
    /// only if it still held it.
    pub(super) fn close_row_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.row_menu_open.take() {
            let cancel = gpui_kit::base::actions::Cancel;
            open.menu.focus_handle(cx).dispatch_action(&cancel, window, cx);
            cx.notify();
        }
    }

    /// The open row menu, deferred above the window like the generic context
    /// menu; the full-window layer keeps the list underneath from scrolling
    /// out from under it.
    pub(super) fn render_row_menu(&self, window: &Window) -> Option<gpui_kit::AnyElement> {
        let open = self.row_menu_open.as_ref()?;
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
}
