//! Backdrops on Direct3D 11, the same passes as gpui_wgpu's `backdrop.rs`: each region is copied
//! out of the frame, box-downsampled, blurred across and down, then composited in its rounded
//! bounds. Intermediates keep each region at its own origin, so one frame-sized texture holds a
//! batch. The shaders compile on first use, so frames without a backdrop never load the compiler.

use anyhow::{Context as _, Result};
use gpui::{Backdrop, BackdropSource, Rgba};
use windows::{
    Win32::Graphics::{
        Direct3D::{Fxc::D3DCompile, ID3DBlob},
        Direct3D11::*,
        Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC},
    },
    core::PCSTR,
};

const SHADER: &str = r#"
cbuffer Params : register(b2) {
    float2 target_size;
    float2 src_origin;
    float2 src_size;
    float2 dst_origin;
    float2 dst_size;
    float2 direction;
    float factor;
    float sigma;
    float saturation;
    float highlight;
    float2 bounds_origin;
    float2 bounds_size;
    float2 mask_origin;
    float2 mask_size;
    float4 radii;
    float4 tint;
    float highlight_width;
    uint mode;
    uint has_source;
    float opacity;
};

Texture2D<float4> source : register(t0);
SamplerState linear_clamp : register(s0);

float4 to_clip(float2 p) {
    float2 ndc = p / target_size * float2(2.0, -2.0) + float2(-1.0, 1.0);
    return float4(ndc, 0.0, 1.0);
}

float2 corner(uint index) {
    return float2(float(index & 1u), float((index >> 1u) & 1u));
}

float4 vs_region(uint index : SV_VertexID) : SV_Position {
    return to_clip(dst_origin + corner(index) * dst_size);
}

float4 vs_composite(uint index : SV_VertexID) : SV_Position {
    float2 lo = max(bounds_origin, mask_origin);
    float2 hi = min(bounds_origin + bounds_size, mask_origin + mask_size);
    return to_clip(lo + corner(index) * max(hi - lo, float2(0.0, 0.0)));
}

float4 sample_clamped(float2 p, float2 lo, float2 hi) {
    float width, height;
    source.GetDimensions(width, height);
    float2 q = clamp(p, lo + 0.5, hi - 0.5);
    return source.SampleLevel(linear_clamp, q / float2(width, height), 0.0);
}

float4 fs_downsample(float4 position : SV_Position) : SV_Target {
    float2 center = src_origin + (position.xy - dst_origin) * factor;
    float2 lo = src_origin;
    float2 hi = src_origin + src_size;
    int n = max(int(factor) / 2, 1);
    float4 sum = float4(0.0, 0.0, 0.0, 0.0);
    for (int y = 0; y < n; y++) {
        for (int x = 0; x < n; x++) {
            float2 offset = (float2(float(x), float(y)) - float(n - 1) * 0.5) * 2.0;
            sum += sample_clamped(center + offset, lo, hi);
        }
    }
    return sum / float(n * n);
}

float4 fs_blur(float4 position : SV_Position) : SV_Target {
    float2 lo = src_origin;
    float2 hi = src_origin + src_size;
    if (sigma < 0.25) {
        return sample_clamped(position.xy, lo, hi);
    }
    int reach = int(ceil(sigma * 3.0));
    float4 sum = float4(0.0, 0.0, 0.0, 0.0);
    float weight = 0.0;
    for (int i = -reach; i <= reach; i++) {
        float w = exp(-float(i * i) / (2.0 * sigma * sigma));
        sum += w * sample_clamped(position.xy + direction * float(i), lo, hi);
        weight += w;
    }
    return sum / weight;
}

float rounded_rect_sdf(float2 p) {
    float2 half_size = bounds_size * 0.5;
    float2 d = p - (bounds_origin + half_size);
    float radius = d.x < 0.0 ? (d.y < 0.0 ? radii.x : radii.w) : (d.y < 0.0 ? radii.y : radii.z);
    radius = min(radius, min(half_size.x, half_size.y));
    float2 q = abs(d) - half_size + radius;
    return length(max(q, float2(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

float4 fs_composite(float4 position : SV_Position) : SV_Target {
    float2 p = position.xy;
    float sdf = rounded_rect_sdf(p);
    float coverage = saturate(0.5 - sdf);
    if (coverage <= 0.0) {
        discard;
    }
    if (mode == 1u) {
        return float4(0.0, 0.0, 0.0, coverage * opacity);
    }

    float4 color = float4(0.0, 0.0, 0.0, 0.0);
    if (mode == 0u && has_source != 0u) {
        float2 blurred = dst_origin + (p - src_origin) / factor;
        color = sample_clamped(blurred, dst_origin, dst_origin + dst_size);
        float luma = dot(color.rgb, float3(0.2126, 0.7152, 0.0722));
        color = float4(max(lerp(float3(luma, luma, luma), color.rgb, saturation), float3(0.0, 0.0, 0.0)), color.a);
    }
    color = float4(tint.rgb * tint.a + color.rgb * (1.0 - tint.a), tint.a + color.a * (1.0 - tint.a));

    if (highlight > 0.0 && highlight_width > 0.0) {
        float e = 0.5;
        float2 normal = normalize(float2(
            rounded_rect_sdf(p + float2(e, 0.0)) - rounded_rect_sdf(p - float2(e, 0.0)),
            rounded_rect_sdf(p + float2(0.0, e)) - rounded_rect_sdf(p - float2(0.0, e))
        ) + float2(1e-6, 1e-6));
        float rim = 1.0 - smoothstep(0.0, highlight_width, -sdf);
        float facing = dot(normal, float2(0.25881905, -0.96592583));
        float light = (facing > 0.0 ? 1.0 : 0.35) * facing * facing;
        float add = saturate(highlight * rim * light);
        color = float4(color.rgb + add * (1.0 - color.rgb), color.a + add * (1.0 - color.a));
    }
    return color * coverage * opacity;
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
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    factor: u32,
}

impl Region {
    fn of(backdrop: &Backdrop, frame: (u32, u32)) -> Option<Self> {
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

    fn reduced(&self) -> (u32, u32) {
        (self.width.div_ceil(self.factor), self.height.div_ceil(self.factor))
    }
}

struct Target {
    texture: ID3D11Texture2D,
    view: Option<ID3D11RenderTargetView>,
    srv: Option<ID3D11ShaderResourceView>,
}

pub(super) struct Backdrops {
    region_vs: ID3D11VertexShader,
    composite_vs: ID3D11VertexShader,
    downsample: ID3D11PixelShader,
    blur: ID3D11PixelShader,
    composite: ID3D11PixelShader,
    replace: ID3D11BlendState,
    over: ID3D11BlendState,
    erase: ID3D11BlendState,
    sampler: Option<ID3D11SamplerState>,
    params: Option<ID3D11Buffer>,
    /// The copy of the frame, and the two textures the blur ping-pongs between.
    scratch: Option<[Target; 3]>,
}

fn compile(entry: &str, target: &str) -> Result<ID3DBlob> {
    let entry = format!("{entry}\0");
    let target = format!("{target}\0");
    let mut code = None;
    let mut errors = None;
    // SAFETY: The source pointer and length describe `SHADER`; the entry and target are
    // nul-terminated and outlive the call.
    let compiled = unsafe {
        D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR::from_raw(entry.as_ptr()),
            PCSTR::from_raw(target.as_ptr()),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(error) = compiled {
        let message = errors
            .map(|blob| {
                // SAFETY: The error blob holds the compiler's nul-terminated message.
                unsafe { std::ffi::CStr::from_ptr(blob.GetBufferPointer() as *const i8) }
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default();
        anyhow::bail!("compiling backdrop shader {entry}: {error} {message}");
    }
    code.context("the compiler returned no backdrop shader")
}

fn bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: The blob owns `GetBufferSize` bytes at `GetBufferPointer` for its lifetime.
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) }
}

fn blend(device: &ID3D11Device, source: D3D11_BLEND, destination: D3D11_BLEND, enable: bool) -> Result<ID3D11BlendState> {
    let mut desc = D3D11_BLEND_DESC::default();
    desc.RenderTarget[0].BlendEnable = enable.into();
    desc.RenderTarget[0].BlendOp = D3D11_BLEND_OP_ADD;
    desc.RenderTarget[0].BlendOpAlpha = D3D11_BLEND_OP_ADD;
    desc.RenderTarget[0].SrcBlend = source;
    desc.RenderTarget[0].SrcBlendAlpha = source;
    desc.RenderTarget[0].DestBlend = destination;
    desc.RenderTarget[0].DestBlendAlpha = destination;
    desc.RenderTarget[0].RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8;
    let mut state = None;
    // SAFETY: `desc` is a valid blend description.
    unsafe { device.CreateBlendState(&desc, Some(&mut state))? };
    state.context("creating a backdrop blend state")
}

impl Backdrops {
    pub(super) fn new(device: &ID3D11Device) -> Result<Self> {
        let vertex = |entry| -> Result<ID3D11VertexShader> {
            super::create_vertex_shader(device, bytes(&compile(entry, "vs_4_1")?))
        };
        let pixel = |entry| -> Result<ID3D11PixelShader> {
            super::create_fragment_shader(device, bytes(&compile(entry, "ps_4_1")?))
        };
        let sampler_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxLOD: D3D11_FLOAT32_MAX,
            ..Default::default()
        };
        let mut sampler = None;
        // SAFETY: `sampler_desc` is a valid sampler description.
        unsafe { device.CreateSamplerState(&sampler_desc, Some(&mut sampler))? };
        let params_desc = D3D11_BUFFER_DESC {
            ByteWidth: std::mem::size_of::<Params>() as u32,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            ..Default::default()
        };
        let mut params = None;
        // SAFETY: `params_desc` describes a 144-byte dynamic constant buffer.
        unsafe { device.CreateBuffer(&params_desc, None, Some(&mut params))? };
        Ok(Self {
            region_vs: vertex("vs_region")?,
            composite_vs: vertex("vs_composite")?,
            downsample: pixel("fs_downsample")?,
            blur: pixel("fs_blur")?,
            composite: pixel("fs_composite")?,
            replace: blend(device, D3D11_BLEND_ONE, D3D11_BLEND_ZERO, false)?,
            over: blend(device, D3D11_BLEND_ONE, D3D11_BLEND_INV_SRC_ALPHA, true)?,
            erase: blend(device, D3D11_BLEND_ZERO, D3D11_BLEND_INV_SRC_ALPHA, true)?,
            sampler,
            params,
            scratch: None,
        })
    }

    fn ensure_scratch(&mut self, device: &ID3D11Device, format: DXGI_FORMAT, width: u32, height: u32) -> Result<()> {
        if let Some([copy, ..]) = &self.scratch {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: `desc` is written by the texture.
            unsafe { copy.texture.GetDesc(&mut desc) };
            if desc.Width == width && desc.Height == height && desc.Format == format {
                return Ok(());
            }
        }
        let target = || -> Result<Target> {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                ..Default::default()
            };
            let mut texture = None;
            // SAFETY: `desc` describes a frame-sized 2D texture in the frame's format.
            unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
            let texture = texture.context("creating a backdrop texture")?;
            let (mut view, mut srv) = (None, None);
            // SAFETY: The texture is bound as both a render target and a shader resource.
            unsafe {
                device.CreateRenderTargetView(&texture, None, Some(&mut view))?;
                device.CreateShaderResourceView(&texture, None, Some(&mut srv))?;
            }
            Ok(Target { texture, view, srv })
        };
        self.scratch = Some([target()?, target()?, target()?]);
        Ok(())
    }

    fn draw(&self, context: &ID3D11DeviceContext, params: &Params, vs: &ID3D11VertexShader, ps: &ID3D11PixelShader) -> Result<()> {
        super::update_buffer(context, self.params.as_ref().context("missing backdrop params")?, std::slice::from_ref(params))?;
        // SAFETY: Every bound object is alive; the constant buffer slot is the shader's b2.
        unsafe {
            context.VSSetShader(vs, None);
            context.PSSetShader(ps, None);
            context.VSSetConstantBuffers(2, Some(std::slice::from_ref(&self.params)));
            context.PSSetConstantBuffers(2, Some(std::slice::from_ref(&self.params)));
            context.Draw(4, 0);
        }
        Ok(())
    }

    /// Draws a batch of backdrops over `frame`, whose render target view is bound, and leaves it
    /// bound again afterwards.
    pub(super) fn draw_batch(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        frame: &ID3D11Texture2D,
        frame_view: &Option<ID3D11RenderTargetView>,
        backdrops: &[Backdrop],
    ) -> Result<()> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` is written by the texture.
        unsafe { frame.GetDesc(&mut desc) };
        let size = (desc.Width, desc.Height);
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

        // SAFETY: The topology, sampler and blend state are valid for every draw below.
        unsafe {
            context.IASetPrimitiveTopology(windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            context.PSSetSamplers(0, Some(std::slice::from_ref(&self.sampler)));
        }

        let blurred = regions.iter().any(Option::is_some);
        if blurred {
            self.ensure_scratch(device, desc.Format, size.0, size.1)?;
            let scratch = self.scratch.as_ref().expect("created above");
            for r in regions.iter().flatten() {
                let region = D3D11_BOX {
                    left: r.x,
                    top: r.y,
                    front: 0,
                    right: r.x + r.width,
                    bottom: r.y + r.height,
                    back: 1,
                };
                // SAFETY: Both textures have the frame's size and format; the box lies inside.
                unsafe { context.CopySubresourceRegion(&scratch[0].texture, 0, r.x, r.y, 0, frame, 0, Some(&region)) };
            }
            let steps = [(1, 0, &self.downsample), (2, 1, &self.blur), (1, 2, &self.blur)];
            for (step, (target, source, shader)) in steps.into_iter().enumerate() {
                // SAFETY: The target is not bound as a shader resource while it is rendered to.
                unsafe {
                    context.PSSetShaderResources(0, Some(&[None]));
                    context.OMSetRenderTargets(Some(std::slice::from_ref(&scratch[target].view)), None);
                    context.PSSetShaderResources(0, Some(std::slice::from_ref(&scratch[source].srv)));
                    context.OMSetBlendState(&self.replace, None, 0xFFFFFFFF);
                }
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
                    self.draw(context, &params, &self.region_vs, shader)?;
                }
            }
        }

        // SAFETY: The blurred texture is unbound as a render target before it is sampled.
        unsafe {
            context.OMSetRenderTargets(Some(std::slice::from_ref(frame_view)), None);
            let source = self.scratch.as_ref().filter(|_| blurred).and_then(|s| s[1].srv.clone());
            context.PSSetShaderResources(0, Some(&[source]));
        }
        for (backdrop, region) in backdrops.iter().zip(&regions) {
            let tint = Rgba::from(backdrop.tint);
            let mask = backdrop.content_mask.bounds;
            let radii = backdrop.corner_radii;
            let mut p = Params {
                bounds_origin: [backdrop.bounds.origin.x.0, backdrop.bounds.origin.y.0],
                bounds_size: [backdrop.bounds.size.width.0, backdrop.bounds.size.height.0],
                mask_origin: [mask.origin.x.0, mask.origin.y.0],
                mask_size: [mask.size.width.0, mask.size.height.0],
                radii: [radii.top_left.0, radii.top_right.0, radii.bottom_right.0, radii.bottom_left.0],
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
            if backdrop.source == BackdropSource::Desktop {
                // SAFETY: A valid blend state.
                unsafe { context.OMSetBlendState(&self.erase, None, 0xFFFFFFFF) };
                self.draw(context, &Params { mode: 1, ..p }, &self.composite_vs, &self.composite)?;
                p.mode = 2;
            }
            // SAFETY: A valid blend state.
            unsafe { context.OMSetBlendState(&self.over, None, 0xFFFFFFFF) };
            self.draw(context, &p, &self.composite_vs, &self.composite)?;
        }
        // SAFETY: Unbinds the blurred texture so the next batch can render to it.
        unsafe { context.PSSetShaderResources(0, Some(&[None])) };
        Ok(())
    }
}
