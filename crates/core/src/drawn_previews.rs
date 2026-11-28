//! Sample strokes drawn already, kept a while by what they were drawn from: the library asks for
//! a brush's again whenever it changes (every row's, after any brush is moved, renamed or edited),
//! and sharing a set or library wants every brush's at the file's size. A brush as it was drawn
//! finds its stroke here, whatever its place, and a brush changed in any way that shows draws
//! anew, as it isn't the brush its kept stroke was drawn from.
//!
//! They are kept on disk too, in the brushes' folder (see [`ON_DISK`]), so that the library opened
//! after a restart draws none it drew before: drawing them all took most of a minute.

use std::collections::{BTreeMap, VecDeque};
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use procraft_engine::brush::Brush;

/// How many bytes of kept strokes are held at most (each a PNG of the sample stroke's canvas,
/// some tens of KB; the library as it ships takes about 44 MB, the rest of it kept on disk): past
/// it the least recently used go.
pub(crate) const KEPT_BYTES: usize = 32 << 20;

/// The strokes kept, the most recently used last.
#[derive(Default)]
pub(crate) struct DrawnPreviews {
    kept: VecDeque<(u64, Arc<[u8]>)>,
    bytes: usize,
    /// How many asks found their stroke kept here, how many drew it, and how many found it kept on
    /// disk instead (see [`kept_on_disk`]): for the tests and the log.
    pub(crate) found: u64,
    pub(crate) missed: u64,
    pub(crate) from_disk: u64,
}

impl DrawnPreviews {
    /// The kept stroke drawn from a brush looking as `key` says (see [`look_key`]), as
    /// [`packed`] packs it; none if none is kept.
    pub(crate) fn find(&mut self, key: u64) -> Option<Arc<[u8]>> {
        let at = self.kept.iter().position(|(k, _)| *k == key)?;
        self.found += 1;
        let entry = self.kept.remove(at)?;
        let png = Arc::clone(&entry.1);
        self.kept.push_back(entry);
        Some(png)
    }

    /// Keeps `png`, the stroke drawn from a brush looking as `key` says, the least recently used
    /// let go past [`KEPT_BYTES`].
    pub(crate) fn keep(&mut self, key: u64, png: Arc<[u8]>) {
        if png.len() > KEPT_BYTES {
            return;
        }
        if let Some(at) = self.kept.iter().position(|(k, _)| *k == key)
            && let Some((_, old)) = self.kept.remove(at)
        {
            self.bytes -= old.len();
        }
        self.bytes += png.len();
        self.kept.push_back((key, png));
        while self.bytes > KEPT_BYTES {
            let Some((_, old)) = self.kept.pop_front() else { break };
            self.bytes -= old.len();
        }
    }

    /// Whether the stroke drawn from a brush looking as `key` says is kept (none counted as found
    /// or used).
    pub(crate) fn holds(&self, key: u64) -> bool {
        self.kept.iter().any(|(k, _)| *k == key)
    }

    /// How many strokes are kept and their bytes.
    #[cfg(test)]
    pub(crate) fn held(&self) -> (usize, usize) {
        (self.kept.len(), self.bytes)
    }
}

/// The sample stroke's canvas (premultiplied RGBA8, `width` × `height`) packed to be kept: a PNG
/// of its coverage alone where it is white wherever it shows (the colour of each pixel its
/// coverage within [`WHITE_WITHIN`], as nearly every brush's is), else of its pixels as they are;
/// none if it can't be. With it, the canvas as it shows, kept or just drawn: as [`unpacked`] gives
/// it back (a white stroke as its coverage), without unpacking it. Packed quickly (a PNG packed
/// tighter took longer than drawing most brushes' strokes, for a file a fifth smaller).
pub(crate) fn packed(width: u32, height: u32, rgba: Vec<u8>) -> (Option<Arc<[u8]>>, Vec<u8>) {
    let (pixels, _) = rgba.as_chunks::<4>();
    let near = |c: u8, a: u8| c.abs_diff(a) <= WHITE_WITHIN;
    if pixels.iter().all(|&[r, g, b, a]| near(r, a) && near(g, a) && near(b, a)) {
        let coverage: Vec<u8> = pixels.iter().map(|p| p[3]).collect();
        let png = quick_png(width, height, png::ColorType::Grayscale, &coverage);
        let shown = coverage.iter().flat_map(|&v| [v; 4]).collect();
        (png, shown)
    } else {
        (quick_png(width, height, png::ColorType::Rgba, &rgba), rgba)
    }
}

/// A PNG of `pixels` (8 bits a sample, as `color` lays them), packed quickly; none if it can't be.
fn quick_png(width: u32, height: u32, color: png::ColorType, pixels: &[u8]) -> Option<Arc<[u8]>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(pixels).ok()?;
    writer.finish().ok()?;
    Some(out.into())
}

/// The canvas a kept stroke was packed from (see [`packed`]), if it is `width` × `height`.
pub(crate) fn unpacked(width: u32, height: u32, png: &[u8]) -> Option<Vec<u8>> {
    let (w, h, channels, pixels) = crate::brush_files::decode_picture(png)?;
    if (w, h) != (width as usize, height as usize) {
        return None;
    }
    match channels {
        1 => Some(pixels.iter().flat_map(|&v| [v; 4]).collect()),
        4 => Some(pixels),
        _ => None,
    }
}

/// How far a white stroke's colour may stray from its coverage, in levels, and be kept as its
/// coverage alone (the GPU rounds the two apart by a level here and there).
const WHITE_WITHIN: u8 = 2;

/// What a brush's sample stroke is drawn from, as a number: everything about it that can show in
/// the stroke, not what can't (its name, its place, its notches and sliders, its author, the
/// preview it came with, the point it resets to).
pub(crate) fn look_key(brush: &Brush) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut into = HashWriter(&mut hasher);
    // (formatted straight into the hash: nothing is held)
    let _ = std::fmt::Write::write_fmt(&mut into, format_args!("{:?}", looks(brush)));
    hasher.finish()
}

/// `brush` without what doesn't show in its stroke (see [`look_key`]), its second the same. Its
/// images at their own size are told by their ids alone (a hash of their pixels), not by the
/// pixels, which it holds only until the library is kept (written out, a megabyte of them took
/// longer than its stroke, and the same brush read back looked like another); an outside engine's
/// data by a hash of it.
fn looks(brush: &Brush) -> Brush {
    let mut looks = Brush {
        name: String::new(),
        notches: Default::default(),
        sliders: [None; 3],
        thumbnail: None,
        author: None,
        reset_point: None,
        secondary: brush.secondary.as_deref().map(|second| Box::new(looks(second))),
        ..brush.clone()
    };
    if let Some(full) = looks.grain_file.as_mut().and_then(|f| f.full.as_mut()) {
        full.pixels = None;
    }
    if let Some(file) = looks.shape_file.as_mut() {
        file.full.pixels = None;
    }
    if let Some(engine) = looks.engine.as_mut() {
        engine.data = Arc::from(fnv(&engine.data).to_le_bytes());
    }
    looks
}

/// FNV-1a of `bytes`.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// The name of the folder in the brushes' folder the sample strokes are kept in between runs
/// starts with this, the rest [`DRAWN_BY`]: a build that draws them differently keeps its own,
/// and takes away the others.
const FOLDER: &str = "brush-previews-";

/// What tells the code that draws the sample strokes from another build's (see `build.rs`): all
/// of the engine's, the GPU's and the core's, and the libraries they are built with.
const DRAWN_BY: &str = env!("PROCRAFT_PREVIEW_KEY");

/// How many bytes of sample strokes are kept on disk at most: the library as it ships takes about
/// a third of it (598 strokes, 44 MB), so it holds that with many brushes brought in and edited;
/// past it the least recently used go.
pub(crate) const DISK_BYTES: u64 = 128 << 20;

/// The key a stroke is kept on disk under: the look its brush has (see [`look_key`]) drawn by the
/// GPU at hand (a driver or GPU that draws it differently keeps its own).
pub(crate) fn disk_key(look: u64, gpu: &procraft_gpu::Gpu) -> u64 {
    let info = gpu.adapter.get_info();
    let drawn_on = format!(
        "{:?} {} {} {} {} {}",
        info.backend, info.name, info.vendor, info.device, info.driver, info.driver_info
    );
    disk_key_on(look, &drawn_on)
}

/// [`disk_key`] for a stroke drawn on the GPU `drawn_on` tells of itself.
pub(crate) fn disk_key_on(look: u64, drawn_on: &str) -> u64 {
    fnv(&look.to_le_bytes().into_iter().chain(drawn_on.bytes()).collect::<Vec<u8>>())
}

/// The sample strokes kept on disk, by the folder they are kept in (this build's in each brushes'
/// folder in use, see [`FOLDER`]): a PNG each, as [`packed`] packs it, named by its
/// [`disk_key`]. Each is written whole or not at all, and one that can't be read or unpacked is
/// drawn again (and its file taken away), so a file cut short or damaged costs only a stroke drawn
/// again.
static ON_DISK: Mutex<BTreeMap<PathBuf, Listed>> = Mutex::new(BTreeMap::new());

/// A folder of kept strokes (see [`ON_DISK`]): its files' keys and sizes, the least recently used
/// first (as a run starts, by when they were written).
struct Listed {
    folder: PathBuf,
    files: VecDeque<(u64, u64)>,
    bytes: u64,
}

/// This build's folder of kept strokes in the brushes' folder `brushes`.
fn folder_in(brushes: &Path) -> PathBuf {
    #[cfg(test)]
    if let Some(drawn_by) = DRAWN_BY_IN_TEST.with(|d| d.borrow().clone()) {
        return brushes.join(format!("{FOLDER}{drawn_by}"));
    }
    brushes.join(format!("{FOLDER}{DRAWN_BY}"))
}

#[cfg(test)]
std::thread_local! {
    /// What a test's strokes are drawn by in place of [`DRAWN_BY`]: another build's drawing.
    pub(crate) static DRAWN_BY_IN_TEST: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The file the stroke kept under `key` is in, in `folder`.
fn file_in(folder: &Path, key: u64) -> PathBuf {
    folder.join(format!("{key:016x}.png"))
}

/// The listing of this build's folder in the brushes' folder `brushes`, of those `disk` holds:
/// listed the first time it is asked for (files half written by a run cut short taken away, and
/// past [`DISK_BYTES`] the oldest), when the other builds' folders there are taken away too, on a
/// thread of their own.
fn listing<'d>(disk: &'d mut BTreeMap<PathBuf, Listed>, brushes: &Path) -> &'d mut Listed {
    listing_of(disk, brushes, folder_in(brushes))
}

/// [`listing`] of the folder of kept strokes `folder` in the brushes' folder `brushes`.
fn listing_of<'d>(disk: &'d mut BTreeMap<PathBuf, Listed>, brushes: &Path, folder: PathBuf) -> &'d mut Listed {
    disk.entry(folder.clone()).or_insert_with(|| list(brushes, folder))
}

/// `folder`'s kept strokes listed (see [`listing`]).
fn list(brushes: &Path, folder: PathBuf) -> Listed {
    let stale: Vec<PathBuf> = std::fs::read_dir(brushes)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with(FOLDER)) && e.path() != folder)
        .map(|e| e.path())
        .collect();
    if !stale.is_empty() {
        let away = move || {
            for old in stale {
                if let Err(e) = std::fs::remove_dir_all(&old) {
                    log::warn!("taking away another build's brush previews: {e}");
                }
            }
        };
        if let Err(e) = std::thread::Builder::new().name("procraft-previews-away".to_owned()).spawn(away) {
            log::warn!("taking away another build's brush previews: {e}");
        }
    }
    let mut found: Vec<(std::time::SystemTime, u64, u64)> = Vec::new();
    for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let key = name.strip_suffix(".png").filter(|k| k.len() == 16).and_then(|k| u64::from_str_radix(k, 16).ok());
        let meta = entry.metadata().ok().filter(std::fs::Metadata::is_file);
        match (key, meta) {
            (Some(key), Some(meta)) => {
                found.push((meta.modified().unwrap_or(std::time::UNIX_EPOCH), key, meta.len()));
            }
            // one half written when a run was cut short
            _ if name.ends_with(".tmp") => {
                let _ = std::fs::remove_file(entry.path());
            }
            _ => {}
        }
    }
    found.sort_unstable();
    let mut listed = Listed { folder, files: found.into_iter().map(|(_, k, b)| (k, b)).collect(), bytes: 0 };
    listed.bytes = listed.files.iter().map(|f| f.1).sum();
    listed.trim(0);
    listed
}

impl Listed {
    /// The least recently used taken away until `more` bytes fit within [`DISK_BYTES`].
    fn trim(&mut self, more: u64) {
        while self.bytes + more > DISK_BYTES {
            let Some((key, bytes)) = self.files.pop_front() else { break };
            self.bytes -= bytes;
            let _ = std::fs::remove_file(file_in(&self.folder, key));
        }
    }

    /// The file kept under `key` dropped from the listing.
    fn drop_key(&mut self, key: u64) {
        if let Some(at) = self.files.iter().position(|f| f.0 == key)
            && let Some((_, bytes)) = self.files.remove(at)
        {
            self.bytes -= bytes;
        }
    }
}

/// The stroke kept on disk in the brushes' folder `brushes` under `key` (see [`disk_key`]), as
/// [`packed`] packed it; none if none is (or it can't be read).
pub(crate) fn kept_on_disk(brushes: &Path, key: u64) -> Option<Arc<[u8]>> {
    let file = {
        let mut disk = ON_DISK.lock().unwrap_or_else(PoisonError::into_inner);
        let listed = listing(&mut disk, brushes);
        let at = listed.files.iter().position(|f| f.0 == key)?;
        // the most recently used last
        let entry = listed.files.remove(at)?;
        listed.files.push_back(entry);
        file_in(&listed.folder, key)
    };
    match std::fs::read(&file) {
        Ok(png) if !png.is_empty() && png.len() as u64 <= DISK_BYTES => Some(png.into()),
        _ => {
            forget_on_disk(brushes, key);
            None
        }
    }
}

/// Whether a stroke is kept on disk in the brushes' folder `brushes` under `key` (as listed: its
/// file isn't read).
pub(crate) fn is_on_disk(brushes: &Path, key: u64) -> bool {
    let mut disk = ON_DISK.lock().unwrap_or_else(PoisonError::into_inner);
    listing(&mut disk, brushes).files.iter().any(|f| f.0 == key)
}

/// Keeps `png`, the stroke drawn under `key`, on disk in the brushes' folder `brushes` now, as
/// [`keep_on_disk_soon`] keeps it a moment later.
#[cfg(test)]
pub(crate) fn keep_on_disk(brushes: &Path, key: u64, png: &[u8]) {
    keep_in(brushes, folder_in(brushes), key, png);
}

/// Keeps `png`, the stroke drawn under `key`, in the folder of kept strokes `folder` in the
/// brushes' folder `brushes` (whole or not at all), the least recently used taken away past
/// [`DISK_BYTES`]. A failure is said once.
fn keep_in(brushes: &Path, folder: PathBuf, key: u64, png: &[u8]) {
    let size = png.len() as u64;
    if size > DISK_BYTES {
        return;
    }
    let folder = {
        let mut disk = ON_DISK.lock().unwrap_or_else(PoisonError::into_inner);
        let listed = listing_of(&mut disk, brushes, folder);
        listed.drop_key(key);
        listed.trim(size);
        listed.folder.clone()
    };
    let written = std::fs::create_dir_all(&folder)
        .map_err(|e| crate::project::file_error("keeping a brush's preview", e))
        .and_then(|()| {
            crate::project::write_atomically(&file_in(&folder, key), |p| {
                crate::storage::write(p, png).map_err(|e| crate::project::file_error("keeping a brush's preview", e))
            })
        });
    match written {
        Ok(()) => {
            let mut disk = ON_DISK.lock().unwrap_or_else(PoisonError::into_inner);
            let listed = listing_of(&mut disk, brushes, folder);
            // (kept meanwhile by another ask for it: counted once)
            listed.drop_key(key);
            listed.trim(size);
            listed.files.push_back((key, size));
            listed.bytes += size;
        }
        Err(e) => {
            static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                log::warn!("brush previews aren't kept between runs: {e}");
            }
        }
    }
}

/// Strokes drawn, waiting to be kept on disk (see [`keep_on_disk_soon`]), and how many.
#[allow(clippy::type_complexity)]
static WAITING: Mutex<Option<std::sync::mpsc::Sender<(PathBuf, PathBuf, u64, Arc<[u8]>)>>> = Mutex::new(None);
static WAITING_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Keeps `png`, the stroke drawn under `key`, on disk in the brushes' folder `brushes` as
/// [`keep_in`] does, a moment later, on a thread of their own that writes them one after
/// another: written as each was drawn, a file made and renamed took longer than drawing a light
/// brush's stroke (a desktop scans each new file). Written now if that thread can't be had.
pub(crate) fn keep_on_disk_soon(brushes: &Path, key: u64, png: Arc<[u8]>) {
    use std::sync::atomic::Ordering::SeqCst;
    let mut waiting = WAITING.lock().unwrap_or_else(PoisonError::into_inner);
    if waiting.is_none() {
        let (send, take) = std::sync::mpsc::channel::<(PathBuf, PathBuf, u64, Arc<[u8]>)>();
        let write = move || {
            for (dir, folder, key, png) in take {
                keep_in(&dir, folder, key, &png);
                WAITING_COUNT.fetch_sub(1, SeqCst);
            }
        };
        match std::thread::Builder::new().name("procraft-previews-on-disk".to_owned()).spawn(write) {
            Ok(_) => *waiting = Some(send),
            Err(e) => log::warn!("brush previews are kept on disk as they are drawn: {e}"),
        }
    }
    WAITING_COUNT.fetch_add(1, SeqCst);
    // (the folder this build keeps them in, as it is here)
    let sent = waiting.as_ref().map(|send| send.send((brushes.to_owned(), folder_in(brushes), key, png)));
    match sent {
        Some(Ok(())) => {}
        Some(Err(std::sync::mpsc::SendError((dir, folder, key, png)))) => {
            *waiting = None;
            drop(waiting);
            keep_in(&dir, folder, key, &png);
            WAITING_COUNT.fetch_sub(1, SeqCst);
        }
        None => {
            drop(waiting);
            WAITING_COUNT.fetch_sub(1, SeqCst);
        }
    }
}

/// Waits until every stroke waiting to be kept on disk is (see [`keep_on_disk_soon`]).
#[cfg(test)]
pub(crate) fn written() {
    let started = std::time::Instant::now();
    while WAITING_COUNT.load(std::sync::atomic::Ordering::SeqCst) > 0 && started.elapsed().as_secs() < 60 {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// The stroke kept on disk under `key` in the brushes' folder `brushes` taken away (it couldn't be
/// read or unpacked): it is drawn again.
pub(crate) fn forget_on_disk(brushes: &Path, key: u64) {
    let file = {
        let mut disk = ON_DISK.lock().unwrap_or_else(PoisonError::into_inner);
        let listed = listing(&mut disk, brushes);
        listed.drop_key(key);
        file_in(&listed.folder, key)
    };
    let _ = std::fs::remove_file(file);
}

/// How many strokes are kept on disk in the brushes' folder `brushes`, and their bytes (as listed).
#[cfg(test)]
pub(crate) fn held_on_disk(brushes: &Path) -> (usize, u64) {
    let mut disk = ON_DISK.lock().unwrap_or_else(PoisonError::into_inner);
    let listed = listing(&mut disk, brushes);
    (listed.files.len(), listed.bytes)
}

/// This build's folder of kept strokes in the brushes' folder `brushes`.
#[cfg(test)]
pub(crate) fn folder_for_test(brushes: &Path) -> PathBuf {
    folder_in(brushes)
}

/// The listing of the brushes' folder `brushes` forgotten, as a new run starts with none.
#[cfg(test)]
pub(crate) fn forget_listing(brushes: &Path) {
    let folder = folder_in(brushes);
    ON_DISK.lock().unwrap_or_else(PoisonError::into_inner).remove(&folder);
}

/// Text written into a hash.
struct HashWriter<'h>(&'h mut std::collections::hash_map::DefaultHasher);

impl std::fmt::Write for HashWriter<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.write(s.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn past_the_most_bytes_the_least_recently_used_strokes_go() {
        let mut kept = DrawnPreviews::default();
        let third: Arc<[u8]> = vec![0u8; KEPT_BYTES / 3].into();
        for key in 1..=3 {
            kept.keep(key, Arc::clone(&third));
        }
        assert_eq!(kept.held(), (3, 3 * (KEPT_BYTES / 3)));
        // 1 used again, so 2 is the least recently used when a fourth comes
        assert!(kept.find(1).is_some());
        kept.keep(4, Arc::clone(&third));
        assert!(kept.find(2).is_none());
        assert!([1, 3, 4].iter().all(|&k| kept.find(k).is_some()));
        assert!(kept.held().1 <= KEPT_BYTES);
        // kept again under the same key: replaced, not held twice
        kept.keep(4, vec![1u8; 10].into());
        assert_eq!(kept.held(), (3, 2 * (KEPT_BYTES / 3) + 10));
        assert_eq!(kept.find(4).as_deref(), Some(&[1u8; 10][..]));
        // one bigger than all may hold is never kept
        kept.keep(5, vec![0u8; KEPT_BYTES + 1].into());
        assert!(kept.find(5).is_none());
        assert_eq!(kept.found, 5);
    }

    #[test]
    fn a_white_stroke_is_kept_as_its_coverage_and_any_other_as_it_is() {
        let white: Vec<u8> = (0..64u32).flat_map(|i| [(i * 4) as u8; 4]).collect();
        let (packed_white, shown) = packed(8, 8, white.clone());
        let packed_white = packed_white.unwrap();
        assert_eq!(unpacked(8, 8, &packed_white).as_deref(), Some(&white[..]));
        assert_eq!(shown, white);
        // a level or two off its coverage: white still, given back (and shown) as its coverage
        let mut near = white.clone();
        near[4 * 9] = near[4 * 9 + 3] + 2;
        let (packed_near, shown) = packed(8, 8, near);
        assert_eq!(unpacked(8, 8, &packed_near.unwrap()).as_deref(), Some(&white[..]));
        assert_eq!(shown, white);
        let mut tinted = white.clone();
        tinted[4 * 9] = 200;
        let (packed_tinted, shown) = packed(8, 8, tinted.clone());
        let packed_tinted = packed_tinted.unwrap();
        assert_eq!(unpacked(8, 8, &packed_tinted).as_deref(), Some(&tinted[..]));
        assert_eq!(shown, tinted);
        // a quarter of the bytes or less, for a white one
        assert!(packed_white.len() < packed_tinted.len());
        assert!(unpacked(4, 16, &packed_white).is_none());
    }

    #[test]
    fn a_brush_s_look_leaves_out_what_its_stroke_never_shows() {
        let pen = Brush::round_pen();
        let key = look_key(&pen);
        assert_eq!(key, look_key(&pen.clone()));
        let renamed = Brush { name: "Another".to_owned(), sliders: [Some([0.2, 0.3]); 3], ..pen.clone() };
        assert_eq!(look_key(&renamed), key);
        let mut spaced = pen.clone();
        spaced.set_setting("spacing", 0.5);
        assert_ne!(look_key(&spaced), key);
        // a second counts, its name doesn't
        let dual = Brush { secondary: Some(Box::new(pen.clone())), ..pen.clone() };
        let named =
            Brush { secondary: Some(Box::new(Brush { name: "Second".to_owned(), ..pen.clone() })), ..pen.clone() };
        assert_ne!(look_key(&dual), key);
        assert_eq!(look_key(&named), look_key(&dual));
        // a picture drawn the first time it is read looks the same before as after
        let source = procraft_engine::brush::Source::drawn_later(
            7,
            procraft_engine::brush::Pixels::later(|| Some(vec![9; procraft_engine::textures::TEXTURE_SIZE.pow(2)])),
        );
        let mut shaped = pen.clone();
        shaped.shape.source = Some(source.clone());
        let before = look_key(&shaped);
        let _ = source.pixels.shared();
        assert_eq!(look_key(&shaped), before);
    }
}
