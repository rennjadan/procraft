//! Brush engines of plugins' own (interface 4). A plugin that lists `brush_engines` reads brush
//! files itself and draws the brushes it makes of them its own way: the app hands it a stroke's
//! input and lays the dabs it answers, on the GPU, as it lays its own stamps.
//!
//! - **Import.** A brush file whose name ends as an engine on says (`accepts`) goes to it as well as
//!   through the app's own reading. Each brush it makes, a name and data of its own (a preview and
//!   a size if it gives them), joins the brush the app read of the same name, or comes in on a
//!   round brush if there is none: [`Brush::engine`] keeps the engine's name and the data. With the
//!   engine off or gone, the brush draws as the app read it, so the app is as it was.
//! - **Look.** Before a stroke, the app asks the engine how the brush looks ([`BrushLook`]): its tips
//!   (grey pictures, one picked per dab), whether its dabs build up over each other or towards their
//!   opacity, its blend mode, whether it erases, a texture dabs read where they land, and a second
//!   stroke whose coverage combines with the first (a dual brush). Kept for the brush's data while
//!   the plugin stays on, and on the disk with the plugin, so the next start needn't ask again: the
//!   engine then only reads the brush to draw it.
//! - **Strokes.** A stroke's samples go to the engine as they come ([`EngineHalf`]), and the dabs it
//!   answers become the stroke's stamps: each its place, size, angle, squash, opacity, flow, tip,
//!   mirroring, its own colour if it has one, and how strongly the texture shows in it.
//!
//! Records are the layer items' (see `plugin_layers.rs`): fields of a 16-bit id, a 32-bit length
//! and the bytes, little-endian. What each holds is in `docs/plugins.md`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use procraft_engine::brush::Rendering;
use procraft_engine::brush::{Brush, OutsideBrush};
use procraft_engine::geom::Vec2;
use procraft_engine::input::Sample;
use procraft_engine::stroke::{OutsideEngine, Stamp, Tint};
use procraft_gpu::{SecondStroke, StrokeStyle, StrokeTool, TipKind};

use crate::CanvasSession;
use crate::hooks::{Hooks, Shared, at_work};
use crate::plugin_layers::{Record, fields};
use crate::plugin_run::{DAB_BYTES, Stop};

/// A brush engine a plugin has: its key (`<id>.<key>`), what it is called and the endings of the
/// names of the brush files it reads.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BrushEngine {
    pub key: String,
    pub label: String,
    pub accepts: Vec<String>,
    /// The action (`<id>.<key>`) that is its brushes' studio, if it has one.
    pub studio: Option<String>,
}

/// A brush an engine made of a file.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MadeBrush {
    pub engine: String,
    pub name: String,
    pub data: Vec<u8>,
    pub thumbnail: Option<Vec<u8>>,
}

/// The ids of the records' fields.
mod field {
    /// The file handed to an engine: its name, its bytes.
    pub const FILE_NAME: u16 = 1;
    pub const FILE_BYTES: u16 = 2;
    /// The brushes made of it, each a record of its name, data and preview.
    pub const MADE: u16 = 1;
    pub const MADE_NAME: u16 = 1;
    pub const MADE_DATA: u16 = 2;
    pub const MADE_THUMBNAIL: u16 = 3;
    /// A look: its tips (each width, height, pixels), how its dabs build up, its blend mode,
    /// whether it erases, its texture and its second stroke.
    pub const TIP: u16 = 1;
    pub const TIP_WIDTH: u16 = 1;
    pub const TIP_HEIGHT: u16 = 2;
    pub const TIP_PIXELS: u16 = 3;
    pub const BUILD: u16 = 2;
    pub const BLEND: u16 = 3;
    pub const ERASES: u16 = 4;
    pub const TEXTURE: u16 = 5;
    pub const TEXTURE_SCALE: u16 = 4;
    pub const TEXTURE_MODE: u16 = 5;
    pub const SECOND: u16 = 6;
    pub const SECOND_COMBINE: u16 = 3;
    /// What its tips are (u8, the GPU's tip kinds; masks if it doesn't say).
    pub const TIP_KIND: u16 = 7;
    /// How often it lays a dab while the pen holds still (f32 ms), if it does (an airbrush).
    pub const AIRBRUSH: u16 = 8;
    /// Its dabs move paint (u8 1): a smudge whose stamps pick up behind them.
    pub const SMUDGES: u16 = 9;
    /// A load: the brush's id and data, and whether the app holds its look already.
    pub const LOAD_ID: u16 = 1;
    pub const LOAD_DATA: u16 = 2;
    pub const LOAD_LOOK_HELD: u16 = 3;
    /// A stroke starting: the brush's id, the colour and the secondary colour, the size now and
    /// where it starts, the opacity slider, the tool, the pointer, a seed and the view's turn.
    pub const BEGIN_BRUSH: u16 = 1;
    pub const BEGIN_COLOUR: u16 = 2;
    pub const BEGIN_SECONDARY: u16 = 3;
    pub const BEGIN_DIAMETER: u16 = 4;
    pub const BEGIN_OPACITY: u16 = 5;
    pub const BEGIN_TOOL: u16 = 6;
    pub const BEGIN_POINTER: u16 = 7;
    pub const BEGIN_SEED: u16 = 8;
    pub const BEGIN_TURN: u16 = 9;
    pub const BEGIN_START_DIAMETER: u16 = 10;
}

/// A dab's flags.
const FLIP_ACROSS: u32 = 1;
const FLIP_DOWN: u32 = 2;
const OWN_COLOUR: u32 = 4;
const FOR_SECOND: u32 = 8;
/// A dab that moves the paint under where the stroke's last lay; that paint made one colour first;
/// that weighed by its tip; the colour it mixes in clear (taking paint away).
const MOVES_PAINT: u32 = 16;
const DULLS: u32 = 32;
const WEIGHED: u32 = 64;
const CLEAR_PAINT: u32 = 128;

/// The most tips, and the largest side of one, and of a texture.
const MOST_TIP_SIDE: u32 = 4096;
/// The side tips are drawn into on the GPU at most, and the bytes a set may take there (before its
/// mip levels).
const MOST_SET_SIDE: u32 = 1024;
const MOST_SET_BYTES: usize = 48 << 20;
const MOST_TEXTURE_SIDE: u32 = 2048;
/// The looks each plugin keeps by the brushes' data.
pub(crate) const LOOKS_KEPT: usize = 16;

/// The tips a stroke picks from, fitted into squares a power of two a side for the GPU, what they
/// are (a byte a pixel for masks, else RGBA, straight), and their id there.
#[derive(Debug, PartialEq)]
pub(crate) struct TipSet {
    pub id: u64,
    pub side: u32,
    pub kind: TipKind,
    pub layers: Vec<Vec<u8>>,
}

/// How a stroke's stamps build up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Build {
    /// Each laid over those before.
    Over,
    /// Each towards its opacity by its flow, never past it nor back.
    Towards,
}

/// A texture the dabs read where they land, as a grain: a square, one tile of it `scale` canvas
/// pixels to one of its own, combined with each dab by `mode`.
#[derive(Debug, PartialEq)]
pub(crate) struct LookTexture {
    pub id: u64,
    pub side: u32,
    pub pixels: Vec<u8>,
    pub tile: f32,
    pub mode: u32,
}

/// How a brush an engine draws looks (see the module's notes).
#[derive(Debug, PartialEq)]
pub(crate) struct BrushLook {
    /// Every how many milliseconds its engine is handed the pen again while it holds still.
    pub airbrush: Option<f32>,
    /// Its dabs move paint (as a smudge's stamps do, picking up behind them).
    pub smudges: bool,
    pub tips: TipSet,
    pub build: Build,
    pub blend: Option<u32>,
    pub erases: bool,
    pub texture: Option<LookTexture>,
    pub second: Option<SecondLook>,
}

/// A brush look's second stroke: its tips, how they build up, and how its coverage combines with
/// the first's (a dual brush's mode).
#[derive(Debug, PartialEq)]
pub(crate) struct SecondLook {
    pub tips: TipSet,
    pub build: Build,
    pub combine: u32,
}

fn u32_of(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(*bytes.first_chunk::<4>()?))
}

fn f32_of(bytes: &[u8]) -> Option<f32> {
    Some(f32::from_le_bytes(*bytes.first_chunk::<4>()?)).filter(|v| v.is_finite())
}

/// The record `bytes` as a look, the tips' set given `id` (and the second's and the texture's
/// near it); none if it doesn't hold up.
pub(crate) fn read_look(bytes: &[u8], id: u64) -> Option<BrushLook> {
    let kind = fields(bytes).find(|(f, _)| *f == field::TIP_KIND).map_or(TipKind::Mask, |(_, v)| match v.first() {
        Some(1) => TipKind::Picture,
        Some(2) => TipKind::Lightness,
        Some(3) => TipKind::Gradient,
        _ => TipKind::Mask,
    });
    let mut tips = Vec::new();
    let (mut build, mut blend, mut erases, mut texture, mut second) = (Build::Over, None, false, None, None);
    let (mut airbrush, mut smudges) = (None, false);
    for (f, value) in fields(bytes) {
        match f {
            field::SMUDGES => smudges = value.first() == Some(&1),
            field::AIRBRUSH => airbrush = f32_of(value).filter(|ms| *ms > 0.0).map(|ms| ms.max(MOST_AIRBRUSH_MS)),
            field::TIP => tips.push(read_tip(value, kind.channels() as usize)?),
            field::BUILD => build = read_build(value),
            field::BLEND => blend = u32_of(value),
            field::ERASES => erases = value.first().is_some_and(|&b| b != 0),
            field::TEXTURE => texture = read_texture(value, id ^ 0x7e47_0001),
            field::SECOND => second = read_second(value, id ^ 0x5eed_0002),
            _ => {}
        }
    }
    Some(BrushLook { airbrush, smudges, tips: tip_set(&tips, id, kind)?, build, blend, erases, texture, second })
}

/// The pen is handed again at most this often (ms), however often an airbrush lays its dabs.
const MOST_AIRBRUSH_MS: f32 = 4.0;

/// A set's tips, as the GPU takes them.
fn layers(set: &TipSet) -> Vec<&[u8]> {
    set.layers.iter().map(Vec::as_slice).collect()
}

fn read_build(value: &[u8]) -> Build {
    if value.first().is_some_and(|&b| b == 1) { Build::Towards } else { Build::Over }
}

/// A tip: its width, height and pixels, `channels` bytes each (a grey, 255 painting; or RGBA).
fn read_tip(bytes: &[u8], channels: usize) -> Option<(u32, u32, Vec<u8>)> {
    let (mut w, mut h, mut pixels) = (None, None, None);
    for (f, value) in fields(bytes) {
        match f {
            field::TIP_WIDTH => w = u32_of(value),
            field::TIP_HEIGHT => h = u32_of(value),
            field::TIP_PIXELS => pixels = Some(value),
            _ => {}
        }
    }
    let (w, h, pixels) = (w?, h?, pixels?);
    let fits = (1..=MOST_TIP_SIDE).contains(&w) && (1..=MOST_TIP_SIDE).contains(&h);
    (fits && pixels.len() == (w as usize) * (h as usize) * channels).then(|| (w, h, pixels.to_vec()))
}

fn read_second(bytes: &[u8], id: u64) -> Option<SecondLook> {
    let mut tips = Vec::new();
    let (mut build, mut combine) = (Build::Towards, 1);
    for (f, value) in fields(bytes) {
        match f {
            field::TIP => tips.push(read_tip(value, 1)?),
            field::BUILD => build = read_build(value),
            field::SECOND_COMBINE => combine = u32_of(value)?,
            _ => {}
        }
    }
    Some(SecondLook { tips: tip_set(&tips, id, TipKind::Mask)?, build, combine })
}

fn read_texture(bytes: &[u8], id: u64) -> Option<LookTexture> {
    let (mut scale, mut mode) = (1.0, 1);
    // its picture's fields are a tip's
    let mut parts = Record::default();
    for (f, value) in fields(bytes) {
        match f {
            field::TIP_WIDTH | field::TIP_HEIGHT | field::TIP_PIXELS => {
                parts.field(f, value);
            }
            field::TEXTURE_SCALE => scale = f32_of(value)?,
            field::TEXTURE_MODE => mode = u32_of(value)?,
            _ => {}
        }
    }
    let (w, h, pixels) = read_tip(&parts.bytes(), 1)?;
    if !(0.01..=1000.0).contains(&scale) {
        return None;
    }
    // a square a power of two a side, as a grain is read from: one tile its longer side
    let side = w.max(h).next_power_of_two().clamp(16, MOST_TEXTURE_SIDE);
    let pixels = stretched(w, h, &pixels, side);
    Some(LookTexture { id, side, pixels, tile: w.max(h) as f32 * scale, mode })
}

/// The tips as a set for the GPU: each fitted into a square, all one side (a power of two, as small
/// as keeps the largest whole, held within what a set may take).
fn tip_set(tips: &[(u32, u32, Vec<u8>)], id: u64, kind: TipKind) -> Option<TipSet> {
    let most = procraft_gpu::MOST_TIPS as usize;
    if tips.is_empty() || tips.len() > most {
        return None;
    }
    let channels = kind.channels() as usize;
    let largest = tips.iter().map(|(w, h, _)| *w.max(h)).max().unwrap_or(1);
    let mut side = largest.next_power_of_two().clamp(16, MOST_SET_SIDE);
    while side > 16 && (side as usize).pow(2) * tips.len() * channels > MOST_SET_BYTES {
        side /= 2;
    }
    let layers = tips
        .iter()
        .map(
            |(w, h, pixels)| {
                if channels == 1 { fitted(*w, *h, pixels, side) } else { fitted_rgba(*w, *h, pixels, side) }
            },
        )
        .collect();
    Some(TipSet { id, side, kind, layers })
}

/// RGBA `pixels` (`w` x `h`, straight) fitted as [`fitted`] fits a grey picture: each channel as
/// it covers, so that clear pixels lend none of their colour; straight again.
fn fitted_rgba(w: u32, h: u32, pixels: &[u8], side: u32) -> Vec<u8> {
    let px = pixels.as_chunks::<4>().0;
    let channel = |c: usize| -> Vec<u8> {
        let plane: Vec<u8> = px
            .iter()
            .map(|&[r, g, b, a]| {
                let v = match c {
                    0 => r,
                    1 => g,
                    2 => b,
                    _ => return a,
                };
                ((u32::from(v) * u32::from(a) + 127) / 255) as u8
            })
            .collect();
        fitted(w, h, &plane, side)
    };
    let [r, g, b, a] = [channel(0), channel(1), channel(2), channel(3)];
    let straight =
        |v: u8, a: u8| if a == 0 { 0 } else { ((u32::from(v) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8 };
    r.iter()
        .zip(&g)
        .zip(&b)
        .zip(&a)
        .flat_map(|(((&r, &g), &b), &a)| [straight(r, a), straight(g, a), straight(b, a), a])
        .collect()
}

/// `pixels` (`w` x `h`) fitted into a square `side` a side, its longer side across all of it,
/// centred: each pixel the average of those it covers, or between the four nearest where it is
/// drawn larger.
fn fitted(w: u32, h: u32, pixels: &[u8], side: u32) -> Vec<u8> {
    let tip = procraft_engine::abr::AbrTip {
        name: String::new(),
        spacing: None,
        width: w,
        height: h,
        pixels: pixels.to_vec(),
    };
    crate::brush_files::fitted_at(&tip, side as usize)
}

/// `pixels` (`w` x `h`) drawn over a square `side` a side, stretched to fill it (a texture's tile).
fn stretched(w: u32, h: u32, pixels: &[u8], side: u32) -> Vec<u8> {
    let (w, h, side) = (w as usize, h as usize, side as usize);
    let get = |x: usize, y: usize| f32::from(pixels.get(y.min(h - 1) * w + x.min(w - 1)).copied().unwrap_or(0));
    let mut out = vec![0_u8; side * side];
    for y in 0..side {
        let v = ((y as f32 + 0.5) * h as f32 / side as f32 - 0.5).max(0.0);
        let (y0, fy) = (v.floor() as usize, v.fract());
        for x in 0..side {
            let u = ((x as f32 + 0.5) * w as f32 / side as f32 - 0.5).max(0.0);
            let (x0, fx) = (u.floor() as usize, u.fract());
            // wrapping round, as a tile does
            let (x1, y1) = ((x0 + 1) % w, (y0 + 1) % h);
            let top = get(x0, y0) * (1.0 - fx) + get(x1, y0) * fx;
            let bottom = get(x0, y1) * (1.0 - fx) + get(x1, y1) * fx;
            if let Some(v) = out.get_mut(y * side + x) {
                *v = (top * (1.0 - fy) + bottom * fy).round() as u8;
            }
        }
    }
    out
}

/// A look as it is kept on the disk: as the GPU takes it (its tips fitted, its texture a square),
/// so that a start only reads it. Fields: 1 its version, 2 its tips (a side, then each layer), 3
/// how they build up, 4 its blend, 5 whether it erases, 6 its texture (a side, its pixels, its
/// tile, its mode) and 7 its second (its tips, how they build up, how it combines).
const KEPT_LOOK_VERSION: u32 = 4;

fn kept_tips(set: &TipSet) -> Vec<u8> {
    let mut r = Record::default();
    r.field(1, &set.side.to_le_bytes()).field(3, &[set.kind as u8]);
    for layer in &set.layers {
        r.field(2, layer);
    }
    r.bytes()
}

fn kept_build(build: Build) -> u8 {
    u8::from(build == Build::Towards)
}

/// `look` as it is kept on the disk.
pub(crate) fn kept_look(look: &BrushLook) -> Vec<u8> {
    let mut r = Record::default();
    r.field(1, &KEPT_LOOK_VERSION.to_le_bytes()).field(2, &kept_tips(&look.tips)).field(3, &[kept_build(look.build)]);
    if let Some(blend) = look.blend {
        r.field(4, &blend.to_le_bytes());
    }
    r.field(5, &[u8::from(look.erases)]);
    if let Some(ms) = look.airbrush {
        r.field(8, &ms.to_le_bytes());
    }
    r.field(9, &[u8::from(look.smudges)]);
    if let Some(t) = &look.texture {
        let mut tex = Record::default();
        tex.field(1, &t.side.to_le_bytes())
            .field(2, &t.pixels)
            .field(3, &t.tile.to_le_bytes())
            .field(4, &t.mode.to_le_bytes());
        r.field(6, &tex.bytes());
    }
    if let Some(s) = &look.second {
        let mut second = Record::default();
        second.field(1, &kept_tips(&s.tips)).field(2, &[kept_build(s.build)]).field(3, &s.combine.to_le_bytes());
        r.field(7, &second.bytes());
    }
    r.bytes()
}

/// A tip set as kept: a side a power of two the GPU takes, each layer that square.
fn tips_kept(bytes: &[u8], id: u64) -> Option<TipSet> {
    let (mut side, mut kind) = (None, TipKind::Mask);
    let mut layers = Vec::new();
    for (f, value) in fields(bytes) {
        match f {
            1 => side = u32_of(value),
            2 => layers.push(value.to_vec()),
            3 => {
                kind = match value.first() {
                    Some(1) => TipKind::Picture,
                    Some(2) => TipKind::Lightness,
                    Some(3) => TipKind::Gradient,
                    _ => TipKind::Mask,
                }
            }
            _ => {}
        }
    }
    let side = side.filter(|s| s.is_power_of_two() && (16..=MOST_SET_SIDE).contains(s))?;
    let fits = !layers.is_empty()
        && layers.len() <= procraft_gpu::MOST_TIPS as usize
        && layers.iter().all(|l| l.len() == (side as usize).pow(2) * kind.channels() as usize);
    fits.then_some(TipSet { id, side, kind, layers })
}

/// The look kept on the disk as `bytes`, its tips' set given `id` (and the rest near it, as
/// [`read_look`] gives them); none if it doesn't hold up, or was kept by another version.
pub(crate) fn look_kept_as(bytes: &[u8], id: u64) -> Option<BrushLook> {
    let (mut version, mut tips, mut build, mut blend, mut erases, mut texture, mut second) =
        (None, None, Build::Over, None, false, None, None);
    let (mut airbrush, mut smudges) = (None, false);
    for (f, value) in fields(bytes) {
        match f {
            1 => version = u32_of(value),
            9 => smudges = value.first() == Some(&1),
            8 => airbrush = f32_of(value).filter(|ms| *ms >= MOST_AIRBRUSH_MS),
            2 => tips = Some(tips_kept(value, id)?),
            3 => build = read_build(value),
            4 => blend = u32_of(value),
            5 => erases = value.first().is_some_and(|&b| b != 0),
            6 => {
                let (mut side, mut pixels, mut tile, mut mode) = (None, None, None, None);
                for (f, value) in fields(value) {
                    match f {
                        1 => side = u32_of(value),
                        2 => pixels = Some(value.to_vec()),
                        3 => tile = f32_of(value),
                        4 => mode = u32_of(value),
                        _ => {}
                    }
                }
                let (side, pixels) = (side?, pixels?);
                let square =
                    side.is_power_of_two() && side <= MOST_TEXTURE_SIDE && pixels.len() == (side as usize).pow(2);
                if !square {
                    return None;
                }
                texture = Some(LookTexture { id: id ^ 0x7e47_0001, side, pixels, tile: tile?, mode: mode? });
            }
            7 => {
                let (mut tips, mut build, mut combine) = (None, Build::Towards, None);
                for (f, value) in fields(value) {
                    match f {
                        1 => tips = Some(tips_kept(value, id ^ 0x5eed_0002)?),
                        2 => build = read_build(value),
                        3 => combine = u32_of(value),
                        _ => {}
                    }
                }
                second = Some(SecondLook { tips: tips?, build, combine: combine? });
            }
            _ => {}
        }
    }
    (version == Some(KEPT_LOOK_VERSION)).then_some(())?;
    Some(BrushLook { airbrush, smudges, tips: tips?, build, blend, erases, texture, second })
}

/// What a brush's load hands its engine: the brush's id and its data, and, if the app holds its
/// look, that it needn't answer one.
fn load_record(id: u64, data: &[u8], look_held: bool) -> Vec<u8> {
    let mut r = Record::default();
    r.field(field::LOAD_ID, &id.to_le_bytes()).field(field::LOAD_DATA, data);
    if look_held {
        r.field(field::LOAD_LOOK_HELD, &[1]);
    }
    r.bytes()
}

/// The id of a brush's data: what its look is kept by, here and on the GPU.
pub(crate) fn data_id(data: &[u8]) -> u64 {
    crate::project::pixels_hash(data) ^ (data.len() as u64).rotate_left(17)
}

/// The brushes the engines on make of a file called `name` (`bytes`): those whose `accepts` has the
/// ending of its name, or every engine for a name without one.
pub(crate) fn make_brushes(hooks: &Hooks, name: &str, bytes: &[u8]) -> Vec<MadeBrush> {
    let ending = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    let mut file = Record::default();
    file.field(field::FILE_NAME, name.as_bytes()).field(field::FILE_BYTES, bytes);
    let file = file.bytes();
    let mut made = Vec::new();
    for (plugin, running, index, key) in hooks.engines_reading(ending.as_deref()) {
        let answer = at_work(&running).brush_import(index, &file);
        match answer {
            Ok(record) => made.extend(read_made(&record, &key)),
            Err(stop) => hooks.stopped(&plugin, stop),
        }
    }
    made
}

/// The brushes in the record an engine `key` answered an import with.
fn read_made(record: &[u8], key: &str) -> Vec<MadeBrush> {
    let mut made = Vec::new();
    for (f, value) in fields(record) {
        if f != field::MADE {
            continue;
        }
        let (mut name, mut data, mut thumbnail) = (None, None, None);
        for (f, value) in fields(value) {
            match f {
                field::MADE_NAME => name = std::str::from_utf8(value).ok().map(|n| n.chars().take(256).collect()),
                field::MADE_DATA => data = Some(value.to_vec()),
                field::MADE_THUMBNAIL => thumbnail = Some(value.to_vec()).filter(|t| t.starts_with(b"\x89PNG")),
                _ => {}
            }
        }
        if let (Some(name), Some(data)) = (name, data) {
            made.push(MadeBrush { engine: key.to_owned(), name, data, thumbnail });
        }
    }
    made
}

/// `brushes` (as the app read a file) with what engines `made` of it: each such brush joins the one
/// of its name not yet joined, or comes in on a round brush after them.
pub(crate) fn join_made(brushes: &mut Vec<Brush>, made: Vec<MadeBrush>) {
    let mut joined = vec![false; brushes.len()];
    for m in made {
        let engine = Some(OutsideBrush { engine: m.engine, data: m.data.into() });
        let at = brushes.iter().zip(&joined).position(|(b, &j)| !j && b.name.trim() == m.name.trim());
        match at.and_then(|i| brushes.get_mut(i).map(|b| (i, b))) {
            Some((i, b)) => {
                b.engine = engine;
                if b.thumbnail.is_none() {
                    b.thumbnail = m.thumbnail.map(Into::into);
                }
                if let Some(j) = joined.get_mut(i) {
                    *j = true;
                }
            }
            None => {
                brushes.push(Brush {
                    name: m.name,
                    thumbnail: m.thumbnail.map(Into::into),
                    engine,
                    ..Brush::round_pen()
                });
                joined.push(true);
            }
        }
    }
}

/// What a stroke tells its engine as it starts, besides its brush.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Starting {
    /// Its colour and secondary colour (sRGB 0..1), its size (canvas pixels) now and where the
    /// brush's size starts, the opacity slider (0..1).
    pub colour: [f32; 3],
    pub secondary: [f32; 3],
    pub diameter: f32,
    pub start_diameter: f32,
    pub opacity: f32,
    /// 0 paint, 1 erase, 2 smudge; 0 finger, 1 pen, 2 mouse.
    pub tool: u8,
    pub pointer: u8,
    pub seed: u32,
    /// How far the canvas is turned on screen (radians).
    pub turn: f32,
}

/// The look of `brush` drawn by its engine, if that is on (asked of it once for its data, then
/// kept, in memory and on the disk), and the id it goes by. One that stops goes off, and the brush
/// draws as the app read it.
pub(crate) fn look_of(hooks: &Hooks, brush: &Brush) -> Option<(Arc<BrushLook>, u64)> {
    let outside = brush.engine.as_ref()?;
    let (plugin, running, index) = hooks.engine(&outside.engine)?;
    let id = data_id(&outside.data);
    if let Some(look) = hooks.look_kept(&outside.engine, id) {
        return Some((look, id));
    }
    // kept on the disk by an earlier start (one that doesn't hold up is asked for again)
    let kept = hooks.look_on_disk(&plugin, &outside.engine, id).and_then(|bytes| look_kept_as(&bytes, id));
    if let Some(look) = kept {
        let look = Arc::new(look);
        hooks.keep_look(&outside.engine, id, Arc::clone(&look));
        return Some((look, id));
    }
    let answer = at_work(&running).brush_load(index, &load_record(id, &outside.data, false));
    let record = match answer {
        Ok(record) => record?,
        Err(stop) => {
            hooks.stopped(&plugin, stop);
            return None;
        }
    };
    let Some(look) = read_look(&record, id) else {
        log::warn!("a brush engine's look of {} doesn't hold up", brush.name);
        return None;
    };
    hooks.keep_look_on_disk(&plugin, &outside.engine, id, &kept_look(&look));
    let look = Arc::new(look);
    hooks.keep_look(&outside.engine, id, Arc::clone(&look));
    Some((look, id))
}

/// A stroke of `brush` drawn by its engine: the halves its first and second strokes take; none if
/// its engine is off or doesn't start it.
pub(crate) fn begin_stroke(
    hooks: &Arc<Hooks>,
    brush: &Brush,
    look: Arc<BrushLook>,
    id: u64,
    starting: Starting,
) -> Option<(EngineHalf, EngineHalf)> {
    let outside = brush.engine.as_ref()?;
    let (plugin, running, index) = hooks.engine(&outside.engine)?;
    let mut record = Record::default();
    let floats = |v: &[f32]| -> Vec<u8> { v.iter().flat_map(|f| f.to_le_bytes()).collect() };
    record
        .field(field::BEGIN_BRUSH, &id.to_le_bytes())
        .field(field::BEGIN_COLOUR, &floats(&starting.colour))
        .field(field::BEGIN_SECONDARY, &floats(&starting.secondary))
        .field(field::BEGIN_DIAMETER, &floats(&[starting.diameter]))
        .field(field::BEGIN_START_DIAMETER, &floats(&[starting.start_diameter]))
        .field(field::BEGIN_OPACITY, &floats(&[starting.opacity]))
        .field(field::BEGIN_TOOL, &[starting.tool])
        .field(field::BEGIN_POINTER, &[starting.pointer])
        .field(field::BEGIN_SEED, &starting.seed.to_le_bytes())
        .field(field::BEGIN_TURN, &floats(&[starting.turn]));
    let record = record.bytes();
    let begun = (|| {
        let mut code = at_work(&running);
        if let Some(number) = code.brush_begin(&record)? {
            return Ok(Some(number));
        }
        // its brush first (its look is held), then again
        if code.brush_load(index, &load_record(id, &outside.data, true))?.is_none() {
            return Ok(None);
        }
        code.brush_begin(&record)
    })();
    let number = match begun {
        Ok(number) => number?,
        Err(stop) => {
            hooks.stopped(&plugin, stop);
            return None;
        }
    };
    let core = Arc::new(Mutex::new(StrokeCore {
        hooks: Arc::clone(hooks),
        plugin,
        running,
        number: Some(number),
        colour: starting.colour,
        look,
        first_ms: None,
        fed: 0,
        ended: false,
        laid: [VecDeque::new(), VecDeque::new()],
    }));
    let half = |second| EngineHalf { core: Arc::clone(&core), second, seen: 0 };
    Some((half(false), half(true)))
}

/// A stroke an engine draws: what both its halves share.
struct StrokeCore {
    hooks: Arc<Hooks>,
    plugin: String,
    running: Shared,
    /// Its number with the engine, until it ends or the plugin stops.
    number: Option<i32>,
    colour: [f32; 3],
    look: Arc<BrushLook>,
    /// When its first sample came (ms).
    first_ms: Option<f64>,
    /// How many samples have gone to the engine, and whether it has ended.
    fed: usize,
    ended: bool,
    /// The stamps each half has yet to take: the first stroke's, the second's.
    laid: [VecDeque<Stamp>; 2],
}

impl StrokeCore {
    /// `samples` handed to the engine, its dabs kept for the halves.
    fn feed(&mut self, samples: &[Sample]) {
        let Some(number) = self.number else { return };
        let first = *self.first_ms.get_or_insert(samples.first().map_or(0.0, |s| s.time_ms));
        let mut bytes = Vec::with_capacity(samples.len() * 32);
        for s in samples {
            for v in [s.pos.x, s.pos.y, s.pressure, s.tilt, s.azimuth, s.roll] {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            bytes.extend_from_slice(&(s.time_ms - first).max(0.0).to_le_bytes());
        }
        let dabs = at_work(&self.running).brush_add(number, &bytes);
        self.take(dabs);
    }

    fn end(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        let Some(number) = self.number else { return };
        let dabs = at_work(&self.running).brush_end(number);
        self.number = None;
        self.take(dabs);
    }

    fn take(&mut self, dabs: Result<Vec<u8>, Stop>) {
        let dabs = match dabs {
            Ok(dabs) => dabs,
            Err(stop) => {
                // off, and the stroke ends where it got to
                self.number = None;
                self.hooks.stopped(&self.plugin, stop);
                return;
            }
        };
        let first_ms = self.first_ms.unwrap_or(0.0);
        for dab in dabs.as_chunks::<DAB_BYTES>().0 {
            let flags = u32_of(dab.get(32..).unwrap_or_default()).unwrap_or(0);
            let second = flags & FOR_SECOND != 0;
            let look = &*self.look;
            let (tips, build) = match (second, &look.second) {
                (false, _) => (look.tips.layers.len(), look.build),
                (true, Some(s)) => (s.tips.layers.len(), s.build),
                (true, None) => continue,
            };
            if let (Some(stamp), Some(laid)) =
                (stamp_of(dab, self.colour, tips, build, first_ms), self.laid.get_mut(usize::from(second)))
            {
                laid.push_back(stamp);
            }
        }
    }
}

impl Drop for StrokeCore {
    fn drop(&mut self) {
        // the engine lets go of a stroke the app let go of unended
        if let Some(number) = self.number.take() {
            let _ = at_work(&self.running).brush_end(number);
        }
    }
}

/// A dab (see the module's notes) as a stamp of a stroke whose brush's colour is `colour`, whose
/// tips number `tips` and whose stamps `build` up so: none if it isn't one.
fn stamp_of(dab: &[u8; DAB_BYTES], colour: [f32; 3], tips: usize, build: Build, first_ms: f64) -> Option<Stamp> {
    let word = |i: usize| -> [u8; 4] { dab.get(i * 4..i * 4 + 4).and_then(|b| b.try_into().ok()).unwrap_or([0; 4]) };
    let float = |i: usize| f32::from_le_bytes(word(i));
    let [x, y, diameter, angle, ratio, opacity, flow] = [0, 1, 2, 3, 4, 5, 6].map(float);
    let (tip, flags) = (u32::from_le_bytes(word(7)), u32::from_le_bytes(word(8)));
    let [r, g, b, strength, tx, ty, lightness, threshold, softness, share, radius] =
        [9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19].map(float);
    if ![x, y, diameter].iter().all(|v| v.is_finite()) || diameter <= 0.0 || tips == 0 {
        return None;
    }
    let unit = |v: f32, none: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { none };
    let (opacity, flow) = (unit(opacity, 1.0), unit(flow, 1.0));
    let center = Vec2::new(x, y);
    let mut s = Stamp::round(center, (diameter * 0.5).min(16_384.0), 1.0);
    (s.alpha, s.transfer) = match build {
        Build::Over => (opacity * flow, 1.0),
        Build::Towards => (flow, opacity),
    };
    s.angle = if angle.is_finite() { angle } else { 0.0 };
    s.roundness = if ratio.is_finite() { ratio.clamp(0.05, 1.0) } else { 1.0 };
    s.flip = [flags & FLIP_ACROSS != 0, flags & FLIP_DOWN != 0];
    s.tip = tip.min(tips as u32 - 1);
    if flags & OWN_COLOUR != 0 {
        s.tint = tint_to(colour, [unit(r, 0.0), unit(g, 0.0), unit(b, 0.0)]);
    }
    // the texture read where the dab lands on the canvas, moved as far as the engine says
    let offset = if tx.is_finite() && ty.is_finite() { Vec2::new(tx, ty) } else { Vec2::ZERO };
    s.grain_offset = center + offset;
    s.grain_depth = unit(strength, 1.0);
    s.lightness = unit(lightness, 1.0);
    if flags & MOVES_PAINT != 0 {
        let own = flags & OWN_COLOUR != 0;
        s.pick_up = Some(procraft_engine::stroke::PickUp {
            paint_share: unit(share, 0.0),
            paint: own.then(|| [unit(r, 0.0), unit(g, 0.0), unit(b, 0.0)]),
            dull: (flags & DULLS != 0)
                .then(|| (if radius.is_finite() { radius.clamp(0.0, 3.0) } else { 0.0 }, flags & WEIGHED != 0)),
            clear: flags & CLEAR_PAINT != 0,
        });
    }
    // made sharp, if it says (a threshold of 0 says not)
    s.threshold =
        if threshold.is_finite() && threshold > 0.0 { [threshold.min(1.0), unit(softness, 0.0)] } else { [0.0; 2] };
    s.time_ms = first_ms;
    Some(s)
}

/// Hue (turns), saturation and brightness of `c`, as the stamp shader reckons them.
fn hsb([r, g, b]: [f32; 3]) -> [f32; 3] {
    let hi = r.max(g).max(b);
    let d = hi - r.min(g).min(b);
    if d <= 1e-5 {
        return [0.0, 0.0, hi];
    }
    let h = if hi == r {
        (g - b) / d
    } else if hi == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [(h / 6.0).rem_euclid(1.0), d / hi, hi]
}

/// The tint taking `from` to `to` exactly: the hue turned to it, the saturation and brightness set.
fn tint_to(from: [f32; 3], to: [f32; 3]) -> Tint {
    let ([h0, ..], [h1, s1, b1]) = (hsb(from), hsb(to));
    Tint { hue: h1 - h0, saturation: [0.0, s1], brightness: [0.0, b1] }
}

/// One of a stroke's two halves (its first stroke, or its second), as the builder of each takes its
/// stamps: the samples go to the engine once, whichever half has them first.
#[derive(Debug)]
pub(crate) struct EngineHalf {
    core: Arc<Mutex<StrokeCore>>,
    second: bool,
    /// How many samples this half has been handed.
    seen: usize,
}

impl std::fmt::Debug for StrokeCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StrokeCore").field("plugin", &self.plugin).field("number", &self.number).finish_non_exhaustive()
    }
}

impl EngineHalf {
    fn core(&self) -> MutexGuard<'_, StrokeCore> {
        self.core.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl OutsideEngine for EngineHalf {
    fn add(&mut self, samples: &[Sample]) -> Vec<Stamp> {
        let seen = self.seen + samples.len();
        let second = self.second;
        let mut core = self.core();
        // what the other half hasn't handed over yet
        if core.fed < seen {
            let new = samples.get(samples.len().saturating_sub(seen - core.fed)..).unwrap_or_default();
            core.feed(new);
            core.fed = seen;
        }
        let laid = core.laid.get_mut(usize::from(second)).map(std::mem::take).unwrap_or_default();
        drop(core);
        self.seen = seen;
        laid.into()
    }

    fn finish(&mut self) -> Vec<Stamp> {
        let second = self.second;
        let mut core = self.core();
        core.end();
        core.laid.get_mut(usize::from(second)).map(std::mem::take).unwrap_or_default().into()
    }
}

/// A stroke a plugin's engine draws: how its first stroke and its second (if it has one) are laid,
/// and the halves of the engine's stroke each builder takes.
pub(crate) struct OutsideStroke {
    /// Every how many milliseconds its engine wants the pen again while it holds still.
    pub airbrush: Option<f32>,
    pub style: StrokeStyle,
    pub second: Option<SecondStroke>,
    pub first: EngineHalf,
    pub second_half: EngineHalf,
}

/// How a stroke of a look is laid on `canvas` with `tool`, as `starting` says (its colours and
/// opacity): its style and its second's, its tips and texture on the GPU. None if the GPU can't
/// hold its tips.
fn look_styles(
    look: &BrushLook,
    tool: StrokeTool,
    starting: &Starting,
    (alpha_lock, preview): (bool, bool),
    canvas: &mut procraft_gpu::Canvas,
    gpu: &procraft_gpu::Gpu,
) -> Option<(StrokeStyle, Option<SecondStroke>)> {
    // a brush whose dabs move paint smudges with the paint tool (an erasing one taking paint away
    // as it does), a brush that erases erases with it (its preview showing what it takes away or
    // moves: there is nothing under it there)
    let tool = match tool {
        StrokeTool::Paint if look.smudges && !preview => StrokeTool::Smudge,
        StrokeTool::Paint if look.erases && !preview => StrokeTool::Erase,
        other => other,
    };
    let paint = |build: Build, opacity: f32| {
        let mut style = StrokeStyle::new(tool, starting.colour, opacity);
        style.preserve_alpha = alpha_lock;
        style.secondary = starting.secondary;
        style.preview = preview;
        style.wetness = StrokeStyle::wetness_at(opacity);
        style.rendering = match build {
            Build::Over => Rendering::BuildUp,
            Build::Towards => Rendering::Capped,
        };
        style
    };
    let mut style = paint(look.build, starting.opacity);
    if tool == StrokeTool::Smudge && look.smudges {
        // what each stamp picks up behind it, laid as it says: nothing kept, nothing blurred
        style.pull = 0.0;
        style.blur = 0.0;
        style.rendering = Rendering::BuildUp;
    }
    if let Some(blend) = look.blend {
        style.blend = blend.min(procraft_engine::brush::BRUSH_BEHIND);
    }
    let mut second = look.second.as_ref().map(|s| SecondStroke {
        style: paint(s.build, 1.0).as_second(),
        mode: s.combine,
        threshold: -1.0,
    });
    canvas.use_tip_set(gpu, &mut style, look.tips.id, look.tips.side, &layers(&look.tips), look.tips.kind);
    // tips the GPU can't hold leave the brush as the app read it
    style.full_shape?;
    if let Some(t) = &look.texture {
        canvas.use_full_grain(gpu, &mut style, t.id, t.side, &t.pixels);
        style.grain_mode = procraft_engine::brush::GrainMode::Moving;
        style.grain_scale = t.tile.max(1.0);
        style.grain_depth = 1.0;
        style.grain_blend = t.mode;
    }
    if let (Some(s), Some(look)) = (second.as_mut(), look.second.as_ref()) {
        canvas.use_tip_set(gpu, &mut s.style, look.tips.id, look.tips.side, &layers(&look.tips), TipKind::Mask);
        if s.style.full_shape.is_none() {
            second = None;
        }
    }
    Some((style, second))
}

/// A stroke of `brush` drawn by the brush engine that made it, if that is on, on `canvas` with
/// `tool`, as `starting` says (`look`: the layer's alpha lock, and whether it is a preview's).
pub(crate) fn engine_stroke(
    hooks: &Arc<Hooks>,
    brush: &Brush,
    tool: StrokeTool,
    starting: Starting,
    look: (bool, bool),
    canvas: &mut procraft_gpu::Canvas,
    gpu: &procraft_gpu::Gpu,
) -> Option<OutsideStroke> {
    brush.engine.as_ref()?;
    let (brush_look, id) = look_of(hooks, brush)?;
    let (style, second) = look_styles(&brush_look, tool, &starting, look, canvas, gpu)?;
    let airbrush = brush_look.airbrush;
    let (first, second_half) = begin_stroke(hooks, brush, brush_look, id, starting)?;
    Some(OutsideStroke { airbrush, style, second, first, second_half })
}

impl crate::State {
    /// `builder` (not fed yet) drawn by its brush's engine as a fresh stroke seeded with `seed`, if
    /// the brush is an engine's and that is on (a builder's copy draws as the app reads the brush).
    pub(crate) fn engine_again(
        &mut self,
        builder: procraft_engine::stroke::StrokeBuilder,
        layer: procraft_engine::document::LayerId,
        seed: u32,
    ) -> procraft_engine::stroke::StrokeBuilder {
        if builder.brush().engine.is_none() || builder.is_outside() {
            return builder;
        }
        let brush = builder.brush().clone();
        let pointer = builder.pointer().into();
        match self.outside_stroke(&brush, self.tool, layer, pointer, builder.diameter(), seed) {
            Some(o) => builder.with_engine(Box::new(o.first)),
            None => builder,
        }
    }

    /// A stroke of `brush` with `tool` on `layer`, drawn by the brush engine that made the brush if
    /// that is on, at `diameter` canvas pixels (the size slider's, the brush scale in it): its
    /// style laid by what the engine says the brush looks like, its tips and texture on the GPU.
    /// None draws it as the app read it.
    pub(crate) fn outside_stroke(
        &mut self,
        brush: &Brush,
        tool: crate::ToolKind,
        layer: procraft_engine::document::LayerId,
        pointer: crate::PointerKind,
        diameter: f32,
        seed: u32,
    ) -> Option<OutsideStroke> {
        brush.engine.as_ref()?;
        let hooks = self.hooks.clone()?;
        let starting = Starting {
            colour: self.color,
            secondary: self.secondary,
            diameter,
            start_diameter: brush.diameter(self.starting_sliders(tool).size),
            opacity: self.sliders(tool).opacity.clamp(0.0, 1.0),
            tool: match tool.stroke_tool() {
                StrokeTool::Erase => 1,
                StrokeTool::Smudge => 2,
                _ => 0,
            },
            pointer: match pointer {
                crate::PointerKind::Finger => 0,
                crate::PointerKind::Mouse => 2,
                _ => 1,
            },
            seed,
            turn: self.view.map_or(0.0, |v| v.rotation()),
        };
        let look = (self.doc.layer(layer).is_some_and(|l| l.alpha_lock), self.pad_strokes.is_some());
        let (canvas, gpu) = (self.canvas.as_mut()?, self.gpu.as_ref()?);
        engine_stroke(&hooks, brush, tool.stroke_tool(), starting, look, canvas, gpu)
    }
}

#[uniffi::export]
impl CanvasSession {
    /// The plugin action that edits the current brush in place of the brush studio: the brush
    /// studio of the brush engine that made it, while that is on. None opens the brush studio as
    /// ever. The plugin's studio then reads and changes that brush (see the `brush.data`
    /// permission) until another opens.
    pub fn brush_studio_action(&self) -> Option<String> {
        let mut st = self.shared.lock();
        let tool = st.tool;
        let at = st.brush_ref(tool);
        let engine = st.brush(tool).engine.as_ref()?.engine.clone();
        let action = st.hooks.as_ref()?.engine_studio(&engine)?;
        st.engine_studio = Some(at);
        Some(action)
    }

    /// Whether a plugin changed a brush of the library since this was last asked (the library to
    /// be read again and kept).
    pub fn take_brushes_changed(&self) -> bool {
        std::mem::take(&mut self.shared.lock().brushes_changed)
    }
}

impl CanvasSession {
    /// The brush a plugin's studio edits (else the current brush), if the brush engine of plugin
    /// `owner` made it.
    fn edited_brush(st: &crate::State, owner: &str) -> Option<(usize, usize)> {
        let at = st.engine_studio.unwrap_or_else(|| st.brush_ref(st.tool));
        let brush = st.library.get(at.0)?.brushes.get(at.1)?;
        let engine = brush.engine.as_ref()?;
        engine.engine.strip_prefix(owner)?.starts_with('.').then_some(at)
    }

    /// The data of the brush plugin `owner`'s studio edits, as its engine keeps it.
    pub(crate) fn edited_brush_data(&self, owner: &str) -> Option<Arc<[u8]>> {
        let st = self.shared.lock();
        let (set, index) = Self::edited_brush(&st, owner)?;
        let brush = st.library.get(set)?.brushes.get(index)?;
        brush.engine.as_ref().map(|e| Arc::clone(&e.data))
    }

    /// The brush plugin `owner`'s studio edits keeps `data` from now on; false if there is none.
    pub(crate) fn set_edited_brush_data(&self, owner: &str, data: Vec<u8>) -> bool {
        let mut st = self.shared.lock();
        let Some((set, index)) = Self::edited_brush(&st, owner) else { return false };
        let Some(brush) = st.library.get_mut(set).and_then(|s| s.brushes.get_mut(index)) else { return false };
        let Some(engine) = brush.engine.as_mut() else { return false };
        engine.data = data.into();
        st.brushes_changed = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tip(w: u32, h: u32) -> Vec<u8> {
        let mut t = Record::default();
        t.field(field::TIP_WIDTH, &w.to_le_bytes())
            .field(field::TIP_HEIGHT, &h.to_le_bytes())
            .field(field::TIP_PIXELS, &vec![255; (w * h) as usize]);
        t.bytes()
    }

    /// A Krita preset of a hard square tip 20 pixels across, spaced 2 diameters apart: a PNG with
    /// only its settings in it.
    pub(crate) fn square_preset() -> Vec<u8> {
        preset_of("")
    }

    /// The square preset with `more` params besides.
    fn preset_of(more: &str) -> Vec<u8> {
        let xml = format!(
            r#"<Preset name="Squares" paintopid="paintbrush"><param name="brush_definition" type="string"><![CDATA[<Brush type="auto_brush" spacing="2" useAutoSpacing="0" angle="0" density="1" randomness="0"><MaskGenerator type="rect" id="default" diameter="20" ratio="1" hfade="1" vfade="1" spikes="2" antialiasEdges="0"/></Brush>]]></param><param name="PaintOpAction" type="string"><![CDATA[1]]></param><param name="CompositeOp" type="string"><![CDATA[normal]]></param><param name="PressureSize" type="string"><![CDATA[false]]></param><param name="OpacityUseCurve" type="string"><![CDATA[false]]></param><param name="FlowUseCurve" type="string"><![CDATA[false]]></param>{more}</Preset>"#
        );
        let chunk = |kind: &[u8], data: &[u8]| {
            let mut c = (data.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(kind);
            c.extend_from_slice(data);
            // the reader doesn't check sums
            c.extend_from_slice(&[0; 4]);
            c
        };
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = Vec::new();
        for v in [1_u32, 1] {
            header.extend_from_slice(&v.to_be_bytes());
        }
        header.extend_from_slice(&[8, 0, 0, 0, 0]);
        png.extend(chunk(b"IHDR", &header));
        let mut text = b"preset\0".to_vec();
        text.extend_from_slice(xml.as_bytes());
        png.extend(chunk(b"tEXt", &text));
        png.extend(chunk(b"IEND", &[]));
        png
    }

    /// The Krita plugin the app comes with (or the one at `KRITA_PLUGIN`) drawing a preset's squares
    /// where Krita lays them, its studio changing their size; with it off, the brush draws as read.
    #[test]
    fn krita_squares_lie_where_the_engine_spaces_them() {
        let packed = match std::env::var("KRITA_PLUGIN") {
            Ok(path) => std::fs::read(path).unwrap(),
            Err(_) => crate::bundled::BUNDLED.iter().find(|(name, _)| *name == "krita-brushes").unwrap().1.to_vec(),
        };
        let Ok(gpu) = procraft_gpu::Gpu::headless() else { return };
        let s = crate::CanvasSession::new(200, 120);
        {
            let mut st = s.shared.lock();
            st.window_size = (200, 120);
            st.view = Some(procraft_engine::view::View::fit((200, 120), (200, 120)));
            st.gpu = Some(gpu);
            st.ensure_canvas();
        }
        let dir = std::env::temp_dir().join(format!("procraft-krita-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let plugins = crate::Plugins::with_store(dir.to_string_lossy().into_owned());
        assert_eq!(plugins.install(packed), None);
        let id = plugins.plugins().into_iter().find(|p| p.name == "Krita brushes").unwrap().id;
        assert!(plugins.set_on(id.clone(), true));
        s.use_plugins(Arc::clone(&plugins));
        // read off the interface's thread, the engine making its brushes there too, then added
        let read = crate::read_brushes(square_preset(), Some("Squares.kpp".to_owned())).unwrap();
        s.make_engine_brushes(Arc::clone(&read));
        let set = s.add_brushes_read(read, 0).unwrap();
        assert!(s.shared.lock().library[set as usize].brushes[0].engine.is_some(), "made by the engine too");
        assert!(s.select_brush(set, 0));
        s.set_color(0.0, 0.0, 0.0);
        let draw = |s: &crate::CanvasSession| {
            let layer = s.add_layer();
            let at = |x: f32| crate::PenSample {
                x,
                y: 60.0,
                pressure: 1.0,
                tilt: 0.0,
                orientation: 0.0,
                time_ms: f64::from(x),
                roll: 0.0,
            };
            s.stroke_begin(crate::PointerKind::Stylus, vec![at(20.0)]);
            s.stroke_add((21..=180).map(|x| at(x as f32)).collect());
            s.stroke_end(vec![]);
            s.shared.lock().layer_pixels(procraft_engine::document::LayerId(layer)).unwrap()
        };
        let alpha = |rgba: &[u8], x: usize, y: usize| rgba[(y * 200 + x) * 4 + 3];
        let on = draw(&s);
        // a square at the start, 20 across; the next 40 on (twice its size), none between
        assert!(alpha(&on, 20, 60) > 240 && alpha(&on, 28, 52) > 200, "the first square");
        assert!(alpha(&on, 60, 60) > 240, "the next, 40 on");
        assert_eq!(alpha(&on, 40, 60), 0, "nothing between them");
        // held still, the line it drew snaps to a line, laid again by the engine: squares 40 apart
        let layer = s.add_layer();
        let at = |x: f32, t: f64| crate::PenSample {
            x,
            y: 60.0,
            pressure: 1.0,
            tilt: 0.0,
            orientation: 0.0,
            time_ms: t,
            roll: 0.0,
        };
        s.stroke_begin(crate::PointerKind::Stylus, vec![at(20.0, 0.0)]);
        s.stroke_add((21..=180).map(|x| at(x as f32, f64::from(x))).collect());
        assert!(s.quickshape_hold().is_some(), "a line");
        s.stroke_end(vec![]);
        s.quickshape_done();
        let shape = s.shared.lock().layer_pixels(procraft_engine::document::LayerId(layer)).unwrap();
        assert!(alpha(&shape, 20, 60) > 240 && alpha(&shape, 60, 60) > 240, "squares along the shape");
        assert_eq!(alpha(&shape, 40, 60), 0, "spaced as the engine spaces them");
        // its preview drawn by the engine: squares too, not the preset's picture
        let preview = s.brush_preview(set, 0, 400, 120).expect("a preview");
        assert!(preview.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
        // an eraser's preview shows what it takes away
        let read = crate::read_brushes(
            preset_of(r#"<param name="EraserMode"><![CDATA[true]]></param>"#),
            Some("Eraser.kpp".to_owned()),
        )
        .unwrap();
        s.make_engine_brushes(Arc::clone(&read));
        let erasers = s.add_brushes_read(read, 0).unwrap();
        let preview = s.brush_preview(erasers, 0, 400, 120).expect("an eraser's preview");
        assert!(preview.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0), "an eraser's preview draws");
        // written out as a brush file and read back, it keeps its engine and data
        let file = s.brush_as_file(set, 0).unwrap();
        let back = s.import_brushes(file).unwrap();
        let engine = |set: u32| s.shared.lock().library[set as usize].brushes[0].engine.clone();
        assert!(engine(back).is_some());
        assert_eq!(engine(back), engine(set));
        // its studio, the plugin's own: the tip made 40 across on its first page
        let studio = s.brush_studio_action().expect("its plugin's studio");
        assert!(plugins.run_action(studio, Arc::clone(&s)));
        let panel = plugins.panel().expect("the studio's panel");
        assert_eq!(panel.title, "Squares");
        let row = panel
            .rows
            .iter()
            .position(|r| matches!(r, crate::PanelRow::Slider { label, .. } if label == "Diameter"))
            .unwrap();
        assert!(plugins.panel_touched(row as u32, 40.0, Arc::clone(&s)));
        assert!(s.take_brushes_changed(), "the brush changed");
        assert!(!s.take_brushes_changed());
        let bigger = draw(&s);
        assert!(alpha(&bigger, 38, 60) > 240, "the first square, 40 across");
        assert!(alpha(&bigger, 100, 60) > 240 && alpha(&bigger, 60, 60) == 0, "the next, 80 on");
        assert!(plugins.set_on(id, false));
        assert_eq!(s.brush_studio_action(), None, "with its plugin off, the brush studio as ever");
        // as the app read it: paint, the app's own way
        let off = draw(&s);
        assert!(off.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
        assert_ne!(off, bigger);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Krita colour smudge brush moves the paint it goes over: dragged from red into the clear, it
    /// lays the red where there was nothing, whatever the colour.
    #[test]
    fn a_krita_colour_smudge_moves_the_paint_it_goes_over() {
        let Ok(gpu) = procraft_gpu::Gpu::headless() else { return };
        let s = crate::CanvasSession::new(200, 120);
        {
            let mut st = s.shared.lock();
            st.window_size = (200, 120);
            st.view = Some(procraft_engine::view::View::fit((200, 120), (200, 120)));
            st.gpu = Some(gpu);
            st.ensure_canvas();
        }
        let dir = std::env::temp_dir().join(format!("procraft-colour-smudge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let plugins = crate::Plugins::with_store(dir.to_string_lossy().into_owned());
        let packed = crate::bundled::BUNDLED.iter().find(|(name, _)| *name == "krita-brushes").unwrap().1;
        assert_eq!(plugins.install(packed.to_vec()), None);
        let id = plugins.plugins().into_iter().find(|p| p.name == "Krita brushes").unwrap().id;
        assert!(plugins.set_on(id, true));
        s.use_plugins(Arc::clone(&plugins));
        let xml = r#"<Preset name="Smear" paintopid="colorsmudge"><param name="brush_definition"><![CDATA[<Brush type="auto_brush" spacing="0.1" BrushVersion="2"><MaskGenerator type="circle" id="default" diameter="20" ratio="1" hfade="1" vfade="1"/></Brush>]]></param><param name="PressureSize"><![CDATA[false]]></param><param name="PressureSmudgeRate"><![CDATA[false]]></param><param name="SmudgeRateValue"><![CDATA[1]]></param><param name="SmudgeRateMode"><![CDATA[0]]></param></Preset>"#;
        let chunk = |kind: &[u8], data: &[u8]| {
            let mut c = (data.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(kind);
            c.extend_from_slice(data);
            c.extend_from_slice(&[0; 4]);
            c
        };
        let mut kpp = b"\x89PNG\r\n\x1a\n".to_vec();
        kpp.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0]));
        kpp.extend(chunk(b"tEXt", &[b"preset\0".as_slice(), xml.as_bytes()].concat()));
        kpp.extend(chunk(b"IEND", &[]));
        let read = crate::read_brushes(kpp, Some("Smear.kpp".to_owned())).unwrap();
        let set = s.add_brushes_read(read, 0).unwrap();
        assert!(s.select_brush(set, 0));
        // red on the left half
        let layer = s.add_layer();
        {
            let mut st = s.shared.lock();
            let rect = procraft_engine::geom::IRect::new(0, 0, 100, 120);
            st.rewrite_layer(procraft_engine::document::LayerId(layer), rect, [255, 0, 0, 255].repeat(100 * 120));
        }
        s.set_color(0.0, 0.0, 1.0);
        let at = |x: f32| crate::PenSample {
            x,
            y: 60.0,
            pressure: 1.0,
            tilt: 0.0,
            orientation: 0.0,
            time_ms: f64::from(x),
            roll: 0.0,
        };
        s.stroke_begin(crate::PointerKind::Stylus, vec![at(60.0)]);
        s.stroke_add((61..=160).map(|x| at(x as f32)).collect());
        s.stroke_end(vec![]);
        let px = s.shared.lock().layer_pixels(procraft_engine::document::LayerId(layer)).unwrap();
        let at_x = |x: usize| &px[(60 * 200 + x) * 4..(60 * 200 + x) * 4 + 4];
        assert!(at_x(130)[0] > 128 && at_x(130)[3] > 128 && at_x(130)[2] < 64, "red dragged out: {:?}", at_x(130));
        assert_eq!(at_x(190)[3], 0, "past the stroke, still clear");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An airbrush's engine is handed the pen again while it holds still, and lays its dabs as
    /// the time passes.
    #[test]
    fn an_airbrush_lays_dabs_while_the_pen_holds_still() {
        let Ok(gpu) = procraft_gpu::Gpu::headless() else { return };
        let s = crate::CanvasSession::new(200, 120);
        {
            let mut st = s.shared.lock();
            st.window_size = (200, 120);
            st.view = Some(procraft_engine::view::View::fit((200, 120), (200, 120)));
            st.gpu = Some(gpu);
            st.ensure_canvas();
        }
        let dir = std::env::temp_dir().join(format!("procraft-airbrush-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let plugins = crate::Plugins::with_store(dir.to_string_lossy().into_owned());
        let packed = crate::bundled::BUNDLED.iter().find(|(name, _)| *name == "krita-brushes").unwrap().1;
        assert_eq!(plugins.install(packed.to_vec()), None);
        let id = plugins.plugins().into_iter().find(|p| p.name == "Krita brushes").unwrap().id;
        assert!(plugins.set_on(id, true));
        s.use_plugins(Arc::clone(&plugins));
        // a dab every 50 ms
        let airbrush = r#"<param name="PaintOpSettings/isAirbrushing" type="string"><![CDATA[true]]></param><param name="PaintOpSettings/rate" type="string"><![CDATA[20]]></param>"#;
        let read = crate::read_brushes(preset_of(airbrush), Some("Air.kpp".to_owned())).unwrap();
        let set = s.add_brushes_read(read, 0).unwrap();
        assert!(s.select_brush(set, 0));
        let at = |t: f64| crate::PenSample {
            x: 50.0,
            y: 60.0,
            pressure: 1.0,
            tilt: 0.0,
            orientation: 0.0,
            time_ms: t,
            roll: 0.0,
        };
        s.stroke_begin(crate::PointerKind::Stylus, vec![at(1000.0)]);
        let count = |s: &crate::CanvasSession| s.shared.lock().stroke.as_ref().map_or(0, |b| b.stamp_count());
        let first = count(&s);
        assert_eq!(s.touch_deadline(), Some(1025.0), "handed again every half interval");
        s.touch_tick(1500.0);
        assert!(count(&s) >= first + 9, "{first} then {}", count(&s));
        s.stroke_end(vec![]);
        assert_eq!(s.touch_deadline(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A look its engine gave is kept on the disk with the plugin: the next start reads it from
    /// there, and the engine only reads the brush to draw it. Kept again, the plugin's looks go.
    #[test]
    fn a_look_is_kept_on_the_disk_until_its_plugin_is_kept_again() {
        let packed = crate::bundled::BUNDLED.iter().find(|(name, _)| *name == "krita-brushes").unwrap().1;
        let dir = std::env::temp_dir().join(format!("procraft-kept-looks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = || crate::Plugins::with_store(dir.to_string_lossy().into_owned());
        let plugins = store();
        assert_eq!(plugins.install(packed.to_vec()), None);
        let id = plugins.plugins().into_iter().find(|p| p.name == "Krita brushes").unwrap().id;
        assert!(plugins.set_on(id.clone(), true));
        let hooks = plugins.hooks();
        let mut brushes = Vec::new();
        join_made(&mut brushes, make_brushes(&hooks, "Squares.kpp", &square_preset()));
        let brush = &brushes[0];
        let key = brush.engine.as_ref().unwrap().engine.clone();
        let (look, data) = look_of(&hooks, brush).unwrap();
        let kept = hooks.look_on_disk(&id, &key, data).expect("kept on the disk");
        assert_eq!(look_kept_as(&kept, data).as_ref(), Some(&*look));
        drop((plugins, hooks));

        // the next start: what is kept there (a look of a 3 x 5 tip put in its place) is taken
        let next = store();
        assert!(next.set_on(id.clone(), true), "on again, as the app turns it on at a start");
        let hooks = next.hooks();
        let mut other = Record::default();
        other.field(field::TIP, &tip(3, 5));
        let other = read_look(&other.bytes(), data).unwrap();
        crate::plugin_file::write_look(&dir, &id, &key, data, &kept_look(&other));
        let (again, _) = look_of(&hooks, brush).unwrap();
        assert_eq!(*again, other);
        // the engine reads the brush, its look held, and draws it
        let starting = Starting {
            colour: [0.0; 3],
            secondary: [1.0; 3],
            diameter: 1.0,
            start_diameter: 1.0,
            opacity: 1.0,
            tool: 0,
            pointer: 1,
            seed: 7,
            turn: 0.0,
        };
        let (mut first, _) = begin_stroke(&hooks, brush, again, data, starting).expect("a stroke");
        let samples: Vec<Sample> =
            (0..100).map(|i| Sample::at(Vec2::new(20.0 + i as f32, 60.0), 1.0, f64::from(i))).collect();
        let stamps = first.add(&samples);
        assert!(stamps.len() >= 2 && stamps.iter().all(|s| s.tip == 0), "squares, 40 apart");
        // the plugin kept again: its looks go
        assert_eq!(next.install(packed.to_vec()), None);
        assert_eq!(hooks.look_on_disk(&id, &key, data), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// How long the bundled Krita plugin takes on a real file, in the interpreter: making its
    /// brushes, reading each, and a stroke of each (`KRITA_FILE=x.bundle cargo test -p
    /// procraft-core krita_timings -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn krita_timings() {
        use std::time::Instant;
        let Ok(path) = std::env::var("KRITA_FILE") else { return };
        let dir = std::env::temp_dir().join(format!("procraft-krita-timings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let plugins = crate::Plugins::with_store(dir.to_string_lossy().into_owned());
        let packed = crate::bundled::BUNDLED.iter().find(|(name, _)| *name == "krita-brushes").unwrap().1;
        assert_eq!(plugins.install(packed.to_vec()), None);
        let id = plugins.plugins().into_iter().find(|p| p.name == "Krita brushes").unwrap().id;
        assert!(plugins.set_on(id.clone(), true));
        let hooks = plugins.hooks();
        let bytes = std::fs::read(&path).unwrap();
        let t = Instant::now();
        let made = make_brushes(&hooks, &path, &bytes);
        println!("{} brushes made of {} bytes in {:?}", made.len(), bytes.len(), t.elapsed());
        let mut brushes = Vec::new();
        join_made(&mut brushes, made);
        let (mut loading, mut drawing, mut dabs) = (std::time::Duration::ZERO, std::time::Duration::ZERO, 0);
        for brush in &brushes {
            let t = Instant::now();
            let Some((look, id)) = look_of(&hooks, brush) else { continue };
            loading += t.elapsed();
            let starting = Starting {
                colour: [0.0; 3],
                secondary: [1.0; 3],
                diameter: 1.0,
                start_diameter: 1.0,
                opacity: 1.0,
                tool: 0,
                pointer: 1,
                seed: 7,
                turn: 0.0,
            };
            let Some((mut first, _)) = begin_stroke(&hooks, brush, look, id, starting) else { continue };
            let t = Instant::now();
            // a second of a pen at 240 samples a second, 4 at a time as frames bring them
            for batch in 0..60 {
                let samples: Vec<Sample> = (0..4)
                    .map(|i| {
                        let k = (batch * 4 + i) as f32;
                        Sample::at(Vec2::new(100.0 + k * 3.0, 300.0 + (k * 0.05).sin() * 40.0), 0.7, f64::from(k) * 4.0)
                    })
                    .collect();
                dabs += first.add(&samples).len();
            }
            dabs += first.finish().len();
            drawing += t.elapsed();
        }
        println!("{} read in {loading:?}; their strokes' {dabs} dabs in {drawing:?}", brushes.len());
        // the next start: each look from the disk, the engine reading each brush only to draw it
        let next = crate::Plugins::with_store(dir.to_string_lossy().into_owned());
        assert!(next.set_on(id, true));
        let hooks = next.hooks();
        let t = Instant::now();
        let (mut begun, mut looking) = (0, std::time::Duration::ZERO);
        for brush in &brushes {
            let t = Instant::now();
            let Some((look, id)) = look_of(&hooks, brush) else { continue };
            looking += t.elapsed();
            let starting = Starting {
                colour: [0.0; 3],
                secondary: [1.0; 3],
                diameter: 1.0,
                start_diameter: 1.0,
                opacity: 1.0,
                tool: 0,
                pointer: 1,
                seed: 7,
                turn: 0.0,
            };
            begun += usize::from(begin_stroke(&hooks, brush, look, id, starting).is_some());
        }
        println!("the next start: {begun} looks kept and strokes begun in {:?} (the looks {looking:?})", t.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_look_holds_its_tips_fitted_into_one_square_side() {
        let mut look = Record::default();
        look.field(field::TIP, &tip(100, 50)).field(field::TIP, &tip(30, 30)).field(field::BUILD, &[1]);
        look.field(field::BLEND, &3_u32.to_le_bytes()).field(field::ERASES, &[1]);
        let look = read_look(&look.bytes(), 7).unwrap();
        assert_eq!(look.tips.side, 128);
        assert_eq!(look.tips.layers.len(), 2);
        assert_eq!((look.build, look.blend, look.erases), (Build::Towards, Some(3), true));
        // the wide tip across all of the square, a quarter of it clear above and below
        let wide = &look.tips.layers[0];
        assert_eq!(wide[64 * 128 + 64], 255);
        assert_eq!(wide[10 * 128 + 64], 0);
        // a tip whose pixels aren't its size, or none, holds nothing up
        let mut bad = Record::default();
        bad.field(field::TIP, &tip(4, 4)[..20]);
        assert!(read_look(&bad.bytes(), 7).is_none());
        assert!(read_look(&[], 7).is_none());
    }

    #[test]
    fn a_dab_becomes_a_stamp_as_its_stroke_builds_up() {
        let mut dab = [0_u8; DAB_BYTES];
        let words: [f32; 7] = [10.0, 20.0, 8.0, 0.5, 0.5, 0.6, 0.25];
        for (i, v) in words.iter().enumerate() {
            dab[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        dab[28..32].copy_from_slice(&9_u32.to_le_bytes());
        dab[32..36].copy_from_slice(&(FLIP_ACROSS | OWN_COLOUR).to_le_bytes());
        for (i, v) in [0.0_f32, 0.0, 1.0, 0.5].iter().enumerate() {
            dab[36 + i * 4..40 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        let over = stamp_of(&dab, [1.0, 0.0, 0.0], 4, Build::Over, 0.0).unwrap();
        assert_eq!((over.center, over.radius, over.roundness), (Vec2::new(10.0, 20.0), 4.0, 0.5));
        assert!((over.alpha - 0.15).abs() < 1e-6 && over.transfer == 1.0);
        assert_eq!((over.tip, over.flip, over.grain_depth), (3, [true, false], 0.5));
        let towards = stamp_of(&dab, [1.0, 0.0, 0.0], 4, Build::Towards, 0.0).unwrap();
        assert_eq!((towards.alpha, towards.transfer), (0.25, 0.6));
        // red tinted to blue: the hue turned two thirds, kept full
        assert!((towards.tint.hue - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!((towards.tint.saturation, towards.tint.brightness), ([0.0, 1.0], [0.0, 1.0]));
        // a dab of no size, or with no tips to draw, is none
        dab[8..12].copy_from_slice(&0.0_f32.to_le_bytes());
        assert!(stamp_of(&dab, [1.0; 3], 4, Build::Over, 0.0).is_none());
    }

    #[test]
    fn made_brushes_join_those_read_by_their_names() {
        let mut brushes = vec![Brush { name: "Pencil".into(), ..Brush::round_pen() }];
        let made = |name: &str| MadeBrush { engine: "a.b.c".into(), name: name.into(), data: vec![1], thumbnail: None };
        join_made(&mut brushes, vec![made("Pencil"), made("Ink")]);
        assert_eq!(brushes.len(), 2);
        assert_eq!(brushes[0].engine.as_ref().map(|e| e.engine.as_str()), Some("a.b.c"));
        assert_eq!(brushes[1].name, "Ink");
        assert!(brushes[1].engine.is_some());
    }
}
