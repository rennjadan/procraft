//! Full screen, as F11 makes it; a finger swipe that shows the taskbar over it; and the curtain
//! the app fades in from as it opens and out to as it closes for an update.
//!
//! The window never changes size or leaves full screen for the taskbar: it is brought up over the
//! app, as the Windows key brings it up over a full-screen game, while the app eases back under a
//! soft veil that follows the finger. A finger or the pen dragged down from the middle of the top
//! bar does it, at any speed (or up from the middle of the bottom, as Prefs say); a tap there, a
//! drag along the bar, a hold or a second finger goes to the app as it came. The next touch or click
//! on the app eases the veil away, and Windows puts the taskbar back behind the app.

use std::time::Instant;

use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, TouchPhase, WindowEvent};
use winit::window::Window;

use crate::files;

/// Where a swipe may start: this far in from its edge (points, the top bar's depth and a little),
/// and the middle part of the window's width.
const SWIPE_DEPTH: f64 = 72.0;
const SWIPE_MIDDLE: f64 = 0.5;
/// How far a touch goes before it is told apart (points).
const DECIDE_AFTER: f64 = 10.0;
/// A touch held still there longer than this (milliseconds) goes to the app (a press held).
const HOLD_MOST_MS: f64 = 600.0;
/// Still, for that: moved less than this (points).
const STILL: f64 = 3.0;
/// How far a pull goes to bring the veil all the way (points), and the part of that past which
/// letting go shows the taskbar.
const PULL_FULL: f64 = 110.0;
const PULL_COMMIT: f32 = 0.35;
/// A flick this fast or faster (points a millisecond) shows it however short it was.
const FLICK: f64 = 0.5;
/// The veil at its darkest, over the whole app.
const VEIL_MOST: f32 = 0.42;
/// How long the veil takes to settle in, and to go (seconds).
const EASE_IN_S: f32 = 0.32;
const EASE_OUT_S: f32 = 0.26;
/// After the app is used again, a touch or click this soon only brings it back (seconds).
const RETURN_GRACE_S: f32 = 0.5;
/// The curtain: frames drawn under it before it lifts, how long it takes to lift, and to fall
/// before the app closes for an update (seconds).
const CURTAIN_FRAMES: u32 = 3;
const CURTAIN_UP_S: f32 = 0.55;
const CURTAIN_DOWN_S: f32 = 0.4;

/// A finger that began where a swipe may, held back until it is known to be one or not.
struct Held {
    id: u64,
    start: PhysicalPosition<f64>,
    since: Instant,
    events: Vec<WindowEvent>,
    /// It has moved (a slow drag): only its lift or its way decides it, not time.
    moved: bool,
}

/// A finger pulling the veil in: where it began, and its last place and time for its speed.
struct Pull {
    id: u64,
    start: PhysicalPosition<f64>,
    last: (f64, Instant),
    speed: f64,
}

/// A level easing from one value to another.
#[derive(Clone, Copy)]
struct Ease {
    from: f32,
    to: f32,
    start: Instant,
    seconds: f32,
}

impl Ease {
    fn at(to: f32) -> Ease {
        Ease { from: to, to, start: Instant::now(), seconds: 0.0 }
    }

    fn level(&self) -> f32 {
        let t = (self.start.elapsed().as_secs_f32() / self.seconds.max(0.01)).min(1.0);
        self.from + (self.to - self.from) * ease(t)
    }

    fn moving(&self) -> bool {
        self.start.elapsed().as_secs_f32() < self.seconds && (self.to - self.from).abs() > 1e-4
    }

    fn to(&self, to: f32, seconds: f32) -> Ease {
        Ease { from: self.level(), to, start: Instant::now(), seconds }
    }
}

/// Full screen, as F11 makes it, kept between runs ("window" in the app's folder), the swipe that
/// shows the taskbar over it, and the curtain. F11 turns full screen off (a plain window) and on.
pub(crate) struct FullScreen {
    /// Full screen wanted (F11 not having turned it off).
    pub(crate) wanted: bool,
    held: Option<Held>,
    pull: Option<Pull>,
    /// The veil's level while a finger pulls it (0..1).
    pulled: f32,
    veil: Ease,
    /// The taskbar is up over the app.
    revealed: bool,
    /// The app was just used again: the next touch or click (within the grace) only does that.
    returned: Option<Instant>,
    /// A touch, or the mouse's press, that only brought the app back: the rest of it reaches nothing.
    swallowed: Option<u64>,
    mouse_swallowed: bool,
    /// Black over everything: lifting once the app has drawn a few frames, falling before it
    /// closes for an update (with what it says meanwhile).
    curtain: Ease,
    frames: u32,
    closing: Option<&'static str>,
    closed: bool,
}

impl FullScreen {
    pub(crate) fn kept() -> FullScreen {
        let text = std::fs::read_to_string(files::app_dir().join("window")).unwrap_or_default();
        let wanted = !text.lines().any(|l| l.trim() == "full_screen false");
        FullScreen {
            wanted,
            held: None,
            pull: None,
            pulled: 0.0,
            veil: Ease::at(0.0),
            revealed: false,
            returned: None,
            swallowed: None,
            mouse_swallowed: false,
            curtain: Ease::at(1.0),
            frames: 0,
            closing: None,
            closed: false,
        }
    }

    /// F11 pressed: full screen on or off from now on, and next time.
    pub(crate) fn chosen(&mut self, full: bool) {
        self.wanted = full;
        self.pull = None;
        self.revealed = false;
        self.veil = self.veil.to(0.0, EASE_OUT_S);
        files::keep_text("window", &format!("full_screen {full}\n"));
    }

    /// The veil's level now (0 none, 1 at its darkest).
    fn level(&self) -> f32 {
        if self.pull.is_some() { self.pulled } else { self.veil.level() }
    }

    /// Whether frames are wanted: the veil or the curtain moving, or a finger held back.
    pub(crate) fn moving(&self) -> bool {
        self.pull.is_some()
            || self.held.is_some()
            || self.veil.moving()
            || self.curtain.moving()
            || self.frames < CURTAIN_FRAMES + 1
    }

    /// The taskbar up, the veil easing the rest of the way in.
    fn reveal(&mut self) {
        log::info!("taskbar swipe: the taskbar shown");
        self.revealed = true;
        self.returned = None;
        self.veil = self.veil.to(1.0, EASE_IN_S);
        show_taskbar();
    }

    /// The app used again: the veil eases away (Windows puts the taskbar back behind it).
    fn back(&mut self) {
        self.revealed = false;
        self.returned = Some(Instant::now());
        self.veil = self.veil.to(0.0, EASE_OUT_S);
    }

    /// The curtain falls, saying `words`, before the app closes (for an update).
    pub(crate) fn close_with(&mut self, words: &'static str) {
        self.closing = Some(words);
        self.curtain = self.curtain.to(1.0, CURTAIN_DOWN_S);
    }

    /// The curtain is down for closing: the app closes now (true once).
    pub(crate) fn take_closed(&mut self) -> bool {
        if self.closing.is_some() && !self.closed && !self.curtain.moving() {
            self.closed = true;
            return true;
        }
        false
    }

    /// A finger held back past its time without being a swipe: what it did, for the app.
    pub(crate) fn take_overdue(&mut self) -> Vec<WindowEvent> {
        match &self.held {
            Some(h) if !h.moved && h.since.elapsed().as_secs_f64() * 1000.0 >= HOLD_MOST_MS => {
                self.held.take().map(|h| h.events).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    /// Draws the veil and the curtain over everything, while they show.
    pub(crate) fn paint(&mut self, ctx: &egui::Context) {
        self.frames = self.frames.saturating_add(1);
        if self.frames == CURTAIN_FRAMES && self.closing.is_none() {
            self.curtain = self.curtain.to(0.0, CURTAIN_UP_S);
        }
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Debug, egui::Id::unique("full screen veil")));
        let screen = ctx.content_rect();
        let veil = self.level().clamp(0.0, 1.0);
        if veil > 0.002 {
            painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha((veil * VEIL_MOST * 255.0).round() as u8));
        }
        let curtain = self.curtain.level().clamp(0.0, 1.0);
        if curtain > 0.002 {
            painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha((curtain * 255.0).round() as u8));
            if let Some(words) = self.closing {
                let ink = egui::Color32::from_white_alpha((curtain * 220.0).round() as u8);
                painter.text(
                    screen.center(),
                    egui::Align2::CENTER_CENTER,
                    words,
                    egui::FontId::proportional(22.0),
                    ink,
                );
            }
        }
        if self.moving() {
            ctx.request_repaint();
        }
    }

    /// Watches the touches and clicks for the swipe and for the app being used again. Returns the
    /// events for the app: none while a finger is held back, those held back (then this one) once
    /// it turns out not to be a swipe, or the event as it came. `edge` is the edge the swipe comes
    /// from: the top (true) or the bottom, or none with it off.
    pub(crate) fn event(&mut self, window: &Window, event: WindowEvent, edge: Option<bool>) -> Vec<WindowEvent> {
        let Some(from_top) = edge.filter(|_| self.wanted && window.fullscreen().is_some()) else {
            self.pull = None;
            let mut out = self.held.take().map(|h| h.events).unwrap_or_default();
            out.push(event);
            return out;
        };
        let points = window.scale_factor().max(0.1);
        let height = f64::from(window.inner_size().height);
        // how far along the swipe `at` is from `start` (down from the top, or up from the bottom)
        let along = |start: PhysicalPosition<f64>, at: PhysicalPosition<f64>| {
            if from_top { at.y - start.y } else { start.y - at.y }
        };
        let width = f64::from(window.inner_size().width);
        let in_area = |at: PhysicalPosition<f64>| {
            let deep = SWIPE_DEPTH * points;
            let across = (at.x - width / 2.0).abs() <= width * SWIPE_MIDDLE / 2.0;
            across && if from_top { at.y <= deep } else { at.y >= height - deep }
        };
        let grace = |r: Option<Instant>| r.is_some_and(|r| r.elapsed().as_secs_f32() < RETURN_GRACE_S);
        match &event {
            WindowEvent::Focused(true) if self.revealed => self.back(),
            WindowEvent::Focused(false) => {
                // a pull cut short by something else taking the screen
                if self.pull.take().is_some() && !self.revealed {
                    self.veil = Ease::at(self.pulled).to(0.0, EASE_OUT_S);
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
                    let travel = along(pull.start, t.location);
                    let ms = now.duration_since(pull.last.1).as_secs_f64() * 1000.0;
                    if ms > 0.5 {
                        pull.speed = (travel - pull.last.0) / points / ms;
                        pull.last = (travel, now);
                    }
                    self.pulled = ((travel / points - DECIDE_AFTER) / PULL_FULL).clamp(0.0, 1.0) as f32;
                    if matches!(t.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                        let lifted = t.phase == TouchPhase::Ended;
                        let commit = lifted && (pull.speed >= FLICK || self.pulled >= PULL_COMMIT);
                        self.pull = None;
                        self.veil = Ease::at(self.pulled);
                        if commit {
                            self.reveal();
                        } else {
                            self.veil = self.veil.to(0.0, EASE_OUT_S);
                        }
                    }
                    return Vec::new();
                }
                // a finger held back while it shows what it is
                if let Some(held) = self.held.as_mut().filter(|h| h.id == t.id) {
                    held.events.push(event.clone());
                    let travel = along(held.start, t.location) / points;
                    let across = (t.location.x - held.start.x).abs() / points;
                    let ms = (held.since.elapsed().as_secs_f64() * 1000.0).max(1.0);
                    let ended = matches!(t.phase, TouchPhase::Ended | TouchPhase::Cancelled);
                    let gone = travel.hypot(across);
                    held.moved |= gone >= STILL;
                    if !ended && travel >= DECIDE_AFTER && travel > across * 1.2 {
                        // a swipe: what the finger began in the app is called off
                        let start = held.start;
                        self.held = None;
                        self.pulled = 0.0;
                        self.pull = Some(Pull { id: t.id, start, last: (travel * points, Instant::now()), speed: 0.0 });
                        log::info!("taskbar swipe: pulling");
                        return Vec::new();
                    }
                    if ended || gone >= DECIDE_AFTER || (ms >= HOLD_MOST_MS && gone < STILL) {
                        // a tap, a stroke or a hold: the app has it as it was
                        return self.held.take().map(|h| h.events).unwrap_or_default();
                    }
                    return Vec::new();
                }
                if t.phase == TouchPhase::Started {
                    // another finger: what was held back is no swipe, it goes first
                    let mut out = self.held.take().map(|h| h.events).unwrap_or_default();
                    // the first touch back on the app only brings it back
                    if self.revealed || grace(self.returned) {
                        if self.revealed {
                            self.back();
                        }
                        self.returned = None;
                        self.swallowed = Some(t.id);
                        return out;
                    }
                    if out.is_empty() && self.pull.is_none() && in_area(t.location) {
                        self.held = Some(Held {
                            id: t.id,
                            start: t.location,
                            since: Instant::now(),
                            events: vec![event],
                            moved: false,
                        });
                        return Vec::new();
                    }
                    out.push(event);
                    return out;
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
        // anything else goes on behind what is held back, in order, once that has gone
        if self.held.is_some() && matches!(event, WindowEvent::Touch(_) | WindowEvent::MouseInput { .. }) {
            let mut out = self.held.take().map(|h| h.events).unwrap_or_default();
            out.push(event);
            return out;
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
