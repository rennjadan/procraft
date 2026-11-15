//! The Krita brush studio: a panel of the plugin's own (the app's `panel` permission) showing a
//! Krita brush's settings as Krita names them, a page at a time: its tip, its spacing, its size,
//! opacity and flow, its turn and squash, scatter and mirroring, colour and texture, each option with
//! the sensor it follows and the curve it takes. The app opens it in place of its own brush studio
//! for a brush this plugin's engine made; each change is written back to the brush (the app's
//! `brush.data` permission) and draws from the next stroke on.

use crate::curve::Curve;
use crate::preset::{Preset, Resource};
use crate::rec::{Record, fields};

#[cfg(target_arch = "wasm32")]
mod app {
    #[link(wasm_import_module = "procraft")]
    unsafe extern "C" {
        pub fn show_panel(at: i32, len: i32) -> i32;
        pub fn close_panel();
        pub fn brush_data_len() -> i32;
        pub fn read_brush_data(to: i32) -> i32;
        pub fn write_brush_data(from: i32, len: i32) -> i32;
    }
}

/// The brush's data, as the app keeps it for this plugin's engine; none without one to edit.
fn read_data() -> Option<Vec<u8>> {
    #[cfg(target_arch = "wasm32")]
    {
        let len = usize::try_from(unsafe { app::brush_data_len() }).ok()?;
        let mut data = vec![0_u8; len];
        if unsafe { app::read_brush_data(data.as_mut_ptr() as i32) } != 1 {
            return None;
        }
        Some(data)
    }
    #[cfg(not(target_arch = "wasm32"))]
    None
}

fn write_data(data: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { app::write_brush_data(data.as_ptr() as i32, data.len() as i32) == 1 }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = data;
        false
    }
}

fn show(json: &str) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        app::show_panel(json.as_ptr() as i32, json.len() as i32);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = json;
}

fn close() {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        app::close_panel();
    }
}

/// The pages, as the page row offers them.
const PAGES: [&str; 8] =
    ["Tip", "Spacing", "Size", "Opacity and flow", "Turn and squash", "Scatter and mirror", "Colour", "Texture"];

/// The sensors an option can be set to follow here, by Krita's ids, and what the page calls them.
const SENSORS: [(&str, &str); 8] = [
    ("pressure", "Pressure"),
    ("speed", "Speed"),
    ("fade", "Fade"),
    ("distance", "Distance"),
    ("drawingangle", "Drawing angle"),
    ("xtilt", "Tilt"),
    ("rotation", "Rotation"),
    ("fuzzy", "Random"),
];

/// Curves to pick, and Krita's way of writing each.
const CURVES: [(&str, &str); 7] = [
    ("Straight", "0,0;1,1;"),
    ("Soft rise", "0,0;0.5,0.25;1,1;"),
    ("Strong rise", "0,0;0.5,0.75;1,1;"),
    ("Late", "0,0;0.75,0.1;1,1;"),
    ("Early", "0,0;0.25,0.9;1,1;"),
    ("Falling", "0,1;1,0;"),
    ("Half to whole", "0,0.5;1,1;"),
];

/// Krita's texturing modes the page offers, by their numbers.
const TEXTURE_MODES: [(u32, &str); 8] = [
    (0, "Multiply"),
    (1, "Subtract"),
    (4, "Darken"),
    (5, "Overlay"),
    (6, "Colour dodge"),
    (7, "Colour burn"),
    (12, "Height"),
    (13, "Linear height"),
];

/// What a row of the page shown does when touched.
#[derive(Clone, Debug, PartialEq)]
enum Does {
    Page,
    /// A setting set to the value (scaled from the slider by `per`).
    Number {
        key: String,
        per: f32,
    },
    Flag(String),
    /// A brush element's attribute (its tip's, or its mask generator's).
    Attribute {
        child: bool,
        name: String,
        per: f32,
    },
    AutoSpacing,
    Sensor(String),
    Curve(String),
    PaintingMode,
    ScatterAxes,
    MirrorAxes,
    TextureMode,
    Reset,
    Nothing,
}

/// The studio's state, in the copy of the code its panel runs in.
#[derive(Default)]
pub struct Studio {
    preset: Option<Preset>,
    /// The pictures kept beside the settings, and the settings as they came (to reset to).
    files: Vec<(String, Vec<u8>)>,
    original: Option<String>,
    page: usize,
    rows: Vec<Does>,
}

impl Studio {
    /// Opens on the brush the app hands over.
    pub fn open(&mut self) {
        let Some(data) = read_data() else {
            show(
                r#"{ "title": "Krita brush studio", "rows": [{ "type": "text", "text": "Pick a brush that came from a Krita file, then open its studio." }] }"#,
            );
            return;
        };
        self.read(&data);
        self.page = 0;
        self.show();
    }

    fn read(&mut self, data: &[u8]) {
        self.files.clear();
        self.original = None;
        let mut text = None;
        for (id, value) in fields(data) {
            match id {
                1 => text = Some(String::from_utf8_lossy(value).into_owned()),
                2 => {
                    let (mut name, mut bytes) = (String::new(), Vec::new());
                    for (f, v) in fields(value) {
                        match f {
                            1 => name = String::from_utf8_lossy(v).into_owned(),
                            2 => bytes = v.to_vec(),
                            _ => {}
                        }
                    }
                    self.files.push((name, bytes));
                }
                3 => self.original = Some(String::from_utf8_lossy(value).into_owned()),
                _ => {}
            }
        }
        self.preset = text.as_deref().and_then(Preset::read);
        if self.original.is_none() {
            self.original = text;
        }
    }

    /// The brush's data as it now is, written back to the app.
    fn save(&self) {
        let Some(p) = &self.preset else { return };
        let mut data = Record::new();
        data.field(1, p.to_xml().as_bytes());
        for (name, bytes) in &self.files {
            let mut r = Record::new();
            r.field(1, name.as_bytes()).field(2, bytes);
            data.field(2, &r.bytes());
        }
        if let Some(original) = &self.original {
            data.field(3, original.as_bytes());
        }
        write_data(&data.bytes());
    }

    /// A touch on row `row` with `value`.
    pub fn touched(&mut self, row: usize, value: f32) {
        let Some(does) = self.rows.get(row).cloned() else { return };
        if does == Does::Page {
            self.page = (value.max(0.0) as usize).min(PAGES.len() - 1);
            self.show();
            return;
        }
        if does == Does::Reset {
            if let Some(original) = self.original.clone() {
                self.preset = Preset::read(&original);
                self.save();
            }
            self.show();
            return;
        }
        let Some(p) = self.preset.as_mut() else {
            close();
            return;
        };
        change(p, &does, value);
        self.save();
        self.show();
    }

    fn show(&mut self) {
        let Some(p) = self.preset.as_ref() else { return };
        let mut page = Page::new(&p.name);
        page.choice(Does::Page, "Page", &PAGES, self.page);
        match self.page {
            0 => tip_page(p, &mut page),
            1 => spacing_page(p, &mut page),
            2 => option_rows(p, &mut page, "Size", "Size", true),
            3 => {
                let wash = p.number("PaintOpAction", 2.0) != 1.0;
                page.choice(Does::PaintingMode, "Painting mode", &["Build up", "Wash"], usize::from(wash));
                option_rows(p, &mut page, "Opacity", "Opacity", false);
                option_rows(p, &mut page, "Flow", "Flow", false);
            }
            4 => {
                option_rows(p, &mut page, "Rotation", "Rotation", true);
                option_rows(p, &mut page, "Ratio", "Ratio", true);
            }
            5 => {
                option_rows(p, &mut page, "Scatter", "Scatter", true);
                let (x, y) = (p.flag("Scattering/AxisX", true), p.flag("Scattering/AxisY", true));
                let axes = match (x, y) {
                    (true, false) => 1,
                    (false, true) => 2,
                    _ => 0,
                };
                page.choice(Does::ScatterAxes, "Scatter", &["Both ways", "Along", "Across"], axes);
                option_rows(p, &mut page, "Mirror", "Mirror", true);
                let (h, v) = (p.flag("HorizontalMirrorEnabled", false), p.flag("VerticalMirrorEnabled", false));
                let mirror = match (h, v) {
                    (true, true) => 2,
                    (false, true) => 1,
                    _ => 0,
                };
                page.choice(Does::MirrorAxes, "Mirror", &["Across", "Down", "Both"], mirror);
            }
            6 => {
                option_rows(p, &mut page, "Darken", "Darken", true);
                option_rows(p, &mut page, "Mix", "Mix", true);
            }
            _ => texture_page(p, &mut page),
        }
        self.rows = page.does.clone();
        show(&page.json());
    }
}

/// A page being written: its rows as the app's panel takes them, and what each does.
struct Page {
    title: String,
    rows: Vec<String>,
    does: Vec<Does>,
}

/// Text as JSON writes it.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A number as JSON writes it.
fn number(v: f32) -> String {
    if v.is_finite() { format!("{v}") } else { "0".to_owned() }
}

/// The most rows a page has.
const MOST_ROWS: usize = 16;

impl Page {
    fn new(name: &str) -> Page {
        let title: String = name.chars().take(40).collect();
        Page { title, rows: Vec::new(), does: Vec::new() }
    }

    fn row(&mut self, json: String, does: Does) {
        if self.rows.len() < MOST_ROWS {
            self.rows.push(json);
            self.does.push(does);
        }
    }

    fn text(&mut self, text: &str) {
        let text: String = text.chars().take(400).collect();
        self.row(format!(r#"{{ "type": "text", "text": {} }}"#, quoted(&text)), Does::Nothing);
    }

    #[allow(clippy::too_many_arguments)]
    fn slider(&mut self, does: Does, label: &str, least: f32, most: f32, step: f32, value: f32, unit: &str) {
        let value = value.clamp(least, most);
        self.row(
            format!(
                r#"{{ "type": "slider", "label": {}, "least": {}, "most": {}, "step": {}, "value": {}, "unit": {} }}"#,
                quoted(label),
                number(least),
                number(most),
                number(step),
                number(value),
                quoted(unit)
            ),
            does,
        );
    }

    fn switch(&mut self, does: Does, label: &str, on: bool) {
        self.row(format!(r#"{{ "type": "switch", "label": {}, "on": {on} }}"#, quoted(label)), does);
    }

    fn choice(&mut self, does: Does, label: &str, options: &[&str], chosen: usize) {
        let options: Vec<String> =
            options.iter().take(8).map(|o| quoted(&o.chars().take(24).collect::<String>())).collect();
        let chosen = chosen.min(options.len().saturating_sub(1));
        self.row(
            format!(
                r#"{{ "type": "choice", "label": {}, "options": [{}], "chosen": {chosen} }}"#,
                quoted(label),
                options.join(", ")
            ),
            does,
        );
    }

    fn button(&mut self, does: Does, label: &str) {
        self.row(format!(r#"{{ "type": "button", "label": {} }}"#, quoted(label)), does);
    }

    fn json(&self) -> String {
        format!(r#"{{ "title": {}, "rows": [{}] }}"#, quoted(&self.title), self.rows.join(", "))
    }
}

/// The preset's `Brush` element, its tip.
fn tip_element(p: &Preset) -> Option<crate::xml::Element> {
    p.element("brush_definition")
}

fn tip_page(p: &Preset, page: &mut Page) {
    let Some(tip) = tip_element(p) else {
        page.text("This brush's tip can't be read.");
        return;
    };
    let attr = |name: &str, child: bool| -> f32 {
        if child { tip.child("MaskGenerator").and_then(|g| g.number(name)) } else { tip.number(name) }.unwrap_or(0.0)
    };
    let set = |name: &str, child: bool, per: f32| Does::Attribute { child, name: name.to_owned(), per };
    if tip.attribute("type") == Some("auto_brush") {
        let g = tip.child("MaskGenerator");
        let shape = match g.and_then(|g| g.attribute("type")) {
            Some("rect") => "rectangle",
            _ => "circle",
        };
        let edge = match g.and_then(|g| g.attribute("id")) {
            Some("soft") => "soft",
            Some("gauss") => "Gaussian",
            _ => "default",
        };
        page.text(&format!("An auto tip: a {shape}, its edge {edge}."));
        page.slider(set("diameter", true, 1.0), "Diameter", 1.0, 1000.0, 1.0, attr("diameter", true), " px");
        page.slider(set("ratio", true, 0.01), "Ratio", 1.0, 100.0, 1.0, attr("ratio", true) * 100.0, "%");
        page.slider(set("hfade", true, 0.01), "Fade across", 0.0, 100.0, 1.0, attr("hfade", true) * 100.0, "%");
        page.slider(set("vfade", true, 0.01), "Fade down", 0.0, 100.0, 1.0, attr("vfade", true) * 100.0, "%");
        page.slider(set("spikes", true, 1.0), "Spikes", 2.0, 20.0, 1.0, attr("spikes", true).max(2.0), "");
        page.slider(set("density", false, 0.01), "Density", 0.0, 100.0, 1.0, attr("density", false) * 100.0, "%");
        page.slider(
            set("randomness", false, 0.01),
            "Randomness",
            0.0,
            100.0,
            1.0,
            attr("randomness", false) * 100.0,
            "%",
        );
    } else {
        let file = tip.attribute("filename").unwrap_or("its picture");
        page.text(&format!("A picture tip: {file}."));
        page.slider(set("scale", false, 0.01), "Scale", 1.0, 1000.0, 1.0, attr("scale", false) * 100.0, "%");
    }
    let degrees = attr("angle", false).to_degrees().rem_euclid(360.0);
    page.slider(set("angle", false, std::f32::consts::PI / 180.0), "Angle", 0.0, 360.0, 1.0, degrees, "°");
    if p.get("MaskingBrush/Enabled").is_some() {
        page.switch(
            Does::Flag("MaskingBrush/Enabled".to_owned()),
            "Masking brush",
            p.flag("MaskingBrush/Enabled", false),
        );
    }
    page.button(Does::Reset, "Back to the preset as it came");
}

fn spacing_page(p: &Preset, page: &mut Page) {
    let Some(tip) = tip_element(p) else { return };
    let auto = matches!(tip.attribute("useAutoSpacing"), Some("1" | "true"));
    page.switch(Does::AutoSpacing, "Auto spacing", auto);
    if auto {
        let coeff = tip.number("autoSpacingCoeff").unwrap_or(1.0);
        page.slider(
            Does::Attribute { child: false, name: "autoSpacingCoeff".to_owned(), per: 0.01 },
            "Spacing",
            10.0,
            1000.0,
            1.0,
            coeff * 100.0,
            "%",
        );
    } else {
        let spacing = tip.number("spacing").unwrap_or(0.1);
        page.slider(
            Does::Attribute { child: false, name: "spacing".to_owned(), per: 0.01 },
            "Spacing",
            1.0,
            1000.0,
            1.0,
            spacing * 100.0,
            "%",
        );
    }
    page.switch(Does::Flag("Spacing/Isotropic".to_owned()), "The same every way", p.flag("Spacing/Isotropic", false));
    option_rows(p, page, "Spacing", "Spacing", true);
}

fn texture_page(p: &Preset, page: &mut Page) {
    let on = p.flag("Texture/Pattern/Enabled", false);
    page.switch(Does::Flag("Texture/Pattern/Enabled".to_owned()), "Texture", on);
    if let Some(name) = p.get("Texture/Pattern/Name").or_else(|| p.get("Texture/Pattern/PatternFileName")) {
        page.text(&format!("Its pattern: {}.", name.trim()));
    }
    let k = |key: &str, otherwise: f32| p.number(&format!("Texture/Pattern/{key}"), otherwise);
    let num = |key: &str, per: f32| Does::Number { key: format!("Texture/Pattern/{key}"), per };
    page.slider(num("Scale", 0.01), "Scale", 1.0, 1000.0, 1.0, k("Scale", 1.0) * 100.0, "%");
    page.slider(num("Brightness", 0.01), "Brightness", -100.0, 100.0, 1.0, k("Brightness", 0.0) * 100.0, "%");
    page.slider(num("Contrast", 0.01), "Contrast", 0.0, 200.0, 1.0, k("Contrast", 1.0) * 100.0, "%");
    let mode = k("TexturingMode", 0.0) as u32;
    let names: Vec<&str> = TEXTURE_MODES.iter().map(|(_, n)| *n).collect();
    let at = TEXTURE_MODES.iter().position(|(m, _)| *m == mode).unwrap_or(0);
    page.choice(Does::TextureMode, "Texturing", &names, at);
    page.switch(Does::Flag("Texture/Pattern/Invert".to_owned()), "Inverted", p.flag("Texture/Pattern/Invert", false));
    page.switch(
        Does::Flag("Texture/Pattern/UseSoftTexturing".to_owned()),
        "Soft texturing",
        p.flag("Texture/Pattern/UseSoftTexturing", false),
    );
    option_rows(p, page, "Texture/Strength/", "Strength", true);
}

/// An option's rows: whether it follows the pen (where it can be switched off), its strength, the
/// sensor it follows and its curve.
fn option_rows(p: &Preset, page: &mut Page, id: &str, label: &str, checkable: bool) {
    if checkable {
        let key = format!("Pressure{id}");
        page.switch(Does::Flag(key.clone()), &format!("{label} follows the pen"), p.flag(&key, false));
    }
    let (per, most, unit) = match id {
        "Rotation" => (1.0 / 180.0, 180.0, "°"),
        "Spacing" | "Scatter" => (0.01, 500.0, "%"),
        _ => (0.01, 100.0, "%"),
    };
    let strength = p.number(&format!("{id}Value"), if id == "Spacing" || id == "Scatter" { 5.0 } else { 1.0 });
    page.slider(Does::Number { key: format!("{id}Value"), per }, label, 0.0, most, 1.0, strength / per, unit);
    let sensors = sensor_ids(p, id);
    let chosen = sensors.first().and_then(|s| SENSORS.iter().position(|(k, _)| k == s)).unwrap_or(0);
    let names: Vec<&str> = SENSORS.iter().map(|(_, n)| *n).collect();
    page.choice(Does::Sensor(id.to_owned()), &format!("{label} by"), &names, chosen);
    if sensors.len() > 1 {
        page.text(&format!("{label} follows {} together.", sensors.join(", ")));
    }
    let curve = p.get(&format!("{id}commonCurve")).unwrap_or("0,0;1,1;").trim().to_owned();
    let mut names: Vec<&str> = CURVES.iter().map(|(n, _)| *n).collect();
    let at = CURVES.iter().position(|(_, c)| same_curve(c, &curve));
    let chosen = at.unwrap_or_else(|| {
        names.push("Its own");
        names.len() - 1
    });
    page.choice(Does::Curve(id.to_owned()), &format!("{label} curve"), &names, chosen);
}

/// Whether two curves are the same, near enough.
fn same_curve(a: &str, b: &str) -> bool {
    match (Curve::parse(a), Curve::parse(b)) {
        (None, None) => true,
        (Some(x), Some(y)) => (0..=16).all(|i| (x.at(i as f32 / 16.0) - y.at(i as f32 / 16.0)).abs() < 0.02),
        _ => false,
    }
}

/// The sensors option `id` follows, by Krita's ids.
fn sensor_ids(p: &Preset, id: &str) -> Vec<String> {
    let Some(root) = p.element(&format!("{id}Sensor")) else { return vec!["pressure".to_owned()] };
    if root.attribute("id") == Some("sensorslist") {
        root.elements().filter_map(|e| e.attribute("id").map(str::to_owned)).collect()
    } else {
        root.attribute("id").map(|s| vec![s.to_owned()]).unwrap_or_default()
    }
}

/// `p` changed as the row that does `does` was set to `value`.
fn change(p: &mut Preset, does: &Does, value: f32) {
    let set = |p: &mut Preset, key: &str, v: String| {
        p.params.insert(key.to_owned(), v);
    };
    match does {
        Does::Number { key, per } => set(p, key, number(value * per)),
        Does::Flag(key) => set(p, key, (value >= 0.5).to_string()),
        Does::Attribute { child, name, per } => {
            if let Some(mut tip) = tip_element(p) {
                let v = number(value * per);
                let target = if *child { tip.child_mut("MaskGenerator") } else { Some(&mut tip) };
                if let Some(e) = target {
                    e.set_attribute(name, &v);
                }
                set(p, "brush_definition", tip.to_xml());
            }
        }
        Does::AutoSpacing => {
            if let Some(mut tip) = tip_element(p) {
                tip.set_attribute("useAutoSpacing", if value >= 0.5 { "1" } else { "0" });
                set(p, "brush_definition", tip.to_xml());
            }
        }
        Does::Sensor(id) => {
            let Some((sensor, _)) = SENSORS.get(value.max(0.0) as usize) else { return };
            set(p, &format!("{id}Sensor"), format!(r#"<!DOCTYPE params> <params id="{sensor}"/> "#));
            set(p, &format!("{id}UseSameCurve"), "true".to_owned());
            set(p, &format!("{id}UseCurve"), "true".to_owned());
        }
        Does::Curve(id) => {
            if let Some((_, curve)) = CURVES.get(value.max(0.0) as usize) {
                set(p, &format!("{id}commonCurve"), (*curve).to_owned());
                set(p, &format!("{id}UseSameCurve"), "true".to_owned());
            }
        }
        Does::PaintingMode => set(p, "PaintOpAction", if value >= 0.5 { "2" } else { "1" }.to_owned()),
        Does::ScatterAxes => {
            let (x, y) = match value as u32 {
                1 => (true, false),
                2 => (false, true),
                _ => (true, true),
            };
            set(p, "Scattering/AxisX", x.to_string());
            set(p, "Scattering/AxisY", y.to_string());
        }
        Does::MirrorAxes => {
            let (h, v) = match value as u32 {
                1 => (false, true),
                2 => (true, true),
                _ => (true, false),
            };
            set(p, "HorizontalMirrorEnabled", h.to_string());
            set(p, "VerticalMirrorEnabled", v.to_string());
        }
        Does::TextureMode => {
            if let Some((mode, _)) = TEXTURE_MODES.get(value.max(0.0) as usize) {
                set(p, "Texture/Pattern/TexturingMode", mode.to_string());
            }
        }
        Does::Page | Does::Reset | Does::Nothing => {}
    }
}

impl Preset {
    /// Its XML again, as Krita writes a preset: its settings, and the pictures it carries.
    pub fn to_xml(&self) -> String {
        let escape = |t: &str| t.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
        let mut out = format!(r#"<Preset name="{}" paintopid="{}">"#, escape(&self.name), escape(&self.paintop));
        for (name, value) in &self.params {
            // a value can't hold the end of its CDATA: split it there
            let value = value.replace("]]>", "]]]]><![CDATA[>");
            out.push_str(&format!(r#"<param name="{}" type="string"><![CDATA[{value}]]></param>"#, escape(name)));
        }
        if !self.resources.is_empty() {
            out.push_str("<resources>");
            for Resource { file, name, base64 } in &self.resources {
                out.push_str(&format!(
                    r#"<resource name="{}" filename="{}"><![CDATA[{base64}]]></resource>"#,
                    escape(name),
                    escape(file),
                ));
            }
            out.push_str("</resources>");
        }
        out.push_str("</Preset>");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preset_written_again_reads_the_same() {
        let text = r#"<Preset name="A &amp; B" paintopid="paintbrush"><param name="SizeValue" type="string"><![CDATA[0.5]]></param><param name="brush_definition" type="string"><![CDATA[<Brush type="auto_brush" spacing="0.1"><MaskGenerator diameter="40"/></Brush>]]></param><resources><resource name="t" filename="t.png" type="brushes"><![CDATA[aGVsbG8=]]></resource></resources></Preset>"#;
        let p = Preset::read(text).unwrap();
        let again = Preset::read(&p.to_xml()).unwrap();
        assert_eq!(p, again);
    }

    #[test]
    fn a_change_on_a_page_reaches_the_brush_s_settings() {
        let text = r#"<Preset name="A" paintopid="paintbrush"><param name="brush_definition" type="string"><![CDATA[<Brush type="auto_brush" spacing="0.1"><MaskGenerator type="circle" id="default" diameter="40"/></Brush>]]></param></Preset>"#;
        let mut studio = Studio { preset: Preset::read(text), ..Studio::default() };
        studio.show();
        // the diameter on the tip page
        let row =
            studio.rows.iter().position(|d| matches!(d, Does::Attribute { name, .. } if name == "diameter")).unwrap();
        studio.touched(row, 120.0);
        let tip = tip_element(studio.preset.as_ref().unwrap()).unwrap();
        assert_eq!(tip.child("MaskGenerator").unwrap().number("diameter"), Some(120.0));
        // the size page: its curve
        studio.touched(0, 2.0);
        let curve = studio.rows.iter().position(|d| matches!(d, Does::Curve(_))).unwrap();
        studio.touched(curve, 5.0);
        assert_eq!(studio.preset.as_ref().unwrap().get("SizecommonCurve"), Some("0,1;1,0;"));
        // every page holds up as a panel
        for page in 0..PAGES.len() {
            studio.touched(0, page as f32);
            assert!(studio.rows.len() <= MOST_ROWS);
        }
    }
}
