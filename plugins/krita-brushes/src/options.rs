//! Krita's dynamics: an option (size, opacity, rotation...) follows sensors of the pen and the
//! stroke, each through a curve of its own (or one they share), the sensors' values combined as the
//! option says (multiplied, added, the most, the least or the difference), then scaled by the
//! option's strength. Sensors that turn (drawing angle, tilt direction, the pen's barrel) and the
//! random ones add rather than scale.

use crate::curve::Curve;
use crate::preset::Preset;
use crate::xml::Element;

/// What a dab is drawn with: the pen's state where it lies, and the stroke's so far.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub pressure: f32,
    /// Degrees the pen leans across and down (-60..60), and its barrel's turn (degrees).
    pub x_tilt: f32,
    pub y_tilt: f32,
    pub rotation: f32,
    /// 0..1, as Krita measures it.
    pub speed: f32,
    /// Radians, the way the stroke goes.
    pub drawing_angle: f32,
    /// Canvas pixels drawn, milliseconds since the stroke began, dabs laid before this one.
    pub distance: f32,
    pub time: f32,
    pub dab: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pressure,
    PressureIn,
    XTilt,
    YTilt,
    TiltDirection,
    TiltElevation,
    Speed,
    DrawingAngle,
    Rotation,
    Distance,
    Time,
    Fade,
    FuzzyDab,
    FuzzyStroke,
    Perspective,
    TangentialPressure,
}

impl Kind {
    fn named(id: &str) -> Option<Kind> {
        Some(match id {
            "pressure" => Kind::Pressure,
            "pressurein" => Kind::PressureIn,
            "xtilt" => Kind::XTilt,
            "ytilt" => Kind::YTilt,
            "ascension" | "tiltdirection" => Kind::TiltDirection,
            "declination" | "tiltelevation" => Kind::TiltElevation,
            "speed" => Kind::Speed,
            "drawingangle" => Kind::DrawingAngle,
            "rotation" => Kind::Rotation,
            "distance" => Kind::Distance,
            "time" => Kind::Time,
            "fade" => Kind::Fade,
            "fuzzy" => Kind::FuzzyDab,
            "fuzzystroke" => Kind::FuzzyStroke,
            "perspective" => Kind::Perspective,
            "tangentialpressure" => Kind::TangentialPressure,
            _ => return None,
        })
    }

    /// It adds to the option rather than scaling it (-1..1).
    fn adds(self) -> bool {
        matches!(self, Kind::Rotation | Kind::TiltDirection | Kind::FuzzyDab | Kind::FuzzyStroke)
    }

    /// It gives the option an angle outright (0..1 round the circle).
    fn absolute(self) -> bool {
        self == Kind::DrawingAngle
    }
}

#[derive(Clone, Debug)]
pub struct Sensor {
    pub kind: Kind,
    curve: Option<Curve>,
    /// For distance, time and fade: how long it takes to reach 1, and whether it starts again.
    length: f32,
    periodic: bool,
    /// For the drawing angle: the angle added (degrees).
    angle_offset: f32,
}

/// Randomness for the fuzzy sensors: a dab's, and the stroke's for an option.
pub trait Chance {
    fn dab(&mut self) -> f32;
    fn stroke(&mut self, key: &str) -> f32;
}

impl Sensor {
    fn value(&self, info: &Info, chance: &mut dyn Chance, option: &str) -> f32 {
        let wrap = |v: f32| v.rem_euclid(1.0);
        let along = |v: f32| {
            let length = self.length.max(1e-3);
            let v = if self.periodic { v.rem_euclid(length) } else { v.min(length) };
            v / length
        };
        match self.kind {
            Kind::Pressure | Kind::PressureIn => info.pressure,
            Kind::XTilt => 1.0 - info.x_tilt.abs() / 60.0,
            Kind::YTilt => 1.0 - info.y_tilt.abs() / 60.0,
            Kind::TiltDirection => 2.0 * tilt_direction(info) - 1.0,
            Kind::TiltElevation => tilt_elevation(info),
            Kind::Speed => info.speed,
            Kind::DrawingAngle => wrap(0.5 + info.drawing_angle / std::f32::consts::TAU + self.angle_offset / 360.0),
            Kind::Rotation => info.rotation / 180.0,
            Kind::Distance => along(info.distance),
            Kind::Time => along(info.time),
            Kind::Fade => along(info.dab as f32),
            Kind::FuzzyDab => 2.0 * chance.dab() - 1.0,
            Kind::FuzzyStroke => 2.0 * chance.stroke(option) - 1.0,
            Kind::Perspective | Kind::TangentialPressure => 1.0,
        }
    }

    /// Its value through its curve: one that adds is taken to 0..1 for the curve and back, one
    /// that gives an angle is turned half round for it and back.
    fn parameter(&self, info: &Info, chance: &mut dyn Chance, option: &str) -> f32 {
        let v = self.value(info, chance, option);
        let Some(curve) = &self.curve else { return v };
        if self.kind.adds() {
            2.0 * curve.at(0.5 * (1.0 + v)) - 1.0
        } else if self.kind.absolute() {
            (curve.at((v + 0.5).rem_euclid(1.0)) + 0.5).rem_euclid(1.0)
        } else {
            curve.at(v)
        }
    }
}

/// The direction the pen leans, 0..1 round the circle, as Krita reckons it.
pub fn tilt_direction(info: &Info) -> f32 {
    let (x, y) = (info.x_tilt, info.y_tilt);
    let angle = if x.abs() < 1e-6 && y.abs() < 1e-6 {
        let sign = |v: f32| if v < 0.0 { -1.0 } else { 1.0 };
        -sign(x) * sign(y) * std::f32::consts::FRAC_PI_2
    } else {
        (-x).atan2(y)
    };
    angle / std::f32::consts::TAU + 0.5
}

/// How upright the pen is, 0..1 (1 upright), as Krita reckons it.
pub fn tilt_elevation(info: &Info) -> f32 {
    let x = (info.x_tilt / 60.0).clamp(-1.0, 1.0);
    let y = (info.y_tilt / 60.0).clamp(-1.0, 1.0);
    let e = if x.abs() > y.abs() { (1.0 + y * y).sqrt() } else { (1.0 + x * x).sqrt() };
    let cos = ((x * x + y * y).sqrt() / e).clamp(-1.0, 1.0);
    cos.acos() / std::f32::consts::FRAC_PI_2
}

/// An option that follows sensors.
#[derive(Clone, Debug)]
pub struct CurveOption {
    pub id: String,
    pub checked: bool,
    use_curve: bool,
    mode: u32,
    pub strength: f32,
    min: f32,
    max: f32,
    sensors: Vec<Sensor>,
}

/// What its sensors come to: a scaling, what adds, and an angle given outright.
#[derive(Default)]
struct Parts {
    scaling: Option<f32>,
    additive: Option<f32>,
    absolute: Option<f32>,
}

impl CurveOption {
    /// The option `id` as `preset` keeps it (its keys after `prefix`, as a masking brush's are),
    /// its strength `min..=max`; `checkable`: whether it can be switched off.
    pub fn read(preset: &Preset, prefix: &str, id: &str, checkable: bool, min: f32, max: f32) -> CurveOption {
        let key = |suffix: &str| format!("{prefix}{id}{suffix}");
        let checked = !checkable || preset.flag(&format!("{prefix}Pressure{id}"), false);
        let use_same = preset.flag(&key("UseSameCurve"), true);
        let common = preset.get(&key("commonCurve")).and_then(Curve::parse);
        let mut sensors = Vec::new();
        if let Some(root) = preset.element(&key("Sensor")) {
            if root.attribute("id") == Some("sensorslist") {
                for child in root.elements().filter(|e| e.name == "ChildSensor") {
                    sensors.extend(sensor(child));
                }
            } else {
                sensors.extend(sensor(&root));
            }
        }
        if sensors.is_empty() {
            sensors.push(Sensor { kind: Kind::Pressure, curve: None, length: 1.0, periodic: false, angle_offset: 0.0 });
        }
        if use_same {
            for s in &mut sensors {
                s.curve = common.clone();
            }
        }
        CurveOption {
            id: id.to_owned(),
            checked,
            use_curve: preset.flag(&key("UseCurve"), true),
            mode: preset.number(&key("curveMode"), 0.0) as u32,
            strength: preset.number(&key("Value"), max).clamp(min, max),
            min,
            max,
            sensors,
        }
    }

    fn parts(&self, info: &Info, chance: &mut dyn Chance) -> Parts {
        let mut parts = Parts::default();
        if !self.use_curve {
            return parts;
        }
        let mut scaling = Vec::new();
        for s in &self.sensors {
            let v = s.parameter(info, chance, &self.id);
            if s.kind.adds() {
                *parts.additive.get_or_insert(0.0) += v;
            } else if s.kind.absolute() {
                parts.absolute = Some(v);
            } else {
                scaling.push(v);
            }
        }
        if !scaling.is_empty() {
            let most = scaling.iter().copied().fold(f32::MIN, f32::max);
            let least = scaling.iter().copied().fold(f32::MAX, f32::min);
            // (one sensor is its value, however several would combine)
            parts.scaling = Some(match self.mode {
                _ if scaling.len() == 1 => most,
                1 => scaling.iter().sum(),
                2 => most,
                3 => least,
                4 => most - least,
                _ => scaling.iter().product(),
            });
        }
        parts
    }

    /// Its value as a size (a share, scaled by its strength if `strength`), held to its range.
    pub fn size_like(&self, info: &Info, chance: &mut dyn Chance, strength: bool) -> f32 {
        let parts = self.parts(info, chance);
        let constant = if strength { self.strength } else { 1.0 };
        let offset = parts.absolute.unwrap_or(1.0);
        let scaling = parts.scaling.unwrap_or(1.0);
        let additive = parts.additive.map_or(1.0, |a| 0.5 * (1.0 + a));
        (constant * offset * scaling * additive).clamp(self.min, self.max)
    }

    /// Its value as a turn (-1..1, a whole turn 2), from `base` (0..1 of a turn) where nothing
    /// gives it outright; the scaling's share taken `coeff` times.
    pub fn rotation_like(&self, info: &Info, chance: &mut dyn Chance, base: f32, coeff: f32) -> f32 {
        let parts = self.parts(info, chance);
        let offset = parts.absolute.unwrap_or(base);
        let scaling = parts.scaling.map_or(0.0, |s| 2.0 * s - 1.0);
        let additive = parts.additive.unwrap_or(0.0);
        let v = 2.0 * offset + self.strength * (coeff * scaling + additive);
        (v + 1.0).rem_euclid(2.0) - 1.0
    }

    /// The option's value as Krita's size-like options give it: its strength while unchecked.
    pub fn apply(&self, info: &Info, chance: &mut dyn Chance) -> f32 {
        if self.checked { self.size_like(info, chance, true) } else { self.strength }
    }

    /// Whether anything about it changes from dab to dab.
    pub fn varies(&self) -> bool {
        self.checked && self.use_curve
    }
}

fn sensor(e: &Element) -> Option<Sensor> {
    let kind = Kind::named(e.attribute("id")?)?;
    let curve = e.child("curve").and_then(|c| Curve::parse(&c.text()));
    let length = e.number("length").unwrap_or(match kind {
        Kind::Fade => 1000.0,
        Kind::Time => 3000.0,
        _ => 30.0,
    });
    Some(Sensor {
        kind,
        curve,
        length,
        periodic: e.attribute("periodic").is_some_and(|p| p == "1" || p == "true"),
        angle_offset: e.number("angleOffset").unwrap_or(0.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Still;
    impl Chance for Still {
        fn dab(&mut self) -> f32 {
            0.5
        }
        fn stroke(&mut self, _: &str) -> f32 {
            0.5
        }
    }

    fn preset(params: &[(&str, &str)]) -> Preset {
        Preset { params: params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), ..Preset::default() }
    }

    #[test]
    fn a_size_follows_the_pressure_through_its_curve() {
        let p = preset(&[
            ("PressureSize", "true"),
            ("SizeSensor", r#"<params id="pressure"/>"#),
            ("SizeUseSameCurve", "true"),
            ("SizecommonCurve", "0,0.5;1,1;"),
            ("SizeValue", "0.8"),
        ]);
        let size = CurveOption::read(&p, "", "Size", true, 0.0, 1.0);
        let info = Info { pressure: 0.0, ..Info::default() };
        assert!((size.apply(&info, &mut Still) - 0.4).abs() < 1e-3);
        let full = Info { pressure: 1.0, ..Info::default() };
        assert!((size.apply(&full, &mut Still) - 0.8).abs() < 1e-3);
        // unchecked, its strength
        let off = CurveOption::read(&preset(&[("SizeValue", "0.6")]), "", "Size", true, 0.0, 1.0);
        assert!((off.apply(&info, &mut Still) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn sensors_listed_together_multiply_and_fuzz_adds() {
        let p = preset(&[
            ("PressureOpacity", "true"),
            (
                "OpacitySensor",
                r#"<params id="sensorslist"><ChildSensor id="pressure"/><ChildSensor id="fuzzy"/></params>"#,
            ),
            ("OpacityUseSameCurve", "true"),
        ]);
        let o = CurveOption::read(&p, "", "Opacity", true, 0.0, 1.0);
        // fuzz at its middle adds nothing: half of the scaling
        let v = o.size_like(&Info { pressure: 0.5, ..Info::default() }, &mut Still, true);
        assert!((v - 0.25).abs() < 1e-4, "{v}");
    }

    #[test]
    fn a_lone_sensor_is_its_value_whatever_the_mode_says() {
        // "difference" of one value would be nothing
        let p = preset(&[("PressureOpacity", "true"), ("OpacitycurveMode", "4"), ("OpacityValue", "0.8")]);
        let o = CurveOption::read(&p, "", "Opacity", true, 0.0, 1.0);
        let v = o.size_like(&Info { pressure: 0.5, ..Info::default() }, &mut Still, true);
        assert!((v - 0.4).abs() < 1e-4, "{v}");
    }
}
