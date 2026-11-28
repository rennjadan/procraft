//! The touches' input as the app's own thread hands it over, never held up by the canvas: taken at
//! once while the canvas is free (or held for no more than a frame usually holds it), and kept, in
//! order, while something holds it longer, for the next call that finds it free to take. The app's
//! thread waiting on the canvas freezes the interface: a heavy stroke's frame, or a read laying a
//! heavy stroke's end at once, held it for seconds on a phone, and the system ended the app as not
//! responding when a touch came meanwhile.
//!
//! What the touches ask of the app comes back with the call that takes the input, so the app does
//! it once the canvas is free. While input is kept, [`CanvasSession::touch_deadline`] asks for a
//! [`CanvasSession::touch_tick`] a frame on, which takes it if no more input comes first.

use std::collections::VecDeque;
use std::time::Duration;

use super::TouchMove;
use crate::{CanvasSession, PenSample, PointerKind, TouchCommand};

/// The longest the app's thread waits for the canvas before keeping its input for later: about a
/// frame, as a frame drawn on a GPU keeping up holds the canvas for a few milliseconds. A hold
/// already this old is taken for a long one, and the input is kept without waiting.
pub(crate) const WAIT_MOST: Duration = Duration::from_millis(20);

/// How soon after the last input the host is asked to tick, while input is kept (ms).
pub(crate) const KEPT_TICK_MS: f64 = 16.0;

/// One input to the touches, kept to be taken in order.
pub(crate) enum Input {
    Down(u64, PointerKind, Vec<PenSample>),
    Moves(Vec<TouchMove>),
    Move(u64, Vec<PenSample>),
    Up(u64, Vec<PenSample>),
    CancelOne(u64),
    Cancel,
    Tick(f64),
    /// Something told between touches that the touches go by (whether the pen says how it leans,
    /// a tool's touch), which input kept before it is taken without.
    Told(Box<dyn FnOnce(&CanvasSession) + Send>),
}

impl Input {
    /// The host's clock as this input tells it (ms), if it does.
    fn time_ms(&self) -> Option<f64> {
        let last = |samples: &[PenSample]| samples.last().map(|s| s.time_ms);
        match self {
            Input::Down(_, _, samples) | Input::Move(_, samples) | Input::Up(_, samples) => last(samples),
            Input::Moves(moves) => moves.iter().filter_map(|m| last(&m.samples)).reduce(f64::max),
            Input::Tick(ms) => Some(*ms),
            Input::CancelOne(_) | Input::Cancel | Input::Told(_) => None,
        }
    }
}

/// The input kept for later, in order, and the host's clock as the newest input told it.
#[derive(Default)]
pub(crate) struct Mail {
    kept: VecDeque<Input>,
    last_ms: f64,
}

impl Mail {
    /// Whether any input is kept.
    pub(crate) fn kept(&self) -> bool {
        !self.kept.is_empty()
    }

    /// Keeps `input` after the rest (a tick in place of one kept last: the newer is the one that
    /// counts).
    fn keep(&mut self, input: Input) {
        if let (Input::Tick(_), Some(Input::Tick(_))) = (&input, self.kept.back()) {
            self.kept.pop_back();
        }
        self.kept.push_back(input);
    }
}

impl CanvasSession {
    /// Hands `input` to the touches (see the module's notes): taken now, after any input kept
    /// before it, if the canvas is free within [`WAIT_MOST`]; else kept. What the touches asked of
    /// the app, from the input taken.
    pub(crate) fn take_input(&self, input: Input) -> Vec<TouchCommand> {
        if !self.shared.touch_mail_on() {
            return self.run_input(input);
        }
        let mut kept = {
            let mut mail = self.shared.touch_mail.lock();
            if let Some(ms) = input.time_ms().filter(|ms| ms.is_finite()) {
                mail.last_ms = mail.last_ms.max(ms);
            }
            if !self.shared.free_soon(WAIT_MOST) {
                mail.keep(input);
                return Vec::new();
            }
            std::mem::take(&mut mail.kept)
        };
        let mut out = Vec::new();
        kept.push_back(input);
        while let Some(next) = kept.pop_front() {
            out.extend(self.run_input(next));
            // something took the canvas for a while meanwhile: the rest kept again, before anything
            // kept since
            if !kept.is_empty() && !self.shared.free_soon(WAIT_MOST) {
                let mut mail = self.shared.touch_mail.lock();
                kept.append(&mut mail.kept);
                mail.kept = kept;
                break;
            }
        }
        out
    }

    /// Tells the touches what `told` does: now, unless input kept for later comes before it, or
    /// (`canvas`: it takes the canvas) something has held the canvas a while; else kept, to be done
    /// in its place among that input.
    pub(crate) fn tell_touches(&self, canvas: bool, told: impl FnOnce(&CanvasSession) + Send + 'static) {
        if self.shared.touch_mail_on() {
            let mut mail = self.shared.touch_mail.lock();
            if !mail.kept.is_empty() || (canvas && !self.shared.free_soon(WAIT_MOST)) {
                mail.keep(Input::Told(Box::new(told)));
                return;
            }
        }
        told(self);
    }

    /// While input is kept for later, when the host is to tick to have it taken (its clock, ms).
    pub(crate) fn kept_input_due(&self) -> Option<f64> {
        let mail = self.shared.touch_mail.lock();
        (!mail.kept.is_empty()).then_some(mail.last_ms + KEPT_TICK_MS)
    }

    fn run_input(&self, input: Input) -> Vec<TouchCommand> {
        match input {
            Input::Down(id, kind, samples) => self.take_down(id, kind, samples),
            Input::Moves(moves) => self.take_moves(moves),
            Input::Move(id, samples) => self.take_move(id, samples),
            Input::Up(id, samples) => self.take_up(id, samples),
            Input::CancelOne(id) => self.take_cancel_one(id),
            Input::Cancel => self.take_cancel(),
            Input::Tick(ms) => self.take_tick(ms),
            Input::Told(told) => {
                told(self);
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::super::TouchSettings;
    use crate::{CanvasSession, PenSample, PointerKind, TouchCommand, TouchMove};

    fn at(x: f32, y: f32, ms: f64) -> Vec<PenSample> {
        vec![PenSample { x, y, pressure: 0.6, tilt: 0.0, orientation: 0.0, time_ms: ms, roll: 0.0 }]
    }

    /// A canvas shown in an 800 × 600 window, the tablets' way with the touches' input.
    fn shown() -> Arc<CanvasSession> {
        let s = CanvasSession::new(400, 300);
        {
            let mut st = s.shared.lock();
            st.window_size = (800, 600);
            st.view = Some(st.fitted_view());
        }
        s.shared.set_touch_mail_on(true);
        // what the interface reads each frame, read once as the artwork opens
        let _ = (s.view_matrix(), s.view_moving(), s.guide(), s.quickshape_info());
        s
    }

    /// What the app's thread does in a while of drawing (a pen stroke, then two fingers pinching
    /// the canvas larger, then what the interface reads each frame as the view settles), each call
    /// timed into `calls` (its name and how long it took): what the touches asked of the app.
    fn draw(s: &CanvasSession, calls: &mut Vec<(String, Duration)>) -> Vec<TouchCommand> {
        let mut out = Vec::new();
        let mut call = |name: &str, f: &mut dyn FnMut() -> Vec<TouchCommand>| {
            let started = Instant::now();
            out.extend(f());
            calls.push((name.to_owned(), started.elapsed()));
        };
        call("set_glass", &mut || {
            s.set_glass(vec![10.0, 10.0, 200.0, 300.0, 12.0, 1.0], Vec::new());
            Vec::new()
        });
        call("set_touch_settings", &mut || {
            s.set_touch_settings(TouchSettings::default());
            Vec::new()
        });
        call("set_touch_panel_open", &mut || {
            s.set_touch_panel_open(false);
            Vec::new()
        });
        call("set_pen_tilt_known", &mut || {
            s.set_pen_tilt_known(true);
            Vec::new()
        });
        call("touch_down (pen)", &mut || s.touch_down(1, PointerKind::Stylus, at(200.0, 200.0, 1000.0)));
        for i in 1..=40 {
            let x = 200.0 + i as f32 * 8.0;
            let ms = 1000.0 + f64::from(i) * 4.0;
            call("touch_moves (pen)", &mut || {
                s.touch_moves(vec![TouchMove { id: 1, samples: at(x, 200.0 + (x / 30.0).sin() * 20.0, ms) }])
            });
            call("stroke_predict", &mut || {
                s.stroke_predict(at(x + 8.0, 200.0, ms + 8.0));
                Vec::new()
            });
        }
        call("touch_up (pen)", &mut || s.touch_up(1, at(520.0, 200.0, 1170.0)));
        call("touch_deadline", &mut || {
            let _ = s.touch_deadline();
            Vec::new()
        });
        call("touch_tick", &mut || s.touch_tick(1180.0));
        call("touch_down (finger)", &mut || s.touch_down(2, PointerKind::Finger, at(300.0, 300.0, 1400.0)));
        call("touch_down (finger)", &mut || s.touch_down(3, PointerKind::Finger, at(500.0, 300.0, 1404.0)));
        for i in 1..=30 {
            let (spread, ms) = (100.0 + i as f32 * 6.0, 1404.0 + f64::from(i) * 5.0);
            call("touch_moves (two fingers)", &mut || {
                s.touch_moves(vec![
                    TouchMove { id: 2, samples: at(400.0 - spread, 300.0, ms) },
                    TouchMove { id: 3, samples: at(400.0 + spread, 300.0, ms) },
                ])
            });
        }
        call("touch_up (finger)", &mut || s.touch_up(3, at(580.0, 300.0, 1560.0)));
        call("touch_up (finger)", &mut || s.touch_up(2, at(220.0, 300.0, 1564.0)));
        call("view_matrix", &mut || {
            let _ = s.view_matrix();
            Vec::new()
        });
        call("view_moving", &mut || {
            let _ = s.view_moving();
            Vec::new()
        });
        call("guide", &mut || {
            let _ = s.guide();
            Vec::new()
        });
        call("quickshape_info", &mut || {
            let _ = s.quickshape_info();
            Vec::new()
        });
        call("touch_deadline", &mut || {
            let _ = s.touch_deadline();
            Vec::new()
        });
        out
    }

    /// Holds `s`'s canvas on a thread of its own for `hold` from when this returns (as a heavy
    /// stroke's frame, or a read laying one's end at once, held it for seconds on a phone).
    fn hold(s: &Arc<CanvasSession>, hold: Duration) -> std::thread::JoinHandle<()> {
        let (taken, held) = std::sync::mpsc::channel();
        let s = Arc::clone(s);
        let holder = std::thread::spawn(move || {
            let _st = s.shared.lock();
            let _ = taken.send(());
            std::thread::sleep(hold);
        });
        let _ = held.recv();
        holder
    }

    /// The app's thread never waits on a canvas something else holds for 3 s: the frosted panels
    /// told, the touches' settings, a pen stroke's every input, a two-finger pinch, the touches'
    /// deadline and tick, and what the interface reads each frame while the view settles each come
    /// back within a frame's wait (bound: 60 ms each; the canvas is waited for 20 ms at most).
    /// Before, each waited for the holder (`set_glass` over 5 s on a tester's phone, and the system
    /// ended the app as not responding). The input kept meanwhile is taken in order by the first
    /// tick after, and does all it did with the canvas free: the same stroke and the same zoom, the
    /// same asks of the app, the panels shown and the pen's lean known. Printed: each call's
    /// longest time.
    #[test]
    fn the_apps_calls_come_back_at_once_while_something_holds_the_canvas_for_seconds() {
        // the same with the canvas free, to compare
        let free = shown();
        let mut free_out = draw(&free, &mut Vec::new());
        let s = shown();
        let holder = hold(&s, Duration::from_secs(3));
        let started = Instant::now();
        let mut calls = Vec::new();
        let mut out = draw(&s, &mut calls);
        let took = started.elapsed();
        assert!(s.shared.touch_input_kept(), "input kept while held");
        let mut longest: Vec<(String, Duration)> = Vec::new();
        for (name, d) in &calls {
            match longest.iter_mut().find(|(n, _)| n == name) {
                Some((_, most)) => *most = (*most).max(*d),
                None => longest.push((name.clone(), *d)),
            }
        }
        for (name, d) in &longest {
            println!("{name}: {:.2} ms at most", d.as_secs_f64() * 1000.0);
        }
        println!("{} calls in {:.0} ms with the canvas held", calls.len(), took.as_secs_f64() * 1000.0);
        holder.join().unwrap();
        for (name, d) in &longest {
            assert!(*d < Duration::from_millis(60), "{name} took {d:?} with the canvas held");
        }
        assert!(took < Duration::from_secs(3), "the calls waited for the canvas: {took:?}");
        // the host ticks as asked, a frame after the last input, and that takes what was kept
        let due = s.touch_deadline().expect("a tick asked for while input is kept");
        assert!((1564.0..=1600.0).contains(&due), "the tick asked for at {due}");
        out.extend(s.touch_tick(due));
        free_out.extend(free.touch_tick(due));
        assert!(!s.shared.touch_input_kept(), "the input kept was taken");
        assert_eq!(out, free_out, "the same asks of the app, in order");
        let (st, free_st) = (s.shared.lock(), free.shared.lock());
        // (the pen's, and the first finger's, taken back as the second came)
        assert_eq!(st.info.strokes, free_st.info.strokes, "the strokes");
        assert!(st.info.strokes >= 1, "the pen's stroke");
        assert!(st.input.tilt_known, "the pen's lean known");
        let zoom = |st: &crate::State| st.view.map_or(0.0, |v| v.zoom());
        assert!((zoom(&st) - zoom(&free_st)).abs() < 1e-5, "zoomed {} as with it free {}", zoom(&st), zoom(&free_st));
        assert!(zoom(&st) > 1.5 * st.fitted_view().zoom(), "the pinch zoomed in");
        drop((st, free_st));
        // the frosted panels, for the next frame to take
        assert!(s.shared.glass_waiting());
        let mut st = s.shared.lock();
        s.shared.take_glass(&mut st);
        assert_eq!(st.backdrops, vec![[10.0, 10.0, 200.0, 300.0, 12.0, 1.0]]);
    }

    /// With the touches' input not kept (the desktop's way, and the tablets' before), a touch
    /// waits for the canvas as long as it is held: printed, the wait for a 1 s hold.
    #[test]
    fn without_the_mail_a_touch_waits_out_the_hold() {
        let s = shown();
        s.shared.set_touch_mail_on(false);
        let holder = hold(&s, Duration::from_secs(1));
        let started = Instant::now();
        s.touch_down(1, PointerKind::Stylus, at(200.0, 200.0, 0.0));
        let waited = started.elapsed();
        holder.join().unwrap();
        println!("a pen's touch waited {:.0} ms on a canvas held 1 s", waited.as_secs_f64() * 1000.0);
        assert!(waited > Duration::from_millis(500), "waited {waited:?}");
    }

    /// A hold no longer than a frame's usual is waited for, as before: the input is taken at once,
    /// none kept.
    #[test]
    fn a_frames_usual_hold_is_waited_for_and_nothing_is_kept() {
        let s = shown();
        let started = Instant::now();
        let holder = hold(&s, Duration::from_millis(5));
        s.touch_down(1, PointerKind::Stylus, at(200.0, 200.0, 0.0));
        let waited = started.elapsed();
        holder.join().unwrap();
        if waited < Duration::from_millis(15) {
            assert!(!s.shared.touch_input_kept(), "a short hold kept the input");
        } else {
            // (a machine too busy to let the holder go within a frame's wait: kept, as it should be)
            println!("the hold ran {waited:?}: the input was kept, and a tick takes it");
            s.touch_tick(16.0);
        }
        assert_eq!(s.shared.lock().info.strokes, 1, "the stroke began");
    }
}
