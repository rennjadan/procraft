//! Five-colour palettes by the colour-harmony rules Adobe Color uses: analogous, monochromatic,
//! triad, complementary, split complementary, square, compound and shades. Built round a base
//! colour (the one painted with, or one picked at random), each palette with its base in the
//! middle; a seed shuffles the rules and nudges the colours, so each shuffle offers others.

/// A palette suggested: its rule's name and five colours (0xRRGGBB).
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct HarmonyPalette {
    pub name: String,
    pub colors: Vec<u32>,
}

const RULES: [&str; 8] =
    ["Analogous", "Monochromatic", "Triad", "Complementary", "Split Complementary", "Square", "Compound", "Shades"];

/// A small seeded generator (xorshift), enough for nudging colours.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> f32 {
        let mut x = self.0.max(1);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Between `lo` and `hi`.
    fn between(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

fn hsv_to_hex(h: f32, s: f32, v: f32) -> u32 {
    let h = h.rem_euclid(360.0) / 60.0;
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let byte = |u: f32| ((u + m).clamp(0.0, 1.0) * 255.0).round() as u32;
    byte(r) << 16 | byte(g) << 8 | byte(b)
}

fn hex_to_hsv(rgb: u32) -> (f32, f32, f32) {
    let [r, g, b] = [rgb >> 16 & 255, rgb >> 8 & 255, rgb & 255].map(|c| c as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, if max > 0.0 { d / max } else { 0.0 }, max)
}

/// The five colours of `rule` round base (`h`, `s`, `v`), its base in the middle.
fn palette(rule: &str, h: f32, s: f32, v: f32, dice: &mut Dice) -> Vec<u32> {
    let mut j = |amount: f32| dice.between(-amount, amount);
    let c = |dh: f32, ds: f32, dv: f32| hsv_to_hex(h + dh, (s * ds).clamp(0.08, 1.0), (v * dv).clamp(0.12, 1.0));
    match rule {
        "Analogous" => vec![
            c(-30.0 + j(5.0), 1.0, 0.85),
            c(-15.0, 0.9, 1.05),
            c(0.0, 1.0, 1.0),
            c(15.0, 0.9, 0.9),
            c(30.0 + j(5.0), 1.0, 0.75),
        ],
        "Monochromatic" => {
            vec![
                c(0.0, 0.35, 1.25),
                c(0.0, 0.6, 0.65),
                c(0.0, 1.0, 1.0),
                c(0.0, 1.15, 0.45),
                c(0.0, 0.5, 0.85 + j(0.1)),
            ]
        }
        "Triad" => vec![c(120.0, 0.9, 0.9), c(0.0, 0.5, 1.15), c(0.0, 1.0, 1.0), c(0.0, 1.0, 0.55), c(240.0, 0.9, 0.9)],
        "Complementary" => {
            vec![
                c(0.0, 0.55, 1.15),
                c(0.0, 1.0, 0.6),
                c(0.0, 1.0, 1.0),
                c(180.0, 1.0, 0.95),
                c(180.0, 0.55, 0.7 + j(0.1)),
            ]
        }
        "Split Complementary" => {
            vec![c(150.0, 0.9, 0.95), c(0.0, 0.5, 1.1), c(0.0, 1.0, 1.0), c(0.0, 1.0, 0.6), c(210.0, 0.9, 0.95)]
        }
        "Square" => {
            vec![c(90.0, 0.9, 0.95), c(0.0, 0.5, 1.15), c(0.0, 1.0, 1.0), c(180.0, 0.9, 0.9), c(270.0, 0.9, 0.95)]
        }
        "Compound" => {
            vec![c(30.0, 1.0, 0.85), c(0.0, 0.6, 1.1), c(0.0, 1.0, 1.0), c(165.0, 0.9, 0.75), c(195.0, 0.8, 0.95)]
        }
        _ => vec![c(0.0, 1.0, 0.3), c(0.0, 1.0, 0.55), c(0.0, 1.0, 1.0), c(0.0, 1.0, 0.8), c(0.0, 1.0, 0.42)],
    }
}

/// Eight palettes, one by each rule in an order `seed` shuffles, round `base` (0xRRGGBB), or with
/// `random_base` round a colour picked at random for each.
#[uniffi::export]
pub fn harmony_palettes(base: u32, random_base: bool, seed: u64) -> Vec<HarmonyPalette> {
    let mut dice = Dice(seed ^ 0x9E37_79B9_7F4A_7C15);
    let mut rules = RULES.to_vec();
    // shuffled (Fisher-Yates)
    for i in (1..rules.len()).rev() {
        let k = ((dice.next() * (i + 1) as f32) as usize).min(i);
        rules.swap(i, k);
    }
    let (bh, bs, bv) = hex_to_hsv(base & 0xFF_FFFF);
    rules
        .into_iter()
        .map(|rule| {
            let (h, s, v) = if random_base {
                (dice.between(0.0, 360.0), dice.between(0.45, 0.9), dice.between(0.6, 0.95))
            } else {
                // a grey base has no hue to build on: given some
                let s = if bs < 0.08 { 0.5 } else { bs };
                (bh + dice.between(-6.0, 6.0), s, bv.max(0.25))
            };
            HarmonyPalette { name: rule.to_owned(), colors: palette(rule, h, s, v, &mut dice) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_rule_gives_five_colours_round_the_base_and_a_seed_shuffles_them() {
        let a = harmony_palettes(0x3366CC, false, 1);
        assert_eq!(a.len(), 8);
        assert!(a.iter().all(|p| p.colors.len() == 5));
        let names: std::collections::HashSet<_> = a.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names.len(), 8, "every rule once");
        // the base's hue in the middle
        let mono = a.iter().find(|p| p.name == "Monochromatic").unwrap();
        let (h, ..) = hex_to_hsv(*mono.colors.get(2).unwrap());
        assert!((h - 220.0).abs() < 10.0, "{h}");
        // the complementary's other side across the wheel
        let comp = a.iter().find(|p| p.name == "Complementary").unwrap();
        let (h4, ..) = hex_to_hsv(*comp.colors.get(3).unwrap());
        assert!(((h4 - 40.0).abs()) < 12.0, "{h4}");
        assert_ne!(harmony_palettes(0x3366CC, true, 2), harmony_palettes(0x3366CC, true, 3));
        assert_eq!(harmony_palettes(0x3366CC, true, 2), harmony_palettes(0x3366CC, true, 2));
    }
}
