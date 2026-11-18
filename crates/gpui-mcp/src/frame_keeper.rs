//! Frames for a window the agent drives while nobody is looking at it.
//!
//! Platforms stop requesting frames for a window that is covered by other
//! windows, minimized, or on another desktop. Every automation step waits for
//! the frame its input caused, so without this the agent could only drive a
//! window kept in front of the user's own work.

use std::sync::{Arc, Weak};
use std::time::Duration;

use async_channel::Receiver;
use gpui::{App, AsyncWindowContext, Window};
use gpui_mcp_protocol::MAX_WAIT_MS;

use crate::registry::SharedState;

/// How long after the last automation request frames are still driven.
///
/// This outlasts the longest wait a single request may ask for.
const ACTIVE_FOR: Duration = Duration::from_millis(MAX_WAIT_MS + 5_000);
/// How often a requested frame is checked.
///
/// A frame that is still requested a whole tick later, with no frame drawn in
/// between, is one the platform withheld: GPUI draws a visible window at least
/// every 33 ms by default, even when it is not focused.
const TICK: Duration = Duration::from_millis(50);

/// Draw the frames the platform withholds while automation is active.
///
/// Each message on `activity` is an authenticated bridge request. The task
/// sleeps until one arrives and stops when the bridge or the window is gone.
pub(crate) fn spawn(window: &Window, cx: &App, state: &Arc<SharedState>, activity: Receiver<()>) {
    let state = Arc::downgrade(state);
    window
        .spawn(cx, async move |cx| {
            while activity.recv().await.is_ok() {
                if !keep_frames(&activity, &state, cx).await {
                    break;
                }
            }
        })
        .detach();
}

/// Draw withheld frames until automation goes quiet. Returns `false` once the
/// bridge or the window is gone.
async fn keep_frames(
    activity: &Receiver<()>,
    state: &Weak<SharedState>,
    cx: &mut AsyncWindowContext,
) -> bool {
    let executor = cx.background_executor().clone();
    let mut last_request = executor.now();
    let mut stall = Stall::default();
    loop {
        executor.timer(TICK).await;
        if activity.try_recv().is_ok() {
            last_request = executor.now();
        }
        if executor.now().saturating_duration_since(last_request) >= ACTIVE_FOR {
            return true;
        }
        let Some(state) = state.upgrade() else {
            return false;
        };
        let completed = state.frame_count();
        let updated = cx.update(|window, cx| {
            if stall.check(window.frame_requested(), completed) {
                window.draw_pending_frame(cx);
            }
        });
        if updated.is_err() {
            return false;
        }
    }
}

/// Spots a requested frame the platform is not going to draw.
#[derive(Default)]
struct Stall {
    requested_since: Option<u64>,
}

impl Stall {
    /// Whether a frame has stayed requested since the last check with no frame
    /// completed in between. A stall is reported once, then tracking restarts.
    fn check(&mut self, requested: bool, completed: u64) -> bool {
        let stalled = requested && self.requested_since == Some(completed);
        self.requested_since = (requested && !stalled).then_some(completed);
        stalled
    }
}

#[cfg(test)]
mod stall_tests {
    use super::Stall;

    #[test]
    fn reports_a_frame_left_requested_with_no_draw_in_between() {
        let mut stall = Stall::default();
        assert!(!stall.check(true, 7));
        assert!(stall.check(true, 7));
        assert!(!stall.check(true, 8), "tracking restarts after a stall");
    }

    #[test]
    fn ignores_windows_the_platform_is_drawing() {
        let mut stall = Stall::default();
        assert!(!stall.check(true, 7));
        assert!(!stall.check(true, 8), "a frame completed in between");
        assert!(!stall.check(false, 9));
        assert!(
            !stall.check(true, 9),
            "nothing was requested at the last check"
        );
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{Context, IntoElement, Render, TestAppContext, Window, div, prelude::*};

    use crate::Automation;

    struct Root;

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().id("root")
        }
    }

    #[gpui::test]
    fn runs_a_requested_frame_without_the_platform(cx: &mut TestAppContext) {
        let automation = Automation::isolated();
        let observed = automation.clone();
        let (_, visual) = cx.add_window_view(move |window, _| {
            observed.attach(window);
            Root
        });
        visual.run_until_parked();
        let before = automation.state.frame_count();
        let ran = Rc::new(Cell::new(false));

        let (drawn, requested, redrawn) = visual.update(|window, cx| {
            let ran = ran.clone();
            window.on_next_frame(move |window, _| {
                ran.set(true);
                window.request_frame();
            });
            let drawn = window.draw_pending_frame(cx);
            (
                drawn,
                window.frame_requested(),
                window.draw_pending_frame(cx),
            )
        });

        assert!(drawn);
        assert!(ran.get(), "next-frame callbacks run first");
        assert!(!requested);
        assert!(!redrawn, "nothing is requested after the frame");
        assert_eq!(automation.state.frame_count(), before + 1);
    }
}
