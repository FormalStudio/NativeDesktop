//! Backdrops on Metal, the same passes as gpui_wgpu's `backdrop.rs`: each region is copied out of
//! the frame, box-downsampled, blurred across and down, then composited in its rounded bounds.
//! Intermediates keep each region at its own origin, so one frame-sized texture holds a batch.

use std::{mem, ptr};

use anyhow::{Context as _, Result};
use gpui::{Backdrop, BackdropSource, Rgba};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLBlitCommandEncoder, MTLCommandBuffer,
    MTLCommandEncoder, MTLDevice, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat,
    MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
    MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLScissorRect, MTLSize,
    MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

const SHADER: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Params {
    packed_float2 target_size;
    packed_float2 src_origin;
    packed_float2 src_size;
    packed_float2 dst_origin;
    packed_float2 dst_size;
    packed_float2 direction;
    float factor;
    float sigma;
    float saturation;
    float highlight;
    packed_float2 bounds_origin;
    packed_float2 bounds_size;
    packed_float2 mask_origin;
    packed_float2 mask_size;
    packed_float4 radii;
    packed_float4 tint;
    float highlight_width;
    uint mode;
    uint has_source;
    float opacity;
};

struct Varyings {
    float4 position [[position]];
};

constexpr sampler linear_clamp(filter::linear, address::clamp_to_edge, coord::normalized);

static float4 to_clip(float2 p, constant Params &params) {
    float2 ndc = p / float2(params.target_size) * float2(2.0, -2.0) + float2(-1.0, 1.0);
    return float4(ndc, 0.0, 1.0);
}

static float2 corner(uint index) {
    return float2(float(index & 1u), float((index >> 1u) & 1u));
}

vertex Varyings backdrop_vs_region(uint index [[vertex_id]], constant Params &params [[buffer(0)]]) {
    Varyings out;
    out.position = to_clip(float2(params.dst_origin) + corner(index) * float2(params.dst_size), params);
    return out;
}

vertex Varyings backdrop_vs_composite(uint index [[vertex_id]], constant Params &params [[buffer(0)]]) {
    float2 lo = max(float2(params.bounds_origin), float2(params.mask_origin));
    float2 hi = min(float2(params.bounds_origin) + float2(params.bounds_size),
                    float2(params.mask_origin) + float2(params.mask_size));
    Varyings out;
    out.position = to_clip(lo + corner(index) * max(hi - lo, float2(0.0)), params);
    return out;
}

static float4 sample_clamped(texture2d<float> source, float2 p, float2 lo, float2 hi) {
    float2 size = float2(source.get_width(), source.get_height());
    float2 q = clamp(p, lo + 0.5, hi - 0.5);
    return source.sample(linear_clamp, q / size, level(0));
}

fragment float4 backdrop_fs_downsample(Varyings in [[stage_in]], constant Params &params [[buffer(0)]],
                                       texture2d<float> source [[texture(0)]]) {
    float2 center = float2(params.src_origin) + (in.position.xy - float2(params.dst_origin)) * params.factor;
    float2 lo = float2(params.src_origin);
    float2 hi = lo + float2(params.src_size);
    int n = max(int(params.factor) / 2, 1);
    float4 sum = float4(0.0);
    for (int y = 0; y < n; y++) {
        for (int x = 0; x < n; x++) {
            float2 offset = (float2(float(x), float(y)) - float(n - 1) * 0.5) * 2.0;
            sum += sample_clamped(source, center + offset, lo, hi);
        }
    }
    return sum / float(n * n);
}

fragment float4 backdrop_fs_blur(Varyings in [[stage_in]], constant Params &params [[buffer(0)]],
                                 texture2d<float> source [[texture(0)]]) {
    float2 lo = float2(params.src_origin);
    float2 hi = lo + float2(params.src_size);
    float sigma = params.sigma;
    if (sigma < 0.25) {
        return sample_clamped(source, in.position.xy, lo, hi);
    }
    int reach = int(ceil(sigma * 3.0));
    float4 sum = float4(0.0);
    float weight = 0.0;
    for (int i = -reach; i <= reach; i++) {
        float w = exp(-float(i * i) / (2.0 * sigma * sigma));
        sum += w * sample_clamped(source, in.position.xy + float2(params.direction) * float(i), lo, hi);
        weight += w;
    }
    return sum / weight;
}

static float rounded_rect_sdf(float2 p, constant Params &params) {
    float2 half_size = float2(params.bounds_size) * 0.5;
    float2 center = float2(params.bounds_origin) + half_size;
    float2 d = p - center;
    float4 radii = float4(params.radii);
    float radius = d.x < 0.0 ? (d.y < 0.0 ? radii.x : radii.w) : (d.y < 0.0 ? radii.y : radii.z);
    radius = min(radius, min(half_size.x, half_size.y));
    float2 q = abs(d) - half_size + radius;
    return length(max(q, float2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fragment float4 backdrop_fs_composite(Varyings in [[stage_in]], constant Params &params [[buffer(0)]],
                                      texture2d<float> source [[texture(0)]]) {
    float2 p = in.position.xy;
    float sdf = rounded_rect_sdf(p, params);
    float coverage = saturate(0.5 - sdf);
    if (coverage <= 0.0) {
        discard_fragment();
    }
    if (params.mode == 1u) {
        return float4(0.0, 0.0, 0.0, coverage * params.opacity);
    }

    float4 color = float4(0.0);
    if (params.mode == 0u && params.has_source != 0u) {
        float2 dst_origin = float2(params.dst_origin);
        float2 blurred = dst_origin + (p - float2(params.src_origin)) / params.factor;
        color = sample_clamped(source, blurred, dst_origin, dst_origin + float2(params.dst_size));
        float luma = dot(color.rgb, float3(0.2126, 0.7152, 0.0722));
        color = float4(max(mix(float3(luma), color.rgb, params.saturation), float3(0.0)), color.a);
    }
    float4 tint = float4(params.tint);
    color = float4(tint.rgb * tint.a + color.rgb * (1.0 - tint.a), tint.a + color.a * (1.0 - tint.a));

    if (params.highlight > 0.0 && params.highlight_width > 0.0) {
        float e = 0.5;
        float2 normal = normalize(float2(
            rounded_rect_sdf(p + float2(e, 0.0), params) - rounded_rect_sdf(p - float2(e, 0.0), params),
            rounded_rect_sdf(p + float2(0.0, e), params) - rounded_rect_sdf(p - float2(0.0, e), params)
        ) + float2(1e-6));
        float rim = 1.0 - smoothstep(0.0, params.highlight_width, -sdf);
        float facing = dot(normal, float2(0.25881905, -0.96592583));
        float light = (facing > 0.0 ? 1.0 : 0.35) * facing * facing;
        float add = saturate(params.highlight * rim * light);
        color = float4(color.rgb + add * (1.0 - color.rgb), color.a + add * (1.0 - color.a));
    }
    return color * coverage * params.opacity;
}
"#;

#[derive(Clone, Copy, Default)]
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
}

const _: () = assert!(std::mem::offset_of!(Params, radii) == 96 && std::mem::size_of::<Params>() == 144);

#[derive(Clone, Copy)]
struct Region {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    factor: usize,
}

impl Region {
    fn of(backdrop: &Backdrop, frame: (usize, usize)) -> Option<Self> {
        let sample = backdrop.sample_bounds();
        let x0 = sample.origin.x.0.floor().max(0.) as usize;
        let y0 = sample.origin.y.0.floor().max(0.) as usize;
        let x1 = (sample.origin.x.0 + sample.size.width.0).ceil().min(frame.0 as f32);
        let y1 = (sample.origin.y.0 + sample.size.height.0).ceil().min(frame.1 as f32);
        if x1 <= x0 as f32 || y1 <= y0 as f32 {
            return None;
        }
        Some(Self {
            x: x0,
            y: y0,
            width: x1 as usize - x0,
            height: y1 as usize - y0,
            factor: backdrop.downsample_factor() as usize,
        })
    }

    fn reduced(&self) -> (usize, usize) {
        (self.width.div_ceil(self.factor), self.height.div_ceil(self.factor))
    }
}

type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Pipeline = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;

pub(crate) struct Backdrops {
    region: Pipeline,
    blur: Pipeline,
    composite: Pipeline,
    erase: Pipeline,
    /// The copy of the frame, and the two textures the blur ping-pongs between.
    scratch: Option<[Texture; 3]>,
    /// Bound where a batch blurs nothing, for the texture the composite function declares.
    placeholder: Texture,
}

enum Blend {
    None,
    Over,
    Erase,
}

impl Backdrops {
    pub(crate) fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<Self> {
        let library = device
            .newLibraryWithSource_options_error(
                &NSString::from_str(SHADER),
                Some(&objc2_metal::MTLCompileOptions::new()),
            )
            .context("building the backdrop Metal library")?;
        let pipeline = |label: &str, vertex: &str, fragment: &str, blend: Blend| -> Result<Pipeline> {
            let vertex_fn = library
                .newFunctionWithName(&NSString::from_str(vertex))
                .with_context(|| format!("locating vertex function {vertex}"))?;
            let fragment_fn = library
                .newFunctionWithName(&NSString::from_str(fragment))
                .with_context(|| format!("locating fragment function {fragment}"))?;
            let descriptor = MTLRenderPipelineDescriptor::new();
            descriptor.setLabel(Some(&NSString::from_str(label)));
            descriptor.setVertexFunction(Some(vertex_fn.as_ref()));
            descriptor.setFragmentFunction(Some(fragment_fn.as_ref()));
            // SAFETY: Index zero is within the pipeline's color attachment array.
            let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
            attachment.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            let (source, destination) = match blend {
                Blend::None => (MTLBlendFactor::One, MTLBlendFactor::Zero),
                Blend::Over => (MTLBlendFactor::One, MTLBlendFactor::OneMinusSourceAlpha),
                Blend::Erase => (MTLBlendFactor::Zero, MTLBlendFactor::OneMinusSourceAlpha),
            };
            attachment.setBlendingEnabled(!matches!(blend, Blend::None));
            attachment.setRgbBlendOperation(MTLBlendOperation::Add);
            attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
            attachment.setSourceRGBBlendFactor(source);
            attachment.setSourceAlphaBlendFactor(source);
            attachment.setDestinationRGBBlendFactor(destination);
            attachment.setDestinationAlphaBlendFactor(destination);
            device
                .newRenderPipelineStateWithDescriptor_error(&descriptor)
                .with_context(|| format!("creating {label} render pipeline"))
        };
        let descriptor = MTLTextureDescriptor::new();
        // SAFETY: A 1x1 2D texture.
        unsafe {
            descriptor.setWidth(1);
            descriptor.setHeight(1);
        }
        descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        descriptor.setStorageMode(MTLStorageMode::Private);
        descriptor.setUsage(MTLTextureUsage::ShaderRead);
        let placeholder = device
            .newTextureWithDescriptor(&descriptor)
            .context("allocating the backdrop placeholder texture")?;
        Ok(Self {
            region: pipeline(
                "backdrop_downsample",
                "backdrop_vs_region",
                "backdrop_fs_downsample",
                Blend::None,
            )?,
            blur: pipeline("backdrop_blur", "backdrop_vs_region", "backdrop_fs_blur", Blend::None)?,
            composite: pipeline(
                "backdrop_composite",
                "backdrop_vs_composite",
                "backdrop_fs_composite",
                Blend::Over,
            )?,
            erase: pipeline(
                "backdrop_erase",
                "backdrop_vs_composite",
                "backdrop_fs_composite",
                Blend::Erase,
            )?,
            scratch: None,
            placeholder,
        })
    }

    fn scratch(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        width: usize,
        height: usize,
    ) -> Result<[Texture; 3]> {
        if let Some(scratch) = &self.scratch
            && scratch[0].width() == width
            && scratch[0].height() == height
        {
            return Ok(scratch.clone());
        }
        let descriptor = MTLTextureDescriptor::new();
        // SAFETY: The dimensions are those of the frame, positive, for a 2D texture.
        unsafe {
            descriptor.setWidth(width);
            descriptor.setHeight(height);
        }
        descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        descriptor.setStorageMode(MTLStorageMode::Private);
        descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        let texture = || {
            device
                .newTextureWithDescriptor(&descriptor)
                .context("allocating a backdrop texture")
        };
        let scratch = [texture()?, texture()?, texture()?];
        self.scratch = Some(scratch.clone());
        Ok(scratch)
    }

    /// Ends nothing and starts nothing: call it between render encoders on `frame`. Blurs every
    /// backdrop's region into the scratch textures and returns what [`Self::composite`] draws.
    pub(crate) fn prepare(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
        frame: &ProtocolObject<dyn MTLTexture>,
        backdrops: &[Backdrop],
    ) -> Result<Prepared> {
        let size = (frame.width(), frame.height());
        let base = Params {
            target_size: [size.0 as f32, size.1 as f32],
            factor: 1.,
            saturation: 1.,
            ..Default::default()
        };
        let regions: Vec<Option<Region>> = backdrops
            .iter()
            .map(|b| match b.source {
                BackdropSource::Window => Region::of(b, size),
                BackdropSource::Desktop => None,
            })
            .collect();

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
                opacity: backdrop.opacity,
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
            let desktop = backdrop.source == BackdropSource::Desktop;
            if desktop {
                p.mode = 2;
            }
            composites.push((p, desktop));
        }

        if regions.iter().all(Option::is_none) {
            return Ok(Prepared {
                composites,
                blurred: None,
            });
        }
        let [copy, a, b] = self.scratch(device, size.0, size.1)?;
        let blit = command_buffer
            .blitCommandEncoder()
            .context("creating the backdrop blit encoder")?;
        for r in regions.iter().flatten() {
            let origin = MTLOrigin {
                x: r.x,
                y: r.y,
                z: 0,
            };
            // SAFETY: Both textures are the frame's size and format; the region lies inside it.
            unsafe {
                blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    frame,
                    0,
                    0,
                    origin,
                    MTLSize {
                        width: r.width,
                        height: r.height,
                        depth: 1,
                    },
                    &copy,
                    0,
                    0,
                    origin,
                );
            }
        }
        blit.endEncoding();

        let steps: [(&Texture, &Pipeline, &Texture, usize); 3] = [
            (&a, &self.region, &copy, 0),
            (&b, &self.blur, &a, 1),
            (&a, &self.blur, &b, 2),
        ];
        for (target, pipeline, source, step) in steps {
            let descriptor = MTLRenderPassDescriptor::new();
            // SAFETY: Index zero is within the render pass's color attachment array.
            let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
            attachment.setTexture(Some(target));
            attachment.setLoadAction(MTLLoadAction::Load);
            attachment.setStoreAction(MTLStoreAction::Store);
            let encoder = command_buffer
                .renderCommandEncoderWithDescriptor(&descriptor)
                .context("creating a backdrop blur encoder")?;
            encoder.setRenderPipelineState(pipeline);
            for (backdrop, r) in backdrops.iter().zip(&regions) {
                let Some(r) = r else { continue };
                let (rw, rh) = r.reduced();
                let origin = [r.x as f32, r.y as f32];
                let reduced = [rw as f32, rh as f32];
                let params = if step == 0 {
                    Params {
                        src_origin: origin,
                        src_size: [r.width as f32, r.height as f32],
                        dst_origin: origin,
                        dst_size: reduced,
                        factor: r.factor as f32,
                        ..base
                    }
                } else {
                    Params {
                        src_origin: origin,
                        src_size: reduced,
                        dst_origin: origin,
                        dst_size: reduced,
                        direction: if step == 1 { [1., 0.] } else { [0., 1.] },
                        sigma: backdrop.blur_radius.0 / r.factor as f32,
                        ..base
                    }
                };
                encoder.setScissorRect(MTLScissorRect {
                    x: r.x,
                    y: r.y,
                    width: rw,
                    height: rh,
                });
                draw(&encoder, &params, source);
            }
            encoder.endEncoding();
        }

        Ok(Prepared {
            composites,
            blurred: Some(a),
        })
    }

    /// Draws the prepared backdrops into the frame's resumed render encoder.
    pub(crate) fn composite(
        &self,
        prepared: &Prepared,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) {
        let source = prepared.blurred.as_ref().unwrap_or(&self.placeholder);
        for (params, desktop) in &prepared.composites {
            if *desktop {
                encoder.setRenderPipelineState(&self.erase);
                draw(encoder, &Params { mode: 1, ..*params }, source);
            }
            encoder.setRenderPipelineState(&self.composite);
            draw(encoder, params, source);
        }
    }
}

fn draw(
    encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    params: &Params,
    source: &Texture,
) {
    // SAFETY: The bytes are a live `Params`, which matches the shader's packed `Params`, copied
    // into slot 0 of both stages; the texture is retained by the encoder.
    unsafe {
        encoder.setVertexBytes_length_atIndex(
            ptr::NonNull::from(params).cast(),
            mem::size_of::<Params>(),
            0,
        );
        encoder.setFragmentBytes_length_atIndex(
            ptr::NonNull::from(params).cast(),
            mem::size_of::<Params>(),
            0,
        );
        encoder.setFragmentTexture_atIndex(Some(source.as_ref()), 0);
        encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::TriangleStrip, 0, 4);
    }
}

pub(crate) struct Prepared {
    composites: Vec<(Params, bool)>,
    /// The blurred regions, where the batch had any.
    blurred: Option<Texture>,
}
