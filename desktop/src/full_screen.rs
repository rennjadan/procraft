//! Full screen, as F11 makes it, with a swipe up from the bottom edge to show the taskbar.

use winit::event::WindowEvent;
use winit::window::Window;

use crate::files;

/// How far up from the window's bottom edge a swipe has to start, and how far up it has to go
/// before the taskbar shows (points).
const EDGE_ZONE: f64 = 22.0;
const EDGE_SWIPE: f64 = 40.0;

/// Full screen, as F11 makes it, kept between runs ("window" in the app's folder). While it is
/// on, a finger or the pen swiped up from the very bottom of the screen lets the taskbar show
/// (the window steps back to filling the screen above it); the next touch, pen or click on the
/// app covers the taskbar again. F11 turns all of it off (a plain window) and back on.
pub(crate) struct FullScreen {
    /// Full screen wanted (F11 not having turned it off).
    pub(crate) wanted: bool,
    /// The taskbar is showing for now, after a swipe up.
    revealed: bool,
    /// A touch that began at the bottom edge: its id and where it began (physical y).
    edge: Option<(u64, f64)>,
    /// The touch that swiped the taskbar up: the rest of it reaches nothing.
    swallowed: Option<u64>,
}

impl FullScreen {
    pub(crate) fn kept() -> FullScreen {
        let text = std::fs::read_to_string(files::app_dir().join("window")).unwrap_or_default();
        let wanted = !text.lines().any(|l| l.trim() == "full_screen false");
        FullScreen { wanted, revealed: false, edge: None, swallowed: None }
    }

    /// F11 pressed: full screen on or off from now on, and next time.
    pub(crate) fn chosen(&mut self, full: bool) {
        self.wanted = full;
        self.revealed = false;
        self.edge = None;
        files::keep_text("window", &format!("full_screen {full}\n"));
    }

    fn covered(window: &Window) {
        window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
    }

    /// Watches the touches (a pen's too) and clicks for the swipe up and for the app being used
    /// again. Returns the event for the app, or none when the swipe took it.
    pub(crate) fn event(&mut self, window: &Window, event: WindowEvent) -> Option<WindowEvent> {
        use winit::event::TouchPhase;
        if !self.wanted {
            return Some(event);
        }
        let points = window.scale_factor().max(0.1);
        match &event {
            WindowEvent::Touch(t) => {
                if self.swallowed == Some(t.id) {
                    if matches!(t.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                        self.swallowed = None;
                    }
                    return None;
                }
                match t.phase {
                    TouchPhase::Started => {
                        if self.revealed {
                            // used again: the taskbar is covered again
                            self.revealed = false;
                            Self::covered(window);
                        } else if window.fullscreen().is_some() {
                            let height = f64::from(window.inner_size().height);
                            self.edge = (t.location.y >= height - EDGE_ZONE * points).then_some((t.id, t.location.y));
                        }
                    }
                    TouchPhase::Moved => {
                        if let Some((id, from)) = self.edge
                            && id == t.id
                            && from - t.location.y >= EDGE_SWIPE * points
                        {
                            self.edge = None;
                            self.swallowed = Some(t.id);
                            self.revealed = true;
                            window.set_fullscreen(None);
                            window.set_maximized(true);
                            // whatever the touch had begun (a stroke at the edge) is called off
                            let mut cancelled = *t;
                            cancelled.phase = TouchPhase::Cancelled;
                            return Some(WindowEvent::Touch(cancelled));
                        }
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        if self.edge.is_some_and(|(id, _)| id == t.id) {
                            self.edge = None;
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state: winit::event::ElementState::Pressed, .. } if self.revealed => {
                self.revealed = false;
                Self::covered(window);
            }
            _ => {}
        }
        Some(event)
    }
}
