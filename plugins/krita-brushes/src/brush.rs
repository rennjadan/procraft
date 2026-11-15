//! A Krita pixel-brush preset made ready to draw: its tip or tips, its spacing, its options, how
//! it paints (washed or built up, its blend), its texture and its masking brush; and how it looks to
//! the app (its tips as grey pictures, the rest as the app's own numbers).

use crate::files::{Rgba, png_decode, png_size};
use crate::gimp::{self, Mask, Selection};
use crate::options::CurveOption;
use crate::preset::Preset;
use crate::rec::Record;
use crate::tips::AutoTip;
use crate::xml::Element;

/// How a tip set's pictures are picked from.
#[derive(Clone, Debug)]
pub enum Tips {
    /// An auto tip: `variants` drawings of it (holes and noise drawn afresh) at each of `levels`
    /// softnesses, by variant then level.
    Auto { variants: u32, levels: u32 },
    /// Pictures: one, or a pipe's cells and how it picks among them.
    Images { count: u32, pipe: Option<PipePick> },
}

#[derive(Clone, Debug)]
pub struct PipePick {
    pub ranks: Vec<u32>,
    pub selections: Vec<Selection>,
}

/// What a tip's pictures are to the app (its tip kinds), by how Krita applies a picture: how much
/// each pixel paints (a grey byte each), or RGBA pictures painting their own colours, their grey
/// the colour's lightness, or their grey a place along the gradient from the colour to the
/// secondary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Mask = 0,
    Stamp = 1,
    Lightness = 2,
    Gradient = 3,
}

/// A tip as a brush (or its masking brush) has it.
#[derive(Clone, Debug)]
pub struct Tip {
    pub tips: Tips,
    pub kind: Kind,
    /// Its own width and height (canvas pixels at size 1), its turn (radians) and its spacing.
    pub width: f32,
    pub height: f32,
    pub angle: f32,
    pub spacing: f32,
    pub auto_spacing: Option<f32>,
    /// Its pictures, for the app.
    pub masks: Vec<Mask>,
}

/// Options a brush and its masking brush both have.
#[derive(Clone, Debug)]
pub struct Dynamics {
    pub size: CurveOption,
    pub opacity: CurveOption,
    pub flow: CurveOption,
    pub rotation: CurveOption,
    pub ratio: CurveOption,
    pub spacing: CurveOption,
    pub scatter: CurveOption,
    pub mirror: CurveOption,
    pub softness: CurveOption,
    pub scatter_axes: (bool, bool),
    pub mirror_axes: (bool, bool),
    pub isotropic: bool,
}

impl Dynamics {
    fn read(p: &Preset, prefix: &str) -> Dynamics {
        let masked = !prefix.is_empty();
        Dynamics {
            size: CurveOption::read(p, prefix, "Size", true, 0.0, 1.0),
            // opacity and flow can be switched off only on a masking brush
            opacity: CurveOption::read(p, prefix, "Opacity", masked, 0.0, 1.0),
            flow: CurveOption::read(p, prefix, "Flow", masked, 0.0, 1.0),
            rotation: CurveOption::read(p, prefix, "Rotation", true, 0.0, 1.0),
            ratio: CurveOption::read(p, prefix, "Ratio", true, 0.0, 1.0),
            spacing: CurveOption::read(p, prefix, "Spacing", true, 0.0, 5.0),
            scatter: CurveOption::read(p, prefix, "Scatter", true, 0.0, 5.0),
            mirror: CurveOption::read(p, prefix, "Mirror", true, 0.0, 1.0),
            softness: CurveOption::read(p, prefix, "Softness", true, 0.1, 1.0),
            scatter_axes: (
                p.flag(&format!("{prefix}Scattering/AxisX"), true),
                p.flag(&format!("{prefix}Scattering/AxisY"), true),
            ),
            mirror_axes: (
                p.flag(&format!("{prefix}HorizontalMirrorEnabled"), false),
                p.flag(&format!("{prefix}VerticalMirrorEnabled"), false),
            ),
            isotropic: p.flag(&format!("{prefix}Spacing/Isotropic"), false),
        }
    }
}

/// A texture as Krita makes its mask of the pattern, and how it combines with each dab.
#[derive(Clone, Debug)]
pub struct Texture {
    pub mask: Mask,
    pub scale: f32,
    pub rule: u32,
    pub offset: (f32, f32),
    pub random_offset: (bool, bool),
    pub strength: CurveOption,
}

#[derive(Clone, Debug)]
pub struct Masking {
    pub tip: Tip,
    pub dynamics: Dynamics,
    /// Its size for each 1 of the main brush's own, and how it combines (the app's numbers).
    pub size: f32,
    pub combine: u32,
}

/// Krita's sharpness option: each dab's mask made hard at a threshold (the option's value), what
/// lies within `softness` of it below kept as it is; `align`: its dabs moved onto whole pixels as
/// far as the value goes.
#[derive(Clone, Debug)]
pub struct Sharpness {
    pub option: CurveOption,
    pub softness: f32,
    pub align: bool,
}

impl Sharpness {
    fn read(p: &Preset) -> Sharpness {
        let mut option = CurveOption::read(p, "", "Sharpness", true, 0.0, 1.0);
        let mut softness = p.number("Sharpness/softness", 0.0);
        // older presets' one number for both
        if let (Some(factor), None) = (p.get("Sharpness/factor"), p.get("SharpnessValue")) {
            let factor = factor.trim().parse::<f32>().unwrap_or(0.5);
            option.strength = factor.clamp(0.0, 1.0);
            softness = (factor * 100.0).trunc();
        }
        Sharpness {
            option,
            softness: (softness / 100.0).clamp(0.0, 1.0),
            align: p.flag("Sharpness/alignoutline", false),
        }
    }
}

/// Krita's colour smudge engine: each dab moves the paint under where the last lay to where it
/// lies (smearing), or that paint's one colour (dulling, picked from a share of it, weighed by the
/// dab's mask in the newer engine), mixing in the colour at its colour rate; its smudge rate how far
/// it lays that. A gradient option moves the colour towards the secondary colour.
#[derive(Clone, Debug)]
pub struct Smudge {
    pub rate: CurveOption,
    pub colour_rate: CurveOption,
    pub radius: CurveOption,
    pub gradient: CurveOption,
    pub dulls: bool,
    pub newer: bool,
    pub smear_alpha: bool,
}

impl Smudge {
    fn read(p: &Preset) -> Smudge {
        let newer = p.flag("SmudgeRateUseNewEngine", false);
        let mut radius = CurveOption::read(p, "", "SmudgeRadius", true, 0.0, 3.0);
        // older presets kept it in hundredths
        let kept = p.number("SmudgeRadiusValue", 0.0);
        let kept = if p.number("SmudgeRadiusVersion", 1.0) < 2.0 { kept / 100.0 } else { kept };
        radius.strength = kept.clamp(0.0, if newer { 1.0 } else { 3.0 });
        Smudge {
            rate: CurveOption::read(p, "", "SmudgeRate", true, 0.0, 1.0),
            colour_rate: CurveOption::read(p, "", "ColorRate", true, 0.0, 1.0),
            radius,
            gradient: CurveOption::read(p, "", "Gradient", true, 0.0, 1.0),
            dulls: p.number("SmudgeRateMode", 0.0) as u32 == 1,
            newer,
            smear_alpha: p.flag("SmudgeRateSmearAlpha", true),
        }
    }

    /// How far a dab lays what it moves, and what share of that is the colour, for its smudge rate,
    /// colour rate and opacity: Krita's two engines' sums, taken as one mix laid at one strength.
    pub fn laid(&self, rate: f32, colour_rate: f32, opacity: f32) -> (f32, f32) {
        if !self.newer {
            let most = (1.0 - self.rate.strength).max(0.2);
            return ((rate * opacity).clamp(0.0, 1.0), (most * colour_rate * opacity).clamp(0.0, 1.0));
        }
        let colour = (colour_rate * colour_rate * opacity).clamp(0.0, 1.0);
        let moved = (if self.dulls { 0.8 } else { 1.0 } * rate * opacity).clamp(0.0, 1.0);
        if self.dulls && !self.smear_alpha && colour > 0.0 {
            // the colour mixed into the dulled one first, that laid as far as the dab moves paint
            return (moved, colour);
        }
        let laid = 1.0 - (1.0 - colour) * (1.0 - moved);
        (laid, if laid > 0.0 { colour / laid } else { 0.0 })
    }
}

/// Krita's airbrush: a dab every `interval` milliseconds (scaled by the rate option) as well as by
/// its spacing, or only by time.
#[derive(Clone, Debug)]
pub struct Airbrush {
    pub interval: f32,
    pub ignore_spacing: bool,
    pub rate: CurveOption,
}

impl Airbrush {
    fn read(p: &Preset) -> Option<Airbrush> {
        if !p.flag("PaintOpSettings/isAirbrushing", false) {
            return None;
        }
        let rate = p.number("PaintOpSettings/rate", 20.0);
        Some(Airbrush {
            interval: if rate > 0.0 { 1000.0 / rate } else { 1.0e9 },
            ignore_spacing: p.flag("PaintOpSettings/ignoreSpacing", false),
            rate: CurveOption::read(p, "", "Rate", true, 0.0, 1.0),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Brush {
    pub tip: Tip,
    pub dynamics: Dynamics,
    pub airbrush: Option<Airbrush>,
    /// Its colour smudge, if it is one.
    pub smudge: Option<Smudge>,
    /// Krita's other engines it may be (see `others.rs`), and a sketch brush's reach (half its tip
    /// across, at size 1).
    pub sketch: Option<crate::others::Sketch>,
    pub spray: Option<crate::others::Spray>,
    pub tangent: Option<crate::others::Tangent>,
    pub sketch_radius: f32,
    /// How strongly a lightness tip's grey shows, and its sharpness.
    pub lightness: CurveOption,
    pub sharpness: Sharpness,
    pub darken: CurveOption,
    pub mix: CurveOption,
    pub hue: CurveOption,
    pub saturation: CurveOption,
    pub value: CurveOption,
    pub wash: bool,
    pub blend: Option<u32>,
    pub erases: bool,
    pub texture: Option<Texture>,
    pub masking: Option<Masking>,
}

/// Krita's engines this one draws: its pixel brush, colour smudge, quick brush, tangent normal,
/// sketch and spray brushes.
pub const ENGINES: [&str; 6] =
    ["paintbrush", "colorsmudge", "roundmarker", "tangentnormal", "sketchbrush", "spraybrush"];

/// Krita's composite ops as the app's blend modes number them.
pub fn blend_number(op: &str) -> Option<u32> {
    Some(match op {
        "normal" | "over" => 0,
        "multiply" => 1,
        "darken" => 2,
        "burn" => 4,
        "linear_burn" => 5,
        "darker color" => 6,
        "lighten" => 7,
        "screen" => 8,
        "dodge" => 9,
        "add" | "linear_dodge" => 10,
        "lighter color" => 11,
        "overlay" => 12,
        "soft_light" | "soft_light_svg" | "soft_light_pegtop_delphi" | "soft_light_ifs_illusions" => 13,
        "hard_light" => 14,
        "vivid_light" => 15,
        "linear light" => 16,
        "pin_light" => 17,
        "hard_mix" | "hard_mix_photoshop" | "hard_mix_softer_photoshop" => 18,
        "diff" => 19,
        "exclusion" => 20,
        "subtract" => 21,
        "divide" => 22,
        "hue" => 23,
        "saturation" => 24,
        "color" => 25,
        "luminize" => 26,
        "behind" => 29,
        _ => return None,
    })
}

/// A masking brush's op as the app's dual brushes combine two strokes: a blend mode's number, or
/// its two heights.
fn combine_number(op: &str) -> u32 {
    match op {
        "height" | "height_photoshop" => 27,
        "linear_height" | "linear_height_photoshop" => 28,
        other => blend_number(other).unwrap_or(1),
    }
}

/// Krita's texturing modes as the app's texture rules number them, soft ones 16 on.
fn texture_rule(mode: u32, soft: bool) -> u32 {
    let rule = match mode {
        0 => 32,
        4 => 34,
        5 => 35,
        6 => 36,
        7 => 37,
        8 => 38,
        9 => 39,
        10 => 40,
        11 => 41,
        12 => 42,
        13 => 43,
        14 => 44,
        15 => 45,
        // lightness and gradient colour the dab, softly or not
        2 => return 46,
        3 => return 47,
        _ => 33,
    };
    if soft { rule + 16 } else { rule }
}

/// A picture file's pictures as Krita takes them (RGBA: see [`gimp::gbr`]): a GIMP brush or pipe's
/// cells, or a PNG; whether they are colour pictures (not masks: a PNG with colour or see-through
/// pixels, a colour GIMP brush) and whether they hold colour (not all grey); a pipe's picking.
/// `light`: only their sizes, no pixels.
struct Pictures {
    cells: Vec<Rgba>,
    image: bool,
    colourful: bool,
    pipe: Option<PipePick>,
}

fn pictures(bytes: &[u8], light: bool) -> Option<Pictures> {
    if bytes.starts_with(b"\x89PNG") {
        if light {
            let (width, height) = png_size(bytes)?;
            let cells = vec![Rgba { width, height, pixels: Vec::new() }];
            return Some(Pictures { cells, image: false, colourful: false, pipe: None });
        }
        let picture = png_decode(bytes)?;
        let grey = gimp::all_grey(&picture);
        let see_through = picture.pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 255);
        let image = !(grey && !see_through);
        return Some(Pictures { cells: vec![picture], image, colourful: !grey, pipe: None });
    }
    if let Some((cell, grey, _)) = gimp::gbr(bytes, !light) {
        let colourful = !grey && !light && !gimp::all_grey(&cell);
        return Some(Pictures { cells: vec![cell], image: !grey, colourful, pipe: None });
    }
    let pipe = gimp::gih(bytes, !light)?;
    let colourful = !pipe.grey && !light && pipe.cells.first().is_some_and(|c| !gimp::all_grey(c));
    let pick = PipePick { ranks: pipe.ranks, selections: pipe.selections };
    Some(Pictures { cells: pipe.cells, image: !pipe.grey, colourful, pipe: Some(pick) })
}

/// How Krita applies a picture tip, by what its `Brush` element says (and, for presets from before
/// it said, whether the picture holds colour).
fn application(brush: &Element, colourful: bool) -> Kind {
    let legacy = |as_mask: bool| if colourful && !as_mask { Kind::Stamp } else { Kind::Mask };
    let as_mask = |otherwise: bool| brush.attribute("ColorAsMask").map_or(otherwise, |v| v.trim() != "0");
    if let Some(lightness) = brush.attribute("preserveLightness") {
        return if lightness.trim().parse::<i32>().unwrap_or(0) != 0 { Kind::Lightness } else { legacy(as_mask(true)) };
    }
    if let Some(application) = brush.attribute("brushApplication") {
        return match application.trim().parse::<u32>().unwrap_or(0) {
            1 => Kind::Stamp,
            2 => Kind::Lightness,
            3 => Kind::Gradient,
            _ => Kind::Mask,
        };
    }
    legacy(as_mask(false))
}

/// Krita's lightness adjustments of a colour picture's grey (not stamped in its own colours): the
/// grey taken as its middle (none: the picture's average), and how much lighter and of more
/// contrast; older presets' read as Krita reads them.
struct Adjustment {
    mid: Option<f32>,
    brightness: f32,
    contrast: f32,
}

impl Adjustment {
    fn read(brush: &Element) -> Adjustment {
        let mut mid = brush.number("AdjustmentMidPoint").map_or(127.0, |m| m.trunc().clamp(0.0, 255.0));
        let mut brightness = brush.number("BrightnessAdjustment").unwrap_or(0.0);
        let mut contrast = brush.number("ContrastAdjustment").unwrap_or(0.0);
        let version = brush.number("AdjustmentVersion").unwrap_or(1.0);
        let auto = brush.attribute("AutoAdjustMidPoint");
        if version < 2.0 && auto.is_none() {
            // applied twice before: about as much again
            mid = (127.0 + (mid - 127.0) * 2.0).clamp(0.0, 255.0);
            brightness *= 2.0;
            contrast *= 2.0;
            if contrast < 0.0 {
                contrast = 1.0 / (1.0 - contrast) - 1.0;
            }
        }
        let auto = auto.is_some_and(|a| a.trim() == "1" || a.trim() == "true");
        Adjustment { mid: (!auto).then_some(mid), brightness, contrast }
    }

    /// Each pixel of `picture` as its adjusted grey and its alpha.
    fn apply(&self, picture: &Rgba) -> Vec<[u8; 2]> {
        let px = picture.pixels.as_chunks::<4>().0;
        let mid = self.mid.unwrap_or_else(|| {
            // the picture's average grey, by its opacity
            let (mut lit, mut alpha) = (0_i64, 0_i64);
            for p in px {
                lit += (f32::from(gimp::grey(p[0], p[1], p[2])) * f32::from(p[3]) / 255.0).round() as i64;
                alpha += i64::from(p[3]);
            }
            if alpha == 0 { 0.0 } else { 255.0 * lit as f32 / alpha as f32 }
        });
        let (b, c) = (self.brightness, self.contrast);
        let plain = (mid - 127.0).abs() <= 0.1 && b == 0.0 && c == 0.0;
        let mid_y = if b > 0.0 { 127.0 + 128.0 * b } else { 127.0 + 127.0 * b };
        let (mut lo_a, mut hi_a, mut lo_b, mut hi_b) = (0.0, 0.0, 0.0, 255.0);
        if c != 1.0 {
            if c > 0.0 {
                lo_a = mid_y / (1.0 - c) / mid;
                hi_a = (255.0 - mid_y) / (1.0 - c) / (255.0 - mid);
            } else {
                lo_a = mid_y * (1.0 + c) / mid;
                hi_a = (255.0 - mid_y) * (1.0 + c) / (255.0 - mid);
            }
            lo_b = mid_y - mid * lo_a;
            hi_b = mid_y - mid * hi_a;
        }
        px.iter()
            .map(|p| {
                let v = f32::from(gimp::grey(p[0], p[1], p[2]));
                let v = if plain {
                    v
                } else if v >= mid {
                    (hi_a * v + hi_b).round().min(255.0)
                } else {
                    (lo_a * v + lo_b).round().max(0.0)
                };
                [v.clamp(0.0, 255.0) as u8, p[3]]
            })
            .collect()
    }
}

/// A picture as the app takes it as a tip of `kind`: how much it paints (its grey, adjusted for a
/// colour picture, what is dark painting as far as it is opaque), or RGBA: its own colours, or its
/// grey (adjusted for a colour picture) and alpha.
fn tip_picture(picture: &Rgba, kind: Kind, image: bool, adjust: &Adjustment) -> Mask {
    let (width, height) = (picture.width, picture.height);
    let px = picture.pixels.as_chunks::<4>().0;
    let pixels = match (kind, image) {
        (Kind::Stamp, _) => picture.pixels.clone(),
        (Kind::Mask, true) => {
            adjust.apply(picture).iter().map(|[v, a]| ((255 - u32::from(*v)) * u32::from(*a) / 255) as u8).collect()
        }
        (Kind::Mask, false) => px.iter().map(|p| 255 - gimp::grey(p[0], p[1], p[2])).collect(),
        (_, true) => adjust.apply(picture).iter().flat_map(|&[v, a]| [v, v, v, a]).collect(),
        (_, false) => px
            .iter()
            .flat_map(|p| {
                let v = gimp::grey(p[0], p[1], p[2]);
                [v, v, v, 255]
            })
            .collect(),
    };
    Mask { width, height, pixels }
}

/// A round tip `diameter` across, for a preset whose picture can't be had.
fn stand_in(diameter: f32) -> Tip {
    let xml = format!(
        r#"<Brush type="auto_brush" spacing="0.1"><MaskGenerator type="circle" id="default" diameter="{diameter}" ratio="1" hfade="1" vfade="1" antialiasEdges="1"/></Brush>"#
    );
    let brush = crate::xml::parse(&xml).unwrap_or_default();
    read_tip(&brush, &|_| None, None, false, true).unwrap_or(Tip {
        tips: Tips::Images { count: 1, pipe: None },
        kind: Kind::Mask,
        width: diameter,
        height: diameter,
        angle: 0.0,
        spacing: 0.1,
        auto_spacing: None,
        masks: vec![Mask { width: 1, height: 1, pixels: vec![255] }],
    })
}

/// The tip a `Brush` element describes, its pictures found by `find`; `softness`: the brush varies
/// its softness, so an auto tip is drawn at several. `light`: its pictures' sizes only, none drawn.
/// `masks`: its pictures taken as masks whatever the preset says (a masking brush's are).
fn read_tip(
    brush: &Element,
    find: &dyn Fn(&str) -> Option<Vec<u8>>,
    softness: Option<&CurveOption>,
    light: bool,
    masks: bool,
) -> Option<Tip> {
    let spacing = brush.number("spacing").unwrap_or(0.1).max(0.0);
    let auto_spacing = (brush.attribute("useAutoSpacing") == Some("1")
        || brush.attribute("useAutoSpacing") == Some("true"))
    .then(|| brush.number("autoSpacingCoeff").unwrap_or(1.0));
    let angle = brush.number("angle").unwrap_or(0.0);
    if brush.attribute("type") == Some("auto_brush") {
        let tip = AutoTip::read(brush)?;
        let (w, h) = tip.size();
        let variants = if tip.speckled() { 4 } else { 1 };
        let levels = if softness.is_some_and(CurveOption::varies) { 4 } else { 1 };
        // drawn at its own size (holes and noise as fine as Krita's), else large enough to scale (the GPU
        // draws it smaller through its mip levels)
        let side = if tip.speckled() {
            tip.diameter.round().clamp(8.0, 256.0)
        } else {
            tip.diameter.round().clamp(64.0, 256.0)
        };
        let mut seed = 0x2545_f491_u32;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 8) as f32 / (1 << 24) as f32
        };
        let mut masks = Vec::new();
        for _ in 0..if light { 0 } else { variants } {
            for level in 0..levels {
                let soft = if levels == 1 { 1.0 } else { 0.1 + 0.9 * level as f32 / (levels - 1) as f32 };
                masks.push(tip.draw(side as u32, soft, &mut random));
            }
        }
        return Some(Tip {
            tips: Tips::Auto { variants, levels },
            kind: Kind::Mask,
            width: w,
            height: h,
            angle,
            spacing,
            auto_spacing,
            masks,
        });
    }
    // presets from before the second version of tips took them at twice their scale
    let older = brush.attribute("BrushVersion").is_none_or(|v| v.trim() == "1");
    let scale = brush.number("scale").unwrap_or(1.0).max(1e-3) * if older { 2.0 } else { 1.0 };
    let file = brush.attribute("filename")?;
    let Pictures { cells, image, colourful, pipe } = pictures(&find(file)?, light)?;
    let first = cells.first()?;
    let (w, h) = (first.width as f32 * scale, first.height as f32 * scale);
    let count = cells.len() as u32;
    let kind = if masks { Kind::Mask } else { application(brush, colourful) };
    let masks = if light {
        cells.iter().map(|c| Mask { width: c.width, height: c.height, pixels: Vec::new() }).collect()
    } else {
        let adjust = Adjustment::read(brush);
        cells.iter().map(|c| tip_picture(c, kind, image, &adjust)).collect()
    };
    Some(Tip { tips: Tips::Images { count, pipe }, kind, width: w, height: h, angle, spacing, auto_spacing, masks })
}

/// A pattern's picture: a PNG or a GIMP pattern. `light`: its size only.
fn pattern(bytes: &[u8], light: bool) -> Option<Rgba> {
    if !bytes.starts_with(b"\x89PNG") {
        return gimp::pat(bytes, !light);
    }
    if light {
        let (width, height) = png_size(bytes)?;
        return Some(Rgba { width, height, pixels: Vec::new() });
    }
    png_decode(bytes)
}

/// The mask Krita makes of a pattern for its texture: each pixel's grey where it is opaque (white
/// where clear), made lighter, of more contrast, inverted, its middle moved, cut outside its
/// bounds.
fn texture_mask(p: &Preset, picture: &Rgba) -> Mask {
    let k = |key: &str, otherwise: f32| p.number(&format!("Texture/Pattern/{key}"), otherwise);
    let (brightness, contrast, neutral) = (k("Brightness", 0.0), k("Contrast", 1.0), k("NeutralPoint", 0.5));
    let invert = p.flag("Texture/Pattern/Invert", false);
    let policy = k("CutoffPolicy", 0.0) as u32;
    let (left, right) = (k("CutoffLeft", 0.0) / 255.0, k("CutoffRight", 255.0) / 255.0);
    let pixels = picture
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|px| {
            let alpha = f32::from(px[3]) / 255.0;
            let g = f32::from(gimp::grey(px[0], px[1], px[2])) / 255.0;
            let lit = g * alpha + (1.0 - alpha);
            let mut m = ((lit - brightness - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
            if invert {
                m = 1.0 - m;
            }
            let n = neutral.clamp(0.0, 1.0);
            m = if n >= 1.0 || (n > 0.0 && m <= n) { m / (2.0 * n) } else { 0.5 + (m - n) / (2.0 - 2.0 * n) };
            let outside = m < left || m > right;
            m = match policy {
                1 if outside => 0.0,
                2 if outside => 1.0,
                _ => m.clamp(0.0, 1.0),
            };
            (m * 255.0).round() as u8
        })
        .collect();
    Mask { width: picture.width, height: picture.height, pixels }
}

impl Brush {
    /// The pixel brush (or colour smudge brush) `p` describes, its pictures found by `find`; none for
    /// another engine's.
    /// `light`: only to draw strokes with, its look not made (its pictures neither drawn nor
    /// decoded).
    pub fn read(p: &Preset, find: &dyn Fn(&str) -> Option<Vec<u8>>, light: bool) -> Option<Brush> {
        if !ENGINES.contains(&p.paintop.as_str()) {
            return None;
        }
        let smudge = (p.paintop == "colorsmudge").then(|| Smudge::read(p));
        let mut dynamics = Dynamics::read(p, "");
        // the quick, sketch and spray brushes have only some of the pixel brush's options: what a
        // preset still says of the rest (from the engine it was made with) does nothing
        let only: Option<&[&str]> = match p.paintop.as_str() {
            "roundmarker" => Some(&["size", "spacing"]),
            "sketchbrush" | "spraybrush" => Some(&["size", "opacity", "rotation"]),
            _ => None,
        };
        if let Some(only) = only {
            let d = &mut dynamics;
            let options = [
                ("opacity", &mut d.opacity),
                ("flow", &mut d.flow),
                ("rotation", &mut d.rotation),
                ("ratio", &mut d.ratio),
                ("spacing", &mut d.spacing),
                ("scatter", &mut d.scatter),
                ("mirror", &mut d.mirror),
                ("softness", &mut d.softness),
            ];
            for (name, option) in options {
                if !only.contains(&name) {
                    option.checked = false;
                    // the brush's opacity stays the preset's; its flow is full
                    if name == "flow" {
                        option.strength = 1.0;
                    }
                }
            }
        }
        let pixel_options = only.is_none();
        let definition = match p.paintop.as_str() {
            // the quick brush: a hard round tip of its own diameter and spacing
            "roundmarker" => crate::xml::parse(&format!(
                r#"<Brush type="auto_brush" spacing="{}" useAutoSpacing="{}" autoSpacingCoeff="{}"><MaskGenerator type="circle" id="default" diameter="{}" ratio="1" hfade="1" vfade="1" antialiasEdges="1"/></Brush>"#,
                p.number("spacing", 0.02),
                u8::from(p.flag("useAutoSpacing", false)),
                p.number("autoSpacingCoeff", 1.0),
                p.number("diameter", 30.0),
            )),
            _ => p.element("brush_definition"),
        };
        let mut tip = definition
            .as_ref()
            .and_then(|b| read_tip(b, find, Some(&dynamics.softness), light, false))
            .unwrap_or_else(|| stand_in(definition.as_ref().and_then(|b| b.number("diameter")).unwrap_or(40.0)));
        let sketch_radius = 0.5 * tip.width.max(tip.height);
        let (sketch, spray) = match p.paintop.as_str() {
            "sketchbrush" => (Some(crate::others::Sketch::read(p)), None),
            "spraybrush" => (None, Some(crate::others::Spray::read(p))),
            _ => (None, None),
        };
        if sketch.is_some() || spray.as_ref().is_some_and(|s| !s.own_tip) {
            // its lines' and particles' tips: a hard round one, a square (unless its particles
            // are its own tip)
            let round = stand_in(64.0).masks.into_iter().next();
            let square = Mask { width: 1, height: 1, pixels: if light { Vec::new() } else { vec![255] } };
            let masks: Vec<Mask> =
                if sketch.is_some() { vec![square] } else { round.into_iter().chain([square]).collect() };
            let count = masks.len() as u32;
            tip = Tip { tips: Tips::Images { count, pipe: None }, kind: Kind::Mask, masks, ..tip };
        }
        let op = p.get("CompositeOp").unwrap_or("normal").trim().to_owned();
        let texture = (pixel_options && p.flag("Texture/Pattern/Enabled", false))
            .then(|| Brush::read_texture(p, find, light))
            .flatten();
        let masking = (pixel_options && p.flag("MaskingBrush/Enabled", false))
            .then(|| Brush::read_masking(p, find, light))
            .flatten();
        let colour = |key: &str| {
            let mut option = CurveOption::read(p, "", key, true, 0.0, 1.0);
            option.checked &= pixel_options;
            option
        };
        let mut sharpness = Sharpness::read(p);
        sharpness.option.checked &= pixel_options;
        Some(Brush {
            tip,
            dynamics,
            airbrush: Airbrush::read(p),
            smudge,
            sketch,
            spray,
            tangent: (p.paintop == "tangentnormal").then(|| crate::others::Tangent::read(p)),
            sketch_radius,
            lightness: colour("LightnessStrength"),
            sharpness,
            darken: colour("Darken"),
            mix: colour("Mix"),
            hue: colour("h"),
            saturation: colour("s"),
            value: colour("v"),
            // washed unless it says it builds up; a masked one always is; the quick, sketch and
            // spray brushes lay each dab over the last
            wash: (p.number("PaintOpAction", 2.0) != 1.0 || masking.is_some())
                && !matches!(p.paintop.as_str(), "roundmarker" | "sketchbrush" | "spraybrush"),
            blend: blend_number(&op),
            erases: op == "erase" || p.flag("EraserMode", false),
            texture,
            masking,
        })
    }

    fn read_texture(p: &Preset, find: &dyn Fn(&str) -> Option<Vec<u8>>, light: bool) -> Option<Texture> {
        // its picture: carried in the setting itself (older presets), else a file it names
        // (read only to draw, only its header: its size)
        let carried =
            p.get("Texture/Pattern/Pattern").map(|t| if light { t.get(..t.len().min(8192)).unwrap_or(t) } else { t });
        let bytes = carried.and_then(crate::files::carried_picture).or_else(|| {
            ["Texture/Pattern/PatternFileName", "Texture/Pattern/Name"].iter().find_map(|k| find(p.get(k)?.trim()))
        })?;
        let picture = pattern(&bytes, light)?;
        let k = |key: &str, otherwise: f32| p.number(&format!("Texture/Pattern/{key}"), otherwise);
        Some(Texture {
            mask: texture_mask(p, &picture),
            scale: k("Scale", 1.0).max(0.01),
            rule: texture_rule(k("TexturingMode", 0.0) as u32, p.flag("Texture/Pattern/UseSoftTexturing", false)),
            offset: (k("OffsetX", 0.0), k("OffsetY", 0.0)),
            random_offset: (
                p.flag("Texture/Pattern/isRandomOffsetX", false),
                p.flag("Texture/Pattern/isRandomOffsetY", false),
            ),
            strength: CurveOption::read(p, "", "Texture/Strength/", true, 0.0, 1.0),
        })
    }

    fn read_masking(p: &Preset, find: &dyn Fn(&str) -> Option<Vec<u8>>, light: bool) -> Option<Masking> {
        let prefix = "MaskingBrush/Preset/";
        let dynamics = Dynamics::read(p, prefix);
        let definition = p.element(&format!("{prefix}brush_definition"))?;
        let tip = read_tip(&definition, find, None, light, true)?;
        let size = if p.flag("MaskingBrush/UseMasterSize", true) {
            p.number("MaskingBrush/MasterSizeCoeff", 1.0).max(0.0)
        } else {
            0.0
        };
        let combine = combine_number(p.get("MaskingBrush/MaskingCompositeOp").unwrap_or("multiply").trim());
        Some(Masking { tip, dynamics, size, combine })
    }

    /// How it looks to the app (see the app's guide to brush engines).
    pub fn look(&self) -> Vec<u8> {
        let mut look = Record::new();
        for m in &self.tip.masks {
            look.field(1, &tip_record(m));
        }
        if self.tip.kind != Kind::Mask {
            look.byte(7, self.tip.kind as u8);
        }
        if let Some(a) = &self.airbrush {
            look.real(8, a.interval);
        }
        if self.smudge.is_some() {
            look.byte(9, 1);
        }
        // (a smudge's dabs are laid each as far as it says)
        look.byte(2, u8::from(self.wash && self.smudge.is_none()));
        if let Some(blend) = self.blend {
            look.word(3, blend);
        }
        look.byte(4, u8::from(self.erases));
        if let Some(t) = &self.texture {
            let mut r = Record::new();
            r.word(1, t.mask.width).word(2, t.mask.height).field(3, &t.mask.pixels).real(4, t.scale).word(5, t.rule);
            look.field(5, &r.bytes());
        }
        if let Some(m) = &self.masking {
            let mut r = Record::new();
            for mask in &m.tip.masks {
                r.field(1, &tip_record(mask));
            }
            r.byte(2, 1).word(3, m.combine);
            look.field(6, &r.bytes());
        }
        look.bytes()
    }
}

fn tip_record(m: &Mask) -> Vec<u8> {
    let mut r = Record::new();
    r.word(1, m.width).word(2, m.height).field(3, &m.pixels);
    r.bytes()
}
