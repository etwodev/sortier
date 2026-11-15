//! Draws media (stills, animation frames, video frames) with wgpu into offscreen frames.
//!
//! GPUI draws the UI; this crate draws the pictures. [`MediaRenderer`] owns a headless wgpu device of
//! its own. Callers [`upload`](MediaRenderer::upload) decoded pixels as textures, describe what one
//! on-screen view shows as a [`View`] (textured quads over a background), [`submit`](MediaRenderer::submit)
//! it to that view's target, and [`take_frame`](MediaRenderer::take_frame) later collects the finished
//! [`Frame`] as BGRA bytes ready to wrap in a `gpui::RenderImage`. Frames come back asynchronously
//! through a ring of staging buffers, so neither side waits for the other.
//!
//! Colour: textures are stored premultiplied in `Rgba8UnormSrgb` and targets are `Bgra8UnormSrgb`, so
//! filtering and blending happen in linear light while the bytes on either side are ordinary sRGB.

mod readback;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};

use crate::readback::{BYTES_PER_PIXEL, ReadbackRing};

const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// BGRA because GPUI's image atlas is BGRA, so frames need no swizzle.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;
const READBACK_SLOTS: usize = 3;
/// Offset between per-draw uniform blocks; the largest `min_uniform_buffer_offset_alignment` any
/// backend requires.
const UNIFORM_STRIDE: u64 = 256;
const QUAD_VERTICES: u32 = 6;

/// Names an uploaded texture. The caller picks the numbers (a file id, a video stream id, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureKey(pub u64);

/// Names a render target made by [`MediaRenderer::create_target`], one per on-screen view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TargetId(pub u32);

/// What one target shows: quads drawn in order over an opaque background.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    /// Size in device pixels; zero is treated as one.
    pub width: u32,
    pub height: u32,
    /// Opaque sRGB colour, components 0 to 1.
    pub background: [f32; 3],
    pub quads: Vec<Quad>,
    /// Echoed back in the [`Frame`] this view produces, so the caller knows what it's looking at.
    pub revision: u64,
}

/// One texture drawn into a rectangle of the target.
#[derive(Clone, Debug, PartialEq)]
pub struct Quad {
    /// A texture that isn't uploaded (any more) is skipped, not an error.
    pub texture: TextureKey,
    /// Destination in target pixels. It may extend past the target, as when zoomed in or panned; only the
    /// visible part is drawn. Empty or non-finite rectangles draw nothing.
    pub rect: Rect,
    /// 0 to 1; scales the whole quad, checkerboard included.
    pub opacity: f32,
    /// Drawn under the image inside `rect`, so transparency is visible.
    pub checker: Option<Checker>,
    pub filter: Filter,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// A checkerboard behind a quad's image. Squares start at the quad's top-left corner, so the pattern moves
/// with the image when it's panned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Checker {
    /// sRGB, components 0 to 1.
    pub light: [f32; 3],
    pub dark: [f32; 3],
    /// Side of one square in target pixels; values under 1 are treated as 1.
    pub size: f32,
}

/// How a texture is sampled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    /// Trilinear (bilinear between the two nearest mip levels).
    #[default]
    Smooth,
    /// Nearest texel when magnifying, so pixel art and zoomed-in pixels stay sharp; trilinear when
    /// minifying.
    Pixelated,
}

/// A finished frame: tightly packed BGRA8 rows, sRGB-encoded, opaque.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    /// The [`View::revision`] it was rendered from.
    pub revision: u64,
}

/// One draw's placement and appearance; matches `Params` in draw.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawUniforms {
    dest: [f32; 4],
    uv: [f32; 4],
    target_size: [f32; 2],
    opacity: f32,
    checker_size: f32,
    checker_origin: [f32; 2],
    padding: [f32; 2],
    checker_light: [f32; 4],
    checker_dark: [f32; 4],
}

/// One GPU texture holding the pixels `x..x + width`, `y..y + height` of an uploaded image.
struct Tile {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

/// An uploaded image: one tile, or several when it's larger than the device allows for one texture.
struct Texture {
    width: u32,
    height: u32,
    mipmaps: bool,
    tiles: Vec<Tile>,
    bytes: u64,
}

/// A target's texture and readback ring, made on its first submit and remade when its size changes.
struct Surface {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: ReadbackRing,
}

/// Error scopes around a stretch of GPU work, so validation errors and running out of GPU memory come back
/// as errors from the call that caused them instead of going to the uncaptured-error handler.
struct ErrorScopes {
    out_of_memory: wgpu::ErrorScopeGuard,
    validation: wgpu::ErrorScopeGuard,
}

impl ErrorScopes {
    fn push(device: &wgpu::Device) -> Self {
        Self {
            out_of_memory: device.push_error_scope(wgpu::ErrorFilter::OutOfMemory),
            validation: device.push_error_scope(wgpu::ErrorFilter::Validation),
        }
    }

    fn check(self, what: &str) -> Result<()> {
        let validation = pollster::block_on(self.validation.pop());
        let out_of_memory = pollster::block_on(self.out_of_memory.pop());
        match out_of_memory.or(validation) {
            Some(error) => Err(anyhow!("{what} failed on the GPU: {error}")),
            None => Ok(()),
        }
    }
}

/// Draws media into offscreen frames with its own headless wgpu device.
///
/// Textures are shared by every target. Upload pixels with [`upload`](Self::upload), make one target per
/// on-screen view with [`create_target`](Self::create_target), then each time a view changes
/// [`submit`](Self::submit) it and poll [`take_frame`](Self::take_frame) (for example once per UI frame
/// while [`has_pending_frames`](Self::has_pending_frames) is true). Nothing here blocks on the GPU except
/// [`wait`](Self::wait).
///
/// It's `Send`, so it can live on the UI thread or move to a worker, but all calls must come from one
/// thread at a time (`&mut self`). wgpu's error scopes are per thread, which is why errors are only
/// attributed to the right call when it's used that way.
pub struct MediaRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_summary: String,
    max_texture_size: u32,
    max_target_size: u32,
    image_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    upload_layout: wgpu::BindGroupLayout,
    draw_pipeline: wgpu::RenderPipeline,
    premultiply_pipeline: wgpu::RenderPipeline,
    downsample_pipeline: wgpu::RenderPipeline,
    smooth_sampler: wgpu::Sampler,
    pixelated_sampler: wgpu::Sampler,
    upload_sampler: wgpu::Sampler,
    draw_uniforms: wgpu::Buffer,
    draw_capacity: u64,
    smooth_binding: wgpu::BindGroup,
    pixelated_binding: wgpu::BindGroup,
    textures: HashMap<TextureKey, Texture>,
    texture_memory: u64,
    targets: HashMap<TargetId, Option<Surface>>,
    next_target: u32,
}

impl MediaRenderer {
    /// Creates a device on the fastest GPU (discrete before integrated), falling back to any adapter
    /// including a software one (WARP on Windows), with the adapter's full limits so large images fit.
    /// Each device is tried through its native API (Direct3D 12, Metal) before Vulkan and OpenGL.
    /// `WGPU_BACKEND` in the environment narrows the APIs considered.
    pub fn new() -> Result<Self> {
        Self::with_max_texture_size(u32::MAX)
    }

    /// Like [`new`](Self::new), but splits images into tiles no larger than `max_texture_size` on a side
    /// (or the device's limit, if that's smaller). Lets tests exercise tiling with small images.
    pub fn with_max_texture_size(max_texture_size: u32) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        adapters.sort_by_key(|adapter| {
            let info = adapter.get_info();
            (device_rank(info.device_type), backend_rank(info.backend))
        });
        let mut failures = Vec::new();
        for adapter in adapters {
            match Self::on_adapter(adapter, max_texture_size) {
                Ok(renderer) => return Ok(renderer),
                Err(error) => failures.push(format!("{error:#}")),
            }
        }
        match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: true,
            compatible_surface: None,
        })) {
            Ok(adapter) => match Self::on_adapter(adapter, max_texture_size) {
                Ok(renderer) => return Ok(renderer),
                Err(error) => failures.push(format!("{error:#}")),
            },
            Err(error) => failures.push(format!("no software adapter: {error}")),
        }
        bail!("no usable GPU adapter: {}", failures.join("; "))
    }

    fn on_adapter(adapter: wgpu::Adapter, max_texture_size: u32) -> Result<Self> {
        let info = adapter.get_info();
        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("sortier-render"),
            required_limits: limits.clone(),
            ..Default::default()
        }))
        .with_context(|| format!("creating a device on {}", info.name))?;
        // Errors outside an error scope are bugs; report them rather than abort the app.
        device.on_uncaptured_error(Arc::new(|error: wgpu::Error| {
            eprintln!("GPU error in Sortier's media renderer: {error}");
        }));
        device.set_device_lost_callback(|reason, message| {
            eprintln!("Sortier's media renderer lost its GPU device ({reason:?}): {message}");
        });

        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image"),
            entries: &[texture_entry(0)],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("draw"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(size_of::<DrawUniforms>() as u64),
                    },
                    count: None,
                },
                sampler_entry(1),
            ],
        });
        let upload_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("upload"),
            entries: &[texture_entry(0), sampler_entry(1)],
        });

        let draw_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("draw"),
            source: wgpu::ShaderSource::Wgsl(include_str!("draw.wgsl").into()),
        });
        let upload_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("upload"),
            source: wgpu::ShaderSource::Wgsl(include_str!("upload.wgsl").into()),
        });
        let over = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let draw_pipeline = render_pipeline(
            &device,
            "draw",
            &[&draw_layout, &image_layout],
            (&draw_shader, "vs_main", "fs_main"),
            TARGET_FORMAT,
            Some(wgpu::BlendState { color: over, alpha: over }),
        );
        let premultiply_pipeline = render_pipeline(
            &device,
            "premultiply",
            &[&upload_layout],
            (&upload_shader, "vs_main", "fs_premultiply"),
            TEXTURE_FORMAT,
            None,
        );
        let downsample_pipeline = render_pipeline(
            &device,
            "downsample",
            &[&upload_layout],
            (&upload_shader, "vs_main", "fs_downsample"),
            TEXTURE_FORMAT,
            None,
        );

        let smooth_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("smooth"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let pixelated_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pixelated"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let upload_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("upload"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let draw_capacity = 64;
        let draw_uniforms = create_draw_uniforms(&device, draw_capacity);
        let smooth_binding = draw_binding(&device, &draw_layout, &draw_uniforms, &smooth_sampler);
        let pixelated_binding = draw_binding(&device, &draw_layout, &draw_uniforms, &pixelated_sampler);

        Ok(Self {
            adapter_summary: format!("{} ({:?})", info.name, info.backend),
            max_texture_size: max_texture_size.clamp(1, limits.max_texture_dimension_2d),
            max_target_size: limits.max_texture_dimension_2d,
            device,
            queue,
            image_layout,
            draw_layout,
            upload_layout,
            draw_pipeline,
            premultiply_pipeline,
            downsample_pipeline,
            smooth_sampler,
            pixelated_sampler,
            upload_sampler,
            draw_uniforms,
            draw_capacity,
            smooth_binding,
            pixelated_binding,
            textures: HashMap::new(),
            texture_memory: 0,
            targets: HashMap::new(),
            next_target: 1,
        })
    }

    /// The adapter's name and graphics API, such as "NVIDIA GeForce RTX 4070 (Dx12)".
    pub fn adapter_summary(&self) -> &str {
        &self.adapter_summary
    }

    /// The largest side of one GPU texture. Bigger uploads are split into tiles of at most this size, which
    /// callers never see. Targets may be as large as the device allows, which is at least this.
    pub fn max_texture_size(&self) -> u32 {
        self.max_texture_size
    }

    /// Uploads straight (non-premultiplied) RGBA8 pixels, sRGB-encoded and tightly packed, as the texture
    /// `key`, replacing any texture with that key.
    ///
    /// When the replaced texture has the same size and `mipmaps`, its GPU allocation is reused, so video
    /// frames can be uploaded every few milliseconds without allocating. With `mipmaps` the full mip chain is
    /// built, so zoomed-out views are smooth; leave it off for frames that are only shown near 1:1. Images
    /// larger than [`max_texture_size`](Self::max_texture_size) are split into tiles internally.
    ///
    /// Each tile goes to the GPU in a submission of its own, which keeps command buffers small when many
    /// images arrive at once. Fails when the pixel count doesn't match the size or the GPU runs out of memory;
    /// the key is then left without a texture.
    pub fn upload(&mut self, key: TextureKey, width: u32, height: u32, rgba: &[u8], mipmaps: bool) -> Result<()> {
        if width == 0 || height == 0 {
            bail!("can't upload an empty {width}x{height} image");
        }
        let expected = u64::from(width) * u64::from(height) * u64::from(BYTES_PER_PIXEL);
        if rgba.len() as u64 != expected {
            bail!("a {width}x{height} image needs {expected} bytes of RGBA, got {}", rgba.len());
        }
        let reusable = self
            .textures
            .get(&key)
            .is_some_and(|texture| texture.width == width && texture.height == height && texture.mipmaps == mipmaps);
        let texture = if reusable {
            self.textures.remove(&key).expect("checked above")
        } else {
            self.release(key);
            let scopes = ErrorScopes::push(&self.device);
            let texture = self.create_texture(width, height, mipmaps);
            scopes.check("allocating a texture")?;
            self.texture_memory += texture.bytes;
            texture
        };
        for tile in &texture.tiles {
            let scopes = ErrorScopes::push(&self.device);
            self.upload_tile(tile, width, rgba);
            if let Err(error) = scopes.check("uploading an image") {
                self.texture_memory -= texture.bytes;
                return Err(error);
            }
        }
        self.textures.insert(key, texture);
        Ok(())
    }

    /// Whether a texture is uploaded under `key`.
    pub fn contains(&self, key: TextureKey) -> bool {
        self.textures.contains_key(&key)
    }

    /// The width and height the texture under `key` was uploaded with.
    pub fn texture_size(&self, key: TextureKey) -> Option<(u32, u32)> {
        self.textures.get(&key).map(|texture| (texture.width, texture.height))
    }

    /// Frees the texture under `key`, if any. Frames already submitted still draw it.
    pub fn release(&mut self, key: TextureKey) {
        if let Some(texture) = self.textures.remove(&key) {
            self.texture_memory -= texture.bytes;
        }
    }

    /// Bytes of GPU memory held by uploaded textures, mip levels included, for the caller's eviction policy.
    /// Targets and their staging buffers (about 4 x 4 bytes per target pixel) aren't counted, nor is the
    /// short-lived copy a transparent upload goes through.
    pub fn texture_memory(&self) -> u64 {
        self.texture_memory
    }

    /// Makes a target for one on-screen view. Its texture is made on the first submit, at the view's size.
    pub fn create_target(&mut self) -> TargetId {
        let id = TargetId(self.next_target);
        self.next_target += 1;
        self.targets.insert(id, None);
        id
    }

    /// Frees a target and any frames still in flight for it.
    pub fn release_target(&mut self, target: TargetId) {
        self.targets.remove(&target);
    }

    /// Renders `view` into `target` and queues its copy back to the CPU; collect it with
    /// [`take_frame`](Self::take_frame).
    ///
    /// Returns `Ok(false)` without rendering when every staging buffer of this target is in flight (the GPU
    /// or the caller is behind); try again next frame. When the view's size differs from the last one, the
    /// target is remade and frames still in flight for the old size are discarded.
    pub fn submit(&mut self, target: TargetId, view: &View) -> Result<bool> {
        let width = view.width.max(1);
        let height = view.height.max(1);
        if width > self.max_target_size || height > self.max_target_size {
            bail!("a {width}x{height} view is larger than the GPU's limit of {}", self.max_target_size);
        }
        let surface = self.targets.get(&target).ok_or_else(|| anyhow!("unknown render target {target:?}"))?;
        if surface.as_ref().is_none_or(|surface| surface.readback.width != width || surface.readback.height != height) {
            let scopes = ErrorScopes::push(&self.device);
            let surface = self.create_surface(width, height);
            scopes.check("allocating a render target")?;
            self.targets.insert(target, Some(surface));
        }
        if !self.targets[&target].as_ref().expect("made above").readback.has_free_slot() {
            return Ok(false);
        }

        let draws = self.plan(view, width, height);
        self.write_uniforms(&draws);
        let scopes = ErrorScopes::push(&self.device);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let surface = self.targets.get_mut(&target).and_then(Option::as_mut).expect("made above");
        {
            let background = view.background.map(|c| f64::from(srgb_to_linear(c)));
            let mut pass = begin_pass(
                &mut encoder,
                &surface.view,
                "frame",
                wgpu::LoadOp::Clear(wgpu::Color {
                    r: background[0],
                    g: background[1],
                    b: background[2],
                    a: 1.0,
                }),
            );
            pass.set_pipeline(&self.draw_pipeline);
            for (index, draw) in draws.iter().enumerate() {
                let binding = match draw.filter {
                    Filter::Smooth => &self.smooth_binding,
                    Filter::Pixelated => &self.pixelated_binding,
                };
                let tile = &self.textures[&draw.texture].tiles[draw.tile];
                pass.set_bind_group(0, binding, &[(index as u64 * UNIFORM_STRIDE) as u32]);
                pass.set_bind_group(1, &tile.bind_group, &[]);
                pass.draw(0..QUAD_VERTICES, 0..1);
            }
        }
        let slot = surface.readback.copy(&mut encoder, &surface.texture).expect("a free slot was checked above");
        self.queue.submit([encoder.finish()]);
        surface.readback.map(slot, view.revision);
        scopes.check("rendering a frame")?;
        Ok(true)
    }

    /// The newest finished frame of `target`, without waiting; older finished frames are dropped and their
    /// staging buffers recycled. `Ok(None)` when nothing has finished yet.
    pub fn take_frame(&mut self, target: TargetId) -> Result<Option<Frame>> {
        self.device.poll(wgpu::PollType::Poll).context("polling the GPU")?;
        let surface = self.targets.get_mut(&target).ok_or_else(|| anyhow!("unknown render target {target:?}"))?;
        let Some(surface) = surface else {
            return Ok(None);
        };
        Ok(surface.readback.take_newest()?.map(|(bgra, revision)| Frame {
            width: surface.readback.width,
            height: surface.readback.height,
            bgra,
            revision,
        }))
    }

    /// Whether `target` has a submitted frame that hasn't been taken yet, finished or not. Keep calling
    /// [`take_frame`](Self::take_frame) (once per UI frame, say) while this is true.
    pub fn has_pending_frames(&self, target: TargetId) -> bool {
        self.targets
            .get(&target)
            .and_then(Option::as_ref)
            .is_some_and(|surface| surface.readback.has_untaken())
    }

    /// Blocks until the GPU has finished everything submitted so far, so the next
    /// [`take_frame`](Self::take_frame) returns the latest frame. For tests and one-off renders such as
    /// thumbnails; the UI thread should poll instead.
    pub fn wait(&self) -> Result<()> {
        self.device.poll(wgpu::PollType::wait_indefinitely()).context("waiting for the GPU")?;
        Ok(())
    }

    /// Allocates the tiles of a `width` x `height` texture, uninitialised.
    fn create_texture(&self, width: u32, height: u32, mipmaps: bool) -> Texture {
        let mut tiles = Vec::new();
        let mut bytes = 0;
        for (y, tile_height) in spans(height, self.max_texture_size) {
            for (x, tile_width) in spans(width, self.max_texture_size) {
                let levels = if mipmaps { tile_width.max(tile_height).ilog2() + 1 } else { 1 };
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("media"),
                    size: extent(tile_width, tile_height),
                    mip_level_count: levels,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: TEXTURE_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("media"),
                    layout: &self.image_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    }],
                });
                bytes += (0..levels)
                    .map(|level| u64::from((tile_width >> level).max(1)) * u64::from((tile_height >> level).max(1)))
                    .sum::<u64>()
                    * u64::from(BYTES_PER_PIXEL);
                tiles.push(Tile {
                    x,
                    y,
                    width: tile_width,
                    height: tile_height,
                    texture,
                    bind_group,
                });
            }
        }
        Texture {
            width,
            height,
            mipmaps,
            tiles,
            bytes,
        }
    }

    /// Fills `tile` from the whole image's pixels and builds its mips, in one submission.
    ///
    /// Opaque pixels are already premultiplied, so they're written straight into the first mip level; other
    /// pixels go through a temporary texture and the premultiply pass.
    fn upload_tile(&self, tile: &Tile, image_width: u32, rgba: &[u8]) {
        let row_bytes = image_width * BYTES_PER_PIXEL;
        let start = (tile.y as usize * image_width as usize + tile.x as usize) * BYTES_PER_PIXEL as usize;
        let pixels = &rgba[start..];
        let layout = wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(row_bytes),
            rows_per_image: Some(tile.height),
        };
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("upload") });
        if is_opaque(pixels, row_bytes, tile.width, tile.height) {
            self.queue.write_texture(texel_copy(&tile.texture), pixels, layout, extent(tile.width, tile.height));
        } else {
            let straight = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("straight-upload"),
                size: extent(tile.width, tile.height),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TEXTURE_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.queue.write_texture(texel_copy(&straight), pixels, layout, extent(tile.width, tile.height));
            let source = straight.create_view(&wgpu::TextureViewDescriptor::default());
            self.upload_pass(&mut encoder, &self.premultiply_pipeline, &source, &level_view(&tile.texture, 0));
        }
        for level in 1..tile.texture.mip_level_count() {
            self.upload_pass(
                &mut encoder,
                &self.downsample_pipeline,
                &level_view(&tile.texture, level - 1),
                &level_view(&tile.texture, level),
            );
        }
        self.queue.submit([encoder.finish()]);
    }

    fn upload_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
    ) {
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("upload"),
            layout: &self.upload_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.upload_sampler),
                },
            ],
        });
        let mut pass = begin_pass(encoder, target, "upload", wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn create_surface(&self, width: u32, height: u32) -> Surface {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("target"),
            size: extent(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Surface {
            texture,
            view,
            readback: ReadbackRing::new(&self.device, width, height, READBACK_SLOTS),
        }
    }

    /// The draws `view` needs on a `width` x `height` target: one per visible tile of each quad whose texture
    /// is uploaded, in order.
    fn plan(&self, view: &View, width: u32, height: u32) -> Vec<Draw> {
        let mut draws = Vec::new();
        for quad in &view.quads {
            let Some(texture) = self.textures.get(&quad.texture) else {
                continue;
            };
            let rect = quad.rect;
            let opacity = quad.opacity.clamp(0.0, 1.0);
            if opacity.is_nan() || opacity == 0.0 || ![rect.x, rect.y, rect.width, rect.height].iter().all(|v| v.is_finite()) {
                continue;
            }
            let checker = quad.checker.filter(|checker| checker.size.is_finite());
            for (index, tile) in texture.tiles.iter().enumerate() {
                let Some((dest, uv)) = place_tile(rect, (texture.width, texture.height), tile, (width, height)) else {
                    continue;
                };
                let linear = |rgb: [f32; 3]| [srgb_to_linear(rgb[0]), srgb_to_linear(rgb[1]), srgb_to_linear(rgb[2]), 1.0];
                draws.push(Draw {
                    texture: quad.texture,
                    tile: index,
                    filter: quad.filter,
                    uniforms: DrawUniforms {
                        dest,
                        uv,
                        target_size: [width as f32, height as f32],
                        opacity,
                        checker_size: checker.map_or(0.0, |checker| checker.size.max(1.0)),
                        checker_origin: [rect.x, rect.y],
                        padding: [0.0; 2],
                        checker_light: checker.map_or([0.0; 4], |checker| linear(checker.light)),
                        checker_dark: checker.map_or([0.0; 4], |checker| linear(checker.dark)),
                    },
                });
            }
        }
        draws
    }

    /// Writes each draw's uniforms at its offset, growing the buffer (and the bind groups using it) first
    /// if needed.
    fn write_uniforms(&mut self, draws: &[Draw]) {
        if draws.is_empty() {
            return;
        }
        let needed = draws.len() as u64;
        if needed > self.draw_capacity {
            self.draw_capacity = needed.next_power_of_two();
            self.draw_uniforms = create_draw_uniforms(&self.device, self.draw_capacity);
            self.smooth_binding = draw_binding(&self.device, &self.draw_layout, &self.draw_uniforms, &self.smooth_sampler);
            self.pixelated_binding =
                draw_binding(&self.device, &self.draw_layout, &self.draw_uniforms, &self.pixelated_sampler);
        }
        let mut bytes = vec![0u8; draws.len() * UNIFORM_STRIDE as usize];
        for (index, draw) in draws.iter().enumerate() {
            let offset = index * UNIFORM_STRIDE as usize;
            bytes[offset..offset + size_of::<DrawUniforms>()].copy_from_slice(bytemuck::bytes_of(&draw.uniforms));
        }
        self.queue.write_buffer(&self.draw_uniforms, 0, &bytes);
    }
}

/// One tile drawn by [`MediaRenderer::submit`].
struct Draw {
    texture: TextureKey,
    tile: usize,
    filter: Filter,
    uniforms: DrawUniforms,
}

/// Where `tile` of an `image`-sized texture drawn into `rect` lands on a `target`-sized frame, clipped to it:
/// the corners (x0, y0, x1, y1) in target pixels and the texture coordinates inside the tile at those
/// corners. `None` when none of it is on the frame.
///
/// Computed in f64 from the whole image's mapping, so neighbouring tiles get exactly the same coordinate
/// for their shared edge and abut without gaps, and clipping keeps coordinates small however far the view
/// is zoomed in.
fn place_tile(rect: Rect, image: (u32, u32), tile: &Tile, target: (u32, u32)) -> Option<([f32; 4], [f32; 4])> {
    let (x0, x1, u0, u1) = clip_span(rect.x, rect.width, image.0, tile.x, tile.width, target.0)?;
    let (y0, y1, v0, v1) = clip_span(rect.y, rect.height, image.1, tile.y, tile.height, target.1)?;
    let narrow = |values: [f64; 4]| values.map(|value| value as f32);
    Some((narrow([x0, y0, x1, y1]), narrow([u0, v0, u1, v1])))
}

/// One axis of [`place_tile`]: the visible span of pixels `start..start + size` of an `image`-pixel axis drawn
/// over `extent` target pixels from `origin`, and the matching coordinates within the tile (0 to 1).
fn clip_span(origin: f32, extent: f32, image: u32, start: u32, size: u32, target: u32) -> Option<(f64, f64, f64, f64)> {
    if !extent.is_finite() || extent <= 0.0 {
        return None;
    }
    let scale = f64::from(extent) / f64::from(image);
    let from = f64::from(origin) + f64::from(start) * scale;
    let to = f64::from(origin) + f64::from(start + size) * scale;
    let visible_from = from.max(0.0);
    let visible_to = to.min(f64::from(target));
    if visible_to <= visible_from {
        return None;
    }
    let length = to - from;
    Some((visible_from, visible_to, (visible_from - from) / length, (visible_to - from) / length))
}

/// How much a kind of device is preferred: discrete GPUs first, software renderers last.
fn device_rank(kind: wgpu::DeviceType) -> u8 {
    match kind {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 3,
        wgpu::DeviceType::Cpu => 4,
    }
}

/// How much a graphics API is preferred for the same device: the native ones before Vulkan, OpenGL last.
fn backend_rank(backend: wgpu::Backend) -> u8 {
    match backend {
        wgpu::Backend::Dx12 | wgpu::Backend::Metal => 0,
        wgpu::Backend::Vulkan => 1,
        _ => 2,
    }
}

/// Splits `0..length` into consecutive spans of at most `max`: (start, size) pairs.
fn spans(length: u32, max: u32) -> impl Iterator<Item = (u32, u32)> {
    (0..length).step_by(max as usize).map(move |start| (start, max.min(length - start)))
}

/// Whether every pixel in a `width` x `height` block of RGBA rows `row_bytes` apart has full alpha.
fn is_opaque(pixels: &[u8], row_bytes: u32, width: u32, height: u32) -> bool {
    (0..height as usize).all(|row| {
        let start = row * row_bytes as usize;
        pixels[start..start + (width * BYTES_PER_PIXEL) as usize].as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255)
    })
}

/// Decodes an sRGB component (0 to 1) to linear light.
fn srgb_to_linear(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
}

fn create_draw_uniforms(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("draw-uniforms"),
        size: capacity.max(1) * UNIFORM_STRIDE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn draw_binding(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, uniforms: &wgpu::Buffer, sampler: &wgpu::Sampler) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("draw"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: uniforms,
                    offset: 0,
                    size: NonZeroU64::new(size_of::<DrawUniforms>() as u64),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn render_pipeline(
    device: &wgpu::Device,
    label: &str,
    groups: &[&wgpu::BindGroupLayout],
    (shader, vertex, fragment): (&wgpu::ShaderModule, &str, &str),
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let groups: Vec<Option<&wgpu::BindGroupLayout>> = groups.iter().map(|group| Some(*group)).collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &groups,
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vertex),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn begin_pass<'encoder>(
    encoder: &'encoder mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    label: &str,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPass<'encoder> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

fn extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}

fn texel_copy(texture: &wgpu::Texture) -> wgpu::TexelCopyTextureInfo<'_> {
    wgpu::TexelCopyTextureInfo {
        texture,
        mip_level: 0,
        origin: wgpu::Origin3d::ZERO,
        aspect: wgpu::TextureAspect::All,
    }
}

/// A view of one mip level of `texture`, for rendering into or sampling just that level.
fn level_view(texture: &wgpu::Texture, level: u32) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        base_mip_level: level,
        mip_level_count: Some(1),
        ..Default::default()
    })
}
