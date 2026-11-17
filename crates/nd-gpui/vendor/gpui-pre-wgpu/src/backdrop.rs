//! Backdrops: what a frame has drawn behind an element, blurred and tinted (`Scene::backdrops`).
//!
//! A batch of backdrops shares a draw order, so their sample regions do not overlap. The main
//! pass ends, each region is copied out of the frame, box-downsampled so the Gaussian that
//! follows needs at most a dozen taps per side, blurred horizontally and vertically, and the main
//! pass resumes with each backdrop composited in its rounded bounds. Every intermediate keeps
//! a region at the same origin as its sample region, so one frame-sized texture holds the whole
//! batch.

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use gpui::{Backdrop, BackdropMask, BackdropSource, Rgba, ScaledPixels};

pub(crate) const SHADER: &str = r#"
// Every vec4 on a 16-byte boundary, so the layout is the same under WGSL's, HLSL's and C's rules.
struct Params {
    target_size: vec2<f32>,
    // Downsample and blur: the source and destination regions in texels.
    src_origin: vec2<f32>,
    src_size: vec2<f32>,
    dst_origin: vec2<f32>,
    dst_size: vec2<f32>,
    direction: vec2<f32>,
    factor: f32,
    sigma: f32,
    saturation: f32,
    highlight: f32,
    // Composite: the element in frame pixels, its clip, its shape and its look.
    bounds_origin: vec2<f32>,
    bounds_size: vec2<f32>,
    mask_origin: vec2<f32>,
    mask_size: vec2<f32>,
    radii: vec4<f32>,
    tint: vec4<f32>,
    highlight_width: f32,
    // 0 composites the blur, 1 clears the shape, 2 lays the tint alone.
    mode: u32,
    has_source: u32,
    opacity: f32,
    // Composite: the coverage texture's bounds in frame pixels, if it has one.
    clip_origin: vec2<f32>,
    clip_size: vec2<f32>,
    has_clip: u32,
    // Reflect the sample region at its edge rather than repeat the edge.
    mirror: u32,
    // Clamp the tinted result to [clamp_low, clamp_high] instead of the saturated blur to 0.
    has_clamp: u32,
    clamp_low: f32,
    clamp_high: f32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;
@group(1) @binding(0) var clip: texture_2d<f32>;

struct Varyings {
    @builtin(position) position: vec4<f32>,
}

fn to_clip(p: vec2<f32>) -> vec4<f32> {
    let ndc = p / params.target_size * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    return vec4<f32>(ndc, 0.0, 1.0);
}

fn corner(index: u32) -> vec2<f32> {
    return vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
}

@vertex
fn vs_region(@builtin(vertex_index) index: u32) -> Varyings {
    var out: Varyings;
    out.position = to_clip(params.dst_origin + corner(index) * params.dst_size);
    return out;
}

@vertex
fn vs_composite(@builtin(vertex_index) index: u32) -> Varyings {
    let lo = max(params.bounds_origin, params.mask_origin);
    let hi = min(params.bounds_origin + params.bounds_size, params.mask_origin + params.mask_size);
    var out: Varyings;
    out.position = to_clip(lo + corner(index) * max(hi - lo, vec2<f32>(0.0)));
    return out;
}

fn sample_clamped(p: vec2<f32>, lo: vec2<f32>, hi: vec2<f32>) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(source));
    var q = p;
    if (params.mirror != 0u) {
        // Reflected however far past the edge the blur reaches, so a region narrower than
        // the blur folds back on itself instead of repeating its edge.
        let span = hi - lo;
        let d = p - lo;
        let m = d - 2.0 * span * floor(d / (2.0 * span));
        q = lo + span - abs(span - m);
    }
    q = clamp(q, lo + 0.5, hi - 0.5);
    return textureSampleLevel(source, source_sampler, q / size, 0.0);
}

@fragment
fn fs_downsample(in: Varyings) -> @location(0) vec4<f32> {
    let center = params.src_origin + (in.position.xy - params.dst_origin) * params.factor;
    let lo = params.src_origin;
    let hi = params.src_origin + params.src_size;
    // Bilinear taps two texels apart, each the mean of a 2x2 block, cover a factor-wide box.
    let n = max(i32(params.factor) / 2, 1);
    var sum = vec4<f32>(0.0);
    for (var y = 0; y < n; y++) {
        for (var x = 0; x < n; x++) {
            let offset = (vec2<f32>(f32(x), f32(y)) - f32(n - 1) * 0.5) * 2.0;
            sum += sample_clamped(center + offset, lo, hi);
        }
    }
    return sum / f32(n * n);
}

@fragment
fn fs_blur(in: Varyings) -> @location(0) vec4<f32> {
    let lo = params.src_origin;
    let hi = params.src_origin + params.src_size;
    let sigma = params.sigma;
    if (sigma < 0.25) {
        return sample_clamped(in.position.xy, lo, hi);
    }
    let reach = i32(ceil(sigma * 3.0));
    var sum = vec4<f32>(0.0);
    var weight = 0.0;
    for (var i = -reach; i <= reach; i++) {
        let w = exp(-f32(i * i) / (2.0 * sigma * sigma));
        sum += w * sample_clamped(in.position.xy + params.direction * f32(i), lo, hi);
        weight += w;
    }
    return sum / weight;
}

fn rounded_rect_sdf(p: vec2<f32>) -> f32 {
    let half_size = params.bounds_size * 0.5;
    let center = params.bounds_origin + half_size;
    let d = p - center;
    var radius: f32;
    if (d.x < 0.0) {
        radius = select(params.radii.w, params.radii.x, d.y < 0.0);
    } else {
        radius = select(params.radii.z, params.radii.y, d.y < 0.0);
    }
    radius = min(radius, min(half_size.x, half_size.y));
    let q = abs(d) - half_size + radius;
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_composite(in: Varyings) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    let sdf = rounded_rect_sdf(p);
    var coverage = saturate(0.5 - sdf);
    if (params.has_clip != 0u) {
        let uv = (p - params.clip_origin) / params.clip_size;
        if (any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) {
            discard;
        }
        let texel = vec2<i32>(uv * vec2<f32>(textureDimensions(clip)));
        coverage *= textureLoad(clip, texel, 0).r;
    }
    if (coverage <= 0.0) {
        discard;
    }
    if (params.mode == 1u) {
        return vec4<f32>(0.0, 0.0, 0.0, coverage * params.opacity);
    }

    var color = vec4<f32>(0.0);
    if (params.mode == 0u && params.has_source != 0u) {
        let blurred = params.dst_origin + (p - params.src_origin) / params.factor;
        color = sample_clamped(blurred, params.dst_origin, params.dst_origin + params.dst_size);
        let luma = dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        let saturated = mix(vec3<f32>(luma), color.rgb, params.saturation);
        color = vec4<f32>(select(max(saturated, vec3<f32>(0.0)), saturated, params.has_clamp != 0u), color.a);
    }
    let tint = params.tint;
    color = vec4<f32>(tint.rgb * tint.a + color.rgb * (1.0 - tint.a), tint.a + color.a * (1.0 - tint.a));
    if (params.has_clamp != 0u) {
        color = vec4<f32>(clamp(color.rgb, vec3<f32>(params.clamp_low * color.a), vec3<f32>(params.clamp_high * color.a)), color.a);
    } else {
        // Premultiplied, so no channel may pass the alpha it is under; bounded after the tint,
        // so a channel the saturation pushes past one still lifts it.
        color = vec4<f32>(min(color.rgb, vec3<f32>(color.a)), color.a);
    }

    if (params.highlight > 0.0 && params.highlight_width > 0.0) {
        let e = 0.5;
        let normal = normalize(vec2<f32>(
            rounded_rect_sdf(p + vec2<f32>(e, 0.0)) - rounded_rect_sdf(p - vec2<f32>(e, 0.0)),
            rounded_rect_sdf(p + vec2<f32>(0.0, e)) - rounded_rect_sdf(p - vec2<f32>(0.0, e))
        ) + vec2<f32>(1e-6));
        let rim = 1.0 - smoothstep(0.0, params.highlight_width, -sdf);
        let facing = dot(normal, vec2<f32>(0.25881905, -0.96592583));
        let light = select(0.35, 1.0, facing > 0.0) * facing * facing;
        let add = saturate(params.highlight * rim * light);
        color = vec4<f32>(color.rgb + add * (1.0 - color.rgb), color.a + add * (1.0 - color.a));
    }
    return color * coverage * params.opacity;
}
"#;

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct Params {
    target_size: [f32; 2],
    src_origin: [f32; 2],
    src_size: [f32; 2],
    dst_origin: [f32; 2],
    dst_size: [f32; 2],
    direction: [f32; 2],
    factor: f32,
    sigma: f32,
    saturation: f32,
    highlight: f32,
    bounds_origin: [f32; 2],
    bounds_size: [f32; 2],
    mask_origin: [f32; 2],
    mask_size: [f32; 2],
    radii: [f32; 4],
    tint: [f32; 4],
    highlight_width: f32,
    mode: u32,
    has_source: u32,
    opacity: f32,
    clip_origin: [f32; 2],
    clip_size: [f32; 2],
    has_clip: u32,
    mirror: u32,
    has_clamp: u32,
    clamp_low: f32,
    clamp_high: f32,
    pad: [u32; 3],
}

const _: () = assert!(std::mem::offset_of!(Params, radii) == 96 && std::mem::size_of::<Params>() == 192);

/// A backdrop's sample region in whole frame pixels and the factor it is downsampled by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub factor: u32,
}

impl Region {
    pub(crate) fn of(backdrop: &Backdrop, frame: (u32, u32)) -> Option<Self> {
        let sample = backdrop.sample_bounds();
        let x0 = sample.origin.x.0.floor().max(0.) as u32;
        let y0 = sample.origin.y.0.floor().max(0.) as u32;
        let x1 = (sample.origin.x.0 + sample.size.width.0).ceil().min(frame.0 as f32);
        let y1 = (sample.origin.y.0 + sample.size.height.0).ceil().min(frame.1 as f32);
        if x1 <= x0 as f32 || y1 <= y0 as f32 {
            return None;
        }
        Some(Self {
            x: x0,
            y: y0,
            width: x1 as u32 - x0,
            height: y1 as u32 - y0,
            factor: backdrop.downsample_factor(),
        })
    }

    /// The downsampled region, kept at the sample region's origin.
    fn reduced(&self) -> (u32, u32) {
        (self.width.div_ceil(self.factor), self.height.div_ceil(self.factor))
    }
}

struct Scratch {
    copy: wgpu::Texture,
    a_view: wgpu::TextureView,
    b_view: wgpu::TextureView,
    from_copy: wgpu::BindGroup,
    from_a: wgpu::BindGroup,
    from_b: wgpu::BindGroup,
}

pub(crate) struct Backdrops {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    region: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    erase: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    params_capacity: u64,
    /// Where the next batch of this frame writes its params.
    params_cursor: u64,
    stride: u64,
    scratch: Option<Scratch>,
    /// Bound where a batch blurs nothing, so no scratch texture is needed.
    placeholder_view: wgpu::TextureView,
    placeholder: wgpu::BindGroup,
    clip_layout: wgpu::BindGroupLayout,
    /// Bound for a backdrop without a mask, which the shader then does not read.
    no_clip: wgpu::BindGroup,
}

impl Backdrops {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backdrop_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(std::mem::size_of::<Params>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let clip_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backdrop_clip_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("backdrop_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("backdrop_composite_layout"),
            bind_group_layouts: &[Some(&layout), Some(&clip_layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("backdrop_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = |label: &str, vs: &str, fs: &str, blend: Option<wgpu::BlendState>| {
            let layout = if fs == "fs_composite" { &composite_layout } else { &pipeline_layout };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let erase_blend = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let region = pipeline("backdrop_downsample", "vs_region", "fs_downsample", None);
        let blur = pipeline("backdrop_blur", "vs_region", "fs_blur", None);
        let composite = pipeline(
            "backdrop_composite",
            "vs_composite",
            "fs_composite",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let erase = pipeline(
            "backdrop_erase",
            "vs_composite",
            "fs_composite",
            Some(wgpu::BlendState {
                color: erase_blend,
                alpha: erase_blend,
            }),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("backdrop_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let stride = (std::mem::size_of::<Params>() as u64)
            .next_multiple_of(device.limits().min_uniform_buffer_offset_alignment as u64);
        let params_capacity = stride * 64;
        let params = Self::params_buffer(device, params_capacity);
        let placeholder_view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("backdrop_placeholder"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let placeholder = bind_group(device, &layout, &params, &sampler, &placeholder_view);
        let no_clip = clip_bind_group(device, &clip_layout, &clip_texture(device, 1, 1));
        Self {
            format,
            layout,
            region,
            blur,
            composite,
            erase,
            sampler,
            params,
            params_capacity,
            params_cursor: 0,
            stride,
            scratch: None,
            placeholder_view,
            placeholder,
            clip_layout,
            no_clip,
        }
    }

    /// Makes room for every backdrop `scene` draws this frame. Call it before the first batch.
    pub(crate) fn begin_frame(&mut self, device: &wgpu::Device, backdrops: &[Backdrop]) {
        self.params_cursor = 0;
        // Three blur draws and up to two composites each.
        let needed = (backdrops.len() as u64 * 5).max(1) * self.stride;
        if needed > self.params_capacity {
            self.params_capacity = needed.next_power_of_two();
            self.params = Self::params_buffer(device, self.params_capacity);
            self.placeholder = bind_group(
                device,
                &self.layout,
                &self.params,
                &self.sampler,
                &self.placeholder_view,
            );
            self.scratch = None;
        }
    }

    fn params_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("backdrop_params"),
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn ensure_scratch(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let fits = self
            .scratch
            .as_ref()
            .is_some_and(|s| s.copy.width() == width && s.copy.height() == height);
        if !fits {
            let texture = |label: &str, usage: wgpu::TextureUsages| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.format,
                    usage: usage | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
            };
            let copy = texture("backdrop_copy", wgpu::TextureUsages::COPY_DST);
            let a = texture("backdrop_a", wgpu::TextureUsages::RENDER_ATTACHMENT);
            let b = texture("backdrop_b", wgpu::TextureUsages::RENDER_ATTACHMENT);
            let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
            let (copy_view, a_view, b_view) = (view(&copy), view(&a), view(&b));
            self.scratch = Some(Scratch {
                from_copy: self.bind_group(device, &copy_view),
                from_a: self.bind_group(device, &a_view),
                from_b: self.bind_group(device, &b_view),
                copy,
                a_view,
                b_view,
            });
        }
    }

    fn bind_group(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        bind_group(device, &self.layout, &self.params, &self.sampler, view)
    }

    /// Blurs the regions of `backdrops` out of `frame` into the scratch textures, ready for
    /// [`Self::composite`]. Call it between render passes on `frame`.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::Texture,
        backdrops: &[Backdrop],
        masks: &[BackdropMask<ScaledPixels>],
    ) -> Prepared {
        let size = (frame.width(), frame.height());
        let has_source = frame.usage().contains(wgpu::TextureUsages::COPY_SRC);
        let regions: Vec<Option<Region>> = backdrops
            .iter()
            .map(|b| match b.source {
                BackdropSource::Window if has_source => Region::of(b, size),
                _ => None,
            })
            .collect();

        let start = self.params_cursor;
        let stride = self.stride as usize;
        let mut params = Vec::<u8>::new();
        let mut push = |p: Params| {
            let offset = start as u32 + params.len() as u32;
            params.extend_from_slice(bytemuck::bytes_of(&p));
            params.resize(params.len().next_multiple_of(stride), 0);
            offset
        };
        let base = Params {
            target_size: [size.0 as f32, size.1 as f32],
            factor: 1.,
            saturation: 1.,
            ..Zeroable::zeroed()
        };

        let mut blur_draws = Vec::new();
        for (backdrop, region) in backdrops.iter().zip(&regions) {
            let Some(r) = region else { continue };
            let (rw, rh) = r.reduced();
            let src_origin = [r.x as f32, r.y as f32];
            let reduced = [rw as f32, rh as f32];
            let sigma = backdrop.blur_radius.0 / r.factor as f32;
            let base = Params {
                mirror: backdrop.mirror as u32,
                ..base
            };
            let down = push(Params {
                src_origin,
                src_size: [r.width as f32, r.height as f32],
                dst_origin: src_origin,
                dst_size: reduced,
                factor: r.factor as f32,
                ..base
            });
            let blur = |direction| Params {
                src_origin,
                src_size: reduced,
                dst_origin: src_origin,
                dst_size: reduced,
                direction,
                sigma,
                ..base
            };
            let across = push(blur([1., 0.]));
            let down_the = push(blur([0., 1.]));
            blur_draws.push((*r, down, across, down_the));
        }

        let mut composites = Vec::with_capacity(backdrops.len());
        for (backdrop, region) in backdrops.iter().zip(&regions) {
            // A surface without COPY_SRC (wgpu's GL backend offers none) cannot be blurred.
            let tint = if backdrop.source == BackdropSource::Window && !has_source {
                backdrop.unblurred_tint()
            } else {
                Rgba::from(backdrop.tint)
            };
            let mask = backdrop.content_mask.bounds;
            let radii = backdrop.corner_radii;
            let mut p = Params {
                bounds_origin: [backdrop.bounds.origin.x.0, backdrop.bounds.origin.y.0],
                bounds_size: [backdrop.bounds.size.width.0, backdrop.bounds.size.height.0],
                mask_origin: [mask.origin.x.0, mask.origin.y.0],
                mask_size: [mask.size.width.0, mask.size.height.0],
                radii: [
                    radii.top_left.0,
                    radii.top_right.0,
                    radii.bottom_right.0,
                    radii.bottom_left.0,
                ],
                tint: [tint.r, tint.g, tint.b, tint.a],
                saturation: backdrop.saturation,
                highlight: backdrop.highlight,
                highlight_width: backdrop.highlight_width.0,
                opacity: backdrop.opacity,
                mirror: backdrop.mirror as u32,
                has_clamp: backdrop.clamp.is_some() as u32,
                clamp_low: backdrop.clamp.map_or(0., |c| c[0]),
                clamp_high: backdrop.clamp.map_or(1., |c| c[1]),
                ..base
            };
            if let Some(r) = region {
                let (rw, rh) = r.reduced();
                p.src_origin = [r.x as f32, r.y as f32];
                p.dst_origin = p.src_origin;
                p.dst_size = [rw as f32, rh as f32];
                p.factor = r.factor as f32;
                p.has_source = 1;
            }
            let clip = (backdrop.mask as usize).checked_sub(1).and_then(|i| masks.get(i)).and_then(|mask| {
                let texture = (mask.width > 0
                    && mask.height > 0
                    && mask.coverage.len() >= (mask.width * mask.height) as usize)
                    .then(|| clip_texture(device, mask.width, mask.height))?;
                queue.write_texture(
                    texture.as_image_copy(),
                    &mask.coverage[..(mask.width * mask.height) as usize],
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(mask.width),
                        rows_per_image: Some(mask.height),
                    },
                    texture.size(),
                );
                p.clip_origin = [mask.bounds.origin.x.0, mask.bounds.origin.y.0];
                p.clip_size = [mask.bounds.size.width.0, mask.bounds.size.height.0];
                p.has_clip = 1;
                Some(clip_bind_group(device, &self.clip_layout, &texture))
            });
            let erase = (backdrop.source == BackdropSource::Desktop).then(|| push(Params { mode: 1, ..p }));
            if backdrop.source == BackdropSource::Desktop {
                p.mode = 2;
            }
            composites.push((push(p), erase, clip));
        }
        queue.write_buffer(&self.params, start, &params);
        self.params_cursor += params.len() as u64;

        if !blur_draws.is_empty() {
            self.ensure_scratch(device, size.0, size.1);
            let scratch = self.scratch.as_ref().expect("created above");
            for (r, ..) in &blur_draws {
                let origin = wgpu::Origin3d {
                    x: r.x,
                    y: r.y,
                    z: 0,
                };
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: frame,
                        mip_level: 0,
                        origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &scratch.copy,
                        mip_level: 0,
                        origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: r.width,
                        height: r.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            let steps = [
                (&scratch.a_view, &self.region, &scratch.from_copy, 1),
                (&scratch.b_view, &self.blur, &scratch.from_a, 2),
                (&scratch.a_view, &self.blur, &scratch.from_b, 3),
            ];
            for (target, pipeline, source, step) in steps {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("backdrop_blur_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    ..Default::default()
                });
                pass.set_pipeline(pipeline);
                for (r, down, across, down_the) in &blur_draws {
                    let offset = match step {
                        1 => *down,
                        2 => *across,
                        _ => *down_the,
                    };
                    let (rw, rh) = r.reduced();
                    pass.set_scissor_rect(r.x, r.y, rw, rh);
                    pass.set_bind_group(0, source, &[offset]);
                    pass.draw(0..4, 0..1);
                }
            }
        }

        Prepared {
            blurred: !blur_draws.is_empty(),
            composites,
        }
    }

    /// Draws the backdrops [`Self::prepare`] blurred into the resumed main pass.
    pub(crate) fn composite(&self, prepared: &Prepared, pass: &mut wgpu::RenderPass<'_>) {
        let blurred = match &self.scratch {
            Some(scratch) if prepared.blurred => &scratch.from_a,
            _ => &self.placeholder,
        };
        for (offset, erase, clip) in &prepared.composites {
            pass.set_bind_group(1, clip.as_ref().unwrap_or(&self.no_clip), &[]);
            if let Some(erase) = erase {
                pass.set_pipeline(&self.erase);
                pass.set_bind_group(0, blurred, &[*erase]);
                pass.draw(0..4, 0..1);
            }
            pass.set_pipeline(&self.composite);
            pass.set_bind_group(0, blurred, &[*offset]);
            pass.draw(0..4, 0..1);
        }
    }
}

fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    params: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
    view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("backdrop_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: params,
                    offset: 0,
                    size: NonZeroU64::new(std::mem::size_of::<Params>() as u64),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

pub(crate) struct Prepared {
    blurred: bool,
    /// The params offsets of each backdrop's composite and, for a desktop backdrop, its erase,
    /// and its mask.
    composites: Vec<(u32, Option<u32>, Option<wgpu::BindGroup>)>,
}

fn clip_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("backdrop_clip"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn clip_bind_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, texture: &wgpu::Texture) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("backdrop_clip_bind_group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&texture.create_view(&wgpu::TextureViewDescriptor::default())),
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, ContentMask, Corners, Hsla, point, px, size};

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// `pixels` (RGBA, `width` x `height`) with `backdrop` drawn over it on the first adapter
    /// there is, or None without one.
    fn draw(width: u32, height: u32, pixels: &[u8], backdrop: Backdrop) -> Option<Vec<u8>> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = gpui::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        let (device, queue) = gpui::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        let frame = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        queue.write_texture(
            frame.as_image_copy(),
            pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            frame.size(),
        );
        let mut backdrops = Backdrops::new(&device, FORMAT);
        backdrops.begin_frame(&device, &[backdrop]);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let prepared = backdrops.prepare(&device, &queue, &mut encoder, &frame, &[backdrop], &[]);
        {
            let view = frame.create_view(&wgpu::TextureViewDescriptor::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            backdrops.composite(&prepared, &mut pass);
        }
        let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            frame.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) },
            },
            frame.size(),
        );
        queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let mapped = buffer.slice(..).get_mapped_range().ok()?;
        Some(mapped.chunks(row as usize).flat_map(|r| r[..(width * 4) as usize].to_vec()).collect())
    }

    /// Black with the region's last column white: a 4 px tall region `width` px wide at (4, 2)
    /// in a 48 x 8 frame, blurred with sigma 60 and reflected at its own edges.
    fn narrow(width: u32) -> Option<Vec<u8>> {
        let (w, h) = (48, 8);
        let pixels: Vec<u8> = (0..h)
            .flat_map(|_| (0..w).flat_map(move |x| if x == 3 + width { [255; 4] } else { [0, 0, 0, 255] }))
            .collect();
        let bounds = Bounds { origin: point(px(4.), px(2.)), size: size(px(width as f32), px(4.)) }.scale(1.);
        let frame = Bounds { origin: point(px(0.), px(0.)), size: size(px(w as f32), px(h as f32)) }.scale(1.);
        let backdrop = Backdrop {
            order: 0,
            source: BackdropSource::Window,
            bounds,
            content_mask: ContentMask { bounds: frame },
            corner_radii: Corners::default(),
            blur_radius: ScaledPixels(60.),
            saturation: 1.,
            tint: Hsla::transparent_black(),
            highlight: 0.,
            highlight_width: ScaledPixels(0.),
            sample_limit: Some(bounds),
            opacity: 1.,
            fallback: Hsla::transparent_black(),
            mask: 0,
            mirror: true,
            clamp: None,
        };
        let out = draw(w, h, &pixels, backdrop)?;
        Some((4..4 + width).map(|x| out[((3 * w + x) * 4) as usize]).collect())
    }

    /// Opaque red under dark regular glass (saturation 1.5, tint (75, 73, 68) at 0.58), 8 x 8.
    fn glass_over_red() -> Option<[u8; 4]> {
        let pixels: Vec<u8> = (0..64).flat_map(|_| [255, 0, 0, 255]).collect();
        let all = Bounds { origin: point(px(0.), px(0.)), size: size(px(8.), px(8.)) }.scale(1.);
        let tint = Rgba { r: 75. / 255., g: 73. / 255., b: 68. / 255., a: 0.58 };
        let backdrop = Backdrop {
            order: 0,
            source: BackdropSource::Window,
            bounds: all,
            content_mask: ContentMask { bounds: all },
            corner_radii: Corners::default(),
            blur_radius: ScaledPixels(2.),
            saturation: 1.5,
            tint: tint.into(),
            highlight: 0.,
            highlight_width: ScaledPixels(0.),
            sample_limit: None,
            opacity: 1.,
            fallback: Hsla::transparent_black(),
            mask: 0,
            mirror: false,
            clamp: None,
        };
        let out = draw(8, 8, &pixels, backdrop)?;
        let i = (4 * 8 + 4) * 4;
        Some([out[i], out[i + 1], out[i + 2], out[i + 3]])
    }

    // The capture compositor asserts the same value: the red the saturation pushes past one
    // lifts the tinted result, and only then is it bounded by alpha.
    #[test]
    fn a_saturated_backdrop_is_bounded_by_alpha_after_its_tint() {
        let Some(got) = glass_over_red() else {
            eprintln!("no wgpu adapter, skipped");
            return;
        };
        for (v, want) in got.into_iter().zip([193, 42, 39, 255]) {
            assert!((v as i32 - want).abs() <= 1, "{got:?}, want [193, 42, 39, 255]");
        }
    }

    // atoll's capture compositor (`atoll-compositor/src/backdrop.rs`) asserts the same values
    // for the same cases, so the two blurs agree to a level.
    #[test]
    fn a_narrow_mirrored_backdrop_blurs_as_the_capture_path_does() {
        for (width, want) in [(17, 16), (3, 96), (1, 255)] {
            let Some(got) = narrow(width) else {
                eprintln!("no wgpu adapter, skipped");
                return;
            };
            for v in got {
                assert!((v as i32 - want).abs() <= 1, "{width} px: {v}, want {want}");
            }
        }
    }
}
