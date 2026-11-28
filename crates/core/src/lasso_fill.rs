//! Strokes that fill what they enclose, as a plugin on the stroke-fill hook asks (see
//! [`crate::hooks::Hooks::stroke_fill`]). The app's, whichever plugin asks: the stroke's path,
//! smoothed as its brush smooths it and closed by a straight edge back to where it began, fills by
//! the non-zero winding rule, one coat however often it winds round (a loop drawn twice is no
//! darker, both lobes of a figure eight fill). It is laid as the stroke's paint would be: its colour
//! at the brush's opacity times the opacity slider, edges smoothed, taking the paper and the
//! brush's grain fixed to the canvas, inside the selection, keeping the alpha lock, erasing with
//! the eraser; one undo step, named [`STEP`]. Each copy a symmetry makes fills too.
//!
//! A region shown as it grows is laid a part at a time: each frame makes and lays again only where
//! it changed since the last (a thin wedge from where the path began to where the pen went), so on
//! a large canvas a frame costs what the region grew by, not all of it. Its pixels are the same
//! however they were made, a part at a time or all at once (see [`raster`]), so as the pen lifts
//! only the last change is laid.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, mpsc};

use procraft_engine::geom::{IRect, Vec2};
use procraft_engine::input::Sample;
use procraft_engine::selection::Mask;
use procraft_engine::stroke::StrokeBuilder;

use crate::{PointerKind, State, ToolKind};

/// What undo calls a fill.
pub(crate) const STEP: &str = "Lasso fill";

/// How a fill stroke shows while it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FillShown {
    /// The region it encloses so far, filled, as it grows.
    Growing,
    /// As a plain stroke, filled as it ends.
    AtLift,
}

impl FillShown {
    /// As the hook answers it: 1 growing, 2 at the lift; none for 0 (it paints as usual).
    fn of(said: u8) -> Option<FillShown> {
        match said {
            1 => Some(FillShown::Growing),
            2 => Some(FillShown::AtLift),
            _ => None,
        }
    }
}

/// The open stroke filling what it encloses.
pub(crate) struct Filling {
    pub(crate) shown: FillShown,
    /// Its path, smoothed as the brush smooths it (see [`StrokeBuilder::fill_path`]).
    path: StrokeBuilder,
    /// Where the pen is now (canvas pixels): the path, which catches up with it only as the pen
    /// lifts, is shown going on to it, so that what shows as it grows is what is laid.
    pen: Option<Vec2>,
    /// What the region last asked for (shown, or being made) was made of.
    shown_at: Option<MadeOf>,
    /// The region's next change being made on a thread of its own as it grows (a big one takes
    /// tens of milliseconds, which the frames, on the desktop the window's thread, would wait for),
    /// and what it is made of: the first frame after it is made lays it.
    making: Option<(Option<MadeOf>, mpsc::Receiver<Grown>)>,
    /// The region laid so far as it grows (none yet, or with the thread making its next change).
    laid: Option<Laid>,
    /// What is selected as it began (it can't change while the stroke goes on), shared with the
    /// regions being made.
    selection: Option<Arc<Mask>>,
}

/// What a fill's region shown was made of: how many of the path's points, and where the pen was.
type MadeOf = (usize, Option<Vec2>);

/// A region a fill lays: its box and how much of each pixel there (see [`coverage`]); none if it
/// fills none.
type Region = Option<(IRect, Vec<u8>)>;

/// A growing fill's region as laid so far: the outlines it was made from and its pixels.
struct Laid {
    outlines: Vec<Vec<Vec2>>,
    pixels: Pixels,
}

/// A growing fill's region made again where it changed: as now laid, and the parts to lay again
/// (rectangles and their coverage). `whole`: none was laid before, so its one part is its box, and
/// the stroke's paint is cleared first.
struct Grown {
    laid: Laid,
    parts: Vec<(IRect, Vec<u8>)>,
    whole: bool,
}

/// What a fill's region is made from, apart from the canvas's state: the outlines (its path and
/// each copy the symmetry makes of it, canvas pixels), the canvas's size and the selection.
struct RegionOf {
    outlines: Vec<Vec<Vec2>>,
    width: u32,
    height: u32,
    selection: Option<Arc<Mask>>,
}

impl RegionOf {
    /// The region the outlines fill together, inside the selection.
    fn region(&self) -> Region {
        let (rect, mut coverage) = coverage(&self.outlines, self.width, self.height)?;
        self.select(rect, &mut coverage);
        coverage.iter().any(|&c| c > 0).then_some((rect, coverage))
    }

    /// `coverage` (of `rect`) kept inside the selection.
    fn select(&self, rect: IRect, coverage: &mut [u8]) {
        if let Some(mask) = &self.selection {
            let selected = crate::layer_ops::mask_part(mask, rect);
            for (c, &m) in coverage.iter_mut().zip(&selected) {
                *c = ((u16::from(*c) * u16::from(m) + 127) / 255) as u8;
            }
        }
    }

    /// The region made again where it changed from `laid` (all of it with none laid), as
    /// [`Grown`] has it.
    fn grow(self, laid: Option<Laid>) -> Grown {
        let edges = edges_of(&self.outlines);
        let (rects, whole) = match &laid {
            Some(laid) => (changed(&laid.outlines, &self.outlines, self.width, self.height), false),
            None => {
                let rect = bounds_of(&self.outlines, self.width, self.height);
                (if rect.is_empty() { Vec::new() } else { vec![rect] }, true)
            }
        };
        let mut pixels = laid.map(|l| l.pixels).unwrap_or_default();
        let parts: Vec<(IRect, Vec<u8>)> = rects
            .into_iter()
            .map(|rect| {
                let mut part = raster(&edges, self.outlines.len(), rect);
                self.select(rect, &mut part);
                pixels.put(rect, &part);
                (rect, part)
            })
            .collect();
        Grown { laid: Laid { outlines: self.outlines, pixels }, parts, whole }
    }
}

/// Where the last of `samples` that is a place is.
fn pen_of(samples: &[Sample]) -> Option<Vec2> {
    samples.iter().rev().map(|s| s.pos).find(|p| p.is_finite())
}

/// Sub-rows a row of pixels is sampled at, and the steps a pixel is cut into across: a pixel is
/// covered by the share of its sub-rows' length inside the region ([`FULL`] all of it).
const SUB_ROWS: usize = 16;
const STEPS: i64 = 256;
const FULL: i64 = SUB_ROWS as i64 * STEPS;

/// The rows of a region laid again together where it changes: its parts are this tall.
const BAND: i64 = 32;

/// One edge of an outline, from its top (`x0`, `y0`) to its bottom (`x1`, `y1`), canvas pixels;
/// which way the outline goes along it (1 down, -1 up), and which outline it is of.
#[derive(Clone, Copy, Debug)]
struct Edge {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    dir: i32,
    outline: usize,
}

impl Edge {
    /// Where it crosses the row at `y` (within its height), in steps of a pixel.
    fn steps_at(&self, y: f64) -> i64 {
        let x = self.x0 + (y - self.y0) / (self.y1 - self.y0) * (self.x1 - self.x0);
        (x * STEPS as f64).round().clamp(-1e15, 1e15) as i64
    }
}

/// The outlines' points that are places, each kept within a million pixels of the canvas.
fn places(outline: &[Vec2]) -> Vec<Vec2> {
    let near = |v: f32| v.clamp(-1e6, 1e6);
    outline.iter().filter(|p| p.is_finite()).map(|p| Vec2::new(near(p.x), near(p.y))).collect()
}

/// The edges of `outlines`, each closed back to its start, that cross rows (of those with three
/// places or more).
fn edges_of(outlines: &[Vec<Vec2>]) -> Vec<Edge> {
    let mut edges = Vec::new();
    for (outline, points) in outlines.iter().map(|o| places(o)).enumerate() {
        if points.len() < 3 {
            continue;
        }
        for (a, b) in points.iter().zip(points.iter().skip(1).chain(points.first())) {
            if a.y == b.y {
                continue;
            }
            let (top, bottom, dir) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
            let (x0, y0, x1, y1) = (f64::from(top.x), f64::from(top.y), f64::from(bottom.x), f64::from(bottom.y));
            edges.push(Edge { x0, y0, x1, y1, dir, outline });
        }
    }
    edges
}

/// The box `outlines` (of three places or more) fill at most, with a pixel to spare for the
/// smoothed edge, on a `width` × `height` canvas.
fn bounds_of(outlines: &[Vec<Vec2>], width: u32, height: u32) -> IRect {
    let (mut l, mut t, mut r, mut b) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for p in outlines.iter().map(|o| places(o)).filter(|o| o.len() >= 3).flatten() {
        (l, t, r, b) = (l.min(p.x), t.min(p.y), r.max(p.x), b.max(p.y));
    }
    if !(l.is_finite() && t.is_finite() && r.is_finite() && b.is_finite()) {
        return IRect::EMPTY;
    }
    IRect::new((l - 1.0).floor() as i32, (t - 1.0).floor() as i32, (r + 1.0).ceil() as i32, (b + 1.0).ceil() as i32)
        .intersect(IRect::from_size(width, height))
}

/// How much of each pixel of `rect` (255 all of it, rows from the top) the region `edges` (of
/// `outlines` outlines) fill: inside where any outline winds round it (each by the non-zero rule,
/// all of them together). Each pixel is worked out from the edges alone, in whole numbers, so it
/// comes out the same whatever rectangle it is made in.
fn raster(edges: &[Edge], outlines: usize, rect: IRect) -> Vec<u8> {
    let (w, h) = (rect.width() as usize, rect.height() as usize);
    let mut out = vec![0u8; w * h];
    if w == 0 || h == 0 {
        return out;
    }
    let (top, bottom) = (f64::from(rect.y0), f64::from(rect.y1));
    // the edges these rows meet, the topmost last
    let mut waiting: Vec<&Edge> = edges.iter().filter(|e| e.y1 > top && e.y0 < bottom).collect();
    waiting.sort_by(|a, b| b.y0.total_cmp(&a.y0));
    let mut active: Vec<&Edge> = Vec::new();
    let mut crossings: Vec<(i64, i32, usize)> = Vec::new();
    let mut winding = vec![0i32; outlines.max(1)];
    // each pixel's part covered, and the whole pixels covered as runs (where each starts and ends)
    let (mut part, mut runs) = (vec![0i64; w + 1], vec![0i64; w + 2]);
    let (lo, hi) = (i64::from(rect.x0) * STEPS, i64::from(rect.x1) * STEPS);
    for (row, pixels) in out.chunks_exact_mut(w).enumerate() {
        let y = top + row as f64;
        while let Some(edge) = waiting.pop_if(|e| e.y0 < y + 1.0) {
            active.push(edge);
        }
        active.retain(|e| e.y1 > y);
        if active.is_empty() {
            continue;
        }
        part.fill(0);
        runs.fill(0);
        let mut any = false;
        for sub in 0..SUB_ROWS {
            let at = y + (sub as f64 + 0.5) / SUB_ROWS as f64;
            crossings.clear();
            crossings
                .extend(active.iter().filter(|e| e.y0 <= at && at < e.y1).map(|e| (e.steps_at(at), e.dir, e.outline)));
            if crossings.is_empty() {
                continue;
            }
            crossings.sort_unstable();
            winding.fill(0);
            // how many outlines wind round the place reached, and where it went inside
            let (mut inside, mut from) = (0usize, 0i64);
            for &(x, dir, outline) in &crossings {
                let Some(wound) = winding.get_mut(outline) else { continue };
                let was = *wound != 0;
                *wound += dir;
                match (was, *wound != 0) {
                    (false, true) => {
                        if inside == 0 {
                            from = x;
                        }
                        inside += 1;
                    }
                    (true, false) => {
                        inside = inside.saturating_sub(1);
                        if inside == 0 {
                            any |= add_span(&mut part, &mut runs, from.max(lo) - lo, x.min(hi) - lo);
                        }
                    }
                    _ => {}
                }
            }
        }
        if !any {
            continue;
        }
        let mut whole = 0i64;
        for ((pixel, &p), &r) in pixels.iter_mut().zip(&part).zip(&runs) {
            whole += r;
            *pixel = (((whole + p) * 255 + FULL / 2) / FULL).clamp(0, 255) as u8;
        }
    }
    out
}

/// A span of a sub-row inside the region, from `l` to `r` (steps from the rectangle's left, within
/// it), added to the pixels it covers: in part at its ends, wholly between. Whether it covers any.
fn add_span(part: &mut [i64], runs: &mut [i64], l: i64, r: i64) -> bool {
    if l >= r {
        return false;
    }
    let add = |to: &mut [i64], i: i64, by: i64| {
        if let Some(v) = usize::try_from(i).ok().and_then(|i| to.get_mut(i)) {
            *v += by;
        }
    };
    let (lp, rp) = (l / STEPS, r / STEPS);
    if lp == rp {
        add(part, lp, r - l);
    } else {
        add(part, lp, (lp + 1) * STEPS - l);
        add(runs, lp + 1, STEPS);
        add(runs, rp, -STEPS);
        add(part, rp, r - rp * STEPS);
    }
    true
}

/// Where the region `old` outlines fill may differ from what `new` fill, on a `width` × `height`
/// canvas: rectangles [`BAND`] rows tall. An outline gone on from where it was changes only within
/// the loop its new edges and its old ones they replace make (from where the two last agree, on to
/// where the new one ends, back to the start, and back along the old one): in each band, from the
/// leftmost of that loop's edges there to the rightmost (what it encloses lies between), with a
/// pixel to spare.
fn changed(old: &[Vec<Vec2>], new: &[Vec<Vec2>], width: u32, height: u32) -> Vec<IRect> {
    let canvas = IRect::from_size(width, height);
    if old.len() != new.len() {
        let all = bounds_of(old, width, height).union(bounds_of(new, width, height)).intersect(canvas);
        return if all.is_empty() { Vec::new() } else { vec![all] };
    }
    // each band's spans across (pixels), one for each outline's loop
    let mut bands: BTreeMap<i64, Vec<(f32, f32)>> = BTreeMap::new();
    for (o, n) in old.iter().zip(new) {
        let mut ring = BTreeMap::new();
        for (a, b) in changed_edges(&places(o), &places(n)) {
            band_spans(&mut ring, a, b, height);
        }
        for (band, span) in ring {
            bands.entry(band).or_default().push(span);
        }
    }
    // each rectangle, and how many pixels the bands' own parts taken into it have
    let mut rects: Vec<(IRect, u64)> = Vec::new();
    let area = |r: IRect| u64::from(r.width()) * u64::from(r.height());
    for (band, mut spans) in bands {
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f32, f32)> = Vec::new();
        for (l, r) in spans {
            match merged.last_mut() {
                Some(last) if l <= last.1 + 8.0 => last.1 = last.1.max(r),
                _ => merged.push((l, r)),
            }
        }
        let (y0, y1) = (band * BAND, (band + 1) * BAND);
        for (l, r) in merged {
            let rect = IRect::new(l.floor() as i32 - 1, y0 as i32, r.ceil() as i32 + 2, y1 as i32).intersect(canvas);
            if rect.is_empty() {
                continue;
            }
            // taken into the one over it where that lays a quarter more at most than their bands'
            // own parts (fewer copies a frame)
            let above = rects.iter().rposition(|(a, _)| a.y1 == rect.y0 && a.x0 < rect.x1 && rect.x0 < a.x1);
            match above.and_then(|i| rects.get_mut(i)) {
                Some((a, own)) if area(a.union(rect)) * 4 <= (*own + area(rect)) * 5 => {
                    *a = a.union(rect);
                    *own += area(rect);
                }
                _ => rects.push((rect, area(rect))),
            }
        }
    }
    rects.into_iter().map(|(rect, _)| rect).collect()
}

/// The edges whose region differs between outline `old` and `new` (places, each closed back to
/// its start): those of the loop from where they last agree on along `new`, back to the start,
/// and back along `old`; all of both if they don't begin alike.
fn changed_edges(old: &[Vec2], new: &[Vec2]) -> Vec<(Vec2, Vec2)> {
    let closed = |o: &[Vec2]| -> Vec<(Vec2, Vec2)> {
        if o.len() < 3 {
            return Vec::new();
        }
        o.iter().copied().zip(o.iter().skip(1).chain(o.first()).copied()).collect()
    };
    let agree = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if old.len() < 3 || new.len() < 3 || agree == 0 {
        let mut all = closed(old);
        all.extend(closed(new));
        return all;
    }
    if agree == old.len() && agree == new.len() {
        return Vec::new();
    }
    let mut ring: Vec<Vec2> = new.get(agree - 1..).unwrap_or_default().to_vec();
    ring.extend(new.first());
    ring.extend(old.get(agree - 1..).unwrap_or_default().iter().rev());
    ring.windows(2)
        .filter_map(|p| match p {
            [a, b] if a != b => Some((*a, *b)),
            _ => None,
        })
        .collect()
}

/// Edge `a`–`b`'s span across in each band of rows it meets on a canvas `height` tall, taken into
/// that band's span in `bands`.
fn band_spans(bands: &mut BTreeMap<i64, (f32, f32)>, a: Vec2, b: Vec2, height: u32) {
    let (top, bottom) = if a.y <= b.y { (a, b) } else { (b, a) };
    let (y0, y1) = (top.y.max(0.0), bottom.y.min(height as f32));
    if y0 > y1 {
        return;
    }
    let x_at = |y: f32| {
        if bottom.y > top.y { top.x + (y - top.y) / (bottom.y - top.y) * (bottom.x - top.x) } else { top.x }
    };
    let (first, last) = ((y0 / BAND as f32).floor() as i64, (y1 / BAND as f32).floor() as i64);
    for band in first..=last {
        let from = y0.max((band * BAND) as f32);
        let to = y1.min(((band + 1) * BAND) as f32);
        let (xa, xb) = if bottom.y > top.y { (x_at(from), x_at(to)) } else { (top.x, bottom.x) };
        let span = bands.entry(band).or_insert((f32::INFINITY, f32::NEG_INFINITY));
        *span = (span.0.min(xa.min(xb)), span.1.max(xa.max(xb)));
    }
}

/// A fill's region as laid, by tiles [`TILE`] pixels square (made where it covers anything), and
/// how many of its pixels it covers at all.
#[derive(Default)]
struct Pixels {
    tiles: HashMap<(u32, u32), Box<[u8]>>,
    covered: u64,
}

const TILE: u32 = 256;

impl Pixels {
    /// `coverage` laid over `rect` (on the canvas).
    fn put(&mut self, rect: IRect, coverage: &[u8]) {
        let (Ok(x0), Ok(y0), Ok(x1)) = (u32::try_from(rect.x0), u32::try_from(rect.y0), u32::try_from(rect.x1)) else {
            return;
        };
        let w = rect.width() as usize;
        if w == 0 || coverage.len() != w * rect.height() as usize {
            return;
        }
        for (y, row) in (y0..).zip(coverage.chunks_exact(w)) {
            let mut x = x0;
            while x < x1 {
                let stop = ((x / TILE + 1) * TILE).min(x1);
                let along = row.get((x - x0) as usize..(stop - x0) as usize).unwrap_or_default();
                let key = (x / TILE, y / TILE);
                if self.tiles.contains_key(&key) || along.iter().any(|&c| c > 0) {
                    let tile =
                        self.tiles.entry(key).or_insert_with(|| vec![0; (TILE * TILE) as usize].into_boxed_slice());
                    let at = ((y % TILE) * TILE + x % TILE) as usize;
                    if let Some(to) = tile.get_mut(at..at + along.len()) {
                        for (was, &c) in to.iter_mut().zip(along) {
                            match (*was > 0, c > 0) {
                                (false, true) => self.covered += 1,
                                (true, false) => self.covered = self.covered.saturating_sub(1),
                                _ => {}
                            }
                            *was = c;
                        }
                    }
                }
                x = stop;
            }
        }
    }

    /// How much of pixel (`x`, `y`) is covered.
    #[cfg(test)]
    fn at(&self, x: u32, y: u32) -> u8 {
        let tile = self.tiles.get(&(x / TILE, y / TILE));
        tile.and_then(|t| t.get(((y % TILE) * TILE + x % TILE) as usize)).copied().unwrap_or(0)
    }
}

/// The region `outlines` (canvas pixels) fill on a `width` × `height` canvas, each by the non-zero
/// winding rule and all of them together: the box (a pixel to spare for the smoothed edge, on the
/// canvas) and how much of each pixel there (255 all of it), rows from the top. None if they
/// fill none.
pub(crate) fn coverage(outlines: &[Vec<Vec2>], width: u32, height: u32) -> Option<(IRect, Vec<u8>)> {
    let rect = bounds_of(outlines, width, height);
    if rect.is_empty() {
        return None;
    }
    let data = raster(&edges_of(outlines), outlines.len(), rect);
    data.iter().any(|&c| c > 0).then_some((rect, data))
}

impl State {
    /// Whether a stroke beginning now with `tool` by `pointer` on `layer` fills what it encloses,
    /// as the plugins on the stroke-fill hook say, and how it shows. Never a smudge, nor on the
    /// brush studio's pad, an adjustment painted on, a model or a plugin's own layer.
    pub(crate) fn fill_asked(
        &self,
        tool: ToolKind,
        pointer: PointerKind,
        layer: procraft_engine::document::LayerId,
    ) -> Option<FillShown> {
        let hooks = self.hooks.as_ref()?;
        let number = match tool {
            ToolKind::Paint => 0,
            ToolKind::Erase => 1,
            ToolKind::Smudge => return None,
        };
        if self.pad_strokes.is_some() || self.adjusting || self.plugin_layer_kind(layer).is_some() {
            return None;
        }
        let pointer = crate::pressure::pointer_number(pointer);
        FillShown::of(hooks.stroke_fill(number, pointer, |keys| self.fill_brush(tool, keys)))
    }

    /// The brush `tool` draws with, as the stroke-fill hook is told it: its name, its set's and the
    /// values of the rows `keys`.
    fn fill_brush(&self, tool: ToolKind, keys: &[String]) -> crate::stroke_sound::SoundBrush {
        let (set, index) = self.brush_ref(tool);
        let set = self.library.get(set);
        let name = set.and_then(|s| s.brushes.get(index)).map_or("", |b| b.name.as_str()).to_owned();
        let set = set.map_or("", |s| s.name.as_str()).to_owned();
        let brush = self.brush(tool);
        let controls: Vec<f32> = keys.iter().map(|k| brush.setting(k)).collect();
        let mut hash = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(&(&name, &set, controls.iter().map(|v| v.to_bits()).collect::<Vec<_>>()), &mut hash);
        crate::stroke_sound::SoundBrush { name, set, controls, key: std::hash::Hasher::finish(&hash) }
    }

    /// Starts filling with the stroke `builder` begins, shown as `shown` says, from its first
    /// `samples` (canvas pixels).
    pub(crate) fn begin_fill(&mut self, shown: FillShown, builder: &StrokeBuilder, samples: &[Sample]) {
        let mut path = builder.fill_path();
        path.add(samples);
        let selection = self.selection.as_ref().filter(|m| !m.is_empty()).map(|m| Arc::new(m.clone()));
        let pen = pen_of(samples);
        self.filling =
            Some(Box::new(Filling { shown, path, pen, shown_at: None, making: None, laid: None, selection }));
    }

    /// More of the fill stroke's samples (canvas pixels): its path goes on. Whether its own stamps
    /// are left out (it shows as its region grows; the next frame fills that).
    pub(crate) fn fill_add(&mut self, samples: &[Sample]) -> bool {
        let Some(filling) = self.filling.as_mut() else { return false };
        filling.path.add(samples);
        filling.pen = pen_of(samples).or(filling.pen);
        filling.shown == FillShown::Growing
    }

    /// The region the fill stroke encloses so far shown, if its path went on since it was last
    /// shown (once a frame, however many samples came): made again where it changed on a thread of
    /// its own, one change at a time, and laid by the first frame after it is made (frames go on
    /// being asked for meanwhile); the path as it went on meanwhile is made next.
    pub(crate) fn show_fill(&mut self) {
        let Some(filling) = self.filling.as_mut().filter(|f| f.shown == FillShown::Growing) else { return };
        if let Some((made_of, made)) = filling.making.take() {
            match made.try_recv() {
                Ok(grown) => {
                    filling.shown_at = made_of;
                    self.lay_grown(grown);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    filling.making = Some((made_of, made));
                    self.needs_frame = true;
                    return;
                }
                // (lost: made whole again next)
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
        let Some(filling) = self.filling.as_ref() else { return };
        let mut path = filling.path.path();
        let now = Some((path.len(), filling.pen));
        if filling.shown_at == now {
            return;
        }
        // on to where the pen is, as the path will be once it lifts
        if let Some(pen) = filling.pen.filter(|p| path.last() != Some(p)) {
            path.push(pen);
        }
        let of = self.region_of(&path, filling.selection.clone());
        let Some(filling) = self.filling.as_mut() else { return };
        // (taken back if no thread can be made)
        let job = Arc::new(parking_lot::Mutex::new(Some((of, filling.laid.take()))));
        let taken = Arc::clone(&job);
        let (sent, made) = mpsc::channel();
        let making = std::thread::Builder::new().name("procraft-fill-region".into()).spawn(move || {
            if let Some((of, laid)) = taken.lock().take() {
                // (the stroke may have ended meanwhile: then nothing waits for it)
                let _ = sent.send(of.grow(laid));
            }
        });
        match making {
            Ok(_) => {
                filling.making = Some((now, made));
                self.needs_frame = true;
            }
            // no thread to make it on: made here
            Err(e) => {
                log::warn!("a fill's region made on the frames' thread: {e}");
                filling.shown_at = now;
                let job = job.lock().take();
                if let Some((of, laid)) = job {
                    self.lay_grown(of.grow(laid));
                }
            }
        }
    }

    /// `grown` laid as the open fill stroke's region: all of it where none was laid before (the
    /// stroke's paint cleared first), else only where it changed.
    fn lay_grown(&mut self, grown: Grown) {
        let Grown { laid, parts, whole } = grown;
        if let Some(canvas) = self.canvas.as_mut() {
            if whole {
                let (rect, coverage) = parts.into_iter().next().unwrap_or((IRect::EMPTY, Vec::new()));
                canvas.fill_stroke(rect, coverage);
            } else {
                canvas.fill_stroke_parts(parts);
            }
        }
        if let Some(filling) = self.filling.as_mut() {
            filling.laid = Some(laid);
        }
    }

    /// Whether the open fill stroke's region is still being made (see [`Self::show_fill`]).
    #[cfg(test)]
    pub(crate) fn fill_being_made(&self) -> bool {
        self.filling.as_ref().is_some_and(|f| f.making.is_some())
    }

    /// Ends the fill stroke with its last `samples` (canvas pixels): its region laid as one step,
    /// or nothing if it encloses nothing. False if no fill stroke is open. Shown as it grew, only
    /// what changed since it was last laid is made and laid now (once the change being made then
    /// is laid); else all of it.
    pub(crate) fn end_fill(&mut self, samples: &[Sample]) -> bool {
        let Some(mut filling) = self.filling.take() else { return false };
        filling.path.add(samples);
        filling.path.finish();
        self.stroke = None;
        self.second = None;
        let of = self.region_of(&filling.path.path(), filling.selection.clone());
        let mut laid = filling.laid.take();
        if let Some((_, made)) = filling.making.take() {
            laid = made.recv().ok().and_then(|grown| {
                let Grown { laid, parts, whole } = grown;
                let canvas = self.canvas.as_mut()?;
                if whole {
                    let (rect, coverage) = parts.into_iter().next().unwrap_or((IRect::EMPTY, Vec::new()));
                    canvas.fill_stroke(rect, coverage);
                } else {
                    canvas.fill_stroke_parts(parts);
                }
                Some(laid)
            });
        }
        let Some(canvas) = self.canvas.as_mut() else { return true };
        let covers = match laid {
            Some(laid) => {
                let grown = of.grow(Some(laid));
                canvas.fill_stroke_parts(grown.parts);
                grown.laid.pixels.covered > 0
            }
            None => match of.region() {
                Some((rect, coverage)) => {
                    canvas.fill_stroke(rect, coverage);
                    true
                }
                None => false,
            },
        };
        if covers {
            canvas.name_next_step(STEP);
            canvas.end_stroke();
        } else {
            canvas.cancel_stroke();
        }
        true
    }

    /// The region `path` (canvas pixels) and each copy the symmetry makes of it enclose, inside
    /// the selection (see [`coverage`]).
    #[cfg(test)]
    fn fill_region(&self, path: &[Vec2]) -> Region {
        let selection = self.selection.as_ref().filter(|m| !m.is_empty()).map(|m| Arc::new(m.clone()));
        self.region_of(path, selection).region()
    }

    /// What the region `path` (canvas pixels) and each copy the symmetry makes of it enclose,
    /// inside `selection`, is made from.
    fn region_of(&self, path: &[Vec2], selection: Option<Arc<Mask>>) -> RegionOf {
        let (width, height) = (self.doc.width(), self.doc.height());
        if path.len() < 3 {
            return RegionOf { outlines: Vec::new(), width, height, selection };
        }
        let guide = self.doc.stroke_guide();
        let copied: Vec<Vec<(Vec2, f32, bool)>> = path.iter().map(|p| guide.copies(*p)).collect();
        let count = copied.iter().map(Vec::len).min().unwrap_or(0);
        let mut outlines = vec![path.to_vec()];
        for k in 0..count {
            outlines.push(copied.iter().filter_map(|c| c.get(k).map(|(p, _, _)| *p)).collect());
        }
        RegionOf { outlines, width, height, selection }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A circle's outline about (`x`, `y`), `r` across, `n` points, once round (`turns` 1) or more.
    pub(crate) fn circle(x: f32, y: f32, r: f32, n: usize, turns: usize) -> Vec<Vec2> {
        (0..n * turns)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec2::new(x + r * a.cos(), y + r * a.sin())
            })
            .collect()
    }

    #[test]
    fn a_loop_drawn_round_twice_fills_once_as_a_loop_drawn_once() {
        let once = coverage(&[circle(50.0, 50.0, 30.0, 90, 1)], 100, 100).unwrap();
        let twice = coverage(&[circle(50.0, 50.0, 30.0, 90, 2)], 100, 100).unwrap();
        assert_eq!(once, twice);
        // solid inside, nothing well outside, smoothed at the rim
        let (rect, data) = &once;
        let at = |x: i32, y: i32| data[((y - rect.y0) * rect.width() as i32 + (x - rect.x0)) as usize];
        assert_eq!(at(50, 50), 255);
        assert_eq!(at(50, 22), 255);
        assert_eq!(at(21, 21), 0);
        let smoothed = data.iter().filter(|&&c| (1..255).contains(&c)).count();
        assert!(smoothed > 60, "a smoothed rim: {smoothed} pixels");
    }

    #[test]
    fn both_lobes_of_a_figure_eight_fill() {
        // round the left lobe one way, the right lobe the other
        let mut eight = Vec::new();
        for i in 0..120 {
            let a = i as f32 / 120.0 * std::f32::consts::TAU;
            eight.push(Vec2::new(50.0 + 40.0 * a.sin(), 50.0 + 20.0 * (2.0 * a).sin()));
        }
        let (rect, data) = coverage(&[eight], 100, 100).unwrap();
        let at = |x: i32, y: i32| data[((y - rect.y0) * rect.width() as i32 + (x - rect.x0)) as usize];
        assert_eq!(at(30, 50 - 8), 255, "one lobe");
        assert_eq!(at(70, 50 + 8), 255, "the other");
        // even-odd would fill them too; a loop inside a loop the same way round tells them apart
        let mut nested = circle(50.0, 50.0, 40.0, 120, 1);
        nested.extend(circle(50.0, 50.0, 15.0, 60, 1));
        let (rect, data) = coverage(&[nested], 100, 100).unwrap();
        let at = |x: i32, y: i32| data[((y - rect.y0) * rect.width() as i32 + (x - rect.x0)) as usize];
        assert_eq!(at(50, 50), 255, "wound twice there: filled, not a hole");
    }

    #[test]
    fn the_path_is_closed_by_a_straight_edge_back_to_its_start() {
        // three sides of a square: the fourth closes it
        let u = vec![Vec2::new(10.0, 10.0), Vec2::new(10.0, 90.0), Vec2::new(90.0, 90.0), Vec2::new(90.0, 10.0)];
        let (rect, data) = coverage(&[u], 100, 100).unwrap();
        assert_eq!(rect, IRect::new(9, 9, 91, 91));
        let at = |x: i32, y: i32| data[((y - rect.y0) * rect.width() as i32 + (x - rect.x0)) as usize];
        assert_eq!((at(50, 11), at(50, 50), at(50, 89)), (255, 255, 255));
        assert_eq!(at(50, 9), 0, "nothing past the closing edge");
    }

    #[test]
    fn mirrored_copies_fill_where_they_overlap_and_nothing_fills_off_the_canvas() {
        let right = circle(60.0, 50.0, 20.0, 60, 1);
        let mirrored: Vec<Vec2> = right.iter().map(|p| Vec2::new(100.0 - p.x, p.y)).collect();
        let (rect, data) = coverage(&[right, mirrored], 100, 100).unwrap();
        let at = |x: i32, y: i32| data[((y - rect.y0) * rect.width() as i32 + (x - rect.x0)) as usize];
        assert_eq!(at(50, 50), 255, "where both cover it");
        assert_eq!(coverage(&[circle(-100.0, -100.0, 20.0, 60, 1)], 100, 100), None);
        assert_eq!(coverage(&[vec![Vec2::new(1.0, 1.0), Vec2::new(5.0, 5.0)]], 100, 100), None);
        assert_eq!(coverage(&[vec![Vec2::new(f32::NAN, 1.0); 5]], 100, 100), None);
    }

    use std::sync::Arc;

    use crate::CanvasSession;
    use crate::hooks::Hooks;
    use crate::plugin_run::{Hook, Running};
    use crate::tests::{frame_of, no_streamline, pixel, sample, session};

    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const RED: [u8; 4] = [255, 0, 0, 255];

    /// A plugin on the stroke-fill hook answering `answer` (a WAT body, given `tool` and
    /// `pointer` as locals 0 and 1) put on `s`.
    fn filling(s: &CanvasSession, answer: &str) -> Arc<Hooks> {
        let hooks = Arc::new(Hooks::default());
        let text = format!(r#"(module (func (export "procraft_stroke_fill") (param i32 i32) (result i32) {answer}))"#);
        let running = Running::start(&wat::parse_str(text).unwrap(), &[]).unwrap();
        hooks.put_on("org.example.fill".to_owned(), running, &[Hook::StrokeFill]);
        s.shared.lock().hooks = Some(Arc::clone(&hooks));
        hooks
    }

    /// A pen stroke through `points` (window pixels, which are canvas pixels here), a sample each.
    fn draw(s: &CanvasSession, points: &[Vec2]) {
        let at = |i: usize, p: &Vec2| sample(p.x, p.y, i as f64 * 8.0);
        let Some(first) = points.first() else { return };
        s.stroke_begin(PointerKind::Stylus, vec![at(0, first)]);
        for (i, p) in points.iter().enumerate().skip(1) {
            s.stroke_add(vec![at(i, p)]);
        }
        s.stroke_end(vec![]);
    }

    /// A red session 100 pixels square, its brush drawing just where the pen goes.
    fn red() -> Option<Arc<CanvasSession>> {
        let s = session(100, 100)?;
        no_streamline(&s);
        s.set_color(1.0, 0.0, 0.0);
        Some(s)
    }

    /// Every pixel of `s`'s composite, as `pixel` reads it, a few apart.
    fn picture(s: &CanvasSession) -> Vec<[u8; 4]> {
        (0..100)
            .step_by(3)
            .flat_map(|y| (0..100).step_by(3).map(move |x| (x, y)))
            .map(|(x, y)| pixel(s, x, y))
            .collect()
    }

    fn square_u() -> Vec<Vec2> {
        // three sides of a square, drawn: the fourth closes it
        let mut u = Vec::new();
        for i in 0..=30 {
            u.push(Vec2::new(20.0, 20.0 + 2.0 * i as f32));
        }
        for i in 1..=30 {
            u.push(Vec2::new(20.0 + 2.0 * i as f32, 80.0));
        }
        for i in 1..=30 {
            u.push(Vec2::new(80.0, 80.0 - 2.0 * i as f32));
        }
        u
    }

    #[test]
    fn a_stroke_a_plugin_says_fills_lays_what_it_encloses_as_one_step_at_the_lift() {
        let Some(s) = red() else { return };
        filling(&s, "(i32.const 2)");
        draw(&s, &square_u());
        // inside, filled; the closing edge's side too; outside, nothing
        assert_eq!(pixel(&s, 50, 50), RED);
        assert_eq!(pixel(&s, 50, 22), RED);
        assert_eq!(pixel(&s, 50, 10), WHITE);
        assert_eq!(pixel(&s, 90, 50), WHITE);
        // one step, named so
        assert_eq!(s.undo().as_deref(), Some(STEP));
        assert_eq!(pixel(&s, 50, 50), WHITE);
        assert!(!s.shared.lock().canvas.as_ref().unwrap().can_undo(), "nothing more to undo");
    }

    #[test]
    fn a_loop_drawn_twice_lays_what_a_loop_drawn_once_does_and_a_figure_eight_both_lobes() {
        let (Some(once), Some(twice), Some(eight)) = (red(), red(), red()) else { return };
        for s in [&once, &twice, &eight] {
            filling(s, "(i32.const 2)");
        }
        // half as opaque, so that a second coat would show
        for s in [&once, &twice] {
            s.set_brush_opacity(0.5);
        }
        draw(&once, &circle(50.0, 50.0, 30.0, 90, 1));
        draw(&twice, &circle(50.0, 50.0, 30.0, 90, 2));
        let half = pixel(&once, 50, 50);
        assert!(half[0] == 255 && (126..=129).contains(&half[1]), "{half:?}");
        // inside, one coat, no hole
        for (x, y) in [(50, 50), (30, 50), (70, 50), (50, 30), (50, 70), (36, 36)] {
            assert_eq!(pixel(&twice, x, y), pixel(&once, x, y), "no darker, no hole at {x}, {y}");
        }
        // the rim as near as the path's points lie round it the second time
        let most = picture(&once)
            .iter()
            .zip(picture(&twice))
            .map(|(a, b)| a.iter().zip(b).map(|(x, y)| x.abs_diff(y)).max().unwrap_or(0))
            .max()
            .unwrap_or(0);
        println!("a loop drawn twice: its rim within {most} levels of one drawn once");
        assert!(most <= 64, "{most}");
        let lobes: Vec<Vec2> = (0..=120)
            .map(|i| i as f32 / 120.0 * std::f32::consts::TAU)
            .map(|a| Vec2::new(50.0 + 40.0 * a.sin(), 50.0 + 20.0 * (2.0 * a).sin()))
            .collect();
        draw(&eight, &lobes);
        assert_eq!((pixel(&eight, 30, 42), pixel(&eight, 70, 58)), (RED, RED));
    }

    /// The pixels [`picture`] reads.
    type Picture = Vec<[u8; 4]>;

    /// What two fills of the same square lay, one undone before the other, the brush's grain a
    /// grunge fixed to the canvas, started somewhere new each stroke (`jitter`) or not.
    fn two_grained_fills(jitter: bool) -> Option<(Picture, Picture)> {
        use procraft_engine::brush::{Grain, GrainMode};
        let s = red()?;
        filling(&s, "(i32.const 2)");
        {
            let mut st = s.shared.lock();
            let tool = st.tool;
            let brush = st.brush_mut(tool)?;
            let grunge = Some(procraft_engine::textures::grain::GRUNGE);
            brush.grain = Grain { image: grunge, source: None, mode: GrainMode::Texturized, scale: 64.0, depth: 1.0 };
            brush.set_setting("grain_offset_jitter", if jitter { 1.0 } else { 0.0 });
        }
        draw(&s, &square_u());
        let first = picture(&s);
        s.undo();
        draw(&s, &square_u());
        Some((first, picture(&s)))
    }

    #[test]
    fn a_fill_whose_brush_starts_its_grain_somewhere_new_each_stroke_does() {
        let Some((first, again)) = two_grained_fills(true) else { return };
        let Some((plain, plain_again)) = two_grained_fills(false) else { return };
        // with Offset jitter each fill's grain starts somewhere new; without, where the last did
        assert!(first != again, "the grain moved between fills");
        assert!(plain == plain_again);
        assert!(first != plain || again != plain);
        // the places a stroke's seed gives spread evenly over the tile
        let starts: Vec<[f32; 2]> = (0..1000u32).map(|i| crate::grain_start(i.wrapping_mul(0x9e37_79b9))).collect();
        assert!(starts.iter().flatten().all(|t| (0.0..1.0).contains(t)));
        let mean = |k: usize| starts.iter().map(|s| s[k]).sum::<f32>() / starts.len() as f32;
        assert!((mean(0) - 0.5).abs() < 0.05 && (mean(1) - 0.5).abs() < 0.05, "{} {}", mean(0), mean(1));
    }

    #[test]
    fn a_fill_is_as_opaque_as_the_brush_and_slider_and_keeps_inside_the_selection() {
        let Some(s) = red() else { return };
        filling(&s, "(i32.const 2)");
        s.set_brush_opacity(0.5);
        draw(&s, &square_u());
        let [r, g, b, a] = pixel(&s, 50, 50);
        assert_eq!((r, a), (255, 255));
        assert!((126..=129).contains(&g) && g == b, "half red over white: {g}");
        let Some(s) = red() else { return };
        filling(&s, "(i32.const 2)");
        s.select_shape(false, 10.0, 10.0, 40.0, 40.0, crate::selection::SelectionJoin::Add);
        draw(&s, &square_u());
        assert_eq!(pixel(&s, 30, 30), RED);
        assert_eq!(pixel(&s, 60, 60), WHITE, "outside the selection");
    }

    #[test]
    fn a_fill_keeps_a_locked_layer_s_clear_pixels_clear_and_the_eraser_takes_away_what_it_encloses() {
        let Some(s) = red() else { return };
        // a red bar, then the layer locked, then blue filled over more than it
        draw(&s, &[Vec2::new(10.0, 50.0), Vec2::new(90.0, 50.0)]);
        let layer = s.shared.lock().doc.current().0;
        assert!(s.set_layer_alpha_lock(layer, true));
        s.set_color(0.0, 0.0, 1.0);
        filling(&s, "(i32.const 2)");
        draw(&s, &square_u());
        assert_eq!(pixel(&s, 50, 50), [0, 0, 255, 255], "the paint there recoloured");
        assert_eq!(pixel(&s, 50, 30), WHITE, "a clear pixel stays clear");
        // the eraser (tool 1) fills only when the plugin says, here always
        assert!(s.set_layer_alpha_lock(layer, false));
        filling(&s, "(i32.const 1)");
        s.set_tool(ToolKind::Erase);
        draw(&s, &square_u());
        assert_eq!(pixel(&s, 50, 50), WHITE, "taken away");
        assert_eq!(pixel(&s, 15, 50), RED, "outside it the bar stays as it was");
    }

    #[test]
    fn a_plugin_that_says_no_leaves_strokes_as_they_were_and_a_smudge_is_never_asked() {
        let (Some(plain), Some(asked)) = (red(), red()) else { return };
        // says fill only for tool 2, which is never asked: a smudge
        filling(&asked, "(select (i32.const 1) (i32.const 0) (i32.eq (local.get 0) (i32.const 2)))");
        for s in [&plain, &asked] {
            draw(s, &square_u());
        }
        assert_eq!(pixel(&plain, 50, 50), WHITE);
        assert!(picture(&plain) == picture(&asked));
    }

    #[test]
    fn a_fill_shown_as_it_grows_shows_what_it_lays() {
        let (Some(shown), Some(laid)) = (red(), red()) else { return };
        filling(&shown, "(i32.const 1)");
        filling(&laid, "(i32.const 2)");
        // drawn but not lifted: the frame shows the region so far, closed back to the start
        let u = square_u();
        shown.stroke_begin(PointerKind::Stylus, vec![sample(u[0].x, u[0].y, 0.0)]);
        for (i, p) in u.iter().enumerate().skip(1) {
            shown.stroke_add(vec![sample(p.x, p.y, i as f64 * 8.0)]);
        }
        let mut growing = frame_of(&shown, 100);
        let made_by = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while shown.shared.lock().fill_being_made() && std::time::Instant::now() < made_by {
            growing = frame_of(&shown, 100);
        }
        shown.stroke_end(vec![]);
        let ended = frame_of(&shown, 100);
        draw(&laid, &u);
        // as it grew, as it was laid: shown on to where the pen was, as the lift's catching up lays
        // it, but for the path's own points along that last stretch (a few rim pixels)
        let (grew, laid_px) = (growing.as_chunks::<4>().0, ended.as_chunks::<4>().0);
        let differ = grew.iter().zip(laid_px).filter(|(a, b)| a != b).count();
        let most = grew
            .iter()
            .zip(laid_px)
            .map(|(a, b)| a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0))
            .max()
            .unwrap_or(0);
        println!("a fill as it grew and as it was laid: {differ} of 10,000 pixels differ, by {most} levels at most");
        assert!(differ <= 60, "{differ}");
        assert!(ended == frame_of(&laid, 100), "as a fill shown plainly lays it");
        let at = |x: usize, y: usize| ended[(y * 100 + x) * 4..][..4].to_vec();
        assert_eq!(at(50, 50), RED.to_vec());
    }

    /// A big fill shown as it grows (a loop 1800 pixels across on a canvas 2048 square): its region,
    /// which takes tens of milliseconds to make (more in a debug build), is made on a thread of its
    /// own, so the frames, the desktop's on its window's thread, never wait for it: the frame's part
    /// in it holds the calling thread a few milliseconds at most. Once made it shows, as it lays.
    /// Prints both times.
    #[test]
    fn a_big_fill_s_region_is_made_beside_the_frames_as_it_grows() {
        use std::time::{Duration, Instant};
        let Some(s) = session(2048, 2048) else { return };
        no_streamline(&s);
        s.set_color(1.0, 0.0, 0.0);
        filling(&s, "(i32.const 1)");
        let rim = circle(1024.0, 1024.0, 900.0, 240, 1);
        s.stroke_begin(PointerKind::Stylus, vec![sample(rim[0].x, rim[0].y, 0.0)]);
        // a frame's part (the region shown) every four samples, as frames come while it is drawn
        let (mut longest, mut frames) = (Duration::ZERO, 0);
        for (i, p) in rim.iter().enumerate().skip(1) {
            s.stroke_add(vec![sample(p.x, p.y, i as f64 * 8.0)]);
            if i % 4 == 0 {
                let t = Instant::now();
                s.shared.lock().show_fill();
                longest = longest.max(t.elapsed());
                frames += 1;
            }
        }
        // what making the region of the whole loop takes, here
        let making = {
            let st = s.shared.lock();
            let t = Instant::now();
            assert!(st.fill_region(&rim).is_some());
            t.elapsed()
        };
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        println!(
            "a fill 1800 px across growing: the frames' part {:.2} ms at most over {frames}; making its region {:.1} ms",
            ms(longest),
            ms(making)
        );
        // made, it shows: the middle red before the lift, and after it
        let middle = |frame: &[u8]| frame[(1024 * 2048 + 1024) * 4..][..4].to_vec();
        let (made_by, mut drawn, mut frame) = (Instant::now() + Duration::from_secs(20), 0, Vec::new());
        while (drawn < 2 || s.shared.lock().fill_being_made()) && Instant::now() < made_by {
            frame = frame_of(&s, 2048);
            drawn += 1;
        }
        assert_eq!(middle(&frame), RED.to_vec(), "shown as it grows");
        s.stroke_end(vec![]);
        assert_eq!(middle(&frame_of(&s, 2048)), RED.to_vec(), "laid");
        assert!(longest < Duration::from_millis(8), "a frame waited {:.1} ms for the region", ms(longest));
        assert!(longest * 4 < making, "the frames' part {:.2} ms against {:.1} ms", ms(longest), ms(making));
    }

    /// What filling a region costs, by its size: run with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn fill_cost() {
        for side in [256.0_f32, 1024.0, 2048.0, 4096.0] {
            let loop_ = circle(side / 2.0, side / 2.0, side * 0.45, (side * 3.0) as usize, 1);
            let start = std::time::Instant::now();
            let runs = 5;
            for _ in 0..runs {
                std::hint::black_box(coverage(
                    std::hint::black_box(std::slice::from_ref(&loop_)),
                    side as u32,
                    side as u32,
                ));
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
            println!("a fill {side} px across, {} points: {ms:.2} ms", loop_.len());
        }
    }

    /// The region as the fill made it before #411 (with tiny-skia's rasterizer, each outline by its
    /// own winding laid over the others), to tell the new one by: as smooth, and as costly.
    fn reference_coverage(outlines: &[Vec<Vec2>], width: u32, height: u32) -> Option<(IRect, Vec<u8>)> {
        let rect = bounds_of(outlines, width, height);
        let mut mask = tiny_skia::Mask::new(rect.width(), rect.height())?;
        let (ox, oy) = (rect.x0 as f32, rect.y0 as f32);
        for outline in outlines.iter().map(|o| places(o)).filter(|o| o.len() >= 3) {
            let mut path = tiny_skia::PathBuilder::new();
            path.move_to(outline[0].x - ox, outline[0].y - oy);
            for p in &outline[1..] {
                path.line_to(p.x - ox, p.y - oy);
            }
            path.close();
            if let Some(path) = path.finish() {
                mask.fill_path(&path, tiny_skia::FillRule::Winding, true, tiny_skia::Transform::identity());
            }
        }
        let data = mask.data().to_vec();
        data.iter().any(|&c| c > 0).then_some((rect, data))
    }

    /// A five-pointed star about (`x`, `y`), `r` across, drawn as one line (crossing itself).
    fn star(x: f32, y: f32, r: f32) -> Vec<Vec2> {
        (0..5)
            .map(|i| {
                let a = (i * 2) as f32 / 5.0 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                Vec2::new(x + r * a.cos(), y + r * a.sin())
            })
            .collect()
    }

    /// `outline` mirrored across x = `at` (it winds the other way).
    fn mirrored(outline: &[Vec2], at: f32) -> Vec<Vec2> {
        outline.iter().map(|p| Vec2::new(2.0 * at - p.x, p.y)).collect()
    }

    /// #411: a region's pixels come out the same made all at once or a rectangle at a time (odd
    /// sizes, crossing every edge), so a growing fill laid where it changed is the fill laid whole.
    #[test]
    fn a_region_s_pixels_are_the_same_made_whole_or_a_part_at_a_time() {
        let mut wobbly = circle(140.0, 100.0, 70.0, 97, 2);
        for (i, p) in wobbly.iter_mut().enumerate() {
            p.x += (i as f32 * 0.7).sin() * 9.3;
            p.y += (i as f32 * 1.3).cos() * 5.1;
        }
        let outlines = vec![
            wobbly.clone(),
            mirrored(&wobbly, 150.0),
            star(80.0, 60.0, 75.0),
            circle(250.0, 30.0, 60.0, 33, 1),
            vec![Vec2::new(10.5, 190.25), Vec2::new(290.75, 170.5), Vec2::new(150.125, 120.0)],
        ];
        let (w, h) = (300u32, 200u32);
        let edges = edges_of(&outlines);
        let whole = raster(&edges, outlines.len(), IRect::from_size(w, h));
        assert!(whole.contains(&255) && whole.iter().filter(|&&c| (1..255).contains(&c)).count() > 500);
        let mut compared = 0;
        for (sx, sy) in [(37, 23), (1, 200), (300, 1), (64, 32), (13, 7)] {
            for y0 in (0..h as i32).step_by(sy) {
                for x0 in (0..w as i32).step_by(sx) {
                    let rect = IRect::new(x0, y0, (x0 + sx as i32).min(w as i32), (y0 + sy as i32).min(h as i32));
                    let part = raster(&edges, outlines.len(), rect);
                    for y in rect.y0..rect.y1 {
                        for x in rect.x0..rect.x1 {
                            let got = part[((y - rect.y0) * rect.width() as i32 + (x - rect.x0)) as usize];
                            assert_eq!(got, whole[(y * w as i32 + x) as usize], "({x}, {y}) in {rect:?}");
                            compared += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(compared, 5 * w * h);
    }

    /// #411: as a fill grows, each step lays again only where it changed, and what it then has
    /// laid is the region made whole, at every step: going on, the pen ahead of the path, going back
    /// on itself, with a mirrored copy, inside a selection. Printed: the share of the region's box
    /// each step laid again. Bound: the same pixels every step; on average under a third of its box
    /// laid again once it is big (a small loop: its bands of rows are a fifth of it tall).
    #[test]
    fn a_fill_laid_where_it_changed_is_the_fill_made_whole_at_every_step() {
        let (w, h) = (240u32, 180u32);
        let mut path = circle(120.0, 90.0, 80.0, 150, 1);
        for (i, p) in path.iter_mut().enumerate() {
            p.y += (i as f32 * 0.21).sin() * 12.0;
        }
        // near its end it doubles back for a while
        let back: Vec<Vec2> = path[110..130].iter().rev().copied().collect();
        path.extend(back);
        let mut mask = Mask { width: w, height: h, data: vec![0; (w * h) as usize] };
        for y in 0..h {
            for x in 20..w - 30 {
                mask.data[(y * w + x) as usize] = if x < 40 { 128 } else { 255 };
            }
        }
        let selection = Some(Arc::new(mask));
        let (mut laid, mut shares) = (None::<Laid>, Vec::new());
        // two points a step, as frames keep up; then many (frames behind the region's making)
        let steps = (3..120).step_by(2).chain((120..path.len()).step_by(13)).chain([path.len()]);
        for n in steps {
            let mut shown = path[..n].to_vec();
            let next = path.get(n).copied().unwrap_or(path[n - 1]);
            shown.push(Vec2::new(next.x + 3.0, next.y - 2.0));
            let outlines = vec![shown.clone(), mirrored(&shown, 120.0)];
            let of = RegionOf { outlines: outlines.clone(), width: w, height: h, selection: selection.clone() };
            let whole = of.region();
            let grown = RegionOf { outlines, width: w, height: h, selection: selection.clone() }.grow(laid.take());
            let pixels = &grown.laid.pixels;
            let at = |x: u32, y: u32| match &whole {
                Some((r, c)) if (r.x0..r.x1).contains(&(x as i32)) && (r.y0..r.y1).contains(&(y as i32)) => {
                    c[((y as i32 - r.y0) * r.width() as i32 + (x as i32 - r.x0)) as usize]
                }
                _ => 0,
            };
            for y in 0..h {
                for x in 0..w {
                    assert_eq!(pixels.at(x, y), at(x, y), "step {n}: ({x}, {y})");
                }
            }
            assert_eq!(pixels.covered > 0, whole.is_some(), "step {n}");
            if let (Some((r, _)), false) = (&whole, grown.whole) {
                let area: i64 = grown.parts.iter().map(|(p, _)| i64::from(p.width()) * i64::from(p.height())).sum();
                if r.width() > 100 && r.height() > 100 {
                    shares.push(area as f64 / (f64::from(r.width()) * f64::from(r.height())));
                }
            }
            laid = Some(grown.laid);
        }
        let mean = shares.iter().sum::<f64>() / shares.len().max(1) as f64;
        let most = shares.iter().copied().fold(0.0, f64::max);
        println!(
            "a fill growing: each step laid again {:.1}% of its box on average, {:.1}% at most",
            mean * 100.0,
            most * 100.0
        );
        assert!(shares.len() > 20 && mean < 0.34, "{mean} over {}", shares.len());
    }

    /// #411: the region made a part at a time is smoothed as the fill smoothed it before, near
    /// enough not to tell: solid and empty where it was, its rims within a few levels on average.
    /// Printed: how far it differs from it. Bound: under 1.5 levels on average, under 80 at most
    /// (a nearly level edge, now sampled 16 times a row where it was 4).
    #[test]
    fn a_fill_is_smoothed_as_it_was() {
        let mut eight = Vec::new();
        for i in 0..120 {
            let a = i as f32 / 120.0 * std::f32::consts::TAU;
            eight.push(Vec2::new(100.0 + 80.0 * a.sin(), 100.0 + 40.0 * (2.0 * a).sin()));
        }
        let ring = circle(110.0, 100.0, 50.0, 90, 1);
        for outlines in [
            vec![circle(100.0, 100.0, 70.3, 240, 1)],
            vec![star(100.0, 105.0, 90.0)],
            vec![eight],
            vec![ring.clone(), mirrored(&ring, 100.0)],
        ] {
            let (rect, ours) = coverage(&outlines, 200, 200).unwrap();
            let (was_rect, was) = reference_coverage(&outlines, 200, 200).unwrap();
            assert_eq!(rect, was_rect);
            let diffs: Vec<u8> = ours.iter().zip(&was).map(|(a, b)| a.abs_diff(*b)).collect();
            let mean = diffs.iter().map(|&d| f64::from(d)).sum::<f64>() / diffs.len() as f64;
            let most = diffs.iter().copied().max().unwrap_or(0);
            let flipped =
                ours.iter().zip(&was).filter(|(a, b)| (**a == 0 && **b == 255) || (**a == 255 && **b == 0)).count();
            println!("against the fill as it was: {mean:.2} levels on average, {most} at most");
            assert_eq!(flipped, 0);
            assert!(mean < 1.5 && most < 80, "{mean} {most}");
        }
    }

    /// A session on a headless GPU, its canvas `width` × `height` fitted to a 1600 square window;
    /// none without a GPU.
    fn sized(width: u32, height: u32) -> Option<Arc<CanvasSession>> {
        let gpu = match procraft_gpu::Gpu::headless() {
            Ok(gpu) => gpu,
            Err(e) => {
                eprintln!("skipped: {e}");
                return None;
            }
        };
        let s = CanvasSession::new(width, height);
        {
            let mut st = s.shared.lock();
            st.window_size = (1600, 1600);
            st.view = Some(procraft_engine::view::View::fit((width, height), (1600, 1600)));
            st.gpu = Some(gpu);
            st.ensure_canvas();
        }
        Some(s)
    }

    /// What a growing fill's frames cost (milliseconds).
    struct GrowCost {
        /// The frames' part, each: the region laid, drawn and done on the GPU.
        frames: Vec<f64>,
        /// The region made (on its own thread), each time.
        making: Vec<f64>,
        /// The lift: the fill laid (and, now, kept as a step), drawn and done; and the pixels of the
        /// region it laid.
        lift: f64,
        lift_pixels: u64,
    }

    impl GrowCost {
        fn mean(v: &[f64]) -> f64 {
            v.iter().sum::<f64>() / v.len().max(1) as f64
        }

        fn most(v: &[f64]) -> f64 {
            v.iter().copied().fold(0.0, f64::max)
        }

        fn print(&self, what: &str) {
            println!(
                "  {what}: frames {:.2} ms on average, {:.2} at most; the region made {:.2} ms on average, {:.2} at \
                 most; the lift {:.1} ms, laying {} pixels of the region",
                Self::mean(&self.frames),
                Self::most(&self.frames),
                Self::mean(&self.making),
                Self::most(&self.making),
                self.lift,
                self.lift_pixels
            );
        }
    }

    /// A fill growing round most of a `width` × `height` canvas (an ellipse 90% of it across,
    /// `samples` samples, a frame every two), each frame as the app makes it: the region made
    /// (as it was: all of it, the old way; now: where it changed), then laid and drawn. None
    /// without a GPU.
    fn grow_cost(width: u32, height: u32, samples: usize, as_it_was: bool) -> Option<GrowCost> {
        use std::time::Instant;
        let s = sized(width, height)?;
        no_streamline(&s);
        s.set_color(1.0, 0.0, 0.0);
        filling(&s, "(i32.const 1)");
        let to_screen = s.shared.lock().view.unwrap().to_screen();
        let (cx, cy, rx, ry) = (width as f32 / 2.0, height as f32 / 2.0, width as f32 * 0.45, height as f32 * 0.45);
        let rim: Vec<Vec2> = (0..samples)
            .map(|i| {
                let a = i as f32 / samples as f32 * std::f32::consts::TAU * 0.97;
                to_screen.apply(Vec2::new(cx + rx * a.cos(), cy + ry * a.sin()))
            })
            .collect();
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
        let drawn = |st: &mut State| {
            if let (Some(c), Some(g)) = (st.canvas.as_mut(), st.gpu.as_ref()) {
                c.render(g, &mut st.doc, None);
                g.flush();
            }
        };
        let outlines_now = |st: &State| {
            let f = st.filling.as_ref().unwrap();
            let mut path = f.path.path();
            if let Some(pen) = f.pen.filter(|p| path.last() != Some(p)) {
                path.push(pen);
            }
            st.region_of(&path, None)
        };
        let mut cost = GrowCost { frames: Vec::new(), making: Vec::new(), lift: 0.0, lift_pixels: 0 };
        let laid = |s: &CanvasSession| s.shared.lock().canvas.as_ref().map_or(0, |c| c.region_pixels_laid());
        s.stroke_begin(PointerKind::Stylus, vec![sample(rim[0].x, rim[0].y, 0.0)]);
        drawn(&mut s.shared.lock());
        for (i, p) in rim.iter().enumerate().skip(1) {
            s.stroke_add(vec![sample(p.x, p.y, i as f64 * 8.0)]);
            if i % 2 != 0 {
                continue;
            }
            let mut guard = s.shared.lock();
            let st = &mut *guard;
            let of = outlines_now(st);
            if as_it_was {
                let t = Instant::now();
                let region = reference_coverage(&of.outlines, width, height);
                cost.making.push(ms(t));
                let t = Instant::now();
                let (rect, coverage) = region.unwrap_or((IRect::EMPTY, Vec::new()));
                st.canvas.as_mut().unwrap().fill_stroke(rect, coverage);
                drawn(st);
                cost.frames.push(ms(t));
            } else {
                let laid = st.filling.as_mut().unwrap().laid.take();
                let t = Instant::now();
                let grown = of.grow(laid);
                cost.making.push(ms(t));
                let t = Instant::now();
                st.lay_grown(grown);
                drawn(st);
                cost.frames.push(ms(t));
            }
        }
        let last = *rim.last().unwrap();
        let before = laid(&s);
        let t = Instant::now();
        if as_it_was {
            // all of it made and laid again as the pen lifts
            let mut guard = s.shared.lock();
            let st = &mut *guard;
            let of = outlines_now(st);
            if let Some((rect, coverage)) = reference_coverage(&of.outlines, width, height) {
                st.canvas.as_mut().unwrap().fill_stroke(rect, coverage);
            }
            drawn(st);
            cost.lift = ms(t);
            drop(guard);
            cost.lift_pixels = laid(&s) - before;
            s.stroke_end(vec![sample(last.x, last.y, samples as f64 * 8.0)]);
        } else {
            s.stroke_end(vec![sample(last.x, last.y, samples as f64 * 8.0)]);
            drawn(&mut s.shared.lock());
            cost.lift = ms(t);
            cost.lift_pixels = laid(&s) - before;
        }
        let st = s.shared.lock();
        assert_eq!(st.gpu.as_ref().unwrap().errors(), 0);
        Some(cost)
    }

    /// #411: a fill growing on a large canvas cost every frame all of its region, made again and
    /// laid again (on a 4096 square canvas, a frame of a big loop laid 100 MB and more); now a frame
    /// lays what the region grew by. Here on a 2048 square canvas. Printed: the frames as it was
    /// and now. Bounds: now a frame costs under a third of what it did on average, and the lift lays
    /// under half the region's pixels the old way laid. (The lift's time is printed: now it keeps
    /// the fill as a step as well, which the old way's isn't timed with, and on an Adreno's OpenGL
    /// that copy came to over half the old way's whole lift one run in two.)
    #[test]
    fn a_growing_fill_s_frames_cost_what_it_grew_by() {
        let (Some(was), Some(now)) = (grow_cost(2048, 2048, 160, true), grow_cost(2048, 2048, 160, false)) else {
            return;
        };
        println!("a fill growing round a 2048 square canvas:");
        was.print("as it was");
        now.print("now");
        let (was_frame, now_frame) = (GrowCost::mean(&was.frames), GrowCost::mean(&now.frames));
        assert!(now_frame * 3.0 < was_frame, "{now_frame:.2} ms against {was_frame:.2} ms");
        assert!(now.lift_pixels * 2 < was.lift_pixels, "{} pixels against {}", now.lift_pixels, was.lift_pixels);
    }

    /// The same at the sizes #411 named: a 4096 square canvas and A3 at 300 dpi (3508 × 4961), and
    /// a 1024 square one to compare. Run with `--ignored --nocapture` (release for real numbers).
    #[test]
    #[ignore]
    fn growing_fill_cost() {
        for (w, h) in [(1024, 1024), (4096, 4096), (3508, 4961)] {
            let (Some(was), Some(now)) = (grow_cost(w, h, 400, true), grow_cost(w, h, 400, false)) else { return };
            println!("a fill growing round a {w} x {h} canvas (400 samples, a frame every two):");
            was.print("as it was");
            now.print("now");
        }
    }
}
