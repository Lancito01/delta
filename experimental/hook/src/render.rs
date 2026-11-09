//! Draws Delta's sprites into the game's back buffer.

use crate::shm;
use delta_shared::{API_D3D11, API_D3D12, Sprite};
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Direct3D11on12::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::core::{IUnknown, Interface, PCSTR, Result, s};

const SHADER: &str = r#"
cbuffer C : register(b0) { float4 rect; };
struct V { float4 pos : SV_Position; float2 uv : TEXCOORD0; };
V vs(uint id : SV_VertexID) {
    float2 t = float2(id & 1, id >> 1);
    V o;
    o.pos = float4(lerp(rect.x, rect.z, t.x), lerp(rect.y, rect.w, t.y), 0, 1);
    o.uv = t;
    return o;
}
Texture2D tex : register(t0);
SamplerState smp : register(s0);
float4 ps(V i) : SV_Target { return tex.Sample(smp, i.uv); }
"#;

struct Tex {
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    w: u32,
    h: u32,
    version: u32,
}

/// Everything needed to draw sprites with one D3D11 device.
struct D11 {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    /// Our own pipeline state, swapped in while drawing so the game's state stays untouched
    /// (None when the context is ours alone, as with D3D11On12).
    state: Option<ID3DDeviceContextState>,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    blend: ID3D11BlendState,
    raster: ID3D11RasterizerState,
    depth: ID3D11DepthStencilState,
    tex: Vec<Option<Tex>>,
}

enum Backend {
    D11 { d: D11, rtv: Option<(usize, ID3D11RenderTargetView)> },
    D12 { d: D11, on12: ID3D11On12Device, buffers: Vec<Option<(ID3D11Resource, ID3D11RenderTargetView)>> },
}

struct Renderer {
    swapchain: usize,
    backend: Backend,
}

unsafe impl Send for Renderer {}

static STATE: Mutex<Option<Renderer>> = Mutex::new(None);

fn compile(entry: PCSTR, target: PCSTR) -> Result<Vec<u8>> {
    unsafe {
        let mut code = None;
        let mut errors = None;
        D3DCompile(SHADER.as_ptr() as *const c_void, SHADER.len(), PCSTR::null(), None, None, entry, target, 0, 0, &mut code, Some(&mut errors))?;
        let code = code.unwrap();
        Ok(std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, code.GetBufferSize()).to_vec())
    }
}

impl D11 {
    fn new(device: ID3D11Device, ctx: ID3D11DeviceContext, isolate: bool) -> Result<D11> {
        unsafe {
            let vsb = compile(s!("vs"), s!("vs_4_0"))?;
            let psb = compile(s!("ps"), s!("ps_4_0"))?;
            let mut vs = None;
            device.CreateVertexShader(&vsb, None, Some(&mut vs))?;
            let mut ps = None;
            device.CreatePixelShader(&psb, None, Some(&mut ps))?;
            let mut cb = None;
            device.CreateBuffer(&D3D11_BUFFER_DESC { ByteWidth: 16, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() }, None, Some(&mut cb))?;
            let mut sampler = None;
            device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MaxLOD: f32::MAX,
                    ..Default::default()
                },
                Some(&mut sampler),
            )?;
            // Premultiplied alpha, as Direct2D renders it.
            let mut bd = D3D11_BLEND_DESC::default();
            bd.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let mut blend = None;
            device.CreateBlendState(&bd, Some(&mut blend))?;
            let mut raster = None;
            device.CreateRasterizerState(&D3D11_RASTERIZER_DESC { FillMode: D3D11_FILL_SOLID, CullMode: D3D11_CULL_NONE, DepthClipEnable: true.into(), ..Default::default() }, Some(&mut raster))?;
            let mut depth = None;
            device.CreateDepthStencilState(&D3D11_DEPTH_STENCIL_DESC { DepthEnable: false.into(), StencilEnable: false.into(), ..Default::default() }, Some(&mut depth))?;
            let state = if isolate {
                let d1: ID3D11Device1 = device.cast()?;
                let mut st = None;
                d1.CreateDeviceContextState(0, &[device.GetFeatureLevel()], D3D11_SDK_VERSION, &ID3D11Device::IID, None, Some(&mut st))?;
                st
            } else {
                None
            };
            Ok(D11 {
                device,
                ctx,
                state,
                vs: vs.unwrap(),
                ps: ps.unwrap(),
                cb: cb.unwrap(),
                sampler: sampler.unwrap(),
                blend: blend.unwrap(),
                raster: raster.unwrap(),
                depth: depth.unwrap(),
                tex: (0..delta_shared::MAX_SPRITES).map(|_| None).collect(),
            })
        }
    }

    /// Uploads changed sprites. Returns false if Delta rewrote them meanwhile (try next frame).
    fn upload(&mut self, seq: u32, sprites: &[Sprite]) -> bool {
        let Some(base) = shm::pixels() else { return false };
        unsafe {
            for (i, s) in sprites.iter().enumerate() {
                if s.w == 0 || s.h == 0 || s.offset as usize + (s.w * s.h * 4) as usize > delta_shared::PIXEL_BYTES {
                    continue;
                }
                let fresh = self.tex[i].as_ref().is_some_and(|t| t.version == s.version && t.w == s.w && t.h == s.h);
                if fresh {
                    continue;
                }
                if self.tex[i].as_ref().is_none_or(|t| t.w != s.w || t.h != s.h) {
                    let desc = D3D11_TEXTURE2D_DESC {
                        Width: s.w,
                        Height: s.h,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                        ..Default::default()
                    };
                    let mut tex = None;
                    if self.device.CreateTexture2D(&desc, None, Some(&mut tex)).is_err() {
                        continue;
                    }
                    let tex = tex.unwrap();
                    let mut srv = None;
                    if self.device.CreateShaderResourceView(&tex, None, Some(&mut srv)).is_err() {
                        continue;
                    }
                    self.tex[i] = Some(Tex { tex, srv: srv.unwrap(), w: s.w, h: s.h, version: 0 });
                }
                let t = self.tex[i].as_mut().unwrap();
                self.ctx.UpdateSubresource(&t.tex, 0, None, base.add(s.offset as usize) as *const c_void, s.w * 4, 0);
                t.version = s.version;
            }
        }
        if !shm::seq_unchanged(seq) {
            for t in self.tex.iter_mut().flatten() {
                t.version = 0;
            }
            return false;
        }
        true
    }

    fn draw(&mut self, rtv: &ID3D11RenderTargetView, w: u32, h: u32, sprites: &[Sprite]) {
        unsafe {
            let mut previous = None;
            let ctx1 = self.ctx.cast::<ID3D11DeviceContext1>().ok();
            if let (Some(st), Some(c1)) = (&self.state, &ctx1) {
                c1.SwapDeviceContextState(st, Some(&mut previous));
            }
            let c = &self.ctx;
            c.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            c.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: w as f32, Height: h as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
            c.RSSetState(&self.raster);
            c.OMSetBlendState(&self.blend, None, 0xFFFF_FFFF);
            c.OMSetDepthStencilState(&self.depth, 0);
            c.IASetInputLayout(None);
            c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            c.VSSetShader(&self.vs, None);
            c.PSSetShader(&self.ps, None);
            c.VSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            c.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            let (fw, fh) = (w as f32, h as f32);
            for (i, s) in sprites.iter().enumerate() {
                let Some(t) = &self.tex[i] else { continue };
                if t.version == 0 {
                    continue;
                }
                let rect = [s.x as f32 / fw * 2.0 - 1.0, 1.0 - s.y as f32 / fh * 2.0, (s.x as f32 + s.w as f32) / fw * 2.0 - 1.0, 1.0 - (s.y as f32 + s.h as f32) / fh * 2.0];
                c.UpdateSubresource(&self.cb, 0, None, rect.as_ptr() as *const c_void, 0, 0);
                c.PSSetShaderResources(0, Some(&[Some(t.srv.clone())]));
                c.Draw(4, 0);
            }
            c.PSSetShaderResources(0, Some(&[None]));
            c.OMSetRenderTargets(None, None);
            if let (Some(prev), Some(c1)) = (previous, &ctx1) {
                c1.SwapDeviceContextState(&prev, None);
            }
        }
    }
}

fn now() -> u64 {
    unsafe { GetTickCount64() }
}

/// Called on every Present.
pub unsafe fn frame(sc: &IDXGISwapChain) {
    unsafe {
        let Ok(desc) = sc.GetDesc() else { return };
        let (w, h) = (desc.BufferDesc.Width, desc.BufferDesc.Height);
        let is12 = sc.GetDevice::<ID3D12Device>().is_ok();
        if let Some(slot) = shm::slot() {
            slot.width.store(w, Ordering::Relaxed);
            slot.height.store(h, Ordering::Relaxed);
            slot.api.store(if is12 { API_D3D12 } else { API_D3D11 }, Ordering::Relaxed);
            slot.frames.fetch_add(1, Ordering::Relaxed);
            slot.last_present.store(now(), Ordering::Release);
        } else if shm::header().is_none() {
            shm::open();
        }
        if !shm::active() || w == 0 || h == 0 {
            return;
        }
        let Some((seq, sprites)) = shm::sprites() else { return };
        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let key = sc.as_raw() as usize;
        if is12 && crate::QUEUE.load(Ordering::Acquire).is_null() {
            return; // the game hasn't submitted work since we arrived; next frame
        }
        if guard.as_ref().is_none_or(|r| r.swapchain != key) {
            *guard = match make(sc, is12) {
                Ok(b) => Some(Renderer { swapchain: key, backend: b }),
                Err(e) => {
                    crate::log(&format!("can't draw in this swap chain: {e}"));
                    return;
                }
            };
        }
        let Some(r) = guard.as_mut() else { return };
        match &mut r.backend {
            Backend::D11 { d, rtv } => {
                let Ok(buf) = sc.GetBuffer::<ID3D11Texture2D>(0) else { return };
                let bkey = buf.as_raw() as usize;
                if rtv.as_ref().is_none_or(|(k, _)| *k != bkey) {
                    let mut v = None;
                    if d.device.CreateRenderTargetView(&buf, None, Some(&mut v)).is_err() {
                        return;
                    }
                    *rtv = Some((bkey, v.unwrap()));
                }
                d.upload(seq, &sprites);
                let view = rtv.as_ref().unwrap().1.clone();
                d.draw(&view, w, h, &sprites);
            }
            Backend::D12 { d, on12, buffers } => {
                let Ok(sc3) = sc.cast::<IDXGISwapChain3>() else { return };
                let idx = sc3.GetCurrentBackBufferIndex() as usize;
                if buffers.len() != desc.BufferCount as usize {
                    buffers.clear();
                    buffers.resize_with(desc.BufferCount as usize, || None);
                }
                if idx >= buffers.len() {
                    return;
                }
                if buffers[idx].is_none() {
                    let Ok(res12) = sc.GetBuffer::<ID3D12Resource>(idx as u32) else { return };
                    let flags = D3D11_RESOURCE_FLAGS { BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32, ..Default::default() };
                    let mut res11: Option<ID3D11Resource> = None;
                    if on12.CreateWrappedResource(&res12, &flags, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_PRESENT, &mut res11).is_err() {
                        return;
                    }
                    let res11 = res11.unwrap();
                    let mut v = None;
                    if d.device.CreateRenderTargetView(&res11, None, Some(&mut v)).is_err() {
                        return;
                    }
                    buffers[idx] = Some((res11, v.unwrap()));
                }
                let (res, view) = buffers[idx].clone().unwrap();
                on12.AcquireWrappedResources(&[Some(res.clone())]);
                d.upload(seq, &sprites);
                d.draw(&view, w, h, &sprites);
                on12.ReleaseWrappedResources(&[Some(res)]);
                d.ctx.Flush();
            }
        }
    }
}

unsafe fn make(sc: &IDXGISwapChain, is12: bool) -> Result<Backend> {
    unsafe {
        if !is12 {
            let device: ID3D11Device = sc.GetDevice()?;
            let ctx = device.GetImmediateContext()?;
            crate::log("drawing with D3D11");
            return Ok(Backend::D11 { d: D11::new(device, ctx, true)?, rtv: None });
        }
        let d12: ID3D12Device = sc.GetDevice()?;
        let q = crate::QUEUE.load(Ordering::Acquire);
        if q.is_null() {
            return Err(windows::core::Error::new(windows::Win32::Foundation::E_FAIL, "waiting for the game's command queue"));
        }
        let queue = IUnknown::from_raw_borrowed(&q).cloned();
        let mut dev = None;
        let mut ctx = None;
        D3D11On12CreateDevice(&d12, D3D11_CREATE_DEVICE_BGRA_SUPPORT.0 as u32, None, Some(&[queue]), 0, Some(&mut dev), Some(&mut ctx), None)?;
        let dev: ID3D11Device = dev.unwrap();
        let on12: ID3D11On12Device = dev.cast()?;
        crate::log("drawing with D3D12 (D3D11On12)");
        Ok(Backend::D12 { d: D11::new(dev, ctx.unwrap(), false)?, on12, buffers: vec![] })
    }
}

/// Lets go of our views of the swap chain's buffers (before ResizeBuffers).
pub unsafe fn release_buffers(sc: *mut c_void) {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = guard.as_mut() {
        if r.swapchain == sc as usize {
            match &mut r.backend {
                // The game's own context: only drop our view, never touch its state.
                Backend::D11 { rtv, .. } => *rtv = None,
                Backend::D12 { buffers, d, .. } => {
                    buffers.clear();
                    unsafe { d.ctx.Flush() };
                }
            }
        }
    }
}
