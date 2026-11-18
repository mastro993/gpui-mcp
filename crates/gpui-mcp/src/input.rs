use gpui::{
    App, InputEvent, Keystroke, Modifiers, MouseButton as GpuiMouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PlatformInput, ScrollDelta, TouchPhase, Window, point, px,
};
use gpui_mcp_protocol::{
    BridgeError, ErrorCode, InputCommand, MAX_KEY_SEQUENCE, MAX_TEXT_BYTES, MouseButton, Point,
    PointerCommand, PointerScrollDelta,
};

pub(crate) fn validate(command: &InputCommand) -> Result<(), BridgeError> {
    match command {
        InputCommand::Key { keystroke } => {
            validate_text(keystroke)?;
            parse_keystroke(keystroke)?;
        }
        InputCommand::KeySequence { keystrokes } => {
            if keystrokes.is_empty() || keystrokes.len() > MAX_KEY_SEQUENCE {
                return Err(invalid("key sequence must contain 1 through 1024 events"));
            }
            for keystroke in keystrokes {
                validate_text(keystroke)?;
                parse_keystroke(keystroke)?;
            }
        }
        InputCommand::TypeText { text } | InputCommand::ReplaceText { text } => {
            validate_text(text)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_pointer(command: &PointerCommand) -> Result<(), BridgeError> {
    match command {
        PointerCommand::MouseMove { point, .. } => validate_point(*point),
        PointerCommand::MouseDown {
            point, click_count, ..
        }
        | PointerCommand::MouseUp {
            point, click_count, ..
        } => {
            validate_point(*point)?;
            validate_click_count(*click_count)
        }
        PointerCommand::ScrollWheel { point, delta } => {
            validate_point(*point)?;
            let (delta_x, delta_y) = match delta {
                PointerScrollDelta::Pixels { delta_x, delta_y }
                | PointerScrollDelta::Lines { delta_x, delta_y } => (*delta_x, *delta_y),
            };
            validate_scroll_delta(delta_x, delta_y)
        }
    }
}

pub(crate) fn dispatch_pointer(
    command: &PointerCommand,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), BridgeError> {
    validate_pointer(command)?;
    let event = match command {
        PointerCommand::MouseMove {
            point: position,
            pressed_button,
        } => PlatformInput::MouseMove(MouseMoveEvent {
            position: native_point(*position),
            pressed_button: pressed_button.map(native_button),
            modifiers: Modifiers::default(),
        }),
        PointerCommand::MouseDown {
            point: position,
            button,
            click_count,
        } => PlatformInput::MouseDown(MouseDownEvent {
            button: native_button(*button),
            position: native_point(*position),
            modifiers: Modifiers::default(),
            click_count: usize::from(*click_count),
            first_mouse: false,
        }),
        PointerCommand::MouseUp {
            point: position,
            button,
            click_count,
        } => PlatformInput::MouseUp(MouseUpEvent {
            button: native_button(*button),
            position: native_point(*position),
            modifiers: Modifiers::default(),
            click_count: usize::from(*click_count),
        }),
        PointerCommand::ScrollWheel {
            point: position,
            delta,
        } => scroll_event(
            native_point(*position),
            match delta {
                PointerScrollDelta::Pixels { delta_x, delta_y } => {
                    ScrollDelta::Pixels(point(px(*delta_x), px(*delta_y)))
                }
                PointerScrollDelta::Lines { delta_x, delta_y } => {
                    ScrollDelta::Lines(point(*delta_x, *delta_y))
                }
            },
        ),
    };
    window.dispatch_event(event, cx);
    Ok(())
}

/// Return the GPUI input pipeline's latest pointer position in window-relative logical pixels.
#[must_use]
pub(crate) fn pointer_location(window: &Window) -> Point {
    let position = window.mouse_position();
    Point {
        x: f32::from(position.x),
        y: f32::from(position.y),
    }
}

pub(crate) fn dispatch_keyboard(
    command: InputCommand,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), BridgeError> {
    validate(&command)?;
    match command {
        InputCommand::Key { keystroke } => {
            window.dispatch_keystroke(parse_keystroke(&keystroke)?, cx);
        }
        InputCommand::KeySequence { keystrokes } => {
            for keystroke in keystrokes {
                window.dispatch_keystroke(parse_keystroke(&keystroke)?, cx);
            }
        }
        InputCommand::TypeText { text } => require_input_handler(
            window.insert_input_text(&text, cx),
            "focused element has no active text input handler",
        )?,
        InputCommand::ReplaceText { text } => require_input_handler(
            window.replace_input_text(&text, cx),
            "focused input cannot expose its complete document range",
        )?,
    }
    Ok(())
}

/// The modifier GPUI's `secondary-` alias stands for: Cmd on macOS, Ctrl elsewhere.
const SECONDARY_MODIFIER: &str = if cfg!(target_os = "macos") {
    "cmd"
} else {
    "ctrl"
};

/// Parse a client keystroke, resolving the `secondary-` alias first. From
/// gpui-pre 0.3.10 `Keystroke::parse` rejects the alias and only
/// `parse_with_os` resolves it, but the `gpui-pre` feature also covers
/// releases without that API. The bridge runs natively, so the target OS is
/// the host OS that `parse_with_os` would be given.
fn parse_keystroke(source: &str) -> Result<Keystroke, BridgeError> {
    let resolved = source
        .split('-')
        .map(|component| {
            if component.eq_ignore_ascii_case("secondary") {
                SECONDARY_MODIFIER
            } else {
                component
            }
        })
        .collect::<Vec<_>>()
        .join("-");
    Keystroke::parse(&resolved).map_err(|_| invalid("invalid GPUI keystroke syntax"))
}

fn require_input_handler(available: bool, message: &'static str) -> Result<(), BridgeError> {
    if !available {
        return Err(BridgeError::new(ErrorCode::Unsupported, message));
    }
    Ok(())
}

/// A moved wheel scroll. gpui-pre 0.3.10 renamed `ScrollWheelEvent` to
/// `ScrollEvent`, keeping the old name as a deprecated alias, and added its
/// `source`; the old name and the default source work with every supported GPUI.
#[allow(deprecated, clippy::needless_update)]
fn scroll_event(position: gpui::Point<gpui::Pixels>, delta: ScrollDelta) -> PlatformInput {
    gpui::ScrollWheelEvent {
        position,
        delta,
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
        ..Default::default()
    }
    .to_platform_input()
}

fn native_point(position: Point) -> gpui::Point<gpui::Pixels> {
    point(px(position.x), px(position.y))
}

const fn native_button(button: MouseButton) -> GpuiMouseButton {
    match button {
        MouseButton::Left => GpuiMouseButton::Left,
        MouseButton::Right => GpuiMouseButton::Right,
        MouseButton::Middle => GpuiMouseButton::Middle,
    }
}

fn validate_click_count(click_count: u8) -> Result<(), BridgeError> {
    if !(1..=3).contains(&click_count) {
        return Err(invalid("click count must be between 1 and 3"));
    }
    Ok(())
}

fn validate_scroll_delta(delta_x: f32, delta_y: f32) -> Result<(), BridgeError> {
    if !delta_x.is_finite() || !delta_y.is_finite() {
        return Err(invalid("scroll deltas must be finite"));
    }
    if delta_x.abs() > 100_000.0 || delta_y.abs() > 100_000.0 {
        return Err(invalid("scroll delta exceeds the safety bound"));
    }
    Ok(())
}

fn validate_point(point: Point) -> Result<(), BridgeError> {
    if !point.is_valid() {
        return Err(invalid("coordinates must be finite"));
    }
    if point.x.abs() > 1_000_000.0 || point.y.abs() > 1_000_000.0 {
        return Err(invalid("coordinates exceed the safety bound"));
    }
    Ok(())
}

fn validate_text(text: &str) -> Result<(), BridgeError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(invalid("text exceeds the 64 KiB safety bound"));
    }
    Ok(())
}

fn invalid(message: &'static str) -> BridgeError {
    BridgeError::new(ErrorCode::InvalidRequest, message)
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use gpui::{
        AppContext as _, Context, InteractiveElement as _, IntoElement, Modifiers,
        MouseButton as GpuiMouseButton, ParentElement as _, Render,
        StatefulInteractiveElement as _, Styled as _, TestAppContext, Window, div, point, px, size,
    };
    use gpui_mcp_protocol::{
        BridgeError, ErrorCode, InputCommand, MAX_KEY_SEQUENCE, MouseButton, Point, PointerCommand,
    };

    use super::{dispatch_keyboard, dispatch_pointer, validate};

    #[test]
    fn key_sequences_are_bounded_and_fully_validated() {
        assert!(
            validate(&InputCommand::KeySequence {
                keystrokes: vec!["home".to_owned(), "right".to_owned()],
            })
            .is_ok()
        );
        assert!(
            validate(&InputCommand::KeySequence {
                keystrokes: Vec::new(),
            })
            .is_err()
        );
        assert!(
            validate(&InputCommand::KeySequence {
                keystrokes: vec!["right".to_owned(); MAX_KEY_SEQUENCE + 1],
            })
            .is_err()
        );
    }

    #[test]
    fn secondary_keystrokes_validate_and_malformed_keystrokes_do_not() {
        for keystroke in [
            "secondary-[",
            "secondary-left",
            "secondary-a",
            "shift-SECONDARY-a",
        ] {
            assert_eq!(
                validate(&InputCommand::Key {
                    keystroke: keystroke.to_owned(),
                }),
                Ok(()),
                "{keystroke}"
            );
        }
        assert_eq!(
            validate(&InputCommand::KeySequence {
                keystrokes: vec!["secondary-a".to_owned(), "a-b".to_owned()],
            }),
            Err(BridgeError::new(
                ErrorCode::InvalidRequest,
                "invalid GPUI keystroke syntax"
            ))
        );
    }

    #[gpui::test]
    fn secondary_keystrokes_reach_the_window_as_the_platform_modifier(cx: &mut TestAppContext) {
        let received = Rc::new(RefCell::new(Vec::new()));
        let received_for_interceptor = received.clone();
        let _interceptor = cx.update(|cx| {
            cx.intercept_keystrokes(move |event, _, _| {
                received_for_interceptor
                    .borrow_mut()
                    .push((event.keystroke.key.clone(), event.keystroke.modifiers));
            })
        });
        let visual = cx.add_empty_window();

        visual.update(|window, cx| {
            assert_eq!(
                dispatch_keyboard(
                    InputCommand::Key {
                        keystroke: "secondary-a".to_owned(),
                    },
                    window,
                    cx,
                ),
                Ok(())
            );
        });

        let secondary = if cfg!(target_os = "macos") {
            Modifiers::command()
        } else {
            Modifiers::control()
        };
        assert_eq!(*received.borrow(), vec![("a".to_owned(), secondary)]);
    }

    #[derive(Clone, Copy)]
    struct DragValue;

    struct DragPreview;

    impl Render for DragPreview {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn synthetic_mouse_move_runs_gpui_hover_handlers(cx: &mut TestAppContext) {
        let hovered = Rc::new(Cell::new(false));
        let hovered_for_handler = hovered.clone();
        let visual = cx.add_empty_window();
        visual.draw(
            point(px(0.0), px(0.0)),
            size(px(300.0), px(100.0)),
            move |_, _| {
                div()
                    .id("native-hover-target")
                    .w(px(100.0))
                    .h(px(100.0))
                    .on_hover(move |value, _, _| hovered_for_handler.set(*value))
            },
        );

        visual.update(|window, cx| {
            assert_eq!(
                dispatch_pointer(
                    &PointerCommand::MouseMove {
                        point: Point { x: 50.0, y: 50.0 },
                        pressed_button: None,
                    },
                    window,
                    cx,
                ),
                Ok(())
            );
        });

        assert!(hovered.get());
    }

    #[gpui::test]
    fn synthetic_platform_drag_runs_gpui_drag_and_drop_handlers(cx: &mut TestAppContext) {
        let pressed = Rc::new(Cell::new(false));
        let drag_started = Rc::new(Cell::new(false));
        let dropped = Rc::new(Cell::new(false));
        let pressed_for_handler = pressed.clone();
        let drag_started_for_handler = drag_started.clone();
        let dropped_for_handler = dropped.clone();
        let visual = cx.add_empty_window();
        visual.draw(
            point(px(0.0), px(0.0)),
            size(px(300.0), px(100.0)),
            move |_, _| {
                div()
                    .flex()
                    .gap(px(50.0))
                    .child(
                        div()
                            .id("native-drag-source")
                            .w(px(100.0))
                            .h(px(100.0))
                            .on_mouse_down(GpuiMouseButton::Left, move |_, _, _| {
                                pressed_for_handler.set(true);
                            })
                            .on_drag(DragValue, move |_, _, _, cx| {
                                drag_started_for_handler.set(true);
                                cx.new(|_| DragPreview)
                            }),
                    )
                    .child(
                        div()
                            .id("native-drop-target")
                            .w(px(100.0))
                            .h(px(100.0))
                            .on_drop(move |_: &DragValue, _, _| {
                                dropped_for_handler.set(true);
                            }),
                    )
            },
        );

        visual.update(|window, cx| {
            assert_eq!(
                dispatch_pointer(
                    &PointerCommand::MouseDown {
                        point: Point { x: 50.0, y: 50.0 },
                        button: MouseButton::Left,
                        click_count: 1,
                    },
                    window,
                    cx,
                ),
                Ok(())
            );
            assert_eq!(
                dispatch_pointer(
                    &PointerCommand::MouseMove {
                        point: Point { x: 60.0, y: 50.0 },
                        pressed_button: Some(MouseButton::Left),
                    },
                    window,
                    cx,
                ),
                Ok(())
            );
            assert_eq!(
                dispatch_pointer(
                    &PointerCommand::MouseMove {
                        point: Point { x: 200.0, y: 50.0 },
                        pressed_button: Some(MouseButton::Left),
                    },
                    window,
                    cx,
                ),
                Ok(())
            );
            assert_eq!(
                dispatch_pointer(
                    &PointerCommand::MouseUp {
                        point: Point { x: 200.0, y: 50.0 },
                        button: MouseButton::Left,
                        click_count: 1,
                    },
                    window,
                    cx,
                ),
                Ok(())
            );
            assert_eq!(window.mouse_position(), point(px(200.0), px(50.0)));
        });

        assert!(pressed.get());
        assert!(drag_started.get());
        assert!(dropped.get());
    }
}
