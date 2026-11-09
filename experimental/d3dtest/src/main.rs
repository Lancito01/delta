//! A tiny window that renders a moving color with D3D11 or D3D12, for testing Delta's in-game hook.
//!
//!   d3dtest [d3d11|d3d12] [--load <path to delta_hook.dll>]
//!
//! `--load` loads the hook into this process directly (instead of Delta injecting it).

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Interface, Result, w};

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if m == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(h, m, wp, lp) }
}

fn pump() -> bool {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                return false;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    true
}

fn color(t: f32) -> [f32; 4] {
    [0.15 + 0.1 * (t * 0.7).sin(), 0.25 + 0.1 * (t * 0.5).cos(), 0.45 + 0.15 * (t * 0.3).sin(), 1.0]
}

fn desc(hwnd: HWND) -> DXGI_SWAP_CHAIN_DESC1 {
    let _ = hwnd;
    DXGI_SWAP_CHAIN_DESC1 {
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        ..Default::default()
    }
}

fn run11(hwnd: HWND, hook: Option<String>) -> Result<()> {
    unsafe {
        let mut dev = None;
        let mut ctx = None;
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx))?;
        let (dev, ctx): (ID3D11Device, ID3D11DeviceContext) = (dev.unwrap(), ctx.unwrap());
        let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
        let sc = factory.CreateSwapChainForHwnd(&dev, hwnd, &desc(hwnd), None, None)?;
        load(hook);
        let start = std::time::Instant::now();
        while pump() {
            let buf: ID3D11Texture2D = sc.GetBuffer(0)?;
            let mut rtv = None;
            dev.CreateRenderTargetView(&buf, None, Some(&mut rtv))?;
            ctx.ClearRenderTargetView(rtv.as_ref().unwrap(), &color(start.elapsed().as_secs_f32()));
            sc.Present(1, DXGI_PRESENT(0)).ok()?;
        }
    }
    Ok(())
}

fn run12(hwnd: HWND, hook: Option<String>) -> Result<()> {
    unsafe {
        let mut dev: Option<ID3D12Device> = None;
        D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut dev)?;
        let dev = dev.unwrap();
        let queue: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() })?;
        let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
        let sc: IDXGISwapChain3 = factory.CreateSwapChainForHwnd(&queue, hwnd, &desc(hwnd), None, None)?.cast()?;
        let heap: ID3D12DescriptorHeap = dev.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV, NumDescriptors: 2, ..Default::default() })?;
        let step = dev.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) as usize;
        let alloc: ID3D12CommandAllocator = dev.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
        let list: ID3D12GraphicsCommandList = dev.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None)?;
        list.Close()?;
        let fence: ID3D12Fence = dev.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
        let event = CreateEventW(None, false, false, None)?;
        let mut fv = 0u64;
        let start = std::time::Instant::now();
        let mut hooked = hook;
        let mut frames = 0;
        while pump() {
            let i = sc.GetCurrentBackBufferIndex();
            let buf: ID3D12Resource = sc.GetBuffer(i)?;
            let rtv = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: heap.GetCPUDescriptorHandleForHeapStart().ptr + i as usize * step };
            dev.CreateRenderTargetView(&buf, None, rtv);
            alloc.Reset()?;
            list.Reset(&alloc, None)?;
            let barrier = |before, after| D3D12_RESOURCE_BARRIER {
                Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
                Anonymous: D3D12_RESOURCE_BARRIER_0 {
                    Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                        pResource: std::mem::ManuallyDrop::new(Some(buf.clone())),
                        Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                        StateBefore: before,
                        StateAfter: after,
                    }),
                },
                ..Default::default()
            };
            list.ResourceBarrier(&[barrier(D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
            list.ClearRenderTargetView(rtv, &color(start.elapsed().as_secs_f32()), None);
            list.ResourceBarrier(&[barrier(D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PRESENT)]);
            list.Close()?;
            queue.ExecuteCommandLists(&[Some(list.cast()?)]);
            sc.Present(1, DXGI_PRESENT(0)).ok()?;
            fv += 1;
            queue.Signal(&fence, fv)?;
            if fence.GetCompletedValue() < fv {
                fence.SetEventOnCompletion(fv, event)?;
                WaitForSingleObject(event, INFINITE);
            }
            frames += 1;
            // Load the hook after a few frames, like an injection into a running game would.
            if frames == 30 {
                load(hooked.take());
            }
        }
    }
    Ok(())
}

fn load(path: Option<String>) {
    if let Some(p) = path {
        let r = unsafe { LoadLibraryW(&HSTRING::from(p.as_str())) };
        eprintln!("load {p}: {}", if r.is_ok() { "ok" } else { "failed" });
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let api = args.get(1).cloned().unwrap_or_else(|| "d3d11".into());
    let hook = args.iter().position(|a| a == "--load").and_then(|i| args.get(i + 1)).cloned();
    unsafe {
        let inst = GetModuleHandleW(None)?;
        let class = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst.into(), lpszClassName: w!("d3dtest"), hCursor: LoadCursorW(None, IDC_ARROW)?, ..Default::default() };
        RegisterClassW(&class);
        let title = HSTRING::from(format!("d3dtest ({api})"));
        let hwnd = CreateWindowExW(Default::default(), w!("d3dtest"), &title, WS_OVERLAPPEDWINDOW | WS_VISIBLE, 200, 200, 960, 600, None, None, Some(inst.into()), None)?;
        if api == "d3d12" { run12(hwnd, hook) } else { run11(hwnd, hook) }
    }
}
