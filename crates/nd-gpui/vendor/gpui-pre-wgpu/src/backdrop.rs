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
use gpui::{Backdrop, BackdropSource, Rgba};

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
    pad: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

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
    let q = clamp(p, lo + 0.5, hi - 0.5);
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
    let coverage = saturate(0.5 - sdf);
    if (coverage <= 0.0) {
        discard;
    }
    if (params.mode == 1u) {
        return vec4<f32>(0.0, 0.0, 0.0, coverage);
    }

    var color = vec4<f32>(0.0);
    if (params.mode == 0u && params.has_source != 0u) {
        let blurred = params.dst_origin + (p - params.src_origin) / params.factor;
        color = sample_clamped(blurred, params.dst_origin, params.dst_origin + params.dst_size);
        let luma = dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        color = vec4<f32>(max(mix(vec3<f32>(luma), color.rgb, params.saturation), vec3<f32>(0.0)), color.a);
    }
    let tint = params.tint;
    color = vec4<f32>(tint.rgb * tint.a + color.rgb * (1.0 - tint.a), tint.a + color.a * (1.0 - tint.a));

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
    return color * coverage;
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
    pad: u32,
}

const _: () = assert!(std::mem::offset_of!(Params, radii) == 96 && std::mem::size_of::<Params>() == 144);

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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("backdrop_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("backdrop_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = |label: &str, vs: &str, fs: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
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
            let tint = Rgba::from(backdrop.tint);
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
            let erase = (backdrop.source == BackdropSource::Desktop).then(|| push(Params { mode: 1, ..p }));
            if backdrop.source == BackdropSource::Desktop {
                p.mode = 2;
            }
            composites.push((push(p), erase));
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
        for (offset, erase) in &prepared.composites {
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
    /// The params offsets of each backdrop's composite and, for a desktop backdrop, its erase.
    composites: Vec<(u32, Option<u32>)>,
}
