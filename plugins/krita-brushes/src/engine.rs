//! A stroke of a Krita pixel brush (or colour smudge brush), dab by dab: its input walked along, a
//! dab laid each time the path has gone as far as the spacing of the last dab reaches (an ellipse
//! turned with the dab, or a circle when the spacing is the same every way), each dab's size, turn,
//! squash, scatter, mirroring, opacity, flow, colour, tip and texture strength from the brush's
//! options and the pen where it lies. A colour smudge brush's dabs move the paint under the last
//! dab to where they lie, mixing in the colour as its rates say. A masking brush walks the same
//! path with its own spacing. The sketch brush lays lines at each move instead, and the spray brush
//! its particles at each dab (see `others.rs`).

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};
use std::rc::Rc;

use crate::brush::{Brush, PipePick, Tip, Tips};
use crate::gimp::Selection;
use crate::options::{Chance, CurveOption, Info};

/// What a stroke starts with: the app's colour and secondary colour (Krita's background), its size
/// now and where the brush's size starts (the preset's own size there), a seed.
#[derive(Clone, Copy, Debug)]
pub struct Begin {
    pub colour: [f32; 3],
    pub secondary: [f32; 3],
    pub diameter: f32,
    pub start_diameter: f32,
    pub seed: u32,
    /// How far the canvas is turned on screen (radians).
    pub turn: f32,
}

/// A sample of the pen: canvas pixels, pressure, lean from upright and its way (radians,
/// clockwise from up), the barrel's turn (radians), milliseconds since the stroke began.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt: f32,
    pub azimuth: f32,
    pub roll: f32,
    pub ms: f64,
}

/// The pen at a point of the path, as Krita takes it.
#[derive(Clone, Copy, Debug, Default)]
struct Point {
    x: f32,
    y: f32,
    pressure: f32,
    x_tilt: f32,
    y_tilt: f32,
    rotation: f32,
    time: f32,
    speed: f32,
}

impl Point {
    fn between(a: &Point, b: &Point, t: f32) -> Point {
        let l = |p: f32, q: f32| p + (q - p) * t;
        Point {
            x: l(a.x, b.x),
            y: l(a.y, b.y),
            pressure: l(a.pressure, b.pressure),
            x_tilt: l(a.x_tilt, b.x_tilt),
            y_tilt: l(a.y_tilt, b.y_tilt),
            rotation: l(a.rotation, b.rotation),
            time: l(a.time, b.time),
            speed: l(a.speed, b.speed),
        }
    }
}

/// xorshift: the stroke's randomness, from its seed.
#[derive(Clone, Debug)]
pub struct Random(u32);

impl Random {
    pub fn new(seed: u32) -> Random {
        Random(seed | 1)
    }

    /// 0..1.
    pub fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1 << 24) as f32
    }
}

struct Fuzz<'a> {
    random: &'a mut Random,
    seed: u32,
}

impl Chance for Fuzz<'_> {
    fn dab(&mut self) -> f32 {
        self.random.next()
    }

    fn stroke(&mut self, key: &str) -> f32 {
        // the same for the whole stroke, apart for each option
        let mut h = self.seed ^ 0x811c_9dc5;
        for b in key.bytes() {
            h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
        }
        Random::new(h).next()
    }
}

/// Where one brush (the main one, or its masking brush) is along the path: how far it has gone
/// since its last dab, across and down in that dab's frame, the spacing that dab set and its turn.
#[derive(Clone, Debug, Default)]
struct Walker {
    gone: (f32, f32),
    spacing: (f32, f32),
    turn: f32,
    dabs: u32,
    /// When its last dab was laid (ms), for an airbrush.
    last_ms: f32,
}

/// Spacing below this (canvas pixels) is taken as this, as Krita does.
const LEAST_SPACING: f32 = 0.5;
/// The most dabs one call lays, and the most points a sketch brush keeps.
const MOST_DABS: usize = 20_000;
const MOST_SKETCH_POINTS: usize = 4096;

/// A number of the standard normal distribution (Box and Muller's way).
fn gaussian(random: &mut Random) -> f32 {
    let u = random.next().max(1e-7);
    let v = random.next();
    (-2.0 * u.ln()).sqrt() * (TAU * v).cos()
}

impl Walker {
    /// How far along `from`..`to` (0..1) its next dab goes, if it goes there at all.
    fn next(&mut self, from: (f32, f32), to: (f32, f32)) -> Option<f32> {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        if dx == 0.0 && dy == 0.0 {
            return None;
        }
        let (a, b) = (1.0 / self.spacing.0.max(LEAST_SPACING), 1.0 / self.spacing.1.max(LEAST_SPACING));
        let (x, y) = self.gone;
        let gamma = (x * a).powi(2) + (y * b).powi(2) - 1.0;
        if gamma >= 0.0 {
            self.gone = (0.0, 0.0);
            return Some(0.0);
        }
        // the move in the frame of the last dab
        let (s, c) = (-self.turn).sin_cos();
        let (mx, my) = ((dx * c - dy * s).abs(), (dx * s + dy * c).abs());
        let alpha = (mx * a).powi(2) + (my * b).powi(2);
        let beta = x * mx * a * a + y * my * b * b;
        let d = beta * beta - alpha * gamma;
        if d >= 0.0 && alpha > 0.0 {
            let k = (-beta + d.sqrt()) / alpha;
            if (0.0..=1.0).contains(&k) {
                self.gone = (0.0, 0.0);
                return Some(k);
            }
        }
        self.gone = (x + mx, y + my);
        None
    }
}

/// How a pipe stands: each of its dimensions' step.
#[derive(Clone, Debug, Default)]
struct PipeState {
    steps: Vec<u32>,
}

/// A dab, as the app takes it (see the app's guide): 24 numbers.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dab {
    pub x: f32,
    pub y: f32,
    pub diameter: f32,
    pub angle: f32,
    pub ratio: f32,
    pub opacity: f32,
    pub flow: f32,
    pub tip: u32,
    pub flags: u32,
    pub colour: [f32; 3],
    pub strength: f32,
    pub texture_offset: (f32, f32),
    /// How strongly a lightness tip's grey shows; its sharpness threshold (0: none) and the share
    /// below it kept as it is.
    pub lightness: f32,
    pub threshold: f32,
    pub softness: f32,
    /// A dab that moves paint: the share of what it lays that is its colour, and the share of where
    /// the last lay it picks one colour from (dulling).
    pub paint_share: f32,
    pub radius: f32,
}

const FLIP_ACROSS: u32 = 1;
const FLIP_DOWN: u32 = 2;
const OWN_COLOUR: u32 = 4;
const FOR_SECOND: u32 = 8;
const MOVES_PAINT: u32 = 16;
const DULLS: u32 = 32;
const WEIGHED: u32 = 64;
const CLEAR_PAINT: u32 = 128;

impl Dab {
    pub fn write(&self, out: &mut Vec<u8>) {
        let words: [u32; 24] = [
            self.x.to_bits(),
            self.y.to_bits(),
            self.diameter.to_bits(),
            self.angle.to_bits(),
            self.ratio.to_bits(),
            self.opacity.to_bits(),
            self.flow.to_bits(),
            self.tip,
            self.flags,
            self.colour[0].to_bits(),
            self.colour[1].to_bits(),
            self.colour[2].to_bits(),
            self.strength.to_bits(),
            self.texture_offset.0.to_bits(),
            self.texture_offset.1.to_bits(),
            self.lightness.to_bits(),
            self.threshold.to_bits(),
            self.softness.to_bits(),
            self.paint_share.to_bits(),
            self.radius.to_bits(),
            0,
            0,
            0,
            0,
        ];
        for w in words {
            out.extend_from_slice(&w.to_le_bytes());
        }
    }
}

pub struct Stroke {
    brush: Rc<Brush>,
    begin: Begin,
    random: Random,
    last: Option<Point>,
    main: Walker,
    second: Walker,
    pipe: PipeState,
    second_pipe: PipeState,
    /// The path drawn so far (canvas pixels), its way (radians) and the dabs laid.
    length: f32,
    angle: f32,
    dabs: u32,
    /// What the stroke's opacity has averaged (washed strokes keep near it).
    average: f32,
    texture_offset: (f32, f32),
    /// For the speed: the last samples' moves and the time between samples, averaged.
    moves: Vec<f32>,
    /// A sketch brush's points passed, the latest last.
    points: Vec<(f32, f32)>,
    pace: f32,
    speed: f32,
    pub out: Vec<Dab>,
}

impl Stroke {
    pub fn new(brush: Rc<Brush>, begin: Begin) -> Stroke {
        let mut random = Random::new(begin.seed);
        let texture_offset = brush.texture.as_ref().map_or((0.0, 0.0), |t| {
            let (w, h) = (t.mask.width as f32 * t.scale, t.mask.height as f32 * t.scale);
            let x = if t.random_offset.0 { random.next() * w } else { t.offset.0 };
            let y = if t.random_offset.1 { random.next() * h } else { t.offset.1 };
            (-x, -y)
        });
        let pipe = |tip: &Tip, random: &mut Random| PipeState {
            steps: match &tip.tips {
                Tips::Images { pipe: Some(p), .. } => p
                    .ranks
                    .iter()
                    .zip(&p.selections)
                    .map(|(&r, s)| if *s == Selection::Random { (random.next() * r as f32) as u32 } else { 0 })
                    .collect(),
                _ => Vec::new(),
            },
        };
        let main_pipe = pipe(&brush.tip, &mut random);
        let second_pipe = brush.masking.as_ref().map(|m| pipe(&m.tip, &mut random)).unwrap_or_default();
        Stroke {
            brush,
            begin,
            random,
            last: None,
            main: Walker::default(),
            second: Walker::default(),
            pipe: main_pipe,
            second_pipe,
            length: 0.0,
            angle: 0.0,
            dabs: 0,
            average: 0.0,
            texture_offset,
            moves: Vec::new(),
            points: Vec::new(),
            pace: 0.0,
            speed: 0.0,
            out: Vec::new(),
        }
    }

    /// The size the app's slider asks for, as a share of the brush's own.
    fn size_factor(&self) -> f32 {
        let (d, s) = (self.begin.diameter, self.begin.start_diameter);
        if d.is_finite() && s.is_finite() && s > 0.0 && d > 0.0 { d / s } else { 1.0 }
    }

    /// The pen at `s`, its speed measured as Krita measures it (the last few moves over the time
    /// samples take on average, in pixels a millisecond, a thirtieth of that the sensor's 1).
    fn point(&mut self, s: &Sample) -> Point {
        let (x_tilt, y_tilt) = lean(s.tilt, s.azimuth);
        if let Some(last) = self.last {
            let moved = ((s.x - last.x).powi(2) + (s.y - last.y).powi(2)).sqrt();
            if moved > 0.0 {
                let gap = (s.ms as f32 - last.time).max(0.0);
                self.pace = if self.pace == 0.0 { gap.max(1.0) } else { 0.8 * self.pace + 0.2 * gap.max(0.5) };
                self.moves.push(moved);
                if self.moves.len() > 64 {
                    self.moves.remove(0);
                }
                let (mut total, mut n) = (0.0, 0);
                for m in self.moves.iter().rev() {
                    total += m;
                    n += 1;
                    if n > 3 && total > 5.0 {
                        break;
                    }
                }
                if total > 5.0 {
                    self.speed = (total / (n as f32 * self.pace)) / 30.0;
                }
            }
        }
        Point {
            x: s.x,
            y: s.y,
            pressure: s.pressure.clamp(0.0, 1.0),
            x_tilt,
            y_tilt,
            rotation: s.roll.to_degrees(),
            time: s.ms as f32,
            speed: self.speed.min(1.0),
        }
    }

    pub fn add(&mut self, samples: &[Sample]) {
        for s in samples {
            if !(s.x.is_finite() && s.y.is_finite()) || self.out.len() >= MOST_DABS {
                continue;
            }
            let p = self.point(s);
            // a sketch brush draws its lines at each move
            if self.brush.sketch.is_some() {
                let from = self.last.unwrap_or(p);
                self.sketch_move(&from, &p);
                self.length += ((p.x - from.x).powi(2) + (p.y - from.y).powi(2)).sqrt();
                self.last = Some(p);
                continue;
            }
            let Some(last) = self.last else {
                // the first point gets a dab of each brush
                self.dab_at(&p, false);
                if self.brush.masking.is_some() {
                    self.dab_at(&p, true);
                }
                self.last = Some(p);
                continue;
            };
            let (dx, dy) = (p.x - last.x, p.y - last.y);
            if dx != 0.0 || dy != 0.0 {
                self.angle = dy.atan2(dx);
            }
            for second in [false, true] {
                if second && self.brush.masking.is_none() {
                    continue;
                }
                let mut at = last;
                let start_length = self.length;
                while self.out.len() < MOST_DABS {
                    // an airbrush lays a dab when its time comes, wherever its spacing has got to
                    let timed = if second { None } else { self.airbrushed(&at, &p) };
                    let until = timed.map_or(p, |k| Point::between(&at, &p, k));
                    let ignore = !second && self.brush.airbrush.as_ref().is_some_and(|a| a.ignore_spacing);
                    let walker = if second { &mut self.second } else { &mut self.main };
                    let spaced = if ignore { None } else { walker.next((at.x, at.y), (until.x, until.y)) };
                    let (t, here) = match (spaced, timed) {
                        (Some(t), _) => (t, Point::between(&at, &until, t)),
                        (None, Some(_)) => {
                            walker.gone = (0.0, 0.0);
                            (1.0, until)
                        }
                        (None, None) => break,
                    };
                    // the distance sensor's: as far along as this dab
                    let done = ((here.x - last.x).powi(2) + (here.y - last.y).powi(2)).sqrt();
                    self.length = start_length + done;
                    self.dab_at(&here, second);
                    if t <= 0.0 && (here.x - at.x).abs() < 1e-6 && (here.y - at.y).abs() < 1e-6 {
                        // a dab where the walk stood: go on from it
                        at = here;
                        continue;
                    }
                    at = here;
                }
                self.length = start_length;
            }
            self.length += (dx * dx + dy * dy).sqrt();
            self.last = Some(p);
        }
    }

    /// How far from `at` to `to` (0..1) the airbrush's next dab comes, if it comes before `to`:
    /// once its interval (scaled by its rate option where `at` is) has passed since the last.
    fn airbrushed(&mut self, at: &Point, to: &Point) -> Option<f32> {
        let airbrush = self.brush.airbrush.clone()?;
        let info = self.info(at, self.main.dabs);
        let seed = self.begin.seed;
        let mut fuzz = Fuzz { random: &mut self.random, seed };
        let scale = if airbrush.rate.checked { airbrush.rate.size_like(&info, &mut fuzz, true) } else { 1.0 };
        if scale <= 0.0 {
            return None;
        }
        let need = airbrush.interval / scale - (at.time - self.main.last_ms);
        let span = to.time - at.time;
        if need <= 0.0 {
            Some(0.0)
        } else if span > 0.0 && need <= span {
            Some(need / span)
        } else {
            None
        }
    }

    /// A dab of a line from `a` to `b`, `width` across, with tip `tip` (a square), `opacity`, its
    /// colour its own if given.
    fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, opacity: f32, colour: Option<[f32; 3]>) {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let length = (dx * dx + dy * dy).sqrt();
        if length < 1e-3 || opacity <= 0.0 || self.out.len() >= MOST_DABS {
            return;
        }
        let long = length.max(width);
        self.out.push(Dab {
            x: (a.0 + b.0) * 0.5,
            y: (a.1 + b.1) * 0.5,
            diameter: long,
            angle: dy.atan2(dx),
            ratio: (width / long).clamp(0.01, 1.0),
            opacity,
            flow: 1.0,
            tip: 0,
            flags: if colour.is_some() { OWN_COLOUR } else { 0 },
            colour: colour.unwrap_or([0.0; 3]),
            strength: 1.0,
            texture_offset: self.texture_offset,
            ..Dab::default()
        });
    }

    /// A sketch brush's move from `last` to `p`: a line between them if it says, and a line from
    /// `p` towards each point it passed within its reach, as likely as its density says.
    fn sketch_move(&mut self, last: &Point, p: &Point) {
        let brush = Rc::clone(&self.brush);
        let Some(sketch) = &brush.sketch else { return };
        self.points.push((p.x, p.y));
        let over = self.points.len().saturating_sub(MOST_SKETCH_POINTS);
        self.points.drain(..over);
        let info = self.info(p, self.dabs);
        let seed = self.begin.seed;
        let k = self.size_factor();
        let mut fuzz = Fuzz { random: &mut self.random, seed };
        let one = |o: &CurveOption, fuzz: &mut Fuzz<'_>| if o.checked { o.size_like(&info, fuzz, true) } else { 1.0 };
        let scale = one(&brush.dynamics.size, &mut fuzz) * k;
        if scale * brush.tip.width <= 0.01 {
            return;
        }
        let width = (one(&sketch.line_width_option, &mut fuzz) * sketch.line_width).max(0.9);
        let offset = one(&sketch.offset_option, &mut fuzz) * sketch.offset * 0.01;
        let density = one(&sketch.density_option, &mut fuzz) * sketch.probability;
        let opacity = if brush.dynamics.opacity.checked {
            brush.dynamics.opacity.size_like(&info, &mut fuzz, true)
        } else {
            brush.dynamics.opacity.strength
        };
        // its reach: half the tip across, at its size
        let reach = brush.sketch_radius * scale;
        let threshold = reach * reach;
        let points = self.points.clone();
        if sketch.connect {
            self.line((last.x, last.y), (p.x, p.y), width, opacity, None);
        }
        let mut chance = 1.0 - density;
        for (qx, qy) in points {
            let (dx, dy) = (qx - p.x, qy - p.y);
            let distance = dx * dx + dy * dy;
            if distance >= threshold {
                continue;
            }
            if sketch.distance_density {
                chance = distance / (threshold * density).max(1e-6);
            }
            if self.random.next() < chance {
                continue;
            }
            let colour = sketch.random_rgb.then(|| {
                let c = self.begin.colour;
                [self.random.next() * c[0], self.random.next() * c[1], self.random.next() * c[2]]
            });
            let mut alpha = opacity;
            if sketch.distance_opacity {
                alpha *= (1.0 - distance / threshold).round();
            }
            if sketch.random_opacity {
                alpha *= self.random.next();
            }
            let (ox, oy) = (dx * offset, dy * offset);
            let to = if sketch.magnetify { (qx - ox, qy - oy) } else { (p.x - ox, p.y - oy) };
            self.line((p.x + ox, p.y + oy), to, width, alpha, colour);
        }
        self.dabs += 1;
    }

    /// A spray brush's dab at `p`: its particles.
    fn spray_at(&mut self, p: &Point, spray: &crate::others::Spray) {
        let brush = Rc::clone(&self.brush);
        let info = self.info(p, self.main.dabs);
        let seed = self.begin.seed;
        let k = self.size_factor();
        let mut fuzz = Fuzz { random: &mut self.random, seed };
        let one = |o: &CurveOption, fuzz: &mut Fuzz<'_>| if o.checked { o.size_like(&info, fuzz, true) } else { 1.0 };
        let scale = one(&brush.dynamics.size, &mut fuzz) * k;
        let rotation = if brush.dynamics.rotation.checked {
            (1.0 - brush.dynamics.rotation.rotation_like(&info, &mut fuzz, 0.0, -1.0)) * PI
        } else {
            0.0
        };
        let opacity = if brush.dynamics.opacity.checked {
            brush.dynamics.opacity.size_like(&info, &mut fuzz, true)
        } else {
            brush.dynamics.opacity.strength
        };
        let radius = spray.diameter * 0.5 * scale;
        let (mut x, mut y) = (p.x, p.y);
        if let Some(amount) = spray.jitter {
            x += (2.0 * radius * self.random.next() - radius) * amount;
            y += (2.0 * radius * self.random.next() - radius) * amount;
        }
        let count = match spray.coverage {
            Some(c) => c * PI * radius * radius,
            None => spray.count,
        };
        let count = (count.max(0.0) as usize).min(MOST_DABS.saturating_sub(self.out.len()));
        let (turn_sin, turn_cos) = (spray.rotation.to_radians() - rotation).sin_cos();
        let (pw, ph) = spray.particle(spray.diameter);
        // particles of the brush's own tip: at its size, turned as it is, a pipe's cell picked
        // as its dabs pick them
        let own = spray.own_tip.then(|| {
            let tip = &brush.tip;
            let index = match &tip.tips {
                Tips::Auto { levels, .. } => levels.saturating_sub(1),
                Tips::Images { count, pipe: Some(pick) } => {
                    pipe_cell(pick, &mut self.pipe, &info, *count, self.main.dabs, &mut self.random)
                }
                Tips::Images { .. } => 0,
            };
            (tip.width, tip.height, tip.angle, index)
        });
        let mut colour = None;
        for _ in 0..count {
            let angle = self.random.next() * TAU;
            let length = if spray.gaussian {
                let n = gaussian(&mut self.random) * spray.deviation;
                if spray.centre_biased { n.abs().min(1.0) } else { n.abs().min(1.0).sqrt() }
            } else if spray.centre_biased {
                self.random.next()
            } else {
                self.random.next().sqrt()
            };
            let mut turn = spray.turn.to_radians();
            if let Some(w) = spray.random_turn {
                turn += self.random.next() * TAU * w;
            }
            if let Some(w) = spray.follow_cursor {
                turn += (angle - turn) * w;
            }
            if let Some(w) = spray.follow_angle {
                turn += (info.drawing_angle - turn) * w;
            }
            let size = if spray.random_size { self.random.next() } else { 1.0 };
            let (nx, ny) = (radius * angle.cos() * length, radius * angle.sin() * length * spray.aspect);
            let (nx, ny) =
                ((nx * turn_cos - ny * turn_sin) * spray.scale, (nx * turn_sin + ny * turn_cos) * spray.scale);
            // its colour: the colour moved at random, once or for each particle
            if colour.is_none() || spray.per_particle {
                let mut c = self.begin.colour;
                if spray.mix_background {
                    let w = p.pressure;
                    c = [0, 1, 2].map(|j| c[j] * w + self.begin.secondary[j] * (1.0 - w));
                }
                if let Some((h, s, v)) = spray.random_hsv {
                    let [mut ch, mut cs, mut cv] = hsv(c);
                    ch = (ch + h / 360.0 * self.random.next()).rem_euclid(1.0);
                    cs = (cs + s / 100.0 * self.random.next()).clamp(0.0, 1.0);
                    cv = (cv + v / 100.0 * self.random.next()).clamp(0.0, 1.0);
                    c = rgb([ch, cs, cv]);
                }
                colour = Some(c);
            }
            let alpha = if spray.random_opacity { opacity * self.random.next() } else { opacity };
            let (w, h) = match (own, spray.shape) {
                (Some((tw, th, ..)), _) => ((tw * size).max(1.0), (th * size).max(1.0)),
                (None, 2 | 3) => (1.0, 1.0),
                (None, _) => ((pw * size).max(1.0), (ph * size).max(1.0)),
            };
            let long = w.max(h);
            self.out.push(Dab {
                x: x + nx,
                y: y + ny,
                diameter: long,
                angle: -turn - own.map_or(0.0, |o| o.2),
                ratio: (w.min(h) / long).clamp(0.01, 1.0),
                opacity: alpha,
                flow: 1.0,
                // an ellipse the round tip, the rest the square, or the brush's own
                tip: own.map_or(u32::from(spray.shape != 0), |o| o.3),
                flags: OWN_COLOUR,
                colour: colour.unwrap_or(self.begin.colour),
                strength: 1.0,
                texture_offset: self.texture_offset,
                ..Dab::default()
            });
        }
        // the next dab a share of its diameter on
        let step = if spray.diameter * 0.5 > 1.0 { spray.diameter * 0.5 * spray.spacing } else { 1.0 };
        self.main.spacing = (step.max(LEAST_SPACING), step.max(LEAST_SPACING));
        self.main.turn = 0.0;
        self.main.dabs += 1;
        self.main.last_ms = p.time;
        self.dabs += 1;
    }

    /// The pen where it lies, for the options.
    fn info(&self, p: &Point, dabs: u32) -> Info {
        Info {
            pressure: p.pressure,
            x_tilt: p.x_tilt,
            y_tilt: p.y_tilt,
            rotation: p.rotation,
            speed: p.speed,
            drawing_angle: self.angle,
            distance: self.length,
            time: p.time,
            dab: dabs,
        }
    }

    /// A dab of the main brush, or its masking brush, at `p`.
    fn dab_at(&mut self, p: &Point, second: bool) {
        let brush = Rc::clone(&self.brush);
        let k = self.size_factor();
        // a spray brush lays its particles there instead
        if let (Some(spray), false) = (&brush.spray, second) {
            self.spray_at(p, spray);
            return;
        }
        let (tip, dynamics, coeff) = match (second, &brush.masking) {
            (true, Some(m)) => (&m.tip, &m.dynamics, Some(m.size)),
            _ => (&brush.tip, &brush.dynamics, None),
        };
        let walker_dabs = if second { self.second.dabs } else { self.main.dabs };
        let info = self.info(p, walker_dabs);
        let seed = self.begin.seed;
        let mut fuzz = Fuzz { random: &mut self.random, seed };
        let one = |o: &CurveOption, fuzz: &mut Fuzz<'_>| if o.checked { o.size_like(&info, fuzz, true) } else { 1.0 };
        // its size: a masking brush's is a share of the main brush's own
        let mut scale = one(&dynamics.size, &mut fuzz) * k;
        let (mut w, mut h) = (tip.width, tip.height);
        if let Some(c) = coeff.filter(|&c| c > 0.0) {
            let main_w = brush.tip.width.max(1e-3);
            let to = main_w * c / tip.width.max(1e-3);
            w *= to;
            h *= to;
        }
        if scale * w < 0.01 || scale * h < 0.01 {
            scale = 0.0;
        }
        let rotation = if dynamics.rotation.checked {
            let v = 1.0 - dynamics.rotation.rotation_like(&info, &mut fuzz, 0.0, -1.0);
            v * PI
        } else {
            0.0
        };
        let turn = rotation + tip.angle;
        let ratio = one(&dynamics.ratio, &mut fuzz).clamp(0.01, 1.0);
        let (dw, dh) = (w * scale, h * scale * ratio);
        // scattered from the path
        let (mut x, mut y) = (p.x, p.y);
        if dynamics.scatter.checked && (dynamics.scatter_axes.0 || dynamics.scatter_axes.1) && scale > 0.0 {
            let reach = dw.max(dh) * dynamics.scatter.size_like(&info, &mut fuzz, true);
            let jitter = (2.0 * fuzz.random.next() - 1.0) * reach;
            if dynamics.scatter_axes.0 && dynamics.scatter_axes.1 {
                let jitter_y = (2.0 * fuzz.random.next() - 1.0) * reach;
                x += jitter;
                y += jitter_y;
            } else {
                let (s, c) = self.angle.sin_cos();
                let (ax, ay) = if dynamics.scatter_axes.0 { (c, s) } else { (-s, c) };
                x += ax * jitter;
                y += ay * jitter;
            }
        }
        let mut flags = if second { FOR_SECOND } else { 0 };
        if dynamics.mirror.checked && (dynamics.mirror_axes.0 || dynamics.mirror_axes.1) {
            let flip = dynamics.mirror.size_like(&info, &mut fuzz, true) >= 0.5;
            if flip && dynamics.mirror_axes.0 {
                flags |= FLIP_ACROSS;
            }
            if flip && dynamics.mirror_axes.1 {
                flags |= FLIP_DOWN;
            }
        }
        let wash = brush.wash || second;
        let opacity = if dynamics.opacity.checked {
            dynamics.opacity.size_like(&info, &mut fuzz, !wash) * if wash { dynamics.opacity.strength } else { 1.0 }
        } else {
            dynamics.opacity.strength
        };
        let flow = if dynamics.flow.checked {
            dynamics.flow.size_like(&info, &mut fuzz, true)
        } else {
            dynamics.flow.strength
        };
        let softness =
            (dynamics.softness.varies() && !second).then(|| dynamics.softness.size_like(&info, &mut fuzz, true));
        let mut dab = Dab {
            x,
            y,
            diameter: w.max(h) * scale,
            // Krita turns its dabs the other way round
            angle: -turn,
            ratio,
            opacity,
            flow,
            tip: 0,
            flags,
            colour: [0.0; 3],
            strength: 1.0,
            texture_offset: self.texture_offset,
            lightness: 1.0,
            threshold: 0.0,
            softness: 0.0,
            paint_share: 0.0,
            radius: 0.0,
        };
        // its tip: a pipe's cell, or an auto tip's drawing (noise afresh, softness as it says)
        dab.tip = match &tip.tips {
            Tips::Auto { variants, levels, .. } => {
                let v = if *variants > 1 { (fuzz.random.next() * *variants as f32) as u32 % variants } else { 0 };
                let level = softness.map_or(levels - 1, |s| (((s - 0.1) / 0.9) * (*levels - 1) as f32).round() as u32);
                v * levels + level.min(levels - 1)
            }
            Tips::Images { count, pipe: Some(pick) } => {
                let state = if second { &mut self.second_pipe } else { &mut self.pipe };
                pipe_cell(pick, state, &info, *count, walker_dabs, fuzz.random)
            }
            Tips::Images { .. } => 0,
        };
        if !second {
            // washed: the stroke keeps near the most its opacity has been lately
            if wash {
                let blend =
                    if self.average < dab.opacity { dab.opacity } else { 0.1 * dab.opacity + 0.9 * self.average };
                self.average = blend;
                if blend > dab.opacity && blend > 0.0 {
                    dab.flow *= dab.opacity / blend;
                    dab.opacity = blend;
                }
            }
            if let Some(t) = &brush.texture {
                dab.strength = t.strength.apply(&info, &mut fuzz).clamp(0.0, 1.0);
            }
            if brush.lightness.checked {
                dab.lightness = brush.lightness.size_like(&info, &mut fuzz, true).clamp(0.0, 1.0);
            }
            if let Some(smudge) = &brush.smudge {
                let one = |o: &CurveOption, otherwise: f32, fuzz: &mut Fuzz<'_>| {
                    if o.checked { o.size_like(&info, fuzz, true) } else { otherwise }
                };
                let rate = one(&smudge.rate, 1.0, &mut fuzz);
                let colour_rate = one(&smudge.colour_rate, 0.0, &mut fuzz);
                let (laid, share) = smudge.laid(rate, colour_rate, dab.opacity);
                // the first dab only marks where the next picks up from
                dab.opacity = if self.dabs == 0 { 0.0 } else { laid };
                dab.flow = 1.0;
                dab.paint_share = share;
                dab.radius = one(&smudge.radius, 0.0, &mut fuzz).max(0.0);
                dab.flags |= MOVES_PAINT;
                if smudge.dulls {
                    dab.flags |= DULLS;
                }
                if smudge.newer {
                    dab.flags |= WEIGHED;
                }
                // Krita mixes the colour in by the brush's composite op: an eraser's takes paint away
                if brush.erases {
                    dab.flags |= CLEAR_PAINT;
                }
                if smudge.gradient.checked {
                    let g = smudge.gradient.size_like(&info, &mut fuzz, true).clamp(0.0, 1.0);
                    let from = if dab.flags & OWN_COLOUR != 0 { dab.colour } else { self.begin.colour };
                    dab.colour = [0, 1, 2].map(|i| from[i] + (self.begin.secondary[i] - from[i]) * g);
                    dab.flags |= OWN_COLOUR;
                }
            }
            let sharp = &brush.sharpness;
            if sharp.option.checked {
                let value = sharp.option.size_like(&info, &mut fuzz, true).clamp(0.0, 1.0);
                // (a threshold of nothing would leave nothing: the app takes 0 as none)
                dab.threshold = value.max(1e-6);
                dab.softness = sharp.softness;
                if sharp.align && sharp.option.strength > 0.0 {
                    // its corner moved towards a whole pixel as far as the value goes
                    let snap = |c: f32, side: f32| {
                        let corner = c - side * 0.5;
                        value * corner.round() + (1.0 - value) * corner + side * 0.5
                    };
                    let side = dab.diameter.round().max(1.0);
                    dab.x = snap(dab.x, side);
                    dab.y = snap(dab.y, side);
                }
            }
            if let Some(colour) = colour_of(&brush, &self.begin, &info, &mut fuzz) {
                dab.colour = colour;
                dab.flags |= OWN_COLOUR;
            }
            // a tangent normal brush's colour: the pen's lean as a normal
            if let Some(t) = &brush.tangent {
                dab.colour = t.colour(&info, self.begin.turn);
                dab.flags |= OWN_COLOUR;
            }
        }
        if scale > 0.0 && dab.diameter.is_finite() {
            self.out.push(dab);
        }
        // the spacing this dab sets for the next
        let extra = one(&dynamics.spacing, &mut Fuzz { random: &mut self.random, seed });
        let spacing = spacing_of(tip, dw.max(1e-3), dh.max(1e-3), extra, dynamics.isotropic);
        let walker = if second { &mut self.second } else { &mut self.main };
        walker.spacing = spacing;
        walker.turn = if dynamics.isotropic { 0.0 } else { turn };
        walker.dabs += 1;
        walker.last_ms = p.time;
        if !second {
            self.dabs += 1;
        }
    }
}

/// A dab's colour where its options change it: mixed with the background colour, darkened, its
/// hue, saturation and value moved; none where none does.
fn colour_of(b: &Brush, begin: &Begin, info: &Info, fuzz: &mut Fuzz<'_>) -> Option<[f32; 3]> {
    {
        if !(b.mix.checked || b.darken.checked || b.hue.checked || b.saturation.checked || b.value.checked) {
            return None;
        }
        let mut c = begin.colour;
        if b.mix.checked {
            let v = b.mix.size_like(info, fuzz, true);
            for (i, ch) in c.iter_mut().enumerate() {
                *ch = *ch * v + begin.secondary[i] * (1.0 - v);
            }
        }
        if b.darken.checked {
            let v = b.darken.size_like(info, fuzz, true);
            for ch in &mut c {
                *ch *= 1.0 - v;
            }
        }
        if b.hue.checked || b.saturation.checked || b.value.checked {
            let [mut h, mut s, mut v] = hsv(c);
            if b.hue.checked {
                h = (h + 0.5 * b.hue.rotation_like(info, fuzz, 0.0, 1.0)).rem_euclid(1.0);
            }
            let moved = |o: &CurveOption, x: f32, fuzz: &mut Fuzz<'_>| {
                let raw = o.size_like(info, fuzz, true);
                let adj = 2.0 * (raw * o.strength + 0.5 - 0.5 * o.strength) - 1.0;
                if adj >= 0.0 { x + (1.0 - x) * adj } else { x * (1.0 + adj) }
            };
            if b.saturation.checked {
                s = moved(&b.saturation, s, fuzz);
            }
            if b.value.checked {
                v = moved(&b.value, v, fuzz);
            }
            c = rgb([h, s.clamp(0.0, 1.0), v.clamp(0.0, 1.0)]);
        }
        Some(c.map(|v| v.clamp(0.0, 1.0)))
    }
}

/// The spacing a dab `w` x `h` sets for the next: across and down its own frame (or the same both
/// ways), automatic (a share of the square root of its size) or a share of its size, scaled by
/// the spacing option.
fn spacing_of(tip: &Tip, w: f32, h: f32, extra: f32, isotropic: bool) -> (f32, f32) {
    let auto = |v: f32, coeff: f32| coeff * if v < 1.0 { v } else { v.sqrt() };
    let (sx, sy) = if isotropic {
        let d = w.max(h);
        let s = match tip.auto_spacing {
            Some(coeff) => auto(d, coeff),
            None => d * tip.spacing,
        };
        (s, s)
    } else {
        match tip.auto_spacing {
            Some(coeff) => (auto(w, coeff), auto(h, coeff)),
            None => (w * tip.spacing, h * tip.spacing),
        }
    };
    (sx * extra, sy * extra)
}

/// The pipe's cell for a dab, each dimension picked as it says; those that go on by themselves
/// (in turn, at random) step on after.
fn pipe_cell(pick: &PipePick, state: &mut PipeState, info: &Info, count: u32, seq: u32, random: &mut Random) -> u32 {
    let mut index = 0_u32;
    let mut stride = 1_u32;
    // the last dimension steps fastest
    let n = pick.ranks.len();
    let mut strides = vec![1_u32; n];
    for i in (0..n).rev() {
        strides[i] = stride;
        stride = stride.saturating_mul(pick.ranks[i].max(1));
    }
    for (i, (&rank, sel)) in pick.ranks.iter().zip(&pick.selections).enumerate() {
        let rank = rank.max(1);
        let current = state.steps.get(i).copied().unwrap_or(0);
        let step = match sel {
            Selection::Pressure => (info.pressure * (rank - 1) as f32 + 0.5) as u32,
            Selection::Angular => {
                let a = (info.drawing_angle + FRAC_PI_2 + FRAC_PI_4).rem_euclid(TAU);
                (a / TAU * rank as f32) as u32
            }
            Selection::TiltX => ((info.x_tilt / 2.0 * rank as f32).round() as i64 + i64::from(rank / 2)).max(0) as u32,
            Selection::TiltY => ((info.y_tilt / 2.0 * rank as f32).round() as i64 + i64::from(rank / 2)).max(0) as u32,
            Selection::Velocity => {
                let v = ((info.speed + 1.0).ln()).min(3.0) / 3.0;
                (v * ((rank - 1) as f32 + 0.5)).round() as u32
            }
            Selection::Constant | Selection::Incremental | Selection::Random => current,
        };
        index = index.wrapping_add(strides[i].wrapping_mul(step));
        // on to the next for those that go by themselves
        let next = match sel {
            Selection::Incremental => (seq + 1) % rank,
            Selection::Random => (random.next() * rank as f32) as u32 % rank,
            _ => current,
        };
        if let Some(s) = state.steps.get_mut(i) {
            *s = next;
        }
    }
    index % count.max(1)
}

/// The pen's lean across and down (degrees, -60..60) from its lean from upright and the way it
/// leans (clockwise from up).
fn lean(tilt: f32, azimuth: f32) -> (f32, f32) {
    if !(tilt.is_finite() && azimuth.is_finite()) || tilt <= 0.0 {
        return (0.0, 0.0);
    }
    let t = tilt.min(FRAC_PI_2 - 1e-3).tan();
    let x = (t * azimuth.sin()).atan().to_degrees();
    let y = (-t * azimuth.cos()).atan().to_degrees();
    (x.clamp(-60.0, 60.0), y.clamp(-60.0, 60.0))
}

fn hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let hi = r.max(g).max(b);
    let d = hi - r.min(g).min(b);
    if d <= 1e-6 {
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

fn rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let f = |n: f32| {
        let k = (n + h * 6.0).rem_euclid(6.0);
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [f(5.0), f(3.0), f(1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_walk_lays_a_dab_each_spacing() {
        let mut w = Walker { spacing: (10.0, 10.0), ..Walker::default() };
        // 25 pixels: one at 10, one at 20, 5 left over
        let mut at = (0.0, 0.0);
        let mut laid = Vec::new();
        while let Some(t) = w.next(at, (25.0, 0.0)) {
            at = (at.0 + (25.0 - at.0) * t, 0.0);
            laid.push(at.0);
        }
        assert_eq!(laid.len(), 2);
        assert!((laid[0] - 10.0).abs() < 1e-3 && (laid[1] - 20.0).abs() < 1e-3);
        assert!((w.gone.0 - 5.0).abs() < 1e-3);
        // an ellipse twice as long across as down: a move down reaches it in half the way
        let mut flat = Walker { spacing: (10.0, 5.0), ..Walker::default() };
        let t = flat.next((0.0, 0.0), (0.0, 20.0)).unwrap();
        assert!((t - 0.25).abs() < 1e-3);
    }

    #[test]
    fn colours_go_round_and_back() {
        let c = [0.2, 0.6, 0.4];
        let back = rgb(hsv(c));
        assert!(c.iter().zip(back).all(|(a, b)| (a - b).abs() < 1e-4));
        let (x, y) = lean(std::f32::consts::FRAC_PI_4, FRAC_PI_2);
        assert!((x - 45.0).abs() < 1e-3 && y.abs() < 1e-3);
    }
}
