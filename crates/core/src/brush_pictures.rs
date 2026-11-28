//! The pictures of the library's brushes (their shapes, grains, previews and makers' pictures),
//! each kept once, in a file of its own in a folder beside the library, named by what tells it
//! apart. The library names them, so it stays small however many brushes are brought in: keeping
//! it after a change writes only the pictures it hasn't kept yet, not every brush's again.
//! Libraries from before, with the pictures inside them, are read as they are.
//!
//! A shape's or grain's pixels kept here stay in their file until a brush draws with them (see
//! [`later`]): a library of thousands of brushes brought in, each with pictures of its own, held
//! every one of them, 128 KB a brush, and a phone closed the app (#415). Brushes read from a file
//! put theirs here as they are read (see [`spill`]), so bringing them in holds only a few.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// The folder beside the library the pictures are kept in.
pub(crate) const FOLDER: &str = "pictures";

/// How the library names a picture in a file, before its name (base64, how pictures were kept
/// inside it before, has no ':').
pub(crate) const MARK: &str = "picture:";

/// How long a picture's file stays, whatever the library says: a brush another window has just
/// brought in may not be in the library kept on disk yet.
const LATELY: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// A picture kept again whose file is older than this is marked as kept now, so that it is never
/// taken for one nothing has wanted lately (once a day at most, not at every save).
const TOUCHED_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// A picture to keep: a file's bytes (a PNG), or a shape's or grain's pixels, drawn or read from
/// their file when first wanted (see [`later`]).
#[derive(Clone)]
pub(crate) enum Held {
    Bytes(Arc<[u8]>),
    Pixels(Arc<Vec<u8>>),
    Later(procraft_engine::brush::Pixels),
}

/// The picture's file at `path`, there already, marked as kept lately if it is old.
fn touch_if_old(path: &Path) {
    let old = fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .is_none_or(|age| age >= TOUCHED_AFTER);
    if old {
        let _ = fs::File::options().write(true).open(path).and_then(|f| f.set_modified(SystemTime::now()));
    }
}

/// The bytes a shape's or grain's pixels take.
const PIXELS_BYTES: u64 = (procraft_engine::textures::TEXTURE_SIZE * procraft_engine::textures::TEXTURE_SIZE) as u64;

/// A shape's or grain's pixels read from their file at `path` the first time they are wanted (and
/// held from then on by those holding them); blank if it can't be read whole.
pub(crate) fn later(path: std::path::PathBuf) -> procraft_engine::brush::Pixels {
    procraft_engine::brush::Pixels::later(move || fs::read(&path).ok().filter(|p| p.len() as u64 == PIXELS_BYTES))
}

/// The id of the shape's or grain's pixels kept by `name` (see [`pixels_name`]).
fn pixels_id(name: &str) -> Option<u64> {
    let id = name.strip_suffix(".pixels")?;
    u64::from_str_radix(id, 16).ok().filter(|_| id.len() == 16)
}

/// The name a picture of `bytes` is kept by.
pub(crate) fn bytes_name(bytes: &[u8]) -> String {
    format!("{:016x}.picture", crate::project::pixels_hash(bytes))
}

/// The name a shape's or grain's pixels, known by `id`, are kept by.
pub(crate) fn pixels_name(id: u64) -> String {
    format!("{id:016x}.pixels")
}

/// Whether `name` is one a picture is kept by: 16 hex digits and its kind, so nothing outside the
/// folder can be named.
fn is_name(name: &str) -> bool {
    let Some((id, kind)) = name.split_once('.') else { return false };
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()) && matches!(kind, "picture" | "pixels")
}

/// Keeps `pictures` in `folder`, each whole or not at all; one already there is left as it is,
/// marked as kept lately if it is old. False if one couldn't be kept. (Not flushed to the disk:
/// that cost a quarter of a second more for 176 new pictures here, and saves only against a power
/// cut in the seconds after a brush is brought in.)
pub(crate) fn keep(folder: &Path, pictures: &[(String, Held)]) -> bool {
    let mut all = true;
    let mut made = false;
    let mut seen = HashSet::new();
    let what = "keeping a brush's picture";
    for (name, held) in pictures {
        // (a reset point or a second brush names its pictures again)
        if !is_name(name) || !seen.insert(name.as_str()) {
            continue;
        }
        let path = folder.join(name);
        // pixels still in their file (or still to be drawn): kept as they are if they are there
        // whole, else drawn apart (the brushes don't hold on to them once written)
        let drawn;
        let bytes: &[u8] = match held {
            Held::Bytes(b) => b,
            Held::Pixels(p) => p,
            Held::Later(p) if !p.is_drawn() && fs::metadata(&path).is_ok_and(|m| m.len() == PIXELS_BYTES) => {
                touch_if_old(&path);
                continue;
            }
            Held::Later(p) => {
                drawn =
                    if p.is_drawn() { p.shared() } else { p.twin().map_or_else(|| p.shared(), |twin| twin.shared()) };
                &drawn
            }
        };
        if fs::metadata(&path).is_ok_and(|meta| meta.len() == bytes.len() as u64) {
            touch_if_old(&path);
            continue;
        }
        let mut written = Ok(());
        if !made {
            written = fs::create_dir_all(folder).map_err(|e| crate::project::file_error(what, e));
            made = written.is_ok();
        }
        let written = written.and_then(|()| {
            crate::project::write_atomically(&path, |p| {
                crate::storage::write(p, bytes).map_err(|e| crate::project::file_error(what, e))
            })
        });
        if let Err(e) = written {
            log::error!("{e}");
            all = false;
        }
    }
    all
}

/// The pictures the library text `json` names, read from `folder` (one that isn't there or can't
/// be read is left out: its brush keeps what it would without it): each read once, and shared by
/// every brush naming it.
pub(crate) fn read(folder: &Path, json: &str) -> HashMap<String, Held> {
    let mut out = HashMap::new();
    // the shapes' and grains' pixels there and their sizes, listed once: a look at each file took
    // seconds with thousands of brushes brought in
    let mut there: Option<HashMap<String, u64>> = None;
    for (at, _) in json.match_indices(MARK) {
        let rest = &json[at + MARK.len()..];
        let name = &rest[..rest.find('"').unwrap_or(rest.len())];
        if !is_name(name) || out.contains_key(name) {
            continue;
        }
        // a shape's or grain's pixels: read when a brush draws with them, if they are there whole
        if name.ends_with(".pixels") {
            match there.get_or_insert_with(|| pixels_in(folder)).get(name) {
                Some(&size) if size == PIXELS_BYTES => {
                    out.insert(name.to_owned(), Held::Later(later(folder.join(name))));
                }
                Some(_) => log::warn!("a brush's picture is damaged: left out"),
                None => log::warn!("a brush's picture isn't there: left out"),
            }
            continue;
        }
        match fs::read(folder.join(name)) {
            Ok(bytes) => {
                out.insert(name.to_owned(), Held::Bytes(bytes.into()));
            }
            Err(e) => log::warn!("a brush's picture can't be read: {e}"),
        }
    }
    out
}

/// The shapes' and grains' pixels in `folder`, by name, and their sizes.
fn pixels_in(folder: &Path) -> HashMap<String, u64> {
    let entries = fs::read_dir(folder).into_iter().flatten().flatten();
    entries
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok().filter(|n| n.ends_with(".pixels"))?;
            Some((name, entry.metadata().ok()?.len()))
        })
        .collect()
}

/// The bytes a kept brush's `text` gives: a picture's in `pictures` (as read by [`read`]), shared
/// with every other brush naming it (a set's brushes often all name their maker's picture), or
/// those the text holds itself (base64), as the library kept them before.
pub(crate) fn bytes_of(text: &str, pictures: &HashMap<String, Held>) -> Option<Arc<[u8]>> {
    match text.strip_prefix(MARK) {
        Some(name) => pictures.get(name).map(|held| match held {
            Held::Bytes(bytes) => Arc::clone(bytes),
            Held::Pixels(pixels) => Arc::from(pixels.as_slice()),
            Held::Later(pixels) => Arc::from(pixels.shared().as_slice()),
        }),
        None => procraft_engine::base64::decode(text).map(Into::into),
    }
}

/// The names of pictures the library text `json` names.
fn named(json: &str) -> HashSet<&str> {
    json.match_indices(MARK)
        .map(|(at, _)| {
            let rest = &json[at + MARK.len()..];
            &rest[..rest.find('"').unwrap_or(rest.len())]
        })
        .collect()
}

/// Whether `name` is a write of a picture cut short (`<16 hex>.<number>.tmp`), as a crash leaves.
fn is_leftover(name: &str) -> bool {
    let mut parts = name.split('.');
    let (Some(id), Some(n), Some("tmp"), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    id.len() == 16
        && id.bytes().all(|b| b.is_ascii_hexdigit())
        && !n.is_empty()
        && n.bytes().all(|b| b.is_ascii_digit())
}

/// The shape or grain a kept brush's `text` gives: the pixels read from a picture's file (see
/// [`read`]), shared with every other brush naming it, or those the text holds itself.
pub(crate) fn source_of(text: &str, pictures: &HashMap<String, Held>) -> Option<procraft_engine::brush::Source> {
    match text.strip_prefix(MARK) {
        Some(name) => match pictures.get(name)? {
            Held::Pixels(pixels) => procraft_engine::brush::Source::shared(Arc::clone(pixels)),
            Held::Bytes(bytes) => procraft_engine::brush::Source::new(bytes.to_vec()),
            Held::Later(pixels) => Some(procraft_engine::brush::Source::drawn_later(pixels_id(name)?, pixels.clone())),
        },
        None => procraft_engine::brush::Source::from_text(text),
    }
}

/// The brushes' folder in use (prepared or opened last), where brushes read from a file put their
/// pictures as they are read (see [`spill`]); each test's thread its own.
static SPILL_TO: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
std::thread_local! {
    static SPILL_TO_HERE: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Brushes read from a file put their pictures in the brushes' folder `dir` from now on.
pub(crate) fn spill_into(dir: &Path) {
    #[cfg(test)]
    SPILL_TO_HERE.with(|s| *s.borrow_mut() = Some(dir.to_owned()));
    #[cfg(not(test))]
    {
        *SPILL_TO.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(dir.to_owned());
    }
}

/// The brushes' folder brushes read from a file put their pictures in, if one is in use.
pub(crate) fn spill_folder() -> Option<std::path::PathBuf> {
    #[cfg(test)]
    if let Some(dir) = SPILL_TO_HERE.with(|s| s.borrow().clone()) {
        return Some(dir);
    }
    SPILL_TO.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// `brush`'s pictures (and its second's) put in their files as it is read from a brush file, not
/// held: each shape's and grain's pixels in `pictures` (this folder), read again from there when
/// a brush draws with them, and its images at their own size in the grains' folder `grains` (see
/// `grain_store`). One that can't be written stays held, as before.
pub(crate) fn spill(pictures: &Path, grains: &Path, brush: &mut procraft_engine::brush::Brush) {
    let sources =
        [&mut brush.shape.source, &mut brush.grain.source, &mut brush.metallic.source, &mut brush.roughness.source];
    let mut held: Vec<&mut procraft_engine::brush::Source> =
        sources.into_iter().flatten().filter(|s| s.pixels.is_drawn() && s.stand_in_for.is_none()).collect();
    let kept: Vec<(String, Held)> = held.iter().map(|s| (pixels_name(s.id), Held::Pixels(s.pixels.shared()))).collect();
    if !kept.is_empty() && keep(pictures, &kept) {
        for source in &mut held {
            source.pixels = later(pictures.join(pixels_name(source.id)));
        }
    }
    let grain = brush.grain_file.as_mut().and_then(|f| f.full.as_mut());
    let shape = brush.shape_file.as_mut().map(|f| &mut f.full);
    for full in grain.into_iter().chain(shape) {
        if let Some(pixels) = &full.pixels
            && crate::grain_store::write_to(grains, full.id, pixels)
        {
            full.pixels = None;
        }
    }
    if let Some(second) = brush.secondary.as_deref_mut() {
        spill(pictures, grains, second);
    }
}

/// Takes the files of pictures none of the libraries kept at `libraries` names any more out of
/// `folder` (they share it), as a brush that was deleted leaves its own behind, and writes of them a
/// crash cut short; one kept lately stays, and all do if one of the libraries can't be read. Files
/// that aren't pictures' stay. Call it holding the libraries' own lock, so that a library being kept
/// meanwhile can't name one it takes.
pub(crate) fn forget_unnamed(folder: &Path, libraries: &[std::path::PathBuf]) {
    let Ok(entries) = fs::read_dir(folder) else { return };
    let old: Vec<(String, std::path::PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok().filter(|n| is_name(n) || is_leftover(n))?;
            let age = entry.metadata().ok()?.modified().ok()?.elapsed().ok()?;
            (age >= LATELY).then(|| (name, entry.path()))
        })
        .collect();
    if old.is_empty() {
        return;
    }
    let Ok(kept) = libraries.iter().map(fs::read_to_string).collect::<Result<Vec<String>, _>>() else { return };
    let named: HashSet<&str> = kept.iter().flat_map(|text| named(text)).collect();
    for (name, path) in old {
        if !named.contains(name.as_str())
            && let Err(e) = fs::remove_file(&path)
        {
            log::warn!("a brush's picture file stays: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("procraft-pictures-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_picture_is_kept_once_and_read_back_by_its_name() {
        let dir = folder("once");
        let png: Arc<[u8]> = vec![1, 2, 3, 4].into();
        let name = bytes_name(&png);
        assert!(keep(&dir, &[(name.clone(), Held::Bytes(Arc::clone(&png)))]));
        let json = format!("{{\"thumbnail\":\"{MARK}{name}\",\"grain\":\"{MARK}0000000000000000.pixels\"}}");
        let read = read(&dir, &json);
        // the one there, read; the one not there, left out
        assert_eq!(read.len(), 1);
        let text = format!("{MARK}{name}");
        assert_eq!(bytes_of(&text, &read).as_deref(), Some(&[1, 2, 3, 4][..]));
        assert_eq!(bytes_of(&format!("{MARK}0000000000000000.pixels"), &read), None);
        // the same bytes for every brush naming it, not a copy each
        let (a, b) = (bytes_of(&text, &read).unwrap(), bytes_of(&text, &read).unwrap());
        assert!(Arc::ptr_eq(&a, &b));
        // the way the library kept them before: inside it
        assert_eq!(bytes_of(&procraft_engine::base64::encode(&[9, 8]), &read).as_deref(), Some(&[9, 8][..]));
        // a shape from its file: the same pixels for every brush naming it, not a copy each
        let side = procraft_engine::textures::TEXTURE_SIZE;
        let pixels: Arc<Vec<u8>> = Arc::new((0..side * side).map(|i| (i % 7) as u8).collect());
        let shapes = HashMap::from([(pixels_name(1), Held::Pixels(Arc::clone(&pixels)))]);
        let text = format!("{MARK}{}", pixels_name(1));
        let (a, b) = (source_of(&text, &shapes).unwrap(), source_of(&text, &shapes).unwrap());
        assert!(Arc::ptr_eq(&a.pixels.shared(), &b.pixels.shared()) && Arc::ptr_eq(&a.pixels.shared(), &pixels));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A shape's or grain's pixels kept here are read from their file only when a brush draws with
    /// them, the same for every brush naming them; kept again, they aren't read (#415). A damaged
    /// file is left out, as one not there.
    #[test]
    fn pixels_kept_are_read_from_their_file_when_first_wanted() {
        use procraft_engine::brush::{Pixels, Source};
        let dir = folder("later");
        let side = procraft_engine::textures::TEXTURE_SIZE;
        let pixels: Arc<Vec<u8>> = Arc::new((0..side * side).map(|i| (i % 11) as u8).collect());
        let source = Source::shared(Arc::clone(&pixels)).unwrap();
        let name = pixels_name(source.id);
        assert!(keep(&dir, &[(name.clone(), Held::Pixels(Arc::clone(&pixels)))]));
        let json = format!("{{\"shape\":\"{MARK}{name}\"}}");
        let read_back = read(&dir, &json);
        let text = format!("{MARK}{name}");
        let (a, b) = (source_of(&text, &read_back).unwrap(), source_of(&text, &read_back).unwrap());
        assert_eq!(a.id, source.id);
        assert!(!a.pixels.is_drawn() && Pixels::same(&a.pixels, &b.pixels));
        // kept again: left in its file, not read
        assert!(keep(&dir, &[(name.clone(), Held::Later(a.pixels.clone()))]));
        assert!(!a.pixels.is_drawn());
        // wanted: read, the same pixels, for both
        assert_eq!(*a.pixels.shared(), *pixels);
        assert!(b.pixels.is_drawn());
        // a damaged file: left out
        fs::write(dir.join(&name), [1, 2, 3]).unwrap();
        assert!(read(&dir, &json).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    /// A brush read from a file puts its pictures in their files as it is read, holding none of
    /// them: its shape and grain in this folder, its grain at its own size in the grains' folder;
    /// read back the same when a brush draws with them (#415).
    #[test]
    fn a_brush_read_puts_its_pictures_in_their_files() {
        use procraft_engine::brush::{Brush, FullGrain, GrainFile, Source};
        let dir = folder("spill");
        let side = procraft_engine::textures::TEXTURE_SIZE;
        let mut brush = Brush::round_pen();
        brush.shape.source = Source::new((0..side * side).map(|i| (i % 5) as u8).collect());
        let grain = Source::new(vec![3; side * side]).unwrap();
        let full = FullGrain::new(512, vec![7; 512 * 512]).unwrap();
        let full_id = full.id;
        brush.grain_file = Some(GrainFile { source: grain.id, width: 512, full: Some(full) });
        brush.grain.source = Some(grain);
        let (pictures, grains) = (dir.join(FOLDER), dir.join("grains"));
        spill(&pictures, &grains, &mut brush);
        let shape = brush.shape.source.as_ref().unwrap();
        let grain = brush.grain.source.as_ref().unwrap();
        assert!(!shape.pixels.is_drawn() && !grain.pixels.is_drawn(), "none held");
        assert!(pictures.join(pixels_name(shape.id)).is_file() && pictures.join(pixels_name(grain.id)).is_file());
        assert!(brush.grain_file.as_ref().unwrap().full.as_ref().unwrap().pixels.is_none());
        assert_eq!(fs::metadata(grains.join(format!("{full_id:016x}.grain"))).unwrap().len(), 512 * 512);
        assert_eq!(shape.pixels.shared()[7], 2);
        assert!(grain.pixels.shared().iter().all(|&v| v == 3));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_picture_kept_again_is_marked_lately_and_leftover_writes_go() {
        let dir = folder("again");
        let png: Arc<[u8]> = vec![5, 6, 7].into();
        let name = bytes_name(&png);
        let pictures = [(name.clone(), Held::Bytes(Arc::clone(&png))), (name.clone(), Held::Bytes(Arc::clone(&png)))];
        assert!(keep(&dir, &pictures));
        let long_ago = SystemTime::now() - 2 * LATELY;
        let age = |p: &Path| fs::metadata(p).unwrap().modified().unwrap().elapsed().unwrap_or_default();
        fs::File::options().write(true).open(dir.join(&name)).unwrap().set_modified(long_ago).unwrap();
        // kept again: no longer old, so nothing takes it while a library naming it is written
        assert!(keep(&dir, &pictures));
        assert!(age(&dir.join(&name)) < TOUCHED_AFTER);
        // a write a crash cut short, old: goes with the next cleanup; the picture named stays
        let leftover = dir.join("0123456789abcdef.3.tmp");
        fs::write(&leftover, [1]).unwrap();
        fs::File::options().write(true).open(&leftover).unwrap().set_modified(long_ago).unwrap();
        fs::File::options().write(true).open(dir.join(&name)).unwrap().set_modified(long_ago).unwrap();
        let library = dir.join("library.json");
        fs::write(&library, format!("[{{\"thumbnail\":\"{MARK}{name}\"}}]")).unwrap();
        forget_unnamed(&dir, &[library]);
        assert!(!leftover.exists() && dir.join(&name).exists());
        assert!(!is_leftover("0123456789abcdef.picture") && !is_leftover("../x.1.tmp"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_reaches_nothing_outside_the_folder() {
        for name in ["../library.json", "0123456789abcdef.picture/..", "0123456789abcdeg.picture", "x.pixels"] {
            assert!(!is_name(name), "{name}");
        }
        assert!(is_name("0123456789abcdef.pixels"));
        let dir = folder("outside");
        assert!(keep(&dir, &[("../outside.picture".to_owned(), Held::Bytes(vec![1].into()))]));
        assert!(!dir.with_file_name("outside.picture").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
