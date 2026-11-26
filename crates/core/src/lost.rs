//! What a lost GPU device leaves of the open artwork, and how it comes back.
//!
//! The layers' pixels and their undo steps live on the GPU, so a device lost (its driver reset or
//! stopped) takes them with it: what was painted since the last save is gone. The artwork's folder
//! still holds that save whole, and it is never written over from a lost device (a save refuses,
//! see `State::save_snapshot`): it is the newest whole copy there is.
//!
//! - [`Shared::let_go_of_lost_canvas`]: the lost canvas and device are let go (their memory freed
//!   at once). From then on no canvas is made from what is left (it would be empty) and nothing
//!   is saved, until the artwork is opened again.
//! - [`Shared::reopen_after_loss`]: the artwork opened again in place on a new device, from its
//!   folder as last saved, just as an open makes it, keeping the app's settings, the window and the
//!   tools; the app is told once what was kept.
//! - [`CanvasSession::save_to_leave`] leaves an artwork whose device is lost at once, as last
//!   saved, with what to tell ([`crate::Leaving::Lost`]): staying would save nothing either.

use std::path::Path;
use std::sync::atomic::Ordering::Relaxed;

use procraft_gpu::Gpu;

use crate::project::file_error;
use crate::{CanvasSession, Leaving, MobileError, Shared, State};

impl State {
    /// Whether the artwork's pixels went with a lost GPU device: the device it is on is lost, or
    /// the canvas that held them was let go for one ([`Shared::let_go_of_lost_canvas`]). Nothing
    /// can be read or saved from it then.
    pub(crate) fn lost_device(&self) -> bool {
        self.pixels_lost || self.gpu.as_ref().is_some_and(Gpu::is_lost)
    }
}

// (called by the render thread once it has made the new device; until then by the tests alone)
#[cfg_attr(not(test), allow(dead_code))]
impl Shared {
    /// Lets go of the canvas and the device, both lost (or about to be: a canvas is never read
    /// again after this), and what else holds the device's memory: the crops kept for undo, the
    /// brush previews' canvas, the sharp picture, the display's table. A canvas let go so held the
    /// artwork's pixels: no canvas is made again from what is left and nothing is saved until
    /// [`Shared::reopen_after_loss`] (a save refuses, the folder keeping the last). The artwork's
    /// edits never go back (the apps compare them). Safe to call more than once.
    pub(crate) fn let_go_of_lost_canvas(&self) {
        let held = {
            let mut guard = self.lock();
            let st = &mut *guard;
            st.revision = st.edits();
            if st.canvas.is_some() {
                st.pixels_lost = true;
            }
            st.stroke = None;
            st.second = None;
            st.airbrush = None;
            st.display_lut = None;
            st.picker_program = None;
            let sharp = std::mem::take(&mut st.sharp);
            (st.canvas.take(), st.crop_undo.take(), st.crop_redo.take(), st.preview.take(), sharp, st.gpu.take())
        };
        // (let go outside the hold: a canvas has many textures)
        drop(held);
        self.drop_queued_input();
    }

    /// Opens the artwork again on `gpu`, a new device made after its own was lost, from its folder
    /// as last saved: the document, every layer, the references and what else the folder keeps,
    /// as [`CanvasSession::open`] makes them, the canvas made (as [`Shared::make_canvas`] makes
    /// it) before this returns. Whatever is left of the lost canvas is let go first (see
    /// [`Shared::let_go_of_lost_canvas`]); it need not be there. Kept as they were: the window, the
    /// view (when the canvas is the same size), the tools, brushes, colours and sliders, the app's
    /// settings and plugins, the time-lapse and the time counting; what was under way on the
    /// canvas (a stroke, a transform, an adjustment, a selection) and its undo steps went with it.
    /// It counts as saved (it is the save), the autosave goes on with the next change, and the app
    /// is told once (`take_failure`) what was kept; [`CanvasSession::reopened_after_loss`] counts
    /// it, for the apps to read the layers and thumbnails again.
    ///
    /// Never saved yet: its document stays, its layers empty. Never shown (what was opened is
    /// still there, no canvas made from it): only the device is new. Let go of
    /// ([`CanvasSession::release_canvas`]): nothing. Call it with neither the state's lock nor the
    /// save's held. An error if the folder's artwork couldn't be read or `gpu` is lost already:
    /// the session then holds `gpu` but makes no canvas and saves nothing (leaving it keeps the
    /// folder as it is), and the app is told.
    pub(crate) fn reopen_after_loss(&self, gpu: Gpu) -> Result<(), MobileError> {
        self.let_go_of_lost_canvas();
        // no save meanwhile (one under way on the lost device finishes first, failing)
        let mut saving = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        let (closed, pixels_lost) = {
            let st = self.lock();
            (st.closed, st.pixels_lost)
        };
        if closed {
            return Ok(());
        }
        if !pixels_lost {
            // what was opened is all there is: only the device is new
            {
                let mut st = self.lock();
                st.gpu = Some(gpu);
                st.canvas_failed = false;
                st.error = None;
                st.reconfigure = true;
                st.needs_frame = true;
            }
            drop(saving);
            self.make_canvas();
            self.wake.notify_all();
            return Ok(());
        }
        let dir = saving.folder().to_path_buf();
        let saved = !dir.as_os_str().is_empty() && dir.join("artwork.json").is_file();
        let fresh = if gpu.is_lost() {
            Err(file_error("opening the artwork again", "the new graphics device was lost too"))
        } else if saved {
            CanvasSession::open(dir.to_string_lossy().into_owned())
        } else {
            let st = self.lock();
            Ok(CanvasSession::with_document(st.doc.clone(), st.name.clone(), Vec::new()))
        };
        let fresh = match fresh {
            Ok(fresh) => fresh,
            Err(e) => {
                log::error!("the artwork couldn't be opened again after the GPU device was lost: {e}");
                let mut st = self.lock();
                st.gpu = Some(gpu);
                st.error = Some(e.to_string());
                st.crop_edit.failure = Some(REOPEN_FAILED_SAID.to_owned());
                st.needs_frame = true;
                return Err(e);
            }
        };
        let fresh_saving = std::mem::take(&mut *fresh.shared.saving.lock().unwrap_or_else(|e| e.into_inner()));
        let edits = {
            let mut guard = self.lock();
            let mut fresh_guard = fresh.shared.lock();
            // the open's state in place, the lost one's in the session let go below
            std::mem::swap(&mut *guard, &mut *fresh_guard);
            let (st, old) = (&mut *guard, &mut *fresh_guard);
            carry_over(st, old);
            if !saved {
                // never saved: what the artwork keeps of itself is all as it was
                keep_unsaved_artwork(st, old);
            }
            st.gpu = Some(gpu);
            st.revision = old.revision.wrapping_add(1);
            st.reconfigure = true;
            st.needs_frame = true;
            st.crop_edit.failure = Some(reopened_said(saved.then_some(dir.as_path())));
            st.edits()
        };
        saving.reopened(fresh_saving);
        drop(saving);
        // (it is the save: nothing to save until it changes)
        self.edits_saved.store(if saved { edits } else { crate::project::NOT_SAVED }, Relaxed);
        self.reopened.fetch_add(1, Relaxed);
        log::warn!(
            "the artwork opened again after the GPU device was lost: {}",
            if saved { "as last saved" } else { "never saved, its layers empty" }
        );
        // the lost artwork's state let go (outside the holds)
        drop(fresh);
        self.make_canvas();
        self.wake.notify_all();
        Ok(())
    }
}

impl CanvasSession {
    /// [`CanvasSession::save_to_leave`] for an artwork whose GPU device was lost: left at once, as
    /// its folder `dir` has it (nothing more can be saved, nor would be by staying), with what to
    /// tell; plainly left if nothing changed since its last save. None while its device is well.
    pub(crate) fn leave_lost(&self, dir: &Path) -> Option<Leaving> {
        let edits = {
            let st = self.shared.lock();
            if !st.lost_device() {
                return None;
            }
            st.edits()
        };
        log::warn!("leaving an artwork whose GPU device was lost: kept as last saved");
        if self.edits_saved() == Some(edits) {
            return Some(Leaving::Left);
        }
        let saved = dir.join("artwork.json").is_file();
        Some(Leaving::Lost { message: left_lost_said(saved.then_some(dir)) })
    }
}

#[uniffi::export]
impl CanvasSession {
    /// How many times the artwork was opened again from its folder after its GPU device was lost
    /// (see `lost.rs`): when it changes, the layers, their thumbnails, the undo buttons and the
    /// canvas's size are to be read again.
    pub fn reopened_after_loss(&self) -> u64 {
        self.shared.reopened.load(Relaxed)
    }
}

/// What is told when the artwork couldn't be opened again after its device was lost.
const REOPEN_FAILED_SAID: &str = "The graphics stopped working, and the artwork couldn't be opened again. \
It is kept as it was last saved: go back to the gallery and open it again.";

/// What is told once the artwork is opened again after its device was lost: from its folder
/// `dir`, saved there how long ago; never saved (none).
fn reopened_said(dir: Option<&Path>) -> String {
    match dir {
        Some(dir) => format!(
            "The graphics stopped working and were started again. The artwork is back as it was last saved{}; \
             changes made after that couldn't be kept.",
            saved_ago(dir).map(|ago| format!(", {ago}")).unwrap_or_default()
        ),
        None => "The graphics stopped working and were started again. The artwork hadn't been saved yet, so its \
                 layers start again empty."
            .to_owned(),
    }
}

/// What is told as an artwork whose device was lost is left: kept in its folder `dir`, saved
/// there how long ago; never saved (none).
fn left_lost_said(dir: Option<&Path>) -> String {
    match dir {
        Some(dir) => format!(
            "The graphics stopped working, so the latest changes couldn't be saved. The artwork is kept as it was \
             last saved{}.",
            saved_ago(dir).map(|ago| format!(", {ago}")).unwrap_or_default()
        ),
        None => "The graphics stopped working before this artwork was first saved, so it couldn't be kept.".to_owned(),
    }
}

/// How long ago the artwork in `dir` was last saved, in words ("12 seconds ago"); none if that
/// can't be told.
fn saved_ago(dir: &Path) -> Option<String> {
    let at = std::fs::metadata(dir.join("artwork.json")).ok()?.modified().ok()?;
    let seconds = std::time::SystemTime::now().duration_since(at).ok()?.as_secs();
    Some(ago(seconds))
}

/// `seconds` ago, in words.
fn ago(seconds: u64) -> String {
    match seconds {
        0..=1 => "a moment ago".to_owned(),
        2..=59 => format!("{seconds} seconds ago"),
        60..=119 => "a minute ago".to_owned(),
        120..=3599 => format!("{} minutes ago", seconds / 60),
        3600..=7199 => "an hour ago".to_owned(),
        _ => format!("{} hours ago", seconds / 3600),
    }
}

/// The artwork opened again in place (see [`Shared::reopen_after_loss`]): what `old`, the lost
/// artwork's state, has that isn't the artwork's taken into `st`, the open's: the window and the
/// frames, the tools and brushes, the colours, the app's settings and plugins, the time-lapse.
/// Everything else is as the open made it: the artwork as last saved, nothing under way on it,
/// nothing kept on the lost device.
fn carry_over(st: &mut State, old: &mut State) {
    use std::mem::swap;
    // the window and its frames
    if (st.doc.width(), st.doc.height()) == (old.doc.width(), old.doc.height()) {
        swap(&mut st.view, &mut old.view);
    }
    swap(&mut st.window_size, &mut old.window_size);
    swap(&mut st.wide_colour_allowed, &mut old.wide_colour_allowed);
    swap(&mut st.frame_due, &mut old.frame_due);
    swap(&mut st.frame_spacing, &mut old.frame_spacing);
    swap(&mut st.holding_frames, &mut old.holding_frames);
    swap(&mut st.frame_held, &mut old.frame_held);
    swap(&mut st.stop, &mut old.stop);
    swap(&mut st.hidden, &mut old.hidden);
    swap(&mut st.projection_size, &mut old.projection_size);
    swap(&mut st.projection_stop, &mut old.projection_stop);
    swap(&mut st.frames, &mut old.frames);
    swap(&mut st.frame_drawn_at, &mut old.frame_drawn_at);
    swap(&mut st.frame_interval_ms, &mut old.frame_interval_ms);
    swap(&mut st.predicting, &mut old.predicting);
    swap(&mut st.display_scale, &mut old.display_scale);
    swap(&mut st.interface_size, &mut old.interface_size);
    swap(&mut st.fit_insets, &mut old.fit_insets);
    swap(&mut st.room_insets, &mut old.room_insets);
    swap(&mut st.backdrops, &mut old.backdrops);
    swap(&mut st.backdrop_windows, &mut old.backdrop_windows);
    swap(&mut st.screen_follow, &mut old.screen_follow);
    swap(&mut st.closed, &mut old.closed);
    // the time-lapse, its frames kept in order
    swap(&mut st.lapse_wanted, &mut old.lapse_wanted);
    swap(&mut st.lapse_keeper, &mut old.lapse_keeper);
    swap(&mut st.tracking_since, &mut old.tracking_since);
    // the tools, brushes and colours
    swap(&mut st.tool, &mut old.tool);
    swap(&mut st.previous_tool, &mut old.previous_tool);
    swap(&mut st.input, &mut old.input);
    swap(&mut st.library, &mut old.library);
    swap(&mut st.libraries, &mut old.libraries);
    swap(&mut st.shipped, &mut old.shipped);
    swap(&mut st.fallback, &mut old.fallback);
    swap(&mut st.paint_brush, &mut old.paint_brush);
    swap(&mut st.erase_brush, &mut old.erase_brush);
    swap(&mut st.smudge_brush, &mut old.smudge_brush);
    swap(&mut st.set_choices, &mut old.set_choices);
    swap(&mut st.recent_brushes, &mut old.recent_brushes);
    swap(&mut st.pinned_brushes, &mut old.pinned_brushes);
    swap(&mut st.quick_brushes, &mut old.quick_brushes);
    swap(&mut st.sliders, &mut old.sliders);
    swap(&mut st.strokes, &mut old.strokes);
    swap(&mut st.drawn_previews, &mut old.drawn_previews);
    swap(&mut st.grain_store, &mut old.grain_store);
    swap(&mut st.library_kept, &mut old.library_kept);
    swap(&mut st.engine_studio, &mut old.engine_studio);
    swap(&mut st.brushes_changed, &mut old.brushes_changed);
    swap(&mut st.studio, &mut old.studio);
    swap(&mut st.studio_second, &mut old.studio_second);
    swap(&mut st.pad_strokes, &mut old.pad_strokes);
    swap(&mut st.pad_sample, &mut old.pad_sample);
    swap(&mut st.color, &mut old.color);
    swap(&mut st.secondary, &mut old.secondary);
    swap(&mut st.colour_history, &mut old.colour_history);
    swap(&mut st.clone_sliders, &mut old.clone_sliders);
    // the app's settings
    swap(&mut st.selection_visibility, &mut old.selection_visibility);
    swap(&mut st.selection_display, &mut old.selection_display);
    swap(&mut st.brush_on_screen, &mut old.brush_on_screen);
    swap(&mut st.reduce_motion, &mut old.reduce_motion);
    swap(&mut st.grey_view, &mut old.grey_view);
    swap(&mut st.grid_origin, &mut old.grid_origin);
    swap(&mut st.light, &mut old.light);
    swap(&mut st.x_ray, &mut old.x_ray);
    swap(&mut st.turns_follow_view, &mut old.turns_follow_view);
    swap(&mut st.transform_mode, &mut old.transform_mode);
    swap(&mut st.interpolation, &mut old.interpolation);
    swap(&mut st.transform_guides, &mut old.transform_guides);
    swap(&mut st.simplified_undos, &mut old.simplified_undos);
    st.adjust = old.adjust.keeping_library();
    // the plugins, as the session last took them up
    swap(&mut st.hooks, &mut old.hooks);
    swap(&mut st.plugin_stepping, &mut old.plugin_stepping);
    swap(&mut st.plugin_look, &mut old.plugin_look);
    swap(&mut st.plugin_dim, &mut old.plugin_dim);
    swap(&mut st.plugin_paper, &mut old.plugin_paper);
    swap(&mut st.plugin_sets, &mut old.plugin_sets);
    swap(&mut st.brushes_dir, &mut old.brushes_dir);
    swap(&mut st.paper_picture, &mut old.paper_picture);
    swap(&mut st.plugin_surround, &mut old.plugin_surround);
    swap(&mut st.surround_picture, &mut old.surround_picture);
    swap(&mut st.filter_layers.defs, &mut old.filter_layers.defs);
    swap(&mut st.sound_heard, &mut old.sound_heard);
    swap(&mut st.sound_hover, &mut old.sound_hover);
    // a plugin's own artwork stays with its plugin (its data is the session's, not the device's)
    if old.plugin_artwork.is_some() {
        swap(&mut st.plugin_artwork, &mut old.plugin_artwork);
        st.plugin_opened = None;
    }
}

/// An artwork never saved, opened again in place: what it keeps of itself (beside its layers'
/// pixels, which are gone) taken from `old` into `st`.
fn keep_unsaved_artwork(st: &mut State, old: &mut State) {
    use std::mem::swap;
    swap(&mut st.info, &mut old.info);
    swap(&mut st.turns, &mut old.turns);
    swap(&mut st.reference_window, &mut old.reference_window);
    swap(&mut st.reference_picture, &mut old.reference_picture);
    swap(&mut st.more_references, &mut old.more_references);
    swap(&mut st.plugin_values, &mut old.plugin_values);
    swap(&mut st.plugin_opened, &mut old.plugin_opened);
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use procraft_gpu::{Gpu, wgpu};

    use super::ago;
    use crate::tests::{pixel, sample, session};
    use crate::{CanvasSession, Leaving, PointerKind};

    /// A folder of the tests' own, empty.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("procraft-lost-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn text(dir: &Path) -> String {
        dir.to_string_lossy().into_owned()
    }

    /// Every file under `dir` with its bytes, in order (the time-lapse's folder left out).
    fn files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut found = Vec::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(at) = todo.pop() {
            for entry in std::fs::read_dir(&at).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n != "timelapse") {
                        todo.push(path);
                    }
                } else {
                    found.push((path.clone(), std::fs::read(&path).unwrap_or_default()));
                }
            }
        }
        found.sort();
        found
    }

    /// How many of `before`'s files differ in `after` (changed, gone or new).
    fn differing(before: &[(PathBuf, Vec<u8>)], after: &[(PathBuf, Vec<u8>)]) -> usize {
        let changed = before.iter().filter(|f| !after.contains(f)).count();
        changed + after.iter().filter(|(p, _)| !before.iter().any(|(q, _)| q == p)).count()
    }

    /// A stroke across the canvas at height `y`.
    fn stroke(s: &CanvasSession, y: f32) {
        s.stroke_begin(PointerKind::Finger, vec![sample(8.0, y, 0.0)]);
        s.stroke_add((1..6).map(|i| sample(8.0 + i as f32 * 8.0, y, f64::from(i) * 8.0)).collect());
        s.stroke_end(vec![]);
    }

    /// Every layer's pixels, as the canvas has them.
    fn contents(s: &CanvasSession) -> Vec<Vec<u8>> {
        let _ = pixel(s, 0, 0);
        s.layers().into_iter().map(|l| s.copy_layer_contents(l.id).map(|c| c.rgba).unwrap_or_default()).collect()
    }

    /// The session's device lost, as a driver reset loses it.
    fn lose(s: &CanvasSession) -> Gpu {
        let gpu = s.shared.lock().gpu.clone().unwrap();
        gpu.device.destroy();
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        assert!(gpu.is_lost());
        gpu
    }

    /// The device lost mid-session with work not yet saved: strokes, saves, exports, the autosave
    /// and leaving all write nothing (not a byte of the folder changes), letting go of the lost
    /// canvas makes no blank one; opened again on a new device the artwork has the last save's
    /// pixels exactly, says so once, counts as saved, and then draws and saves (the autosave
    /// going on) as before.
    #[test]
    fn a_lost_device_writes_nothing_and_the_artwork_comes_back_as_last_saved_then_draws_and_saves() {
        let Some(s) = session(64, 64) else { return };
        let dir = folder("session");
        let path = text(&dir);
        stroke(&s, 16.0);
        s.add_layer();
        stroke(&s, 32.0);
        s.save(path.clone()).unwrap();
        let saved = contents(&s);
        let kept = files(&dir);
        assert!(saved.iter().all(|px| px.iter().any(|&v| v != 0)), "every layer painted");
        // painted on since, and the device goes before the autosave's turn
        stroke(&s, 48.0);
        let _ = pixel(&s, 0, 0);
        let lost = lose(&s);
        Arc::clone(&s).start_autosave(path.clone());
        for y in [10.0, 30.0, 50.0] {
            stroke(&s, y);
        }
        assert!(s.save(path.clone()).is_err(), "a save from the lost device fails");
        let export = std::env::temp_dir().join(format!("procraft-lost-export-{}.png", std::process::id()));
        assert!(s.export_png(text(&export)).is_err());
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(s.take_save_failure(), None, "the loss alone is told, not a failed save on top");
        let written = differing(&kept, &files(&dir));
        // the device side lets go of the lost canvas: still nothing written, and no canvas of
        // empty layers made from what is left, even with a working device given
        s.shared.let_go_of_lost_canvas();
        {
            let st = s.shared.lock();
            assert!(st.gpu.is_none() && st.canvas.is_none() && st.lost_device());
        }
        assert!(s.save(path.clone()).is_err());
        let Ok(spare) = Gpu::headless() else { return };
        {
            let mut st = s.shared.lock();
            st.gpu = Some(spare);
            st.ensure_canvas();
            assert!(st.canvas.is_none(), "no blank canvas from what a lost one left");
        }
        assert!(s.save(path.clone()).is_err(), "nor a save of one");
        s.shared.make_canvas();
        assert!(s.shared.lock().canvas.is_none());
        let written_after = differing(&kept, &files(&dir));
        // opened again on a new device (its pipelines made first, as the app's device has them
        // kept: what is timed is the opening again)
        let Ok(gpu) = Gpu::headless() else { return };
        let warmed = Instant::now();
        gpu.prepare_canvases();
        let warming = warmed.elapsed();
        let started = Instant::now();
        s.shared.reopen_after_loss(gpu).unwrap();
        let took = started.elapsed();
        let back = contents(&s);
        let off = back.iter().zip(&saved).map(|(a, b)| a.iter().zip(b).filter(|(x, y)| x != y).count()).sum::<usize>();
        let told = s.take_failure();
        eprintln!(
            "lost device: {written} files written while lost, {written_after} after letting go; reopened in {took:?} \
             (the new device's pipelines {warming:?} before), {} layers back of {}, {off} bytes off the last save; told \
             {told:?}",
            back.len(),
            saved.len()
        );
        assert_eq!(written, 0, "nothing written while lost");
        assert_eq!(written_after, 0, "nor once the lost canvas went");
        assert_eq!(back.len(), saved.len());
        assert_eq!(off, 0, "the last save's pixels exactly");
        // (0.44 s alone on the PC; a bound for a wait gone wrong, not a benchmark)
        assert!(took < Duration::from_secs(10), "reopened in {took:?}");
        assert!(told.as_deref().is_some_and(|t| t.contains("last saved")), "{told:?}");
        assert_eq!(s.take_failure(), None, "told once");
        assert_eq!(s.reopened_after_loss(), 1);
        assert_eq!(s.edits_saved(), Some(s.edits()), "it is the save: nothing to save");
        assert!(!s.shared.lock().lost_device());
        drop(lost);
        // the next strokes draw and the autosave keeps them
        stroke(&s, 56.0);
        let drawn = contents(&s);
        assert_ne!(drawn, saved, "drawn on");
        let edits = s.edits();
        let until = Instant::now() + Duration::from_secs(20);
        while s.edits_saved() != Some(edits) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        s.stop_autosave();
        assert_eq!(s.edits_saved(), Some(edits), "the autosave went on after the device came back");
        let again = CanvasSession::open(path.clone()).unwrap();
        let Some(gpu) = s.shared.lock().gpu.clone() else { return };
        again.use_gpu(gpu);
        assert_eq!(contents(&again), drawn, "the next session opens with what was drawn after");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&export);
    }

    /// Leaving an artwork whose device was lost never asks to stay (staying couldn't save it): it
    /// is left at once, its folder as last saved, with what to tell; plainly left if nothing had
    /// changed since; and one never saved says it couldn't be kept.
    #[test]
    fn leaving_with_the_device_lost_never_traps_and_keeps_the_last_save() {
        let Some(s) = session(64, 64) else { return };
        let dir = folder("leave");
        let path = text(&dir);
        stroke(&s, 20.0);
        s.save(path.clone()).unwrap();
        let kept = files(&dir);
        stroke(&s, 40.0);
        let _ = pixel(&s, 0, 0);
        lose(&s);
        let Leaving::Lost { message } = Arc::clone(&s).save_to_leave(path.clone()) else {
            panic!("asked to stay on a lost device")
        };
        assert!(message.contains("kept as it was last saved"), "{message}");
        assert_eq!(differing(&kept, &files(&dir)), 0);
        // nothing changed since the last save
        let Some(unchanged) = session(64, 64) else { return };
        let dir2 = folder("leave-unchanged");
        stroke(&unchanged, 20.0);
        unchanged.save(text(&dir2)).unwrap();
        lose(&unchanged);
        assert_eq!(Arc::clone(&unchanged).save_to_leave(text(&dir2)), Leaving::Left);
        // never saved
        let Some(new) = session(64, 64) else { return };
        let dir3 = folder("leave-new");
        stroke(&new, 20.0);
        let _ = pixel(&new, 0, 0);
        lose(&new);
        let Leaving::Lost { message } = Arc::clone(&new).save_to_leave(text(&dir3)) else { panic!("asked to stay") };
        assert!(message.contains("first saved"), "{message}");
        assert!(!dir3.join("artwork.json").exists(), "nothing written for it");
        for d in [dir, dir2, dir3] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// One never saved comes back on the new device with its document (its layers) and its layers
    /// empty, and says so; the tools and settings the app gave stay.
    #[test]
    fn an_artwork_never_saved_comes_back_empty_with_its_layers_and_the_apps_settings() {
        let Some(s) = session(64, 64) else { return };
        s.add_layer();
        s.set_color(0.2, 0.6, 0.9);
        stroke(&s, 30.0);
        let layers = s.layers().len();
        let colour = s.color();
        let window = s.shared.lock().window_size;
        lose(&s);
        s.shared.let_go_of_lost_canvas();
        let Ok(gpu) = Gpu::headless() else { return };
        s.shared.reopen_after_loss(gpu).unwrap();
        let back = contents(&s);
        assert_eq!(back.len(), layers);
        assert!(back.iter().all(|px| px.iter().all(|&v| v == 0)), "its layers empty");
        assert!(s.take_failure().is_some_and(|t| t.contains("hadn't been saved")));
        assert_eq!(s.edits_saved(), None);
        assert_eq!(s.color(), colour);
        assert_eq!(s.shared.lock().window_size, window);
        // it draws
        stroke(&s, 30.0);
        assert!(contents(&s).iter().any(|px| px.iter().any(|&v| v != 0)));
    }

    /// A time-lapse frame asked for on a lost device is none, never tried on the GPU (so no
    /// error is logged for it), and doesn't stop the artwork being saved once it is back; the
    /// frames go on after.
    #[test]
    fn time_lapse_frames_on_a_lost_device_are_none_and_never_stop_a_save() {
        let Some(s) = session(64, 64) else { return };
        let dir = folder("lapse");
        let path = text(&dir);
        stroke(&s, 20.0);
        s.save(path.clone()).unwrap();
        let lapse = crate::TimeLapse::new(path.clone());
        assert!(s.record_time_lapse(Arc::clone(&lapse)));
        assert_eq!(lapse.count(), 1);
        stroke(&s, 40.0);
        let lost = lose(&s);
        let errors = lost.errors();
        assert!(!s.record_time_lapse(Arc::clone(&lapse)), "no frame from a lost device");
        assert!(s.time_lapse_frame(256).is_none());
        assert_eq!(lapse.count(), 1);
        assert_eq!(lost.errors(), errors, "nothing tried on the lost device for it");
        let Ok(gpu) = Gpu::headless() else { return };
        s.shared.reopen_after_loss(gpu).unwrap();
        stroke(&s, 50.0);
        assert!(s.save(path.clone()).is_ok(), "saved once back");
        assert!(s.record_time_lapse(Arc::clone(&lapse)));
        assert_eq!(lapse.count(), 2, "the frames go on");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn how_long_ago_a_save_was_is_said_in_words() {
        assert_eq!(ago(0), "a moment ago");
        assert_eq!(ago(12), "12 seconds ago");
        assert_eq!(ago(61), "a minute ago");
        assert_eq!(ago(600), "10 minutes ago");
        assert_eq!(ago(3700), "an hour ago");
        assert_eq!(ago(3 * 3600 + 5), "3 hours ago");
    }
}
