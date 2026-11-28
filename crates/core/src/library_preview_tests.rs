//! The brush library's sample strokes kept on disk between runs (see `drawn_previews`): a restart
//! drawing none it drew before (the same pixels), what draws one again (the brush changed, another
//! build's drawing, a damaged file), a blank file preview still drawn (#141), the memory and disk
//! they hold, and how long the library takes to show every stock brush the first time and after.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::CanvasSession;
use crate::drawn_previews::{self, DISK_BYTES, KEPT_BYTES};

/// The size the Android and iPad libraries ask for their rows' strokes.
const ROW: (u32, u32) = (530, 162);

/// A folder of its own for a test's brushes, empty.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("procraft-previews-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A session with a GPU and its brushes opened from `dir`, as the app opens them; none without a
/// GPU.
fn opened(dir: &Path) -> Option<Arc<CanvasSession>> {
    let s = CanvasSession::new(64, 64);
    if !s.prepare_offscreen() {
        eprintln!("skipped: no GPU");
        return None;
    }
    s.open_brushes(dir.to_string_lossy().into_owned());
    Some(s)
}

/// The app started again: the kept strokes' listing gone with the run (their files stay), and a
/// session of its own opening the brushes from `dir`.
fn restarted(dir: &Path) -> Option<Arc<CanvasSession>> {
    drawn_previews::written();
    drawn_previews::forget_listing(dir);
    opened(dir)
}

/// Every stock brush's place, the first `most` of them.
fn places(s: &CanvasSession, most: usize) -> Vec<(u32, u32)> {
    s.brush_library().iter().flat_map(|set| set.brushes.iter().map(|b| (b.set, b.index))).take(most).collect()
}

/// How many strokes the session found kept in memory, drew, and found on disk.
fn counts(s: &CanvasSession) -> (u64, u64, u64) {
    let st = s.shared.lock();
    (st.drawn_previews.found, st.drawn_previews.missed, st.drawn_previews.from_disk)
}

/// Whether `a` and `b` are the same stroke drawn twice: alike but for a level or two in a few
/// channels, as the GPU lays the same work when other programs work it hard (see
/// `tests::panel_pictures_and_strokes_come_out_alike_where_copies_start_at_multiples_of_512_bytes`):
/// at most two levels, in at most one channel in ten thousand (16 at the least). Strokes taken
/// from memory or disk are the very pixels kept, and are compared whole.
fn drawn_alike(a: &[u8], b: &[u8]) -> Result<(), String> {
    let (most, count) =
        a.iter().zip(b).fold((0u8, 0usize), |(most, n), (x, y)| (most.max(x.abs_diff(*y)), n + usize::from(x != y)));
    let allowed = (a.len() / 10_000).max(16);
    if a.len() == b.len() && most <= 2 && count <= allowed {
        Ok(())
    } else {
        Err(format!("{} and {} channels, {count} differing (most {most} levels, {allowed} allowed)", a.len(), b.len()))
    }
}

/// Each brush at `at`'s row stroke.
fn rows(s: &CanvasSession, at: &[(u32, u32)]) -> Vec<Vec<u8>> {
    at.iter().map(|&(set, index)| s.brush_preview(set, index, ROW.0, ROW.1).expect("a preview").rgba).collect()
}

/// The library opened again after a restart draws none of the strokes it drew: each comes from
/// disk, the same pixels, and the brushes look the same read back (their keys hold across runs).
/// A session with no brushes' folder keeps none on disk.
#[test]
fn a_restart_draws_none_of_the_strokes_drawn_before() {
    let dir = folder("restart");
    let Some(first) = opened(&dir) else { return };
    let at = places(&first, 24);
    let drawn = rows(&first, &at);
    assert_eq!(counts(&first), (0, 24, 0), "all drawn the first time");
    drawn_previews::written();
    let (files, bytes) = drawn_previews::held_on_disk(&dir);
    assert_eq!(files, 24);
    let looks = |s: &CanvasSession| -> Vec<u64> {
        let st = s.shared.lock();
        at.iter().map(|&(set, i)| drawn_previews::look_key(&st.library[set as usize].brushes[i as usize])).collect()
    };
    let before = looks(&first);
    assert!(first.keep_brushes(dir.to_string_lossy().into_owned()));
    drop(first);
    let second = restarted(&dir).unwrap();
    assert_eq!(looks(&second), before, "the brushes read back look as they did");
    let t = Instant::now();
    assert_eq!(rows(&second, &at), drawn, "the same pixels from disk");
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(counts(&second), (0, 0, 24), "none drawn after the restart");
    // and in memory from then on
    let _ = rows(&second, &at);
    assert_eq!(counts(&second), (24, 0, 24));
    assert!(second.preview_canvas_bytes().is_none(), "no canvas made to draw on");
    println!("24 strokes kept on disk ({} KB), read back in {ms:.1} ms", bytes >> 10);
    // a session without a brushes' folder keeps none on disk (nor finds those kept)
    let bare = CanvasSession::new(64, 64);
    assert!(bare.prepare_offscreen());
    let _ = rows(&bare, &at[..2]);
    assert_eq!(counts(&bare), (0, 2, 0));
    drop(second);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A brush changed in a way that shows is drawn anew (and kept so), its stroke as it was staying
/// for the brush as it was; another build's drawing (or another GPU) finds none of them, and the
/// folder another build kept is taken away.
#[test]
fn a_brush_changed_or_another_build_draws_anew() {
    let dir = folder("changed");
    let Some(s) = opened(&dir) else { return };
    let (set, index) = {
        let b = s.current_brush();
        (b.set, b.index)
    };
    let as_it_was = s.brush_preview(set, index, ROW.0, ROW.1).unwrap().rgba;
    assert_eq!(counts(&s).1, 1);
    s.studio_begin();
    let size = s.studio_settings(vec!["preview_size".to_owned()])[0];
    s.set_studio_setting("preview_size".to_owned(), if size < 0.5 { size + 0.3 } else { size - 0.3 });
    s.studio_end(true);
    let changed = s.brush_preview(set, index, ROW.0, ROW.1).unwrap().rgba;
    assert_eq!(counts(&s), (0, 2, 0), "the brush changed is drawn");
    assert_ne!(changed, as_it_was);
    drawn_previews::written();
    assert_eq!(drawn_previews::held_on_disk(&dir).0, 2);
    assert!(s.keep_brushes(dir.to_string_lossy().into_owned()));
    drop(s);
    // after a restart: the changed brush's stroke, from disk
    let s = restarted(&dir).unwrap();
    assert_eq!(s.brush_preview(set, index, ROW.0, ROW.1).unwrap().rgba, changed);
    assert_eq!(counts(&s), (0, 0, 1));
    drop(s);
    // another build's drawing: its own folder, every stroke drawn again, the old folder taken away
    let old = dir.join(format!("brush-previews-{}", env!("PROCRAFT_PREVIEW_KEY")));
    assert!(old.is_dir());
    drawn_previews::DRAWN_BY_IN_TEST.with(|d| *d.borrow_mut() = Some("another-build".to_owned()));
    let s = restarted(&dir).unwrap();
    let again = s.brush_preview(set, index, ROW.0, ROW.1).unwrap().rgba;
    if let Err(e) = drawn_alike(&again, &changed) {
        panic!("drawn the same: {e}");
    }
    assert_eq!(counts(&s), (0, 1, 0), "another build draws it again");
    let gone = Instant::now();
    while old.exists() && gone.elapsed().as_secs() < 10 {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!old.exists(), "the other build's strokes taken away");
    assert!(dir.join("brush-previews-another-build").is_dir());
    drawn_previews::forget_listing(&dir);
    drawn_previews::DRAWN_BY_IN_TEST.with(|d| *d.borrow_mut() = None);
    drop(s);
    drawn_previews::forget_listing(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A brush brought in with a blank preview in its file (#141) shows a drawn one, kept on disk like
/// any other: still drawn, not blank, after a restart.
#[test]
fn a_blank_file_preview_is_drawn_and_kept() {
    let dir = folder("blank");
    let Some(s) = opened(&dir) else { return };
    let (set, index) = {
        let b = s.current_brush();
        (b.set, b.index)
    };
    let blank: Arc<[u8]> = crate::project::rgba_png(8, 8, &[0u8; 8 * 8 * 4]).into();
    s.shared.lock().library[set as usize].brushes[index as usize].thumbnail = Some(Arc::clone(&blank));
    let painted = |rgba: &[u8]| rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 0).count();
    let drawn = s.brush_preview(set, index, ROW.0, ROW.1).unwrap().rgba;
    assert!(painted(&drawn) > 1000, "{} pixels painted", painted(&drawn));
    assert_eq!(counts(&s), (0, 1, 0));
    drop(s);
    let s = restarted(&dir).unwrap();
    s.shared.lock().library[set as usize].brushes[index as usize].thumbnail = Some(blank);
    assert_eq!(s.brush_preview(set, index, ROW.0, ROW.1).unwrap().rgba, drawn);
    assert_eq!(counts(&s), (0, 0, 1), "from disk, not blank");
    // a file preview that shows something is shown as it is, never kept
    let marker = [200u8, 40, 40, 255].repeat(16 * 16);
    s.shared.lock().library[set as usize].brushes[index as usize].thumbnail =
        Some(crate::project::rgba_png(16, 16, &marker).into());
    assert_eq!(s.brush_preview(set, index, 16, 16).unwrap().rgba, marker);
    assert_eq!(counts(&s), (0, 0, 1));
    drop(s);
    drawn_previews::forget_listing(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A kept stroke's file damaged (cut short, or not a picture) or left half written by a run cut
/// short costs only that stroke drawn again: the damaged file is replaced, the half written one
/// taken away as the folder is first listed.
#[test]
fn a_damaged_kept_stroke_is_drawn_again() {
    let dir = folder("damaged");
    let Some(s) = opened(&dir) else { return };
    let at = places(&s, 3);
    let drawn = rows(&s, &at);
    drawn_previews::written();
    drop(s);
    let kept = dir.join(format!("brush-previews-{}", env!("PROCRAFT_PREVIEW_KEY")));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&kept).unwrap().flatten().map(|e| e.path()).collect();
    files.sort();
    assert_eq!(files.len(), 3);
    let whole = std::fs::read(&files[0]).unwrap();
    std::fs::write(&files[0], &whole[..whole.len() / 2]).unwrap();
    std::fs::write(&files[1], b"not a picture").unwrap();
    let half = kept.join("0123456789abcdef.7.tmp");
    std::fs::write(&half, b"half").unwrap();
    let s = restarted(&dir).unwrap();
    for (again, first) in rows(&s, &at).iter().zip(&drawn) {
        if let Err(e) = drawn_alike(again, first) {
            panic!("the same strokes: {e}");
        }
    }
    assert!(!half.exists(), "a half written file taken away");
    assert_eq!(counts(&s), (0, 2, 1), "the two damaged drawn again, the whole one from disk");
    drawn_previews::written();
    // written whole again: the same stroke (drawn again, a level apart here and there at most)
    let (cw, ch) = (procraft_engine::preview::CANVAS.0 as u32, procraft_engine::preview::CANVAS.1 as u32);
    let unpack = |png: &[u8]| drawn_previews::unpacked(cw, ch, png).expect("a whole stroke");
    if let Err(e) = drawn_alike(&unpack(&std::fs::read(&files[0]).unwrap()), &unpack(&whole)) {
        panic!("written whole again: {e}");
    }
    drop(s);
    drawn_previews::forget_listing(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The key a stroke is kept on disk under: its brush's look on the GPU it was drawn on, the same
/// each time, another for another look or another GPU or driver.
#[test]
fn a_stroke_is_kept_on_disk_by_its_look_and_the_gpu() {
    let on = |look: u64, gpu: &str| drawn_previews::disk_key_on(look, gpu);
    assert_eq!(on(7, "Vulkan Adreno 1.2"), on(7, "Vulkan Adreno 1.2"));
    assert_ne!(on(7, "Vulkan Adreno 1.2"), on(8, "Vulkan Adreno 1.2"));
    assert_ne!(on(7, "Vulkan Adreno 1.2"), on(7, "Gl Adreno 1.2"));
    assert_ne!(on(7, "Vulkan Adreno 1.2"), on(7, "Vulkan Adreno 1.3"));
}

/// Past the most bytes kept on disk the least recently used go, files and all; the listing a
/// run keeps of them is a few bytes a stroke.
#[test]
fn past_the_most_kept_on_disk_the_least_recently_used_go() {
    let dir = folder("most");
    let kept = drawn_previews::folder_for_test(&dir);
    std::fs::create_dir_all(&kept).unwrap();
    // three files a third of what may be kept and a little more, written one after another
    let third = (DISK_BYTES / 3 + 1) as usize;
    for key in 1..=3u64 {
        std::fs::write(kept.join(format!("{key:016x}.png")), vec![1u8; third]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    // listed: the oldest goes, as the three don't fit
    let (files, bytes) = drawn_previews::held_on_disk(&dir);
    assert_eq!(files, 2);
    assert!(bytes <= DISK_BYTES);
    assert!(!kept.join(format!("{:016x}.png", 1)).exists());
    // 2 used again: 3 is the least recently used when another comes
    assert!(drawn_previews::kept_on_disk(&dir, 2).is_some());
    drawn_previews::keep_on_disk(&dir, 4, &vec![2u8; third]);
    assert!(!kept.join(format!("{:016x}.png", 3)).exists());
    assert!(kept.join(format!("{:016x}.png", 2)).exists() && kept.join(format!("{:016x}.png", 4)).exists());
    assert!(drawn_previews::held_on_disk(&dir).1 <= DISK_BYTES);
    // one bigger than all that may be kept is never kept
    drawn_previews::keep_on_disk(&dir, 5, &vec![0u8; DISK_BYTES as usize + 1]);
    assert!(drawn_previews::kept_on_disk(&dir, 5).is_none());
    drawn_previews::forget_listing(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The library showing every stock brush, as its rows ask: the first time after the app is
/// installed (every stroke drawn), again in the same run, and the first time after a restart.
/// Prints each time and what the strokes hold. Bounds: after the first time none is drawn again
/// (the second pass all kept, the restart all from disk), each of those under a fifth of the first
/// time; memory within its most, disk within its most.
/// `cargo test -p procraft-core --lib --release library_shows_every_stock_brush -- --ignored --nocapture`
#[test]
#[ignore]
fn library_shows_every_stock_brush_first_then_kept() {
    let dir = folder("every");
    let Some(s) = opened(&dir) else { return };
    let at = places(&s, usize::MAX);
    let n = at.len() as f64;
    let pass = |s: &CanvasSession| {
        let t = Instant::now();
        for &(set, index) in &at {
            assert!(s.brush_preview(set, index, ROW.0, ROW.1).is_some());
        }
        t.elapsed().as_secs_f64() * 1000.0
    };
    let first = pass(&s);
    let after_first = counts(&s);
    let again = pass(&s);
    let after_again = counts(&s);
    drawn_previews::written();
    let (held, held_bytes) = s.shared.lock().drawn_previews.held();
    let (files, disk) = drawn_previews::held_on_disk(&dir);
    assert!(s.keep_brushes(dir.to_string_lossy().into_owned()));
    drop(s);
    let s = restarted(&dir).unwrap();
    let restart = pass(&s);
    let after_restart = counts(&s);
    println!(
        "{} stock brushes at {}x{}: first {first:.0} ms ({:.1} ms each, {after_first:?} found/drawn/from disk); \
         again {again:.0} ms ({:.2} ms each, {after_again:?}); after a restart {restart:.0} ms ({:.2} ms each, \
         {after_restart:?}); memory {held} strokes {} KB of {} KB; disk {files} files {} KB of {} KB",
        at.len(),
        ROW.0,
        ROW.1,
        first / n,
        again / n,
        restart / n,
        held_bytes >> 10,
        KEPT_BYTES >> 10,
        disk >> 10,
        DISK_BYTES >> 10
    );
    assert_eq!(after_first.1, at.len() as u64, "all drawn the first time");
    assert_eq!(after_again.1, after_first.1, "none drawn again");
    assert_eq!(after_restart.1, 0, "none drawn after a restart");
    assert_eq!(after_restart.2, at.len() as u64, "all from disk after a restart");
    assert!(again < first / 5.0 && restart < first / 5.0, "{first:.0} / {again:.0} / {restart:.0} ms");
    assert!(held_bytes <= KEPT_BYTES && disk <= DISK_BYTES);
    assert_eq!(files, at.len(), "every stroke kept on disk");
    drop(s);
    drawn_previews::forget_listing(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The library opens with its lists as they stood when it closed only on the same set of the same
/// library: on another set (the brush in use moved there meanwhile), the Recent set, or in another
/// library, none, and the shown set and the brush in use are brought into sight instead.
#[test]
fn the_library_opens_as_it_closed_only_on_the_same_set_of_the_same_library() {
    use crate::LibraryScroll;
    let dir = folder("scroll");
    let s = CanvasSession::new(64, 64);
    s.open_brushes(dir.to_string_lossy().into_owned());
    assert_eq!(s.library_opens(3), None, "never closed");
    let spot =
        LibraryScroll { shown_set: 3, sets_first: 2, sets_offset: 14.5, brushes_first: 20, brushes_offset: 31.0 };
    s.library_closed(spot);
    assert_eq!(s.library_opens(3), Some(spot));
    assert_eq!(s.library_opens(3), Some(spot), "as often as it opens");
    assert_eq!(s.library_opens(2), None, "another set");
    assert_eq!(s.library_opens(-1), None, "the Recent set");
    s.library_closed(LibraryScroll { shown_set: -1, ..spot });
    assert_eq!(s.library_opens(-1).map(|o| o.brushes_first), Some(20));
    // a measure that isn't one is taken as the top
    s.library_closed(LibraryScroll { sets_offset: f32::NAN, brushes_offset: -4.0, ..spot });
    assert_eq!(s.library_opens(3).map(|o| (o.sets_offset, o.brushes_offset)), Some((0.0, 0.0)));
    // another library in use: none, and none back in the first either (it closed on the other)
    s.library_closed(spot);
    let other = s.new_brush_library().expect("a new library");
    assert!(s.open_brush_library(other));
    assert_eq!(s.library_opens(3), None);
    s.library_closed(LibraryScroll { shown_set: 0, ..spot });
    assert!(s.open_brush_library(0));
    assert_eq!(s.library_opens(0), None);
    let _ = std::fs::remove_dir_all(&dir);
}
