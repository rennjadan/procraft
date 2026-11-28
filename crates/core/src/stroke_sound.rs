//! Brush sounds (a plugin's): what the stroke sounds like is all the plugin's, on the
//! `stroke_sound` hook. The app tells it how the stroke moves (its speed, pressure, how long since
//! its last input, whether the pen hovers and where) and what the brush is, and the apps play what
//! it hands back.

use std::sync::Arc;
use std::time::Instant;

use crate::hooks::Hooks;
use crate::{CanvasSession, ToolKind};

/// A hover older than this is no longer heard (the pen has gone).
const HOVER_STALE_MS: u128 = 100;
/// While a frame holds the canvas, the sound goes on as last read for this long (milliseconds; a
/// long frame's worth, as the pen's input waits for it too); held longer, the stroke counts as
/// quiet from then on.
const SOUND_HOLD_MS: f32 = 150.0;
/// How far back the speed is measured over, milliseconds.
const SPEED_SPAN_MS: f64 = 20.0;

/// What the sound is told of the stroke, as bits: one is open, the pen hovers, the tool erases.
const PRESSED: i32 = 1;
const HOVERING: i32 = 2;
const ERASING: i32 = 4;

/// The stroke as the sound goes by it: its speed (window heights a second), its pressure (a finger
/// as pressed all the way), how long since its last input (milliseconds; below 0 with none open),
/// what is going on (bits: [`PRESSED`], [`HOVERING`], [`ERASING`]), where the pen hovers (window
/// pixels) and the window's height.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SoundMotion {
    pub(crate) speed: f32,
    pub(crate) pressure: f32,
    pub(crate) since_ms: f32,
    pub(crate) state: i32,
    pub(crate) hover: (f32, f32),
    pub(crate) height: f32,
}

/// What the sound went by when it last read the canvas: the plugins making it, the stroke's motion
/// and the brush, and when it read them.
#[derive(Clone)]
pub(crate) struct SoundRead {
    hooks: Arc<Hooks>,
    motion: SoundMotion,
    brush: SoundBrush,
    at: Instant,
}

/// The brush the sound is told of: its name, its set's, the values of the plugin's rows in the brush
/// studio, and all of it as one number, to tell a change by.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SoundBrush {
    pub(crate) name: String,
    pub(crate) set: String,
    pub(crate) controls: Vec<f32>,
    pub(crate) key: u64,
}

impl crate::State {
    /// The stroke as the sound goes by it now.
    fn sound_motion(&self) -> SoundMotion {
        let pressed = self.stroke.is_some();
        let height = self.window_size.1.max(1) as f32;
        let hover =
            self.sound_hover.filter(|(_, at)| at.elapsed().as_millis() <= HOVER_STALE_MS).map(|(p, _)| (p.x, p.y));
        let mut state = if self.tool == ToolKind::Erase { ERASING } else { 0 };
        if hover.is_some() {
            state |= HOVERING;
        }
        let (hover, height) = (hover.unwrap_or((0.0, 0.0)), height);
        let Some(last) = self.drawn_samples.last().filter(|_| pressed) else {
            return SoundMotion { since_ms: -1.0, state, hover, height, ..SoundMotion::default() };
        };
        let since_ms = self.sound_heard.map_or(f32::MAX, |at| at.elapsed().as_secs_f32() * 1000.0);
        let pressure = if last.pressure > 0.0 { last.pressure.clamp(0.0, 1.0) } else { 1.0 };
        let earlier = self
            .drawn_samples
            .iter()
            .rev()
            .take(32)
            .find(|s| last.time_ms - s.time_ms >= SPEED_SPAN_MS)
            .or(self.drawn_samples.first());
        let ms = earlier.map_or(0.0, |e| (last.time_ms - e.time_ms) as f32);
        let speed = match earlier {
            Some(e) if ms > 0.0 => {
                let zoom = self.view.map_or(1.0, |v| v.zoom());
                last.pos.distance(e.pos) * zoom / ms * 1000.0 / height
            }
            _ => 0.0,
        };
        SoundMotion { speed, pressure, since_ms, state: state | PRESSED, hover, height }
    }

    /// The brush the stroke is drawn with now, for the sound: with the values of the rows `keys`.
    fn sound_brush(&self, keys: &[String]) -> SoundBrush {
        let (set, index) = self.brush_ref(self.tool);
        let set = self.library.get(set);
        let name = set.and_then(|s| s.brushes.get(index)).map_or("", |b| b.name.as_str()).to_owned();
        let set = set.map_or("", |s| s.name.as_str()).to_owned();
        let brush = self.brush(self.tool);
        let controls: Vec<f32> = keys.iter().map(|k| brush.setting(k)).collect();
        let mut hash = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(&(&name, &set, controls.iter().map(|v| v.to_bits()).collect::<Vec<_>>()), &mut hash);
        SoundBrush { name, set, controls, key: std::hash::Hasher::finish(&hash) }
    }
}

#[uniffi::export]
impl CanvasSession {
    /// The pen (or the mouse) hovers over the canvas at (`x`, `y`), window pixels: a sound plugin
    /// is told where (a spray can's ball rattles shaken back and forth).
    pub fn sound_hover(&self, x: f32, y: f32) {
        if x.is_finite() && y.is_finite() {
            self.shared.lock().sound_hover = Some((procraft_engine::geom::Vec2::new(x, y), std::time::Instant::now()));
        }
    }

    /// The next `frames` samples (mono, -1 to 1, at `rate` a second) of the stroke's sound, as the
    /// plugin on the `stroke_sound` hook makes them: silence with none on. An app plays them one
    /// after another as fast as they are used, on a thread of their own. Never waits on the canvas:
    /// while a frame holds it (a big fill's on a large canvas took a tenth of a second and more, and
    /// the sound, waiting for it, ran dry and crackled), it goes on as it last read it.
    pub fn stroke_sound(&self, frames: u32, rate: u32) -> Vec<f32> {
        let frames = frames.min(48_000) as usize;
        let rate = rate.clamp(8_000, 192_000);
        let Some(SoundRead { hooks, motion, brush, .. }) = self.sound_read() else {
            return vec![0.0; frames];
        };
        hooks
            .stroke_sound(frames, rate, &motion, &brush)
            .filter(|s| s.len() == frames)
            .unwrap_or_else(|| vec![0.0; frames])
    }
}

impl CanvasSession {
    /// What the sound goes by now (none with no plugin making it): read from the canvas if nothing
    /// holds it, and kept; else as last read, the time since the stroke's last input going on only
    /// once it has been held past [`SOUND_HOLD_MS`].
    fn sound_read(&self) -> Option<SoundRead> {
        if let Some(st) = self.shared.try_lock() {
            let read = st.hooks.clone().filter(|h| h.makes_stroke_sound()).map(|hooks| {
                let keys = hooks.sound_brush_keys();
                let (motion, brush) = (st.sound_motion(), st.sound_brush(&keys));
                SoundRead { hooks, motion, brush, at: Instant::now() }
            });
            drop(st);
            self.shared.sound_read.lock().clone_from(&read);
            return read;
        }
        let mut read = self.shared.sound_read.lock().clone()?;
        let held_past = read.at.elapsed().as_secs_f32() * 1000.0 - SOUND_HOLD_MS;
        if held_past > 0.0 && read.motion.since_ms >= 0.0 {
            read.motion.since_ms += held_past;
        }
        Some(read)
    }
}
