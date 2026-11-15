//! Krita's engines besides its pixel brush and colour smudge, as far as dabs draw them: the sketch
//! brush (lines from the pen to the points it passed near, each a thin dab), the spray brush
//! (particles scattered around the pen, each a dab), the quick brush (hard round dabs, read as a
//! pixel brush) and the tangent normal brush (a pixel brush whose colour is the pen's lean as a
//! normal). The rest (shape, filter, deform, clone, the bristle, hatching, curve, particle, grid,
//! dyna and chalk brushes) come in as the app reads them.

use crate::options::{CurveOption, Info};
use crate::preset::Preset;

/// The sketch brush (Krita's "Harmony"-like one): at each move it draws a line from the pen to
/// each point it passed within its tip's reach, as likely as its density says (and the further,
/// the more likely), each line `line_width` wide and moved by its offset.
#[derive(Clone, Debug)]
pub struct Sketch {
    pub probability: f32,
    pub offset: f32,
    pub line_width: f32,
    pub connect: bool,
    pub magnetify: bool,
    pub random_rgb: bool,
    pub random_opacity: bool,
    pub distance_density: bool,
    pub distance_opacity: bool,
    pub line_width_option: CurveOption,
    pub offset_option: CurveOption,
    pub density_option: CurveOption,
}

impl Sketch {
    pub fn read(p: &Preset) -> Sketch {
        Sketch {
            probability: p.number("Sketch/probability", 0.5),
            offset: p.number("Sketch/offset", 30.0),
            line_width: p.number("Sketch/lineWidth", 1.0).trunc().max(0.0),
            connect: p.flag("Sketch/makeConnection", true),
            magnetify: p.flag("Sketch/magnetify", true),
            random_rgb: p.flag("Sketch/randomRGB", false),
            random_opacity: p.flag("Sketch/randomOpacity", false),
            distance_density: p.flag("Sketch/distanceDensity", true),
            distance_opacity: p.flag("Sketch/distanceOpacity", false),
            line_width_option: CurveOption::read(p, "", "Line width", true, 0.0, 1.0),
            offset_option: CurveOption::read(p, "", "Offset scale", true, 0.0, 1.0),
            density_option: CurveOption::read(p, "", "Density", true, 0.0, 1.0),
        }
    }
}

/// The spray brush: at each of its dabs (spaced by a share of its diameter), particles scattered
/// over a circle `diameter` across (squashed by its aspect, turned, scaled), as many as its count
/// or as cover its area, each an ellipse, a rectangle or a pixel `size` big (or a share of the
/// diameter), or with its shape option off the brush's own tip, turned at random, sized at random,
/// coloured at random if it says.
#[derive(Clone, Debug)]
pub struct Spray {
    pub diameter: f32,
    pub aspect: f32,
    pub rotation: f32,
    pub scale: f32,
    pub spacing: f32,
    pub jitter: Option<f32>,
    pub count: f32,
    pub coverage: Option<f32>,
    pub gaussian: bool,
    pub centre_biased: bool,
    pub deviation: f32,
    pub shape: u32,
    /// Its particles are the brush's tip (its shape option off).
    pub own_tip: bool,
    pub size: (f32, f32),
    pub proportional: bool,
    pub random_size: bool,
    pub turn: f32,
    pub random_turn: Option<f32>,
    pub follow_cursor: Option<f32>,
    pub follow_angle: Option<f32>,
    pub random_hsv: Option<(f32, f32, f32)>,
    pub random_opacity: bool,
    pub per_particle: bool,
    pub mix_background: bool,
}

impl Spray {
    pub fn read(p: &Preset) -> Spray {
        let n = |k: &str, otherwise: f32| p.number(k, otherwise);
        // its shape's dynamics as a newer preset keeps them, else as Krita 2.2 did
        let newer = p.get("ShapeDynamicsVersion").is_some_and(|v| v.trim() != "2.2");
        let dynamics = |key: &str| if newer { format!("ShapeDynamics/{key}") } else { format!("SprayShape/{key}") };
        let on = !newer || p.flag("ShapeDynamics/enabled", false);
        let weight =
            |flag: &str, w: &str| (on && p.flag(&dynamics(flag), false)).then(|| n(&dynamics(w), 0.5).clamp(0.0, 1.0));
        let radial = p.get("Spray/radialDistributionType").map(str::trim).unwrap_or("");
        Spray {
            diameter: n("Spray/diameter", 100.0).trunc().max(1.0),
            aspect: n("Spray/aspect", 1.0),
            rotation: n("Spray/rotation", 0.0),
            scale: n("Spray/scale", 1.0),
            spacing: n("Spray/spacing", 0.5),
            jitter: p.flag("Spray/jitterMovement", false).then(|| n("Spray/jitterMoveAmount", 0.0)),
            count: n("Spray/particleCount", 12.0).max(0.0),
            coverage: p.flag("Spray/useDensity", false).then(|| n("Spray/coverage", 10.0) / 100.0),
            gaussian: radial == "gaussian" || (radial.is_empty() && p.flag("Spray/gaussianDistribution", false)),
            centre_biased: p.flag("Spray/radialDistributionCenterBiased", true),
            deviation: n("Spray/radialDistributionStdDeviation", 0.5),
            shape: n("SprayShape/shape", 0.0) as u32,
            own_tip: !p.flag("SprayShape/enabled", true),
            size: (n("SprayShape/width", 6.0).trunc().max(0.0), n("SprayShape/height", 6.0).trunc().max(0.0)),
            proportional: p.flag("SprayShape/proportional", false),
            random_size: on && p.flag(&dynamics("randomSize"), false),
            turn: if on && p.flag(&dynamics("fixedRotation"), false) {
                n(&dynamics("fixedAngle"), 0.0).trunc()
            } else {
                0.0
            },
            random_turn: weight("randomRotation", "randomRotationWeight"),
            follow_cursor: weight("followCursor", "followCursorWeigth"),
            follow_angle: weight("followDrawingAngle", "followDrawingAngleWeigth"),
            random_hsv: p
                .flag("ColorOption/useRandomHSV", false)
                .then(|| (n("ColorOption/hue", 0.0), n("ColorOption/saturation", 0.0), n("ColorOption/value", 0.0))),
            random_opacity: p.flag("ColorOption/useRandomOpacity", false),
            per_particle: p.flag("ColorOption/colorPerParticle", false),
            mix_background: p.flag("ColorOption/mixBgColor", false),
        }
    }

    /// A particle's width and height, for the spray `diameter` across now.
    pub fn particle(&self, diameter: f32) -> (f32, f32) {
        let (w, h) = self.size;
        let (w, h) = if self.proportional { (w / 100.0 * diameter, h / 100.0 * diameter) } else { (w, h) };
        ((w * self.scale).max(1.0), (h * self.scale).max(1.0))
    }
}

/// The tangent normal brush's colour: the pen's lean (or the stroke's way, or the pen's turn) as
/// a normal, each axis to the channel its swizzle says.
#[derive(Clone, Debug)]
pub struct Tangent {
    pub channels: [u32; 3],
    pub direction: u32,
    pub sensitivity: f32,
    pub mix: f32,
}

impl Tangent {
    pub fn read(p: &Preset) -> Tangent {
        let n = |k: &str, otherwise: f32| p.number(k, otherwise);
        Tangent {
            channels: [n("Tangent/swizzleRed", 0.0), n("Tangent/swizzleGreen", 2.0), n("Tangent/swizzleBlue", 4.0)]
                .map(|v| v as u32),
            direction: n("Tangent/directionType", 0.0) as u32,
            sensitivity: n("Tangent/elevationSensitivity", 100.0),
            mix: n("Tangent/mixValue", 50.0),
        }
    }

    /// The colour for the pen as `info` has it, the canvas turned `turn` (radians).
    pub fn colour(&self, info: &Info, turn: f32) -> [f32; 3] {
        let tilt_direction = crate::options::tilt_direction(info) * 360.0;
        let elevation = crate::options::tilt_elevation(info) * 90.0;
        let drawing = (0.75 + info.drawing_angle / std::f32::consts::TAU) * 360.0;
        let (mut direction, elevation) = match self.direction {
            1 => (drawing, 0.0),
            2 => (info.rotation, elevation),
            3 => {
                let m = self.mix / 100.0;
                (tilt_direction * (1.0 - m) + drawing * m, elevation)
            }
            _ => (tilt_direction, elevation),
        };
        if self.direction != 1 {
            direction = (direction - turn.to_degrees()).rem_euclid(360.0);
        }
        let s = self.sensitivity / 100.0;
        let elevation = (elevation * s + (90.0 - s * 90.0)).trunc().to_radians();
        let direction = direction.to_radians();
        let half = |v: f32| if v > 0.0 { 0.5 + v.abs() * 0.5 } else { 0.5 - v.abs() * 0.5 };
        let horizontal = half(elevation.cos() * direction.sin());
        let vertical = half(elevation.cos() * direction.cos());
        let depth = elevation.sin();
        self.channels.map(|c| match c {
            0 => horizontal,
            1 => 1.0 - horizontal,
            2 => vertical,
            3 => 1.0 - vertical,
            4 => depth,
            _ => 1.0 - depth,
        })
    }
}
