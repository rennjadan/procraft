//! Textures on a GPU whose new textures start out stale, and every texture made and written to
//! through here (see [`Gpu::texture`], [`Gpu::copy_texture_to_texture`], [`Gpu::write_texture`]).
//!
//! Intel's Iris Xe graphics on Direct3D 12 get each texture as a resource of its own whose memory
//! isn't cleared first (wgpu makes them so on these GPUs only, gfx-rs/wgpu#10651), and their driver
//! leaves out of a copy into such a texture every block of 4 by 4 texels the copy would write all
//! zero: those blocks keep whatever the memory last held, another texture's pixels. wgpu clears a
//! texture before its first use unless that use writes all of it, as a copy of a whole tile or
//! layer does, and its own clear of a texture it can't draw into is such a copy itself. So boxes
//! of stale colour came out wherever a fresh texture was copied a picture with clear parts into:
//! the layer around strokes, the tiles wet paint keeps, undo steps, and so the layers' and the
//! brushes' pictures (#386). Here, on those GPUs, every texture is made one that can be drawn into
//! and is cleared by drawing as it is made, before anything else reaches it: what a copy then
//! leaves out is clear.
//!
//! Tests on other GPUs can make textures as such a GPU's (see [`Gpu::emulate_stale_textures`]):
//! with the clear, or without it, when a copy into a texture not yet written leaves the blocks it
//! would write all zero holding [`STALE`] instead, as the memory would.
//!
//! PowerVR's OpenGL ES driver (a Rogue GE8320's) copies a texture (wgpu's `glCopyTexSubImage2D`)
//! from what it held before what was drawn into it just before: the composite, copied the picture
//! of the layers below the one worked on as soon as that was drawn, came out clear but for the
//! layer worked on, and only the boxes composited again later showed the drawing. Drawn into and
//! then read by a draw, a texture holds what was drawn on that GPU, so there copies are drawn
//! instead (see [`draws_copies`], [`Gpu::copies_drawn`]): each texel read and written as it is,
//! between textures of one format that can be (others are copied as before).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use crate::Gpu;

/// How a device's new textures start out (see the module's note).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaleTextures {
    /// Cleared before they are first read (wgpu's own doing): every GPU but those below.
    Fresh,
    /// Stale until written, so each is made one that can be drawn into and is cleared by drawing
    /// as it is made: Iris Xe on Direct3D 12.
    Cleared,
    /// Stale, and left so (a test's, to see what the clear keeps from happening): a copy into a
    /// texture not yet written leaves the blocks it would write all zero holding [`STALE`].
    Left,
}

impl StaleTextures {
    fn from_u8(v: u8) -> StaleTextures {
        match v {
            1 => StaleTextures::Cleared,
            2 => StaleTextures::Left,
            _ => StaleTextures::Fresh,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            StaleTextures::Fresh => 0,
            StaleTextures::Cleared => 1,
            StaleTextures::Left => 2,
        }
    }
}

/// What a stale texture's blocks hold where a copy left them out, as a test makes them (see
/// [`Gpu::emulate_stale_textures`]): another texture's colour, as the memory would hold.
pub const STALE: [u8; 4] = [200, 30, 220, 250];

/// [`STALE`] as a colour to clear a texture of a float or (`uint`) unsigned integer format to.
fn stale_colour(uint: bool) -> wgpu::Color {
    let [r, g, b, a] = STALE.map(|c| if uint { f64::from(c) } else { f64::from(c) / 255.0 });
    wgpu::Color { r, g, b, a }
}

/// A layer a copy leaves stale blocks in (see [`Gpu::copy_as_the_driver`]), and whether all of it
/// was stale before the copy.
pub(crate) struct StaleLayer {
    layer: u32,
    all: bool,
}

/// Whether new textures start stale on a GPU (see the module's note): Iris Xe on Direct3D 12, as
/// wgpu tells them (by name, as it does itself).
pub fn starts_stale(info: &wgpu::AdapterInfo) -> bool {
    info.backend == wgpu::Backend::Dx12 && info.name.contains("Iris(R) Xe")
}

/// A part of a texture not yet written, made while textures were made as stale ones without the
/// clear (see [`StaleTextures::Left`]): the texture, its mip level and layer, and whether it was
/// asked for as one to draw into (wgpu then clears it by drawing, before any copy that doesn't
/// write all of it).
struct Unwritten {
    texture: wgpu::Texture,
    mip: u32,
    layer: u32,
    drawn_into: bool,
}

/// A device's textures as stale ones (see the module's note).
pub(crate) struct Stale {
    how: AtomicU8,
    /// Set by a test (see [`Gpu::emulate_stale_textures`]): copies are made as the driver makes
    /// them.
    emulated: std::sync::atomic::AtomicBool,
    unwritten: Mutex<Vec<Unwritten>>,
    /// The pipelines a copy as the driver makes it draws with, by the texture format drawn into.
    copiers: Mutex<Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>>,
    /// Texture copies are drawn on this device (see the module's note and [`draws_copies`]), how
    /// many have been and how many couldn't be (copied as before; the first told in the log), and
    /// the pipelines they are drawn with, by the format drawn into.
    drawn: AtomicBool,
    copies_drawn: AtomicU64,
    copies_not_drawn: AtomicU64,
    drawers: Mutex<Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>>,
}

impl Stale {
    pub(crate) fn new(starts_stale: bool, draws_copies: bool) -> Stale {
        let how = if starts_stale { StaleTextures::Cleared } else { StaleTextures::Fresh };
        Stale {
            how: AtomicU8::new(how.as_u8()),
            emulated: std::sync::atomic::AtomicBool::new(false),
            unwritten: Mutex::new(Vec::new()),
            copiers: Mutex::new(Vec::new()),
            drawn: AtomicBool::new(draws_copies),
            copies_drawn: AtomicU64::new(0),
            copies_not_drawn: AtomicU64::new(0),
            drawers: Mutex::new(Vec::new()),
        }
    }
}

/// Whether texture copies are drawn on a GPU (see the module's note): PowerVR's on OpenGL ES,
/// as wgpu tells them.
pub fn draws_copies(info: &wgpu::AdapterInfo) -> bool {
    info.backend == wgpu::Backend::Gl && info.name.to_ascii_lowercase().contains("powervr")
}

/// A drawn copy (see [`Gpu::copies_drawn`]): a triangle over the viewport, which lies over the
/// texture drawn into where `source` would to put each of its texels where it goes (the scissor
/// keeping to the copy), each texel read where it lies in `source`. `{T}` is the texel's type.
const DRAWN_COPY: &str = r"
@group(0) @binding(0) var source: texture_2d<{T}>;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) texel: vec2<f32>,
}

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let corner = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: Out;
    out.position = vec4<f32>(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);
    out.texel = corner * vec2<f32>(textureDimensions(source));
    return out;
}

@fragment
fn fs(@location(0) texel: vec2<f32>) -> @location(0) vec4<{T}> {
    return textureLoad(source, vec2<i32>(floor(texel)), 0);
}
";

/// [`DRAWN_COPY`] for texels of unsigned integers (`uint`) or floats.
pub(crate) fn drawn_copy_source(uint: bool) -> String {
    DRAWN_COPY.replace("{T}", if uint { "u32" } else { "f32" })
}

/// The 4 by 4 blocks of a copy into a stale texture, as its driver writes them (see the module's
/// note): those the copy (in `copied`, from `at` on in the texture drawn into, `size` across)
/// writes all zero keep what the memory held. `{T}` is the texel's type, `{WRITTEN}` whether the
/// texel at `q` has a channel of the format's other than zero (a format's missing ones read as 0
/// or 1).
const COPY_AS_THE_DRIVER: &str = r"
struct Part {
    at: vec2<u32>,
    size: vec2<u32>,
    stale: vec4<f32>,
}

@group(0) @binding(0) var<uniform> part: Part;
@group(0) @binding(1) var copied: texture_2d<{T}>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = array(vec2(-1.0, -3.0), vec2(-1.0, 1.0), vec2(3.0, 1.0));
    return vec4(p[i], 0.0, 1.0);
}

// whether any texel the copy writes into the block of `p` is other than zero
fn block_written(p: vec2<u32>) -> bool {
    let corner = p / 4u * 4u;
    for (var dy = 0u; dy < 4u; dy = dy + 1u) {
        for (var dx = 0u; dx < 4u; dx = dx + 1u) {
            let q = corner + vec2(dx, dy);
            if (all(q >= part.at) && all(q < part.at + part.size)) {
                if ({WRITTEN}) {
                    return true;
                }
            }
        }
    }
    return false;
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<{T}> {
    let p = vec2<u32>(pos.xy);
    if (block_written(p)) {
        return textureLoad(copied, p - part.at, 0);
    }
    return vec4<{T}>(part.stale * {SCALE});
}
";

impl Gpu {
    /// How this device's new textures start out (see the module's note).
    pub fn stale_textures(&self) -> StaleTextures {
        StaleTextures::from_u8(self.stale.how.load(Ordering::Relaxed))
    }

    /// Whether new textures start stale here, so each is cleared as it is made (see the module's
    /// note).
    pub fn textures_start_stale(&self) -> bool {
        self.stale_textures() != StaleTextures::Fresh
    }

    /// Textures made from now on as `how` says, on this device and its clones, copies into them
    /// made as the driver of a GPU whose textures start stale makes them (see the module's note):
    /// [`StaleTextures::Fresh`] as this GPU makes them. For tests on other GPUs.
    #[doc(hidden)]
    pub fn emulate_stale_textures(&self, how: StaleTextures) {
        self.stale.how.store(how.as_u8(), Ordering::Relaxed);
        self.stale.emulated.store(how != StaleTextures::Fresh, Ordering::Relaxed);
        if how == StaleTextures::Fresh {
            self.stale.unwritten.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
        }
    }

    /// Whether a texture as `desc` asks can be cleared by drawing into it: a 2D colour texture of
    /// a format this GPU draws into.
    fn clears_by_drawing(&self, desc: &wgpu::TextureDescriptor<'_>) -> bool {
        let format = desc.format;
        desc.dimension == wgpu::TextureDimension::D2
            && desc.sample_count == 1
            && !format.has_depth_aspect()
            && !format.has_stencil_aspect()
            && self
                .adapter
                .get_texture_format_features(format)
                .allowed_usages
                .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
    }

    /// Whether texture copies are drawn on this device (see the module's note).
    pub fn copies_drawn(&self) -> bool {
        self.stale.drawn.load(Ordering::Relaxed)
    }

    /// Texture copies drawn from now on (`on`), on this device and its clones, as on a GPU whose
    /// copies are (see the module's note), textures made from now on made so they can be: for
    /// tests on other GPUs.
    #[doc(hidden)]
    pub fn draw_copies(&self, on: bool) {
        self.stale.drawn.store(on, Ordering::Relaxed);
    }

    /// How many texture copies have been drawn on this device (see [`Gpu::copies_drawn`]).
    #[doc(hidden)]
    pub fn copies_drawn_count(&self) -> u64 {
        self.stale.copies_drawn.load(Ordering::Relaxed)
    }

    /// How many texture copies couldn't be drawn where copies are (copied as before).
    #[doc(hidden)]
    pub fn copies_not_drawn_count(&self) -> u64 {
        self.stale.copies_not_drawn.load(Ordering::Relaxed)
    }

    /// What a texture as `desc` asks is made for: where copies are drawn (see the module's note),
    /// a 2D colour texture of one layer and level that is copied to or from is also one to read
    /// and to draw into, so that its copies can be drawn, where its format allows.
    fn usage_for_copies(&self, desc: &wgpu::TextureDescriptor<'_>) -> wgpu::TextureUsages {
        let copied = desc.usage.intersects(wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST);
        let single = desc.size.depth_or_array_layers == 1 && desc.mip_level_count == 1;
        if !self.copies_drawn() || !copied || !single || texel_kind(desc.format).is_none() {
            return desc.usage;
        }
        let allowed = self.adapter.get_texture_format_features(desc.format).allowed_usages;
        let both = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT;
        if self.clears_by_drawing(desc) && allowed.contains(both) { desc.usage | both } else { desc.usage }
    }

    /// A texture made as `desc` says. Where new textures start stale (see the module's note), one
    /// that can also be drawn into, cleared by drawing every mip level and layer of it as it is
    /// made, before any copy or write reaches it. Where copies are drawn, one that can be read
    /// and drawn into when it is copied (see [`Gpu::copies_drawn`]).
    pub fn texture(&self, desc: &wgpu::TextureDescriptor<'_>) -> wgpu::Texture {
        let desc = &wgpu::TextureDescriptor { usage: self.usage_for_copies(desc), ..desc.clone() };
        let how = self.stale_textures();
        if how == StaleTextures::Fresh {
            return self.device.create_texture(desc);
        }
        let drawn = self.clears_by_drawing(desc);
        let usage = if drawn { desc.usage | wgpu::TextureUsages::RENDER_ATTACHMENT } else { desc.usage };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor { usage, ..desc.clone() });
        match how {
            StaleTextures::Cleared if drawn => self.clear_by_drawing(&texture),
            StaleTextures::Left if self.stale.emulated.load(Ordering::Relaxed) => {
                let drawn_into = desc.usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT);
                let mut unwritten = self.stale.unwritten.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                for mip in 0..desc.mip_level_count {
                    for layer in 0..desc.array_layer_count() {
                        unwritten.push(Unwritten { texture: texture.clone(), mip, layer, drawn_into });
                    }
                }
            }
            _ => {}
        }
        texture
    }

    /// [`Gpu::texture`] holding `data`: each layer's mip levels in turn, each level's rows packed
    /// (as `wgpu::util::DeviceExt::create_texture_with_data` lays them out, layer by layer).
    pub fn texture_with_data(&self, desc: &wgpu::TextureDescriptor<'_>, data: &[u8]) -> wgpu::Texture {
        let desc = wgpu::TextureDescriptor { usage: desc.usage | wgpu::TextureUsages::COPY_DST, ..desc.clone() };
        let texture = self.texture(&desc);
        let format = desc.format;
        let (block_w, block_h) = format.block_dimensions();
        let block = u64::from(format.block_copy_size(None).unwrap_or(4));
        let mut at = 0usize;
        for layer in 0..desc.array_layer_count() {
            for mip in 0..desc.mip_level_count {
                let Some(size) = desc.mip_level_size(mip) else { continue };
                let size = wgpu::Extent3d { depth_or_array_layers: 1, ..size.physical_size(format) };
                let row = u64::from(size.width / block_w) * block;
                let bytes = usize::try_from(row * u64::from(size.height / block_h)).unwrap_or(usize::MAX);
                let Some(level) = data.get(at..at.saturating_add(bytes)) else { return texture };
                self.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: mip,
                        origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                        aspect: wgpu::TextureAspect::All,
                    },
                    level,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(u32::try_from(row).unwrap_or(u32::MAX)),
                        rows_per_image: None,
                    },
                    size,
                );
                at += bytes;
            }
        }
        texture
    }

    /// Clears every mip level and layer of `texture` by drawing (made able to be drawn into).
    fn clear_by_drawing(&self, texture: &wgpu::Texture) {
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("texture cleared") });
        for mip in 0..texture.mip_level_count() {
            for layer in 0..texture.depth_or_array_layers() {
                let view = texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("texture cleared"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                });
                drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("texture cleared"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                }));
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    /// The layers of `into` a copy of `layers` layers to it reaches that are still unwritten and
    /// that it leaves stale blocks in, as the driver of a GPU whose textures start stale copies
    /// (see the module's note): every such layer, the copy writing all of it (`whole`) or wgpu not
    /// clearing it first by drawing (one not asked for as drawn into: wgpu clears it with a copy,
    /// which leaves all of it stale before this one). Those layers are written from now on.
    fn leaves_stale(&self, into: &wgpu::TexelCopyTextureInfo<'_>, layers: u32, whole: bool) -> Vec<StaleLayer> {
        if !self.stale.emulated.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let mut unwritten = self.stale.unwritten.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut stale = Vec::new();
        for layer in into.origin.z..into.origin.z.saturating_add(layers) {
            let at = unwritten
                .iter()
                .position(|u| u.texture == *into.texture && u.mip == into.mip_level && u.layer == layer);
            if let Some(i) = at {
                let part = unwritten.swap_remove(i);
                if whole || !part.drawn_into {
                    stale.push(StaleLayer { layer, all: !whole });
                }
            }
        }
        stale
    }

    /// Whether a copy of `size` to `into` writes all of the mip level (each layer it reaches).
    fn writes_whole(into: &wgpu::TexelCopyTextureInfo<'_>, size: wgpu::Extent3d) -> bool {
        let level = into.texture.size().mip_level_size(into.mip_level, into.texture.dimension());
        into.origin.x == 0 && into.origin.y == 0 && size.width >= level.width && size.height >= level.height
    }

    /// `encoder.copy_texture_to_texture`, made as the driver of a GPU whose textures start stale
    /// makes it where a test asks (see [`Gpu::emulate_stale_textures`]), and drawn where copies
    /// are drawn and it can be (see [`Gpu::copies_drawn`]).
    pub fn copy_texture_to_texture(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        from: wgpu::TexelCopyTextureInfo<'_>,
        into: wgpu::TexelCopyTextureInfo<'_>,
        size: wgpu::Extent3d,
    ) {
        let stale = self.leaves_stale(&into, size.depth_or_array_layers, Gpu::writes_whole(&into, size));
        if stale.is_empty() {
            if !self.drew_copy(encoder, &from, &into, size) {
                if self.copies_drawn() && self.stale.copies_not_drawn.fetch_add(1, Ordering::Relaxed) == 0 {
                    let (a, b) = (from.texture, into.texture);
                    log::info!(
                        "a texture copy couldn't be drawn, copied as before: {:?} {}x{}x{} level {} into {:?} \
                         {}x{}x{} level {} ({:?}, {:?})",
                        a.format(),
                        a.width(),
                        a.height(),
                        a.depth_or_array_layers(),
                        from.mip_level,
                        b.format(),
                        b.width(),
                        b.height(),
                        b.depth_or_array_layers(),
                        into.mip_level,
                        a.usage(),
                        b.usage()
                    );
                }
                encoder.copy_texture_to_texture(from, into, size);
            }
            return;
        }
        let copied = self.copied_texture(into.texture.format(), size);
        encoder.copy_texture_to_texture(from, copied.as_image_copy(), size);
        self.copy_as_the_driver(encoder, &copied, into, size, &stale);
    }

    /// `queue.write_texture`, made as the driver of a GPU whose textures start stale makes it
    /// where a test asks (see [`Gpu::emulate_stale_textures`]).
    pub fn write_texture(
        &self,
        into: wgpu::TexelCopyTextureInfo<'_>,
        data: &[u8],
        layout: wgpu::TexelCopyBufferLayout,
        size: wgpu::Extent3d,
    ) {
        let stale = self.leaves_stale(&into, size.depth_or_array_layers, Gpu::writes_whole(&into, size));
        if stale.is_empty() {
            self.queue.write_texture(into, data, layout, size);
            return;
        }
        let copied = self.copied_texture(into.texture.format(), size);
        self.queue.write_texture(copied.as_image_copy(), data, layout, size);
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("stale write") });
        self.copy_as_the_driver(&mut encoder, &copied, into, size, &stale);
        self.queue.submit([encoder.finish()]);
    }

    /// The parts of `into` a copy of `size` from a buffer to it leaves stale blocks in (see
    /// [`Gpu::leaves_stale`]); a copy that leaves some goes through `copied` (a texture of its
    /// own) and is drawn on from there.
    pub(crate) fn stale_after_buffer_copy(
        &self,
        into: &wgpu::TexelCopyTextureInfo<'_>,
        size: wgpu::Extent3d,
    ) -> Option<(Vec<StaleLayer>, wgpu::Texture)> {
        let stale = self.leaves_stale(into, size.depth_or_array_layers, Gpu::writes_whole(into, size));
        (!stale.is_empty()).then(|| (stale, self.copied_texture(into.texture.format(), size)))
    }

    /// A texture to put a copy in before it is drawn on as the driver would write it.
    fn copied_texture(&self, format: wgpu::TextureFormat, size: wgpu::Extent3d) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stale copy"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    /// Draws `copied` (`size`, its layers those of `into` in turn) into `into` as the driver of a
    /// GPU whose textures start stale copies it, on the layers `stale` names: what it would write
    /// all zero in a block, [`STALE`] instead (and all the rest of a layer stale as a whole). The
    /// other layers are copied plainly.
    pub(crate) fn copy_as_the_driver(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        copied: &wgpu::Texture,
        into: wgpu::TexelCopyTextureInfo<'_>,
        size: wgpu::Extent3d,
        stale: &[StaleLayer],
    ) {
        let format = into.texture.format();
        let uint = matches!(format.sample_type(None, None), Some(wgpu::TextureSampleType::Uint));
        let float = matches!(format.sample_type(None, None), Some(wgpu::TextureSampleType::Float { .. }));
        let drawn = into.texture.usage().contains(wgpu::TextureUsages::RENDER_ATTACHMENT);
        for z in 0..size.depth_or_array_layers {
            let layer = into.origin.z + z;
            let from = wgpu::TexelCopyTextureInfo {
                texture: copied,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z },
                aspect: wgpu::TextureAspect::All,
            };
            let one = wgpu::Extent3d { depth_or_array_layers: 1, ..size };
            let to = wgpu::TexelCopyTextureInfo { origin: wgpu::Origin3d { z: layer, ..into.origin }, ..into };
            let Some(this) = stale.iter().find(|s| s.layer == layer).filter(|_| drawn && (uint || float)) else {
                encoder.copy_texture_to_texture(from, to, one);
                continue;
            };
            let pipeline = self.copier(format, uint);
            let part: Vec<u8> = [into.origin.x, into.origin.y, size.width, size.height]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .chain(STALE.iter().flat_map(|&c| (f32::from(c) / 255.0).to_le_bytes()))
                .collect();
            let part = crate::canvas::buffer_init(&self.device, "stale copy", &part, wgpu::BufferUsages::UNIFORM);
            let source = copied.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: z,
                array_layer_count: Some(1),
                ..Default::default()
            });
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("stale copy"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: part.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&source) },
                ],
            });
            let target = into.texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_mip_level: into.mip_level,
                mip_level_count: Some(1),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..Default::default()
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("stale copy"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if this.all { wgpu::LoadOp::Clear(stale_colour(uint)) } else { wgpu::LoadOp::Load },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_scissor_rect(into.origin.x, into.origin.y, size.width, size.height);
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// The pipeline that copies into `format` as the driver of a GPU whose textures start stale
    /// does (texels of unsigned integers: `uint`), made once.
    fn copier(&self, format: wgpu::TextureFormat, uint: bool) -> wgpu::RenderPipeline {
        let mut copiers = self.stale.copiers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, p)) = copiers.iter().find(|(f, _)| *f == format) {
            return p.clone();
        }
        let written = match format.components() {
            1 => "textureLoad(copied, q - part.at, 0).x != {T}()",
            2 => "any(textureLoad(copied, q - part.at, 0).xy != vec2<{T}>())",
            3 => "any(textureLoad(copied, q - part.at, 0).xyz != vec3<{T}>())",
            _ => "any(textureLoad(copied, q - part.at, 0) != vec4<{T}>())",
        };
        let source = COPY_AS_THE_DRIVER
            .replace("{WRITTEN}", written)
            .replace("{T}", if uint { "u32" } else { "f32" })
            .replace("{SCALE}", if uint { "255.0" } else { "1.0" });
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("stale copy"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        // (read with textureLoad only: any float format binds, filterable or not)
        let sample_type =
            if uint { wgpu::TextureSampleType::Uint } else { wgpu::TextureSampleType::Float { filterable: false } };
        let binds = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("stale copy"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("stale copy"),
            bind_group_layouts: &[Some(&binds)],
            immediate_size: 0,
        });
        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("stale copy"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        copiers.push((format, pipeline.clone()));
        pipeline
    }

    /// Whether a copy of `size` from `from` into `into` can be drawn here, and if so whether its
    /// texels are unsigned integers: copies are drawn on this device (see the module's note), and
    /// it goes between two textures of one format that a draw reads and writes back as it is (no
    /// sRGB, no depth, no signed integers), within both, read from a texture of one layer and
    /// at its first level (OpenGL reads a layer of an array as nothing) and drawn into at its
    /// first level, one layer of it.
    fn copy_drawable(
        &self,
        from: &wgpu::TexelCopyTextureInfo<'_>,
        into: &wgpu::TexelCopyTextureInfo<'_>,
        size: wgpu::Extent3d,
    ) -> Option<bool> {
        if !self.copies_drawn() || from.texture == into.texture {
            return None;
        }
        let (source, target) = (from.texture, into.texture);
        let format = target.format();
        let uint = texel_kind(format)?;
        let plain = |t: &wgpu::Texture| t.dimension() == wgpu::TextureDimension::D2 && t.sample_count() == 1;
        let within = |t: &wgpu::Texture, at: wgpu::Origin3d| {
            at.x.checked_add(size.width).is_some_and(|x| x <= t.width())
                && at.y.checked_add(size.height).is_some_and(|y| y <= t.height())
                && at.z < t.depth_or_array_layers()
        };
        let drawable = source.format() == format
            && plain(source)
            && plain(target)
            && source.depth_or_array_layers() == 1
            && size.depth_or_array_layers == 1
            && size.width > 0
            && size.height > 0
            && from.mip_level == 0
            && into.mip_level == 0
            && from.aspect == wgpu::TextureAspect::All
            && into.aspect == wgpu::TextureAspect::All
            && source.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING)
            && target.usage().contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
            && within(source, from.origin)
            && within(target, into.origin);
        drawable.then_some(uint)
    }

    /// Draws a copy of `size` from `from` into `into` where it can be drawn (see
    /// [`Gpu::copy_drawable`]); false where it can't, nothing done.
    fn drew_copy(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        from: &wgpu::TexelCopyTextureInfo<'_>,
        into: &wgpu::TexelCopyTextureInfo<'_>,
        size: wgpu::Extent3d,
    ) -> bool {
        let Some(uint) = self.copy_drawable(from, into, size) else { return false };
        let pipeline = self.drawer(into.texture.format(), uint);
        let one = |base_array_layer| wgpu::TextureViewDescriptor {
            label: Some("drawn copy"),
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_mip_level: 0,
            mip_level_count: Some(1),
            base_array_layer,
            array_layer_count: Some(1),
            ..Default::default()
        };
        let source = from.texture.create_view(&one(0));
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("drawn copy"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&source) }],
        });
        let target = into.texture.create_view(&one(into.origin.z));
        // a copy of the whole level needs nothing it held (a tiled GPU then reads none of it in)
        let load = if Gpu::writes_whole(into, size) {
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
        } else {
            wgpu::LoadOp::Load
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("drawn copy"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // the source laid over the target where its part goes (as far up or left of it as need be)
        let x = into.origin.x as f32 - from.origin.x as f32;
        let y = into.origin.y as f32 - from.origin.y as f32;
        pass.set_viewport(x, y, from.texture.width() as f32, from.texture.height() as f32, 0.0, 1.0);
        pass.set_scissor_rect(into.origin.x, into.origin.y, size.width, size.height);
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        self.stale.copies_drawn.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// The pipeline copies into `format` are drawn with (texels of unsigned integers: `uint`),
    /// made once.
    fn drawer(&self, format: wgpu::TextureFormat, uint: bool) -> wgpu::RenderPipeline {
        let mut drawers = self.stale.drawers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, p)) = drawers.iter().find(|(f, _)| *f == format) {
            return p.clone();
        }
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("drawn copy"),
            source: wgpu::ShaderSource::Wgsl(drawn_copy_source(uint).into()),
        });
        // (read with textureLoad only: any float format binds, filterable or not)
        let sample_type =
            if uint { wgpu::TextureSampleType::Uint } else { wgpu::TextureSampleType::Float { filterable: false } };
        let binds = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("drawn copy"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("drawn copy"),
            bind_group_layouts: &[Some(&binds)],
            immediate_size: 0,
        });
        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("drawn copy"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        drawers.push((format, pipeline.clone()));
        pipeline
    }
}

/// How a draw reads and writes back texels of `format` exactly, if it can: as unsigned integers
/// (true) or floats (false); none for sRGB (read as linear, written back again), compressed,
/// depth, stencil and signed integer formats.
fn texel_kind(format: wgpu::TextureFormat) -> Option<bool> {
    if format.is_srgb() || format.is_compressed() {
        return None;
    }
    match format.sample_type(None, None) {
        Some(wgpu::TextureSampleType::Uint) => Some(true),
        Some(wgpu::TextureSampleType::Float { .. }) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIDE: u32 = 64;

    /// A picture whose left half is clear but for one texel (so one block of it holds paint), its
    /// right half painted.
    fn half_clear() -> Vec<u8> {
        let n = SIDE as usize;
        let mut rgba = vec![0u8; n * n * 4];
        for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = (i % n, i / n);
            if x >= n / 2 {
                *px = [(x * 4) as u8, (y * 4) as u8, 90, 255];
            }
        }
        // one texel painted in the block at (8, 8): the rest of that block stays clear
        rgba[(9 * n + 9) * 4..(9 * n + 9) * 4 + 4].copy_from_slice(&[10, 20, 30, 40]);
        rgba
    }

    /// What a picture as [`half_clear`] reads back as, copied into a texture that started stale and
    /// was left so: every block it holds all zero, [`STALE`].
    fn as_left_stale(picture: &[u8]) -> Vec<u8> {
        let n = SIDE as usize;
        let mut out = picture.to_vec();
        for by in 0..n / 4 {
            for bx in 0..n / 4 {
                let texels = || (0..16).map(|k| ((by * 4 + k / 4) * n + bx * 4 + k % 4) * 4);
                if texels().all(|i| picture[i..i + 4] == [0, 0, 0, 0]) {
                    for i in texels() {
                        out[i..i + 4].copy_from_slice(&STALE);
                    }
                }
            }
        }
        out
    }

    fn descriptor(usage: wgpu::TextureUsages) -> wgpu::TextureDescriptor<'static> {
        wgpu::TextureDescriptor {
            label: Some("stale test"),
            size: wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: usage | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }
    }

    /// The ways a picture is put in a texture: written, copied from another texture, copied from a
    /// buffer.
    #[derive(Clone, Copy, Debug)]
    enum Way {
        Written,
        FromTexture,
        FromBuffer,
    }

    /// `picture` put whole into a new texture (`usage` asked for) the way `way` says, and read back.
    fn put(gpu: &Gpu, way: Way, usage: wgpu::TextureUsages, picture: &[u8]) -> Vec<u8> {
        let layout = wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIDE * 4), rows_per_image: None };
        let size = wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 };
        let texture = gpu.texture(&descriptor(usage));
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        match way {
            Way::Written => gpu.write_texture(texture.as_image_copy(), picture, layout, size),
            Way::FromTexture => {
                // (a texture of the GPU's own making: not stale)
                let source = gpu.device.create_texture(&descriptor(wgpu::TextureUsages::empty()));
                gpu.queue.write_texture(source.as_image_copy(), picture, layout, size);
                gpu.copy_texture_to_texture(&mut encoder, source.as_image_copy(), texture.as_image_copy(), size);
            }
            Way::FromBuffer => {
                let usage = wgpu::BufferUsages::COPY_SRC;
                let buffer = crate::canvas::buffer_init(&gpu.device, "stale test", picture, usage);
                let from = wgpu::TexelCopyBufferInfo { buffer: &buffer, layout };
                gpu.copy_buffer_to_texture(&mut encoder, from, texture.as_image_copy(), size);
            }
        }
        gpu.queue.submit([encoder.finish()]);
        crate::read_texture(gpu, &texture, SIDE, SIDE).unwrap()
    }

    #[test]
    fn new_textures_start_stale_only_on_iris_xe_with_direct3d_12() {
        let gpu = |name: &str, backend| wgpu::AdapterInfo {
            name: name.into(),
            ..wgpu::AdapterInfo::new(wgpu::DeviceType::IntegratedGpu, backend)
        };
        assert!(starts_stale(&gpu("Intel(R) Iris(R) Xe Graphics", wgpu::Backend::Dx12)));
        assert!(!starts_stale(&gpu("Intel(R) Iris(R) Xe Graphics", wgpu::Backend::Vulkan)));
        assert!(!starts_stale(&gpu("Intel(R) UHD Graphics 620", wgpu::Backend::Dx12)));
        assert!(!starts_stale(&gpu("NVIDIA GeForce RTX 5060", wgpu::Backend::Dx12)));
    }

    /// A picture with clear parts put whole into a new texture, written or copied from a texture
    /// or a buffer, as on Iris Xe with Direct3D 12, whose driver leaves out the blocks the copy
    /// would write all zero: left stale, those blocks hold what the memory held (as the boxes of
    /// #386 did); with each texture cleared as it is made, the picture comes back exactly as on
    /// any other GPU. A texture to draw into, copied part of a picture, is cleared by wgpu first
    /// and keeps nothing stale; one not to draw into is cleared by wgpu with a copy, which leaves
    /// all of it stale.
    #[test]
    fn a_copy_into_a_new_texture_leaves_nothing_stale_once_textures_are_cleared_as_they_are_made() {
        let Ok(gpu) = Gpu::headless() else { return eprintln!("skipped: no GPU") };
        // (copies as Iris Xe makes them, not drawn, whatever the tests run as)
        gpu.draw_copies(false);
        let picture = half_clear();
        let stale = as_left_stale(&picture);
        let blocks = stale.chunks(4).zip(picture.chunks(4)).filter(|(a, b)| a != b).count() / 16;
        // the left half's blocks but the one painted texel's
        assert_eq!(blocks, (SIDE as usize / 8) * (SIDE as usize / 4) - 1);
        let drawn = wgpu::TextureUsages::RENDER_ATTACHMENT;
        for way in [Way::Written, Way::FromTexture, Way::FromBuffer] {
            for usage in [wgpu::TextureUsages::empty(), wgpu::TextureUsages::TEXTURE_BINDING, drawn] {
                gpu.emulate_stale_textures(StaleTextures::Fresh);
                assert_eq!(put(&gpu, way, usage, &picture), picture, "{way:?} {usage:?} as this GPU makes it");
                gpu.emulate_stale_textures(StaleTextures::Left);
                assert!(put(&gpu, way, usage, &picture) == stale, "{way:?} {usage:?} left stale");
                gpu.emulate_stale_textures(StaleTextures::Cleared);
                assert_eq!(put(&gpu, way, usage, &picture), picture, "{way:?} {usage:?} cleared as made");
            }
        }
        // the top half of the picture into a new texture, left stale: one to draw into is cleared
        // by wgpu first; one not to draw into holds what the memory held below it as well
        let layout = wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIDE * 4), rows_per_image: None };
        let part = wgpu::Extent3d { width: SIDE, height: SIDE / 2, depth_or_array_layers: 1 };
        let rows = (SIDE * SIDE / 2 * 4) as usize;
        for (usage, top, below) in [(drawn, &picture, [0; 4]), (wgpu::TextureUsages::TEXTURE_BINDING, &stale, STALE)] {
            gpu.emulate_stale_textures(StaleTextures::Left);
            let texture = gpu.texture(&descriptor(usage));
            gpu.write_texture(texture.as_image_copy(), &picture, layout, part);
            let got = crate::read_texture(&gpu, &texture, SIDE, SIDE).unwrap();
            assert!(got[..rows] == top[..rows], "{usage:?}: the top half");
            assert!(got[rows..].chunks(4).all(|p| p == below), "{usage:?}: below it");
            // cleared as made, nothing stale either way
            gpu.emulate_stale_textures(StaleTextures::Cleared);
            let texture = gpu.texture(&descriptor(usage));
            gpu.write_texture(texture.as_image_copy(), &picture, layout, part);
            let got = crate::read_texture(&gpu, &texture, SIDE, SIDE).unwrap();
            assert_eq!(&got[..rows], &picture[..rows]);
            assert!(got[rows..].iter().all(|&c| c == 0), "{usage:?}: below it, cleared");
        }
        gpu.emulate_stale_textures(StaleTextures::Fresh);
        assert_eq!(gpu.errors(), 0);
    }

    /// Made as a stale GPU's, a texture asked for without being drawn into can be drawn into all
    /// the same (that is how it is cleared), every mip level and layer of an array cleared, and
    /// one made with data holds it; a depth texture is made as asked. As this GPU makes them,
    /// textures are as asked.
    #[test]
    fn textures_made_as_a_stale_gpu_s_are_cleared_by_drawing_every_level_and_layer() {
        let Ok(gpu) = Gpu::headless() else { return eprintln!("skipped: no GPU") };
        let desc = wgpu::TextureDescriptor {
            label: Some("stale array"),
            size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 3 },
            mip_level_count: 3,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        };
        assert_eq!(gpu.texture(&desc).usage(), desc.usage);
        gpu.emulate_stale_textures(StaleTextures::Cleared);
        assert!(gpu.textures_start_stale());
        let texture = gpu.texture(&desc);
        assert_eq!(texture.usage(), desc.usage | wgpu::TextureUsages::RENDER_ATTACHMENT);
        assert_eq!((texture.mip_level_count(), texture.depth_or_array_layers()), (3, 3));
        let depth = wgpu::TextureDescriptor {
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            mip_level_count: 1,
            size: wgpu::Extent3d { width: 8, height: 8, depth_or_array_layers: 1 },
            ..desc.clone()
        };
        assert_eq!(gpu.texture(&depth).usage(), wgpu::TextureUsages::RENDER_ATTACHMENT);
        // with data: the data, each layer in turn
        let data: Vec<u8> = (0..32 * 32 * 2).map(|i| (i % 7) as u8).collect();
        let two = wgpu::TextureDescriptor {
            mip_level_count: 1,
            size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 2 },
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            ..desc.clone()
        };
        let with = gpu.texture_with_data(&two, &data);
        gpu.emulate_stale_textures(StaleTextures::Fresh);
        assert!(!gpu.textures_start_stale());
        let read = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * 32 * 2,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            with.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &read,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(32) },
            },
            wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 2 },
        );
        gpu.queue.submit([encoder.finish()]);
        read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        gpu.wait().unwrap();
        let got = read.slice(..).get_mapped_range().unwrap().to_vec();
        for row in 0..64 {
            assert_eq!(&got[row * 256..row * 256 + 32], &data[row * 32..row * 32 + 32], "row {row}");
        }
        assert_eq!(gpu.errors(), 0);
    }

    /// What clearing each new texture as it is made costs: tiles of a big brush's wet paint (512
    /// pixels a side), made one after another, as a stroke makes them. A few milliseconds a tile
    /// more at most.
    #[test]
    fn clearing_new_textures_as_they_are_made_costs_little() {
        let Ok(gpu) = Gpu::headless() else { return eprintln!("skipped: no GPU") };
        let tile = wgpu::TextureDescriptor {
            label: Some("tile"),
            size: wgpu::Extent3d { width: 512, height: 512, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        };
        let n = 48;
        let mut per_tile = [f64::MAX; 2];
        // (the quickest of a few rounds, each way in turn: other work on the GPU slows some)
        for _ in 0..5 {
            for (i, how) in [StaleTextures::Fresh, StaleTextures::Cleared].into_iter().enumerate() {
                gpu.emulate_stale_textures(how);
                let started = std::time::Instant::now();
                let made: Vec<wgpu::Texture> = (0..n).map(|_| gpu.texture(&tile)).collect();
                gpu.wait().unwrap();
                per_tile[i] = per_tile[i].min(started.elapsed().as_secs_f64() * 1000.0 / f64::from(n));
                drop(made);
            }
        }
        gpu.emulate_stale_textures(StaleTextures::Fresh);
        let more = per_tile[1] - per_tile[0];
        eprintln!(
            "a 512 px tile made in {:.3} ms, cleared as made {:.3} ms ({more:.3} ms more)",
            per_tile[0], per_tile[1]
        );
        assert!(more < 5.0, "clearing a tile as it is made took {more:.3} ms more");
        assert_eq!(gpu.errors(), 0);
    }

    /// The formats the canvas copies besides its layers' (unsigned integers, single floats, half
    /// floats), in an array's second layer: left stale, the blocks a copy writes all zero hold
    /// what the memory held, the rest as copied, the first layer (not written) clear; cleared as
    /// made, all of it as copied.
    #[test]
    fn copies_of_every_kind_of_texel_leave_stale_blocks_only_where_left_stale() {
        let Ok(gpu) = Gpu::headless() else { return eprintln!("skipped: no GPU") };
        let side = 64u32;
        for (format, texel) in [
            (wgpu::TextureFormat::R32Uint, 4u32),
            (wgpu::TextureFormat::R32Float, 4),
            (wgpu::TextureFormat::Rgba16Float, 8),
        ] {
            let row = (side * texel) as usize;
            // the left half zero, the right half bytes of 60 (a texel of each format other than zero)
            let picture: Vec<u8> = (0..row * side as usize).map(|i| if i % row < row / 2 { 0 } else { 60 }).collect();
            let desc = wgpu::TextureDescriptor {
                label: Some("stale formats"),
                size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 2 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            };
            let layout =
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row as u32), rows_per_image: None };
            let size = wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 };
            let mut got = Vec::new();
            for how in [StaleTextures::Left, StaleTextures::Cleared] {
                gpu.emulate_stale_textures(StaleTextures::Fresh);
                gpu.emulate_stale_textures(how);
                let texture = gpu.texture(&wgpu::TextureDescriptor {
                    usage: desc.usage | wgpu::TextureUsages::COPY_DST,
                    ..desc.clone()
                });
                let into = wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: 1 },
                    aspect: wgpu::TextureAspect::All,
                };
                gpu.write_texture(into, &picture, layout, size);
                let read = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: (row * side as usize * 2) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                encoder.copy_texture_to_buffer(
                    texture.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &read,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(row as u32),
                            rows_per_image: Some(side),
                        },
                    },
                    wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 2 },
                );
                gpu.queue.submit([encoder.finish()]);
                read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                gpu.wait().unwrap();
                got.push(read.slice(..).get_mapped_range().unwrap().to_vec());
            }
            gpu.emulate_stale_textures(StaleTextures::Fresh);
            let (layer0, layer1) = got[0].split_at(row * side as usize);
            assert!(layer0.iter().all(|&b| b == 0), "{format:?}: the layer not written, left stale");
            let n = texel as usize;
            for (i, (g, p)) in layer1.chunks(n).zip(picture.chunks(n)).enumerate() {
                let left = i * n % row < row / 2;
                assert!(
                    if left { g.iter().any(|&b| b != 0) } else { g == p },
                    "{format:?}: texel {i} left stale: {g:?}"
                );
            }
            assert!(got[1][..row * side as usize].iter().all(|&b| b == 0), "{format:?}: the first layer, cleared");
            assert_eq!(&got[1][row * side as usize..], &picture[..], "{format:?}: cleared as made");
        }
        assert_eq!(gpu.errors(), 0);
    }

    #[test]
    fn copies_are_drawn_only_on_powervr_gpus_with_opengl_es() {
        let gpu = |name: &str, backend| wgpu::AdapterInfo {
            name: name.into(),
            ..wgpu::AdapterInfo::new(wgpu::DeviceType::IntegratedGpu, backend)
        };
        assert!(draws_copies(&gpu("PowerVR Rogue GE8320", wgpu::Backend::Gl)));
        assert!(draws_copies(&gpu("PowerVR Rogue GE8300", wgpu::Backend::Gl)));
        assert!(draws_copies(&gpu("PowerVR B-Series BXM-8-256", wgpu::Backend::Gl)));
        assert!(!draws_copies(&gpu("PowerVR Rogue GE8320", wgpu::Backend::Vulkan)));
        assert!(!draws_copies(&gpu("Mali-G57 MC2", wgpu::Backend::Gl)));
        assert!(!draws_copies(&gpu("Adreno (TM) 750", wgpu::Backend::Gl)));
        assert!(!draws_copies(&gpu("NVIDIA GeForce RTX 5060 Laptop GPU/PCIe/SSE2", wgpu::Backend::Gl)));
    }

    /// `n` bytes of a texture's texels in `format`, made up from `seed`: any bytes, but floats
    /// kept normal (no NaN, infinity or denormal, which a GPU's shader may change on the way).
    fn texels(format: wgpu::TextureFormat, n: usize, seed: u32) -> Vec<u8> {
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        match format {
            wgpu::TextureFormat::Rgba16Float => (0..n / 2)
                .flat_map(|_| {
                    let r = next();
                    let exponent = 1 + (r >> 10) % 30;
                    let half = (r & 0x83ff) as u16 | ((exponent as u16) << 10);
                    half.to_le_bytes()
                })
                .collect(),
            wgpu::TextureFormat::R32Float => (0..n / 4)
                .flat_map(|_| {
                    let r = next();
                    let exponent = 1 + (r >> 23) % 254;
                    ((r & 0x807f_ffff) | (exponent << 23)).to_le_bytes()
                })
                .collect(),
            _ => (0..n).map(|_| next() as u8).collect(),
        }
    }

    /// Every layer of `texture` (`side` square, `texel` bytes a texel), read back.
    fn read_layers(gpu: &Gpu, texture: &wgpu::Texture, side: u32, texel: u32) -> Vec<u8> {
        let row = side * texel;
        let padded = row.div_ceil(256) * 256;
        let layers = texture.depth_or_array_layers();
        let read = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * side * layers),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &read,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(side),
                },
            },
            wgpu::Extent3d { width: side, height: side, depth_or_array_layers: layers },
        );
        gpu.queue.submit([encoder.finish()]);
        read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        gpu.wait().unwrap();
        let padded_rows = read.slice(..).get_mapped_range().unwrap().to_vec();
        padded_rows.chunks(padded as usize).flat_map(|r| r[..row as usize].to_vec()).collect()
    }

    /// A copy as a test makes it: from where in which texture to where in which, how big.
    struct CopyCase {
        name: &'static str,
        format: wgpu::TextureFormat,
        source_layers: u32,
        source_mips: u32,
        target_layers: u32,
        from: (u32, u32, u32),
        into: (u32, u32, u32),
        size: (u32, u32),
        /// Within one texture (the source is the target).
        within: bool,
        /// Its source made around the GPU's own textures, without being read by draws.
        unread: bool,
        drawn: bool,
    }

    /// Copies of every kind the canvas makes, drawn where copies are drawn and copied as before
    /// otherwise, come out texel for texel as the GPU's own copies: whole layers (the canvas's,
    /// unsigned integers as carried parts keep, half and single floats), parts of them at other
    /// places, into an array's second layer. Copies a draw can't make are copied as before: from an
    /// array's layer or a level past the first, sRGB and signed integer texels, within one texture,
    /// from a texture a draw can't read. What the texture drawn into held round a part is kept.
    #[test]
    fn drawn_copies_are_the_gpu_s_own_copies_texel_for_texel() {
        let Ok(gpu) = Gpu::headless() else { return eprintln!("skipped: no GPU") };
        let side = 64u32;
        let case = |name, format, from, into, size, drawn| CopyCase {
            name,
            format,
            source_layers: 1,
            source_mips: 1,
            target_layers: 1,
            from,
            into,
            size,
            within: false,
            unread: false,
            drawn,
        };
        use wgpu::TextureFormat as F;
        let both = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT;
        let whole = (side, side);
        let mut cases = Vec::new();
        for format in [F::Rgba8Unorm, F::R8Unorm, F::Rgba16Float, F::R32Float, F::Rgba32Uint, F::R32Uint] {
            cases.push(case("whole", format, (0, 0, 0), (0, 0, 0), whole, true));
            cases.push(case("a part elsewhere", format, (5, 7, 0), (30, 40, 0), (23, 17), true));
            cases.push(CopyCase {
                target_layers: 2,
                ..case("into an array's second layer", format, (11, 0, 0), (2, 3, 1), (40, 21), true)
            });
        }
        cases.push(CopyCase {
            source_layers: 2,
            ..case("from an array's second layer", F::Rgba8Unorm, (0, 0, 1), (0, 0, 0), whole, false)
        });
        cases.push(CopyCase {
            source_mips: 2,
            ..case("from the second level", F::Rgba8Unorm, (0, 0, 0), (0, 0, 0), (side / 2, side / 2), false)
        });
        cases.push(case("sRGB", F::Rgba8UnormSrgb, (0, 0, 0), (0, 0, 0), whole, false));
        cases.push(case("signed integers", F::Rgba8Sint, (0, 0, 0), (8, 8, 0), (20, 20), false));
        cases.push(CopyCase {
            within: true,
            target_layers: 2,
            ..case("within one texture", F::Rgba8Unorm, (0, 0, 0), (32, 32, 1), (20, 20), false)
        });
        cases.push(CopyCase {
            unread: true,
            ..case("from a texture not read", F::Rgba8Unorm, (0, 0, 0), (0, 0, 0), whole, false)
        });
        for (seed, c) in cases.iter().enumerate() {
            let texel = c.format.block_copy_size(None).unwrap();
            let mut got = Vec::new();
            for drawn in [false, true] {
                gpu.draw_copies(drawn);
                let desc = |layers, mips| wgpu::TextureDescriptor {
                    label: Some("copied"),
                    size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: layers },
                    mip_level_count: mips,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: c.format,
                    usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                };
                let filled = |t: &wgpu::Texture, layers: u32, salt: u32| {
                    for mip in 0..t.mip_level_count() {
                        let s = side >> mip;
                        let bytes = (s * s * texel) as usize;
                        for layer in 0..layers {
                            gpu.write_texture(
                                wgpu::TexelCopyTextureInfo {
                                    texture: t,
                                    mip_level: mip,
                                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                                    aspect: wgpu::TextureAspect::All,
                                },
                                &texels(c.format, bytes, salt + layer * 7 + mip * 13 + seed as u32 * 101),
                                wgpu::TexelCopyBufferLayout {
                                    offset: 0,
                                    bytes_per_row: Some(s * texel),
                                    rows_per_image: None,
                                },
                                wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
                            );
                        }
                    }
                };
                // (an array the canvas copies into is one it draws into)
                let drawn_into = if c.target_layers > 1 { both } else { wgpu::TextureUsages::empty() };
                let target = gpu.texture(&wgpu::TextureDescriptor {
                    usage: desc(c.target_layers, 1).usage | drawn_into,
                    ..desc(c.target_layers, 1)
                });
                filled(&target, c.target_layers, 1);
                let made;
                let source = if c.within {
                    &target
                } else {
                    made = if c.unread {
                        gpu.device.create_texture(&desc(c.source_layers, c.source_mips))
                    } else {
                        gpu.texture(&desc(c.source_layers, c.source_mips))
                    };
                    filled(&made, c.source_layers, 2);
                    &made
                };
                let before = gpu.copies_drawn_count();
                let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                let at = |t, (x, y, z), mip| wgpu::TexelCopyTextureInfo {
                    texture: t,
                    mip_level: mip,
                    origin: wgpu::Origin3d { x, y, z },
                    aspect: wgpu::TextureAspect::All,
                };
                gpu.copy_texture_to_texture(
                    &mut encoder,
                    at(source, c.from, c.source_mips - 1),
                    at(&target, c.into, 0),
                    wgpu::Extent3d { width: c.size.0, height: c.size.1, depth_or_array_layers: 1 },
                );
                gpu.queue.submit([encoder.finish()]);
                let drew = gpu.copies_drawn_count() - before;
                let expected =
                    drawn && c.drawn && gpu.adapter.get_texture_format_features(c.format).allowed_usages.contains(both);
                assert_eq!(drew, u64::from(expected), "{} {:?}: copies drawn (drawn: {drawn})", c.name, c.format);
                got.push(read_layers(&gpu, &target, side, texel));
            }
            gpu.draw_copies(false);
            assert!(got[0] == got[1], "{} {:?}: drawn, not as copied", c.name, c.format);
            assert_eq!(gpu.errors(), 0, "{} {:?}", c.name, c.format);
        }
        assert_eq!(gpu.errors(), 0);
    }

    /// Where copies are drawn, a texture of one layer and level that is copied is made one a draw
    /// reads and draws into, where its format allows; others, and every texture elsewhere, are
    /// made as asked.
    #[test]
    fn textures_copied_where_copies_are_drawn_can_be_read_and_drawn_into() {
        let Ok(gpu) = Gpu::headless() else { return eprintln!("skipped: no GPU") };
        gpu.draw_copies(false);
        let copied = wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST;
        let both = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT;
        let desc = |format, layers, mips, usage| wgpu::TextureDescriptor {
            label: Some("usage"),
            size: wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: layers },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        };
        let plain = desc(wgpu::TextureFormat::Rgba8Unorm, 1, 1, copied);
        assert_eq!(gpu.texture(&plain).usage(), copied);
        gpu.draw_copies(true);
        assert_eq!(gpu.texture(&plain).usage(), copied | both);
        for (asked, why) in [
            (desc(wgpu::TextureFormat::Rgba8Unorm, 3, 1, copied), "an array"),
            (desc(wgpu::TextureFormat::Rgba8Unorm, 1, 2, copied), "levels"),
            (desc(wgpu::TextureFormat::Rgba8UnormSrgb, 1, 1, copied), "sRGB"),
            (desc(wgpu::TextureFormat::Rgba8Sint, 1, 1, copied), "signed integers"),
            (desc(wgpu::TextureFormat::Rgba8Unorm, 1, 1, wgpu::TextureUsages::TEXTURE_BINDING), "never copied"),
            (desc(wgpu::TextureFormat::Depth32Float, 1, 1, wgpu::TextureUsages::RENDER_ATTACHMENT), "depth"),
        ] {
            assert_eq!(gpu.texture(&asked).usage(), asked.usage, "{why}");
        }
        gpu.draw_copies(false);
        assert_eq!(gpu.errors(), 0);
    }

    /// What drawing a copy costs against the GPU's own copy (OpenGL's `glCopyTexSubImage2D` on its
    /// backend): a layer 2048 pixels square, and 256 tiles of 64 a side in one go (as wet paint's
    /// windows take them), each the quickest of a few rounds and waited for. Drawn, a layer takes
    /// no more than three times as long or 8 ms more, and a tile no more than 1 ms more, with every
    /// other test on the GPU at once (alone on the PC's GPU, 2026-11-25: a layer 0.33 ms copied and
    /// 0.35 ms drawn on Vulkan, 0.15 and 0.22 ms on OpenGL; a tile 0.004 and 0.028 ms on Vulkan,
    /// 0.0025 and 0.017 ms on OpenGL).
    #[test]
    fn a_drawn_copy_costs_about_what_a_copy_does() {
        let Ok(gpu) = Gpu::headless() else { return eprintln!("skipped: no GPU") };
        let desc = |side| wgpu::TextureDescriptor {
            label: Some("timed"),
            size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        };
        gpu.draw_copies(true);
        let (from, into) = (gpu.texture(&desc(2048)), gpu.texture(&desc(2048)));
        let tiles: Vec<wgpu::Texture> = (0..256).map(|_| gpu.texture(&desc(64))).collect();
        let tile = wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 };
        // (the quickest of a few rounds, each way in turn: other work on the GPU slows some)
        let mut layer = [f64::MAX; 2];
        let mut per_tile = [f64::MAX; 2];
        for round in 0..12 {
            for (i, drawn) in [false, true].into_iter().enumerate() {
                gpu.draw_copies(drawn);
                let started = std::time::Instant::now();
                let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                gpu.copy_texture_to_texture(&mut encoder, from.as_image_copy(), into.as_image_copy(), from.size());
                gpu.queue.submit([encoder.finish()]);
                gpu.wait().unwrap();
                let ms = started.elapsed().as_secs_f64() * 1000.0;
                let started = std::time::Instant::now();
                let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                for (k, t) in tiles.iter().enumerate() {
                    let at = wgpu::Origin3d { x: (k as u32 % 32) * 64, y: (k as u32 / 32) * 64, z: 0 };
                    let part = wgpu::TexelCopyTextureInfo { origin: at, ..from.as_image_copy() };
                    gpu.copy_texture_to_texture(&mut encoder, part, t.as_image_copy(), tile);
                }
                gpu.queue.submit([encoder.finish()]);
                gpu.wait().unwrap();
                let tile_ms = started.elapsed().as_secs_f64() * 1000.0 / tiles.len() as f64;
                // (the first round makes the pipeline and warms the textures)
                if round > 0 {
                    layer[i] = layer[i].min(ms);
                    per_tile[i] = per_tile[i].min(tile_ms);
                }
            }
        }
        gpu.draw_copies(false);
        eprintln!(
            "a 2048 px layer copied in {:.3} ms, drawn {:.3} ms; a 64 px tile copied in {:.4} ms, drawn {:.4} ms ({:?})",
            layer[0],
            layer[1],
            per_tile[0],
            per_tile[1],
            gpu.adapter.get_info().backend
        );
        assert!(layer[1] <= (3.0 * layer[0]).max(layer[0] + 8.0), "a layer: {layer:?} ms");
        assert!(per_tile[1] <= per_tile[0] + 1.0, "a tile: {per_tile:?} ms");
        assert_eq!(gpu.errors(), 0);
    }

    /// Every texture outside the tests, here and in the core, is made through [`Gpu::texture`] (so
    /// it is cleared as it is made where new textures start stale), and copied and written to
    /// through the GPU's own copies (so tests can make them as such a GPU's driver does).
    #[test]
    fn every_texture_outside_the_tests_is_made_and_written_through_the_gpu() {
        fn sources(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    sources(&path, found);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    found.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        sources(&root.join("src"), &mut files);
        sources(&root.join("../core/src"), &mut files);
        assert!(files.len() > 40, "the sources found: {}", files.len());
        let raw = [
            "create_texture(",
            "create_texture_with_data(",
            "encoder.copy_texture_to_texture(",
            "queue.write_texture(",
            "encoder.copy_buffer_to_texture(",
        ];
        let mut found = Vec::new();
        for path in files {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_owned();
            // (the core's files of tests, and the copies themselves)
            if name.ends_with("tests.rs") || name == "stale.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let code = text
                .split(
                    "
mod tests {",
                )
                .next()
                .unwrap_or(&text);
            for (i, line) in code.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                for pattern in raw {
                    let placed_own = name == "placed.rs" && pattern == "encoder.copy_buffer_to_texture(";
                    if line.contains(pattern) && !placed_own {
                        found.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                    }
                }
            }
        }
        assert!(found.is_empty(), "made or written to around the GPU's own: {found:#?}");
    }
}
