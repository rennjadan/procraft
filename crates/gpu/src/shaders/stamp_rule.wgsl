// Stamps laid by their stroke's rule pixel by pixel, where drawing them "over" or keeping the
// strongest can't follow it: a stroke building up gradually (each stamp adds its cover of what
// is left, squared) and one building up towards its transfer, never past it and never back.
// Each pixel goes through the stamps over its tile in order, as its own paint stands after each,
// and is written once. Appended to stamp.wgsl, whose stamp_paint shades each stamp as the
// fragment shader does.

// The part of the canvas worked on: its corner, its tiles across, and the rule (1: gradual,
// 2: towards the transfer); then each tile's (first, count) into the stamp list after them.
@group(0) @binding(1) var<storage, read> rule_stamps: array<f32>;
@group(0) @binding(2) var<storage, read> rule_tiles: array<u32>;
// the stroke as it stands before these stamps, and where the part goes, a row of RULE_ROW
// pixels each, as half floats
@group(0) @binding(3) var rule_before: texture_2d<f32>;
@group(0) @binding(4) var<storage, read_write> rule_out: array<vec2<u32>>;

const RULE_TILE: u32 = 16u;
const RULE_ROW: u32 = 1024u;
// floats a stamp instance takes (see the canvas's StampInstance)
const RULE_STAMP_FLOATS: u32 = 28u;

// what the fragment shader is given for stamp `k` at canvas point `p`, if its quad reaches it
fn rule_stamp(k: u32, p: vec2<f32>, out: ptr<function, StampOut>) -> bool {
    let b = k * RULE_STAMP_FLOATS;
    let center = vec2<f32>(rule_stamps[b], rule_stamps[b + 1u]);
    let radius = rule_stamps[b + 2u];
    let angle = rule_stamps[b + 4u];
    let roundness = rule_stamps[b + 5u];
    let r = max(radius, select(0.25, 0.01, su.shape >= 0));
    let half = vec2<f32>(r, max(r * roundness, 1e-3));
    let d = p - center;
    let c = cos(angle);
    let s = sin(angle);
    let q = vec2<f32>(d.x * c + d.y * s, -d.x * s + d.y * c) / half;
    // the quad, a pixel to spare past the stamp
    let reach = (half + 1.0) / half;
    if (abs(q.x) > reach.x || abs(q.y) > reach.y) {
        return false;
    }
    let grain_offset = vec2<f32>(rule_stamps[b + 6u], rule_stamps[b + 7u]);
    let flip = vec2<f32>(rule_stamps[b + 8u], rule_stamps[b + 9u]);
    let tint = vec4<f32>(rule_stamps[b + 10u], rule_stamps[b + 11u], rule_stamps[b + 12u], rule_stamps[b + 13u]);
    let look = vec4<f32>(rule_stamps[b + 14u], rule_stamps[b + 15u], rule_stamps[b + 16u], rule_stamps[b + 17u]);
    let more = vec4<f32>(rule_stamps[b + 18u], rule_stamps[b + 19u], rule_stamps[b + 20u], rule_stamps[b + 21u]);
    let turn = rule_stamps[b + 23u];
    let gc = cos(turn);
    let gs = sin(turn);
    (*out).pos = vec4<f32>(p, 0.0, 1.0);
    (*out).q = q;
    (*out).radius = r;
    (*out).alpha = rule_stamps[b + 3u];
    (*out).grain_at = grain_offset + vec2<f32>(d.x * gc - d.y * gs, d.x * gs + d.y * gc);
    (*out).fill = flip * half / (half + QUAD_REACH);
    (*out).color = mix(tinted(su.color.rgb, tint, rule_stamps[b + 22u]), su.secondary.rgb, look.z);
    (*out).look = look;
    (*out).grain_press = more.x;
    (*out).grain_bleed = more.y;
    (*out).transfer = more.z;
    (*out).layer = su.shape + i32(rule_stamps[b + 24u]);
    (*out).finish = vec3<f32>(rule_stamps[b + 25u], rule_stamps[b + 26u], rule_stamps[b + 27u]);
    return true;
}

@compute @workgroup_size(16, 16)
fn cs_stamp_rule(@builtin(workgroup_id) tile: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>) {
    let corner = vec2<i32>(bitcast<i32>(rule_tiles[0]), bitcast<i32>(rule_tiles[1]));
    let across = rule_tiles[2];
    let rule = rule_tiles[3];
    let at = tile.xy * RULE_TILE + local.xy;
    let px = corner + vec2<i32>(at);
    let size = vec2<i32>(textureDimensions(rule_before));
    if (px.x >= size.x || px.y >= size.y) {
        return;
    }
    let entry = 4u + 2u * (tile.y * across + tile.x);
    let first = rule_tiles[entry];
    let count = rule_tiles[entry + 1u];
    var paint = textureLoad(rule_before, px, 0);
    let p = vec2<f32>(px) + 0.5;
    var stamp: StampOut;
    for (var j = 0u; j < count; j++) {
        if (!rule_stamp(rule_tiles[first + j], p, &stamp)) {
            continue;
        }
        let laid = stamp_paint(stamp);
        let cover = laid.a;
        if (cover <= 0.0) {
            continue;
        }
        let a = clamp(paint.a, 0.0, 1.0);
        // how far towards the stamp's colour the pixel goes, drawn "over": what the rule adds,
        // over what was left
        var w = 0.0;
        if (rule == 1u) {
            // its cover of what is left, squared
            w = cover * (1.0 - a);
        } else {
            // towards its transfer by its cover, never past it, and never back from what is there
            let t = clamp(stamp.transfer, 0.0, 1.0);
            w = select(0.0, cover * (t - a) / max(1.0 - a, 1e-6), t > a);
        }
        w = clamp(w, 0.0, 1.0);
        paint = vec4<f32>(stamp.color * w, w) + paint * (1.0 - w);
    }
    let o = at.y * RULE_ROW + at.x;
    rule_out[o] = vec2<u32>(pack2x16float(paint.rg), pack2x16float(paint.ba));
}
