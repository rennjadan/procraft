//! Tracing a layer's paint into filled shapes, as an image trace does: its colours grouped into a
//! few, specks too small to keep merged into what is round them, each group's outline followed
//! along the pixels' edges, smoothed and simplified, and the shapes stacked so no seam shows
//! between neighbours: the most-covering colour at the bottom, covering every place a colour above
//! it covers too, so each place shows the colour on top there.
//!
//! What comes out is the colour and opacity of each shape, bottom first, and its outlines (filled
//! even-odd, in canvas pixels): what a vector layer's fill items hold.

use std::collections::HashMap;

/// One filled shape: its colour (sRGB, 0 to 1), its opacity, and its outlines, filled even-odd.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shape {
    pub colour: [f32; 3],
    pub opacity: f32,
    pub outlines: Vec<Vec<[f32; 2]>>,
}

/// How a layer is traced: how many colours its paint is grouped into, the smallest speck kept
/// (pixels), and how far a simplified outline may stray from the pixels' (pixels).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Detail {
    pub colours: usize,
    pub least_speck: usize,
    pub stray: f32,
}

impl Default for Detail {
    fn default() -> Detail {
        Detail { colours: 24, least_speck: 6, stray: 0.7 }
    }
}

/// Paint this transparent or less is none.
const CLEAR: u8 = 20;
/// A colour group this opaque on average or more is drawn opaque (its soft edges are the fill's
/// own smoothing).
const OPAQUE: f32 = 0.82;
/// Pixels looked at to find the colours, at most.
const SAMPLED: usize = 60_000;
/// Rounds of finding the colours.
const ROUNDS: usize = 12;
/// The most points all of a layer's outlines may have: past it, they are simplified further.
const MOST_POINTS: usize = 1_500_000;
/// Rounds of smoothing each outline.
const SMOOTHING: usize = 3;
/// No label: clear.
const NONE: u16 = u16::MAX;

/// Traces `rgba` (premultiplied, `width` × `height`, rows from the top) into shapes, bottom first.
pub(crate) fn trace(rgba: &[u8], width: u32, height: u32, detail: Detail) -> Vec<Shape> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return Vec::new();
    }
    let colours = straight(rgba, w * h);
    // grain and dither smoothed away first (a painted texture's speckle would be traced shape by
    // shape), the groups found and each pixel put in one on the smoothed colours; the averages
    // below are still the paint's own
    let smooth = softened(&colours, w, h, SOFTEN);
    // the groups are the paint's own colours (no blend of two neighbours becomes one of its own)
    let centres = find_colours(&colours, detail.colours.clamp(1, 250));
    if centres.is_empty() {
        return Vec::new();
    }
    let mut labels = label(&smooth, &centres);
    for _ in 0..TIDY_ROUNDS {
        labels = tidied(&labels, w, h);
    }
    // the smallest speck kept grows with the picture (a few hundredths of a percent of it)
    merge_specks(&mut labels, w, h, detail.least_speck.max(w * h / 40_000));
    // each colour group: how much it covers, its colour and opacity as the pixels in it average
    let mut stats = vec![(0usize, [0f64; 4]); centres.len()];
    for (l, c) in labels.iter().zip(&colours) {
        if let Some((n, sum)) = stats.get_mut(usize::from(*l)) {
            *n += 1;
            for (s, v) in sum.iter_mut().zip(c) {
                *s += f64::from(*v);
            }
        }
    }
    let count = |k: usize| stats.get(k).map_or(0, |s| s.0);
    let mean =
        |k: usize| stats.get(k).map_or([0.0; 4], |(n, sum)| sum.map(|s| (s / (*n).max(1) as f64) as f32 / 255.0));
    let used: Vec<usize> = (0..centres.len()).filter(|k| count(*k) > 0).collect();
    // the opaque groups stacked, most-covering at the bottom; the see-through ones on top, each
    // as it is
    let (mut solid, mut clear): (Vec<usize>, Vec<usize>) = used.iter().partition(|k| mean(**k)[3] >= OPAQUE);
    solid.sort_by_key(|k| std::cmp::Reverse(count(*k)));
    clear.sort_by_key(|k| std::cmp::Reverse(count(*k)));
    let mut rank = vec![usize::MAX; centres.len()];
    for (r, k) in solid.iter().enumerate() {
        if let Some(slot) = rank.get_mut(*k) {
            *slot = r;
        }
    }
    let rank_of = |l: u16| rank.get(usize::from(l)).copied().unwrap_or(usize::MAX);
    let mut stray = detail.stray.max(0.1);
    loop {
        let mut shapes = Vec::with_capacity(used.len());
        let mut points = 0;
        for (r, &k) in solid.iter().enumerate() {
            // this colour, and every opaque colour stacked above it
            let inside = |l: u16| l != NONE && rank_of(l) != usize::MAX && rank_of(l) >= r;
            let outlines = outlines(&labels, w, h, &inside, stray);
            points += outlines.iter().map(Vec::len).sum::<usize>();
            let [r, g, b, _] = mean(k);
            shapes.push(Shape { colour: [r, g, b], opacity: 1.0, outlines });
        }
        for &k in &clear {
            let me = u16::try_from(k).unwrap_or(NONE);
            let outlines = outlines(&labels, w, h, &|l: u16| l == me, stray);
            points += outlines.iter().map(Vec::len).sum::<usize>();
            let [r, g, b, a] = mean(k);
            shapes.push(Shape { colour: [r, g, b], opacity: a.clamp(0.0, 1.0), outlines });
        }
        shapes.retain(|s| !s.outlines.is_empty());
        if points <= MOST_POINTS || stray > 8.0 {
            return shapes;
        }
        stray *= 2.0;
    }
}

/// How far (pixels) colours are averaged with their neighbours' before grouping.
const SOFTEN: usize = 2;
/// Rounds of tidying the groups' edges.
const TIDY_ROUNDS: usize = 2;

/// `colours` (`w` × `h`) each averaged with its neighbours `r` around (a box, twice: near a
/// gaussian), so grain and dither even out.
fn softened(colours: &[[u8; 4]], w: usize, h: usize, r: usize) -> Vec<[u8; 4]> {
    // colour weighed by opacity (clear pixels lend none), opacity alongside
    let mut a: Vec<[u32; 4]> = colours
        .iter()
        .map(|c| {
            let k = u32::from(c[3]);
            [u32::from(c[0]) * k, u32::from(c[1]) * k, u32::from(c[2]) * k, k]
        })
        .collect();
    let mut b = a.clone();
    let window = (2 * r + 1) as u32;
    for _ in 0..2 {
        for y in 0..h {
            let row = y * w;
            for x in 0..w {
                let mut sum = [0u32; 4];
                for dx in 0..=2 * r {
                    let xx = (x + dx).saturating_sub(r).min(w - 1);
                    if let Some(p) = a.get(row + xx) {
                        for (s, v) in sum.iter_mut().zip(p) {
                            *s += v;
                        }
                    }
                }
                if let Some(o) = b.get_mut(row + x) {
                    *o = sum.map(|s| s / window);
                }
            }
        }
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0u32; 4];
                for dy in 0..=2 * r {
                    let yy = (y + dy).saturating_sub(r).min(h - 1);
                    if let Some(p) = b.get(yy * w + x) {
                        for (s, v) in sum.iter_mut().zip(p) {
                            *s += v;
                        }
                    }
                }
                if let Some(o) = a.get_mut(y * w + x) {
                    *o = sum.map(|s| s / window);
                }
            }
        }
    }
    a.into_iter()
        .zip(colours)
        .map(|(c, orig)| {
            // what was clear stays clear (and solid stays solid): only colour is smoothed
            let k = c[3].max(1);
            let un = |v: u32| (v / k).min(255) as u8;
            if orig[3] == 0 { *orig } else { [un(c[0]), un(c[1]), un(c[2]), orig[3]] }
        })
        .collect()
}

/// Each pixel's group as most of the pixels round it (two each way) have it: ragged edges between
/// groups made smooth, lone pixels taken in.
fn tidied(labels: &[u16], w: usize, h: usize) -> Vec<u16> {
    let mut out = labels.to_vec();
    let mut counts: Vec<(u16, u8)> = Vec::with_capacity(25);
    for y in 0..h {
        for x in 0..w {
            counts.clear();
            for yy in y.saturating_sub(2)..(y + 3).min(h) {
                for xx in x.saturating_sub(2)..(x + 3).min(w) {
                    let l = labels.get(yy * w + xx).copied().unwrap_or(NONE);
                    match counts.iter_mut().find(|(k, _)| *k == l) {
                        Some((_, n)) => *n += 1,
                        None => counts.push((l, 1)),
                    }
                }
            }
            let own = labels.get(y * w + x).copied().unwrap_or(NONE);
            // a clear pixel stays clear, and paint stays paint (edges kept where they are)
            if let Some((best, _)) =
                counts.iter().filter(|(k, _)| (*k == NONE) == (own == NONE)).max_by_key(|(_, n)| *n)
                && let Some(o) = out.get_mut(y * w + x)
            {
                *o = *best;
            }
        }
    }
    out
}

/// Each pixel's colour, no longer premultiplied, and its opacity (0 to 255).
fn straight(rgba: &[u8], pixels: usize) -> Vec<[u8; 4]> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .take(pixels)
        .map(|&[r, g, b, a]| {
            if a == 0 {
                return [0, 0, 0, 0];
            }
            let un = |v: u8| ((u32::from(v) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
            [un(r), un(g), un(b), a]
        })
        .collect()
}

/// Opacity weighs this much against a colour's channels when colours are grouped.
const OPACITY_WEIGHT: f32 = 0.6;

fn distance(a: [f32; 4], b: [f32; 4]) -> f32 {
    let [r, g, b2, o] = [a[0] - b[0], a[1] - b[1], a[2] - b[2], (a[3] - b[3]) * OPACITY_WEIGHT];
    r * r + g * g + b2 * b2 + o * o
}

fn as_f32(c: [u8; 4]) -> [f32; 4] {
    c.map(f32::from)
}

/// The colours the paint is grouped into (k-means from a spread of its pixels), at most `most`.
fn find_colours(colours: &[[u8; 4]], most: usize) -> Vec<[f32; 4]> {
    let painted: Vec<[u8; 4]> = colours.iter().copied().filter(|c| c[3] > CLEAR).collect();
    if painted.is_empty() {
        return Vec::new();
    }
    let step = painted.len().div_ceil(SAMPLED).max(1);
    let sample: Vec<[f32; 4]> = painted.iter().step_by(step).map(|c| as_f32(*c)).collect();
    // the first centres spread out: each next the sample farthest from those so far
    let Some(&middle) = sample.get(sample.len() / 2) else { return Vec::new() };
    let mut centres = vec![middle];
    let mut near: Vec<f32> = sample.iter().map(|s| distance(*s, middle)).collect();
    while centres.len() < most {
        let Some((far, d)) = near.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1)) else { break };
        // nothing left that is a colour of its own
        let Some(&c) = sample.get(far).filter(|_| d >= 64.0) else { break };
        centres.push(c);
        for (n, s) in near.iter_mut().zip(&sample) {
            *n = n.min(distance(*s, c));
        }
    }
    for _ in 0..ROUNDS {
        let mut sums = vec![([0f64; 4], 0f64); centres.len()];
        for s in &sample {
            if let Some((sum, n)) = sums.get_mut(nearest(&centres, *s)) {
                for (t, v) in sum.iter_mut().zip(s) {
                    *t += f64::from(*v);
                }
                *n += 1.0;
            }
        }
        for (c, (sum, n)) in centres.iter_mut().zip(&sums) {
            if *n > 0.0 {
                *c = sum.map(|t| (t / n) as f32);
            }
        }
    }
    centres
}

fn nearest(centres: &[[f32; 4]], c: [f32; 4]) -> usize {
    let mut best = (0, f32::INFINITY);
    for (k, centre) in centres.iter().enumerate() {
        let d = distance(*centre, c);
        if d < best.1 {
            best = (k, d);
        }
    }
    best.0
}

/// Each pixel's colour group, or [`NONE`] where it is clear (colours seen before looked up again).
fn label(colours: &[[u8; 4]], centres: &[[f32; 4]]) -> Vec<u16> {
    let mut seen: HashMap<[u8; 4], u16> = HashMap::new();
    colours
        .iter()
        .map(|c| {
            if c[3] <= CLEAR {
                return NONE;
            }
            *seen.entry(*c).or_insert_with(|| u16::try_from(nearest(centres, as_f32(*c))).unwrap_or(NONE))
        })
        .collect()
}

/// Patches of one group smaller than `least` pixels (touching side to side) take the group most
/// round them; clear specks that small are filled the same way, and painted ones in the clear go.
fn merge_specks(labels: &mut [u16], w: usize, h: usize, least: usize) {
    if least <= 1 {
        return;
    }
    let mut seen = vec![false; labels.len()];
    let mut patch = Vec::new();
    let mut stack = Vec::new();
    for start in 0..labels.len() {
        if seen.get(start).is_none_or(|s| *s) {
            continue;
        }
        let Some(&me) = labels.get(start) else { continue };
        patch.clear();
        stack.push(start);
        if let Some(s) = seen.get_mut(start) {
            *s = true;
        }
        while let Some(i) = stack.pop() {
            patch.push(i);
            for n in neighbours(i, w, h).into_iter().flatten() {
                if labels.get(n) == Some(&me)
                    && let Some(s) = seen.get_mut(n)
                    && !*s
                {
                    *s = true;
                    stack.push(n);
                }
            }
        }
        if patch.len() >= least {
            continue;
        }
        // what is round it the most
        let mut round: HashMap<u16, usize> = HashMap::new();
        for &i in &patch {
            for n in neighbours(i, w, h).into_iter().flatten() {
                if let Some(&l) = labels.get(n).filter(|l| **l != me) {
                    *round.entry(l).or_default() += 1;
                }
            }
        }
        if let Some((&to, _)) = round.iter().max_by_key(|(l, n)| (**n, std::cmp::Reverse(**l))) {
            for &i in &patch {
                if let Some(l) = labels.get_mut(i) {
                    *l = to;
                }
            }
        }
    }
}

fn neighbours(i: usize, w: usize, h: usize) -> [Option<usize>; 4] {
    let (x, y) = (i % w, i / w);
    [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)]
}

/// The outlines round the pixels `inside` takes: each loop along the pixels' edges, smoothed and
/// simplified to within `stray` pixels.
fn outlines(labels: &[u16], w: usize, h: usize, inside: &dyn Fn(u16) -> bool, stray: f32) -> Vec<Vec<[f32; 2]>> {
    let at = |x: isize, y: isize| {
        x >= 0
            && y >= 0
            && (x as usize) < w
            && (y as usize) < h
            && labels.get(y as usize * w + x as usize).is_some_and(|l| inside(*l))
    };
    // the edges between a pixel in and one out, each going round the inside clockwise (on the
    // screen, y down): from a corner to the next
    let mut from: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();
    for y in 0..h as isize {
        for x in 0..w as isize {
            if !at(x, y) {
                continue;
            }
            let (cx, cy) = (x as i32, y as i32);
            if !at(x, y - 1) {
                from.entry((cx, cy)).or_default().push((cx + 1, cy));
            }
            if !at(x + 1, y) {
                from.entry((cx + 1, cy)).or_default().push((cx + 1, cy + 1));
            }
            if !at(x, y + 1) {
                from.entry((cx + 1, cy + 1)).or_default().push((cx, cy + 1));
            }
            if !at(x - 1, y) {
                from.entry((cx, cy + 1)).or_default().push((cx, cy));
            }
        }
    }
    let mut loops = Vec::new();
    let starts: Vec<(i32, i32)> = {
        let mut s: Vec<(i32, i32)> = from.keys().copied().collect();
        s.sort_unstable();
        s
    };
    for start in starts {
        while let Some(next) = from.get_mut(&start).and_then(Vec::pop) {
            let mut corners = vec![start];
            let mut came = (next.0 - start.0, next.1 - start.1);
            let mut here = next;
            while here != start {
                corners.push(here);
                let Some(ways) = from.get_mut(&here) else { break };
                // where two loops touch at a corner, the one turning right keeps them apart
                let pick = ways
                    .iter()
                    .position(|w| (w.0 - here.0, w.1 - here.1) == (-came.1, came.0))
                    .or_else(|| ways.iter().position(|w| (w.0 - here.0, w.1 - here.1) == came))
                    .unwrap_or(0);
                if ways.is_empty() {
                    break;
                }
                let to = ways.swap_remove(pick);
                came = (to.0 - here.0, to.1 - here.1);
                here = to;
            }
            if corners.len() >= 4 {
                loops.push(smoothed(&corners, stray));
            }
        }
    }
    loops.retain(|l| l.len() >= 3);
    loops
}

/// A loop of pixel corners (one pixel's edge apart) as a smooth outline: the middle of each edge,
/// so a staircase becomes a slope and a square corner is cut by half a pixel, then simplified to
/// within `stray`.
fn smoothed(corners: &[(i32, i32)], stray: f32) -> Vec<[f32; 2]> {
    let next = corners.iter().cycle().skip(1);
    let middles: Vec<[f32; 2]> =
        corners.iter().zip(next).map(|(a, b)| [(a.0 + b.0) as f32 / 2.0, (a.1 + b.1) as f32 / 2.0]).collect();
    // a light smoothing, so the steps a soft edge leaves become a curve (a corner moves under a
    // pixel)
    let mut points = middles;
    for _ in 0..SMOOTHING {
        let n = points.len();
        if n < 5 {
            break;
        }
        let before = points.iter().cycle().skip(n - 1);
        let after = points.iter().cycle().skip(1);
        points = points
            .iter()
            .zip(before)
            .zip(after)
            .map(|((p, b), a)| [p[0] * 0.5 + (a[0] + b[0]) * 0.25, p[1] * 0.5 + (a[1] + b[1]) * 0.25])
            .collect();
    }
    simplified(&points, stray)
}

/// A closed loop with the points it can do without (within `stray`) left out.
fn simplified(points: &[[f32; 2]], stray: f32) -> Vec<[f32; 2]> {
    let n = points.len();
    if n <= 4 {
        return points.to_vec();
    }
    let at = |i: usize| points.get(i % n).copied().unwrap_or_default();
    // split at the point farthest from the first, each half simplified as a line
    let first = at(0);
    let far = (1..n).max_by(|a, b| span(at(*a), first).total_cmp(&span(at(*b), first))).unwrap_or(n / 2);
    let mut keep = vec![false; n];
    let kept = |i: usize, keep: &mut Vec<bool>| {
        if let Some(k) = keep.get_mut(i) {
            *k = true;
        }
    };
    kept(0, &mut keep);
    kept(far, &mut keep);
    let mut stack = vec![(0, far), (far, n)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (start, end) = (at(a), at(b));
        let mut worst = (a, 0.0f32);
        for i in a + 1..b {
            let d = off_line(at(i), start, end);
            if d > worst.1 {
                worst = (i, d);
            }
        }
        if worst.1 > stray {
            kept(worst.0, &mut keep);
            stack.push((a, worst.0));
            stack.push((worst.0, b));
        }
    }
    points.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

fn span(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// How far `p` is from the line through `a` and `b`.
fn off_line(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = dx.hypot(dy);
    if length < 1e-6 {
        return span(p, a);
    }
    ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx).abs() / length
}

/// A fill item for a vector layer of `shape`, as the app writes items: the paint tool, its colour,
/// its opacity and its outlines (field 25), each field a 16-bit id, a 32-bit length and its bytes.
pub(crate) fn fill_item(shape: &Shape) -> Vec<u8> {
    let mut out = Vec::new();
    let mut field = |id: u16, bytes: &[u8]| {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_le_bytes());
        out.extend_from_slice(bytes);
    };
    field(1, &[0]);
    let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    field(3, &floats(&shape.colour));
    field(5, &floats(&[1.0]));
    field(6, &floats(&[shape.opacity]));
    field(7, &floats(&[1.0]));
    let mut outline = Vec::new();
    outline.extend_from_slice(&u32::try_from(shape.outlines.len()).unwrap_or(0).to_le_bytes());
    for o in &shape.outlines {
        outline.extend_from_slice(&u32::try_from(o.len()).unwrap_or(0).to_le_bytes());
        for [x, y] in o {
            outline.extend_from_slice(&x.to_le_bytes());
            outline.extend_from_slice(&y.to_le_bytes());
        }
    }
    field(25, &outline);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w` × `h` picture, clear, with `paint` putting opaque colours where it says.
    fn picture(w: usize, h: usize, paint: impl Fn(usize, usize) -> Option<[u8; 3]>) -> Vec<u8> {
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                if let Some([r, g, b]) = paint(x, y) {
                    out[(y * w + x) * 4..][..4].copy_from_slice(&[r, g, b, 255]);
                }
            }
        }
        out
    }

    /// Whether `p` is inside the outlines, even-odd.
    fn inside(outlines: &[Vec<[f32; 2]>], p: [f32; 2]) -> bool {
        let mut odd = false;
        for o in outlines {
            for i in 0..o.len() {
                let (a, b) = (o[i], o[(i + 1) % o.len()]);
                if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) {
                    odd = !odd;
                }
            }
        }
        odd
    }

    #[test]
    fn two_colours_trace_into_two_stacked_shapes_with_no_seam() {
        // a red square with a blue one inside it, on clear
        let paint = |x: usize, y: usize| {
            let red = (10..90).contains(&x) && (10..90).contains(&y);
            let blue = (30..60).contains(&x) && (30..60).contains(&y);
            if blue {
                Some([0, 0, 255])
            } else if red {
                Some([255, 0, 0])
            } else {
                None
            }
        };
        let shapes = trace(&picture(100, 100, paint), 100, 100, Detail::default());
        assert_eq!(shapes.len(), 2, "{shapes:?}");
        // the red at the bottom covers the blue's place too, so nothing shows through between them
        assert!(shapes[0].colour[0] > 0.9 && shapes[0].colour[2] < 0.1);
        assert!(inside(&shapes[0].outlines, [45.0, 45.0]) && inside(&shapes[0].outlines, [15.0, 15.0]));
        assert!(!inside(&shapes[0].outlines, [5.0, 5.0]));
        assert!(shapes[1].colour[2] > 0.9 && inside(&shapes[1].outlines, [45.0, 45.0]));
        assert!(!inside(&shapes[1].outlines, [15.0, 15.0]));
        // straight sides come down to a few points each
        assert!(shapes.iter().all(|s| s.outlines.iter().map(Vec::len).sum::<usize>() < 40), "{shapes:?}");
    }

    #[test]
    fn a_ring_keeps_its_hole_and_a_thin_line_survives() {
        let paint = |x: usize, y: usize| {
            let (dx, dy) = (x as f32 - 50.0, y as f32 - 50.0);
            let d = dx.hypot(dy);
            let ring = (20.0..35.0).contains(&d);
            let line = y == 90 && (5..95).contains(&x);
            (ring || line).then_some([20, 20, 20])
        };
        let shapes = trace(&picture(100, 100, paint), 100, 100, Detail::default());
        assert_eq!(shapes.len(), 1);
        let o = &shapes[0].outlines;
        assert!(inside(o, [50.0 + 27.0, 50.0]) && !inside(o, [50.0, 50.0]), "the hole is kept");
        assert!(inside(o, [50.0, 90.5]), "the one-pixel line is kept");
    }

    #[test]
    fn specks_go_and_clear_paint_is_nothing() {
        let paint = |x: usize, y: usize| ((x, y) == (50, 50)).then_some([255, 255, 255]);
        assert!(trace(&picture(100, 100, paint), 100, 100, Detail::default()).is_empty());
        assert!(trace(&vec![0; 40 * 40 * 4], 40, 40, Detail::default()).is_empty());
    }

    #[test]
    fn a_fill_item_reads_as_the_app_writes_one() {
        let shape =
            Shape { colour: [1.0, 0.5, 0.0], opacity: 0.75, outlines: vec![vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0]]] };
        let item = fill_item(&shape);
        let mut fields = Vec::new();
        let mut at = 0;
        while at + 6 <= item.len() {
            let id = u16::from_le_bytes([item[at], item[at + 1]]);
            let len = u32::from_le_bytes(item[at + 2..at + 6].try_into().unwrap()) as usize;
            fields.push((id, len));
            at += 6 + len;
        }
        assert_eq!(at, item.len());
        assert_eq!(fields, [(1, 1), (3, 12), (5, 4), (6, 4), (7, 4), (25, 4 + 4 + 3 * 8)]);
    }
}

#[cfg(test)]
mod quality {
    use super::*;

    /// A painting-like picture: soft blobs of colour, a gradient sky and dark lines.
    fn painting(w: usize, h: usize) -> Vec<u8> {
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
                let mut c = [80.0 + 150.0 * fy, 120.0 + 60.0 * fx, 220.0 - 100.0 * fy];
                for (cx, cy, r, col) in [(0.3, 0.4, 0.15, [230.0, 90.0, 40.0]), (0.7, 0.6, 0.2, [40.0, 160.0, 70.0])] {
                    let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                    let t = ((r - d) / 0.02).clamp(0.0, 1.0);
                    for i in 0..3 {
                        c[i] = c[i] * (1.0 - t) + col[i] * t;
                    }
                }
                if ((fx * 7.0).fract() - 0.5).abs() < 0.004 || ((fy * 5.0 + fx).fract() - 0.5).abs() < 0.003 {
                    c = [20.0, 20.0, 25.0];
                }
                let i = (y * w + x) * 4;
                out[i..i + 4].copy_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8, 255]);
            }
        }
        out
    }

    #[test]
    #[ignore = "measures a canvas-sized trace; run by hand"]
    fn a_canvas_sized_painting_traces_in_time() {
        let (w, h) = (1500, 2100);
        let p = painting(w, h);
        let start = std::time::Instant::now();
        let shapes = trace(&p, w as u32, h as u32, Detail::default());
        let points: usize = shapes.iter().flat_map(|s| s.outlines.iter().map(Vec::len)).sum();
        eprintln!("{} shapes, {points} points, {:?}", shapes.len(), start.elapsed());
        if let Some(dir) = std::env::var_os("TRACE_OUT") {
            let dir = std::path::PathBuf::from(dir);
            let mut svg = format!("<svg xmlns='http://www.w3.org/2000/svg' width='{w}' height='{h}'>");
            for s in &shapes {
                let d: String = s
                    .outlines
                    .iter()
                    .map(|o| {
                        let pts: Vec<String> = o.iter().map(|[x, y]| format!("{x},{y}")).collect();
                        format!("M{}Z", pts.join("L"))
                    })
                    .collect();
                let [r, g, b] = s.colour.map(|v| (v * 255.0) as u8);
                svg += &format!(
                    "<path fill-rule='evenodd' fill='rgb({r},{g},{b})' fill-opacity='{}' d='{d}'/>",
                    s.opacity
                );
            }
            svg += "</svg>";
            std::fs::write(dir.join("traced.svg"), svg).unwrap();
            let mut ppm = format!("P6 {w} {h} 255\n").into_bytes();
            for px in p.as_chunks::<4>().0 {
                ppm.extend_from_slice(&px[..3]);
            }
            std::fs::write(dir.join("source.ppm"), ppm).unwrap();
        }
        assert!(!shapes.is_empty());
    }
}

#[cfg(test)]
mod look_test {
    /// Traces the picture at TRACE_IN and writes how the shapes look to TRACE_OUT (to see a trace
    /// by eye): `TRACE_IN=a.png TRACE_OUT=b.png cargo test -p procraft-core trace_picture -- --ignored`.
    #[test]
    #[ignore]
    fn trace_picture() {
        let (Some(input), Some(output)) = (std::env::var_os("TRACE_IN"), std::env::var_os("TRACE_OUT")) else { return };
        let img = image::open(input).unwrap().to_rgba8();
        let (w, h) = img.dimensions();
        // premultiplied, as layers are
        let mut px = img.into_raw();
        for p in px.chunks_exact_mut(4) {
            let a = u32::from(p[3]);
            for c in p.iter_mut().take(3) {
                *c = ((u32::from(*c) * a + 127) / 255) as u8;
            }
        }
        let started = std::time::Instant::now();
        let shapes = super::trace(&px, w, h, super::Detail::default());
        eprintln!("{} shapes in {:?}", shapes.len(), started.elapsed());
        let mut out = vec![255u8; (w * h * 4) as usize];
        for s in &shapes {
            // even-odd: each outline flips what it covers
            let mut cover = vec![false; (w * h) as usize];
            for o in &s.outlines {
                let pts: Vec<procraft_engine::geom::Vec2> =
                    o.iter().map(|&[x, y]| procraft_engine::geom::Vec2::new(x, y)).collect();
                let m = procraft_engine::selection::Mask::polygon(w, h, &pts);
                for (c, &v) in cover.iter_mut().zip(&m.data) {
                    if v > 127 {
                        *c = !*c;
                    }
                }
            }
            let col = s.colour.map(|c| (c * 255.0).round() as u8);
            for (i, &c) in cover.iter().enumerate() {
                if c {
                    let a = s.opacity;
                    for k in 0..3 {
                        let d = &mut out[i * 4 + k];
                        *d = (f32::from(*d) * (1.0 - a) + f32::from(col[k]) * a).round() as u8;
                    }
                }
            }
        }
        image::save_buffer(output, &out, w, h, image::ColorType::Rgba8).unwrap();
    }
}
