//! Full screen, as F11 makes it, with a swipe in from the top (or bottom) edge to show the taskbar.
//!
//! The window never changes size or leaves full screen for it: the taskbar is brought up over the
//! app, as the Windows key brings it up over a full-screen game, while the app eases back under
//! a soft veil that follows the finger. The next touch or click on the app eases the veil away,
//! and Windows puts the taskbar back behind the app.

use std::time::Instant;

use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, TouchPhase, WindowEvent};
use winit::window::Window;

use crate::files;

/// How far in from the edge a swipe has to start (points): a little way in, as Windows keeps the
/// very edge for its own swipes.
const EDGE_ZONE: f64 = 44.0;
/// How far in it travels before it counts as a pull and not a tap (points).
const PULL_START: f64 = 10.0;
/// How far in a pull goes to bring the veil all the way (points), and the part of that past which
/// letting go shows the taskbar.
const PULL_FULL: f64 = 90.0;
const PULL_COMMIT: f32 = 0.4;
/// A flick in this fast or faster (points a millisecond) shows it however short it was.
const FLICK: f64 = 0.45;
/// The veil at its darkest, over the whole app.
const VEIL_MOST: f32 = 0.42;
/// How long the veil takes to settle in, and to go (seconds).
const EASE_IN_S: f32 = 0.32;
const EASE_OUT_S: f32 = 0.26;
/// After the app is used again, a touch or click this soon only brings it back (seconds).
const RETURN_GRACE_S: f32 = 0.5;

/// A touch that began at the edge, held back until it is known to be a pull or not.
struct Edge {
    id: u64,
    start: PhysicalPosition<f64>,
    held: Vec<WindowEvent>,
}

/// A touch pulling the veil in: where it began, and its last place and time for its speed.
struct Pull {
    id: u64,
    start: PhysicalPosition<f64>,
    last: (f64, Instant),
    speed: f64,
}

/// The veil easing from one level to another.
#[derive(Clone, Copy)]
struct Ease {
    from: f32,
    to: f32,
    start: Instant,
    seconds: f32,
}

/// Full screen, as F11 makes it, kept between runs ("window" in the app's folder), and the swipe
/// that shows the taskbar over it. F11 turns all of it off (a plain window) and back on.
pub(crate) struct FullScreen {
    /// Full screen wanted (F11 not having turned it off).
    pub(crate) wanted: bool,
    edge: Option<Edge>,
    pull: Option<Pull>,
    /// The veil's level while a finger pulls it (0..1).
    pulled: f32,
    ease: Option<Ease>,
    /// The taskbar is up over the app.
    revealed: bool,
    /// The app was just used again: the next touch or click (within the grace) only does that.
    returned: Option<Instant>,
    /// A touch, or the mouse's press, that only brought the app back: the rest of it reaches nothing.
    swallowed: Option<u64>,
    mouse_swallowed: bool,
}

impl FullScreen {
    pub(crate) fn kept() -> FullScreen {
        let text = std::fs::read_to_string(files::app_dir().join("window")).unwrap_or_default();
        let wanted = !text.lines().any(|l| l.trim() == "full_screen false");
        FullScreen {
            wanted,
            edge: None,
            pull: None,
            pulled: 0.0,
            ease: None,
            revealed: false,
            returned: None,
            swallowed: None,
            mouse_swallowed: false,
        }
    }

    /// F11 pressed: full screen on or off from now on, and next time.
    pub(crate) fn chosen(&mut self, full: bool) {
        self.wanted = full;
        self.edge = None;
        self.pull = None;
        self.revealed = false;
        self.settle_to(0.0, EASE_OUT_S);
        files::keep_text("window", &format!("full_screen {full}\n"));
    }

    /// The veil's level now (0 none, 1 at its darkest).
    fn level(&self) -> f32 {
        if self.pull.is_some() {
            return self.pulled;
        }
        match self.ease {
            Some(e) => {
                let t = (e.start.elapsed().as_secs_f32() / e.seconds.max(0.01)).min(1.0);
                e.from + (e.to - e.from) * ease(t)
            }
            None => 0.0,
        }
    }

    /// Whether the veil is still moving (a frame is wanted).
    pub(crate) fn moving(&self) -> bool {
        self.pull.is_some()
            || self.ease.is_some_and(|e| e.start.elapsed().as_secs_f32() < e.seconds && (e.to - e.from).abs() > 1e-4)
    }

    fn settle_to(&mut self, to: f32, seconds: f32) {
        let from = self.level();
        self.ease = Some(Ease { from, to, start: Instant::now(), seconds });
    }

    /// The taskbar up, the veil easing the rest of the way in.
    fn reveal(&mut self) {
        log::info!("taskbar swipe: the taskbar shown");
        self.revealed = true;
        self.returned = None;
        self.settle_to(1.0, EASE_IN_S);
        show_taskbar();
    }

    /// The app used again: the veil eases away (Windows puts the taskbar back behind it).
    fn back(&mut self) {
        self.revealed = false;
        self.returned = Some(Instant::now());
        self.settle_to(0.0, EASE_OUT_S);
    }

    /// Draws the veil over everything, while it shows.
    pub(crate) fn paint(&self, ctx: &egui::Context) {
        let level = self.level().clamp(0.0, 1.0);
        if level > 0.002 {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Debug, egui::Id::unique("taskbar veil")));
            let alpha = (level * VEIL_MOST * 255.0).round() as u8;
            painter.rect_filled(ctx.content_rect(), 0.0, egui::Color32::from_black_alpha(alpha));
        }
        if self.moving() {
            ctx.request_repaint();
        }
    }

    /// Watches the touches (a pen's too) and clicks for the pull in from the edge and for the app
    /// being used again. Returns the events for the app: none while a touch is held back, those
    /// held back once it turns out not to be a pull, or the event as it came.
    /// `edge` is the edge the swipe comes from: the top (true) or the bottom, or none with it off.
    pub(crate) fn event(&mut self, window: &Window, event: WindowEvent, edge: Option<bool>) -> Vec<WindowEvent> {
        let Some(from_top) = edge.filter(|_| self.wanted && window.fullscreen().is_some()) else {
            self.edge = None;
            self.pull = None;
            return vec![event];
        };
        let points = window.scale_factor().max(0.1);
        let height = f64::from(window.inner_size().height);
        // how far in from the edge `at` is, from `start`
        let inward = |start: PhysicalPosition<f64>, at: PhysicalPosition<f64>| {
            if from_top { at.y - start.y } else { start.y - at.y }
        };
        let in_zone = |at: PhysicalPosition<f64>| {
            if from_top { at.y <= EDGE_ZONE * points } else { at.y >= height - EDGE_ZONE * points }
        };
        let grace = |r: Option<Instant>| r.is_some_and(|r| r.elapsed().as_secs_f32() < RETURN_GRACE_S);
        match &event {
            WindowEvent::Focused(true) if self.revealed => self.back(),
            WindowEvent::Focused(false) => {
                // a pull cut short by something else taking the screen
                if self.pull.take().is_some() && !self.revealed {
                    self.ease = Some(Ease { from: self.pulled, to: 0.0, start: Instant::now(), seconds: EASE_OUT_S });
                }
            }
            WindowEvent::Touch(t) => {
                if self.swallowed == Some(t.id) {
                    if matches!(t.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                        self.swallowed = None;
                    }
                    return Vec::new();
                }
                // the veil following a pull
                if let Some(pull) = self.pull.as_mut().filter(|p| p.id == t.id) {
                    let now = Instant::now();
                    let travel = inward(pull.start, t.location);
                    let ms = now.duration_since(pull.last.1).as_secs_f64() * 1000.0;
                    if ms > 0.5 {
                        pull.speed = (travel - pull.last.0) / points / ms;
                        pull.last = (travel, now);
                    }
                    self.pulled = ((travel / points - PULL_START) / PULL_FULL).clamp(0.0, 1.0) as f32;
                    if matches!(t.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                        let flicked = pull.speed >= FLICK && t.phase == TouchPhase::Ended;
                        let far = self.pulled >= PULL_COMMIT && t.phase == TouchPhase::Ended;
                        self.pull = None;
                        self.ease = Some(Ease { from: self.pulled, to: self.pulled, start: now, seconds: 0.0 });
                        if flicked || far {
                            self.reveal();
                        } else {
                            self.settle_to(0.0, EASE_OUT_S);
                        }
                    }
                    return Vec::new();
                }
                // a touch at the edge, held back while it shows what it is
                if let Some(edge) = self.edge.as_mut().filter(|e| e.id == t.id) {
                    edge.held.push(event.clone());
                    let travel = inward(edge.start, t.location) / points;
                    let across = (t.location.x - edge.start.x).abs() / points;
                    let ended = matches!(t.phase, TouchPhase::Ended | TouchPhase::Cancelled);
                    if !ended && travel >= PULL_START && travel > across * 1.2 {
                        let start = edge.start;
                        self.edge = None;
                        self.pulled = 0.0;
                        self.pull = Some(Pull { id: t.id, start, last: (travel * points, Instant::now()), speed: 0.0 });
                        return Vec::new();
                    }
                    if ended || travel.hypot(across) >= PULL_START {
                        // a tap or a drag along the edge: the app has it as it was
                        return self.edge.take().map(|e| e.held).unwrap_or_default();
                    }
                    return Vec::new();
                }
                if t.phase == TouchPhase::Started {
                    // the first touch back on the app only brings it back
                    if self.revealed || grace(self.returned) {
                        if self.revealed {
                            self.back();
                        }
                        self.returned = None;
                        self.swallowed = Some(t.id);
                        return Vec::new();
                    }
                    if self.edge.is_none() && self.pull.is_none() && in_zone(t.location) {
                        log::info!("taskbar swipe: a touch at the edge ({:.0}, {:.0})", t.location.x, t.location.y);
                        self.edge = Some(Edge { id: t.id, start: t.location, held: vec![event] });
                        return Vec::new();
                    }
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, .. } if self.revealed || grace(self.returned) => {
                if self.revealed {
                    self.back();
                }
                self.returned = None;
                self.mouse_swallowed = true;
                return Vec::new();
            }
            WindowEvent::MouseInput { state: ElementState::Released, .. } if self.mouse_swallowed => {
                self.mouse_swallowed = false;
                return Vec::new();
            }
            _ => {}
        }
        vec![event]
    }
}

/// Slow at the end, as a page settles: a quintic ease out.
fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(5)
}

/// Brings the taskbar up over the full-screen app, as the Windows key does, the app staying as it
/// is underneath. Touching the app again puts the taskbar back behind it.
#[cfg(windows)]
#[allow(unsafe_code)]
fn show_taskbar() {
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SW_SHOW, SetForegroundWindow, ShowWindow};
    use windows::core::{PCWSTR, w};
    // SAFETY: the class name is a static string, and the window found is only shown and brought
    // to the front, nothing kept of it
    unsafe {
        if let Ok(tray) = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) {
            let _ = ShowWindow(tray, SW_SHOW);
            let _ = SetForegroundWindow(tray);
        }
    }
}

#[cfg(not(windows))]
fn show_taskbar() {}
