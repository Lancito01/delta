//! Delta's in-game hook. Loaded into a game by Delta, it detours DXGI's Present so that, right
//! before each frame is shown, it draws the overlay pixels Delta prepared in shared memory.
//!
//! - D3D11 games: drawn with the game's own device, inside a separate device-context state so the
//!   game's pipeline state is untouched.
//! - D3D12 games: drawn through D3D11On12 on the game's direct command queue.
//!
//! The hook never decides anything on its own: it only draws while Delta is running and has named
//! this process as the target. If Delta goes away, it goes idle.

#![allow(clippy::missing_safety_doc)]

mod render;
mod shm;

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use windows::Win32::Foundation::{HINSTANCE, HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::{DisableThreadLibraryCalls, GetModuleHandleW};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HRESULT, Interface, w};

type PresentFn = unsafe extern "system" fn(*mut c_void, u32, u32) -> HRESULT;
type Present1Fn = unsafe extern "system" fn(*mut c_void, u32, u32, *const c_void) -> HRESULT;
type ResizeFn = unsafe extern "system" fn(*mut c_void, u32, u32, u32, DXGI_FORMAT, u32) -> HRESULT;
type ExecFn = unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void);

static ORIG_PRESENT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ORIG_PRESENT1: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ORIG_RESIZE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ORIG_EXEC: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
/// The game's direct command queue (D3D12), captured from ExecuteCommandLists. AddRef'd.
pub(crate) static QUEUE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static STARTED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static IN_HOOK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn log(msg: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("delta_hook.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{}] {msg}", std::process::id());
    }
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        unsafe {
            let _ = DisableThreadLibraryCalls(HMODULE(module.0));
        }
        // Nothing heavy may run inside DllMain; set up on a thread of our own.
        if !STARTED.swap(true, Ordering::SeqCst) {
            std::thread::spawn(|| {
                if let Err(e) = unsafe { install() } {
                    log(&format!("install failed: {e}"));
                }
            });
        }
    }
    true.into()
}

unsafe fn vtable(obj: *mut c_void, index: usize) -> *mut c_void {
    unsafe { *(*(obj as *const *const *mut c_void)).add(index) }
}

/// Finds Present/ResizeBuffers (and ExecuteCommandLists for D3D12) through throwaway objects, then
/// detours them. Those functions are shared by every swap chain in the process.
unsafe fn install() -> Result<(), String> {
    unsafe {
        shm::open();
        let hwnd = CreateWindowExW(Default::default(), w!("STATIC"), w!("delta-hook-dummy"), WS_OVERLAPPED, 0, 0, 64, 64, None, None, None, None).map_err(|e| e.to_string())?;
        let desc = DXGI_SWAP_CHAIN_DESC {
            BufferDesc: DXGI_MODE_DESC { Width: 64, Height: 64, Format: DXGI_FORMAT_B8G8R8A8_UNORM, ..Default::default() },
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            OutputWindow: hwnd,
            Windowed: true.into(),
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ..Default::default()
        };
        let mut sc = None;
        let mut dev = None;
        D3D11CreateDeviceAndSwapChain(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&desc), Some(&mut sc), Some(&mut dev), None, None)
            .map_err(|e| format!("dummy device: {e}"))?;
        let sc: IDXGISwapChain = sc.ok_or("no dummy swap chain")?;
        let raw = sc.as_raw();
        let present = vtable(raw, 8);
        let resize = vtable(raw, 13);
        let present1 = sc.cast::<IDXGISwapChain1>().ok().map(|s1| vtable(s1.as_raw(), 22));

        // D3D12: the queue that presents is needed to draw; catch it as the game submits work.
        let mut exec = None;
        if GetModuleHandleW(w!("d3d12.dll")).is_ok() {
            let mut d12: Option<ID3D12Device> = None;
            if D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut d12).is_ok() {
                if let Some(d12) = d12 {
                    let qd = D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() };
                    if let Ok(q) = d12.CreateCommandQueue::<ID3D12CommandQueue>(&qd) {
                        exec = Some(vtable(q.as_raw(), 10));
                    }
                }
            }
        }
        drop(sc);
        drop(dev);
        let _ = DestroyWindow(hwnd);

        let hook = |target: *mut c_void, detour: *mut c_void, slot: &AtomicPtr<c_void>, name: &str| match minhook::MinHook::create_hook(target, detour) {
            Ok(tramp) => slot.store(tramp, Ordering::SeqCst),
            Err(e) => log(&format!("hook {name} failed: {e:?}")),
        };
        hook(present, present_detour as *mut c_void, &ORIG_PRESENT, "Present");
        hook(resize, resize_detour as *mut c_void, &ORIG_RESIZE, "ResizeBuffers");
        if let Some(p1) = present1 {
            hook(p1, present1_detour as *mut c_void, &ORIG_PRESENT1, "Present1");
        }
        if let Some(e) = exec {
            hook(e, exec_detour as *mut c_void, &ORIG_EXEC, "ExecuteCommandLists");
        }
        minhook::MinHook::enable_all_hooks().map_err(|e| format!("enable hooks: {e:?}"))?;
        log("hooks installed");
        Ok(())
    }
}

/// Runs the overlay drawing for this swap chain, guarding against re-entry (other overlays, or
/// D3D11On12 presenting internally).
unsafe fn on_present(this: *mut c_void, flags: u32) {
    if flags & DXGI_PRESENT_TEST.0 != 0 {
        return;
    }
    if IN_HOOK.with(|f| f.replace(true)) {
        return;
    }
    let r = std::panic::catch_unwind(|| unsafe {
        if let Some(sc) = IDXGISwapChain::from_raw_borrowed(&this) {
            render::frame(sc);
        }
    });
    if r.is_err() {
        log("panic while drawing; overlay disabled for this frame");
    }
    IN_HOOK.with(|f| f.set(false));
}

unsafe extern "system" fn present_detour(this: *mut c_void, sync: u32, flags: u32) -> HRESULT {
    unsafe {
        on_present(this, flags);
        let orig: PresentFn = std::mem::transmute(ORIG_PRESENT.load(Ordering::Relaxed));
        orig(this, sync, flags)
    }
}

unsafe extern "system" fn present1_detour(this: *mut c_void, sync: u32, flags: u32, params: *const c_void) -> HRESULT {
    unsafe {
        on_present(this, flags);
        let orig: Present1Fn = std::mem::transmute(ORIG_PRESENT1.load(Ordering::Relaxed));
        orig(this, sync, flags, params)
    }
}

unsafe extern "system" fn resize_detour(this: *mut c_void, count: u32, w: u32, h: u32, fmt: DXGI_FORMAT, flags: u32) -> HRESULT {
    unsafe {
        // Our views of the old buffers must go before the swap chain can resize them.
        render::release_buffers(this);
        let orig: ResizeFn = std::mem::transmute(ORIG_RESIZE.load(Ordering::Relaxed));
        orig(this, count, w, h, fmt, flags)
    }
}

unsafe extern "system" fn exec_detour(this: *mut c_void, n: u32, lists: *const *mut c_void) {
    unsafe {
        if QUEUE.load(Ordering::Relaxed).is_null() {
            if let Some(q) = ID3D12CommandQueue::from_raw_borrowed(&this) {
                if q.GetDesc().Type == D3D12_COMMAND_LIST_TYPE_DIRECT {
                    let owned = q.clone();
                    let raw = owned.into_raw();
                    if QUEUE.compare_exchange(std::ptr::null_mut(), raw, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                        drop(ID3D12CommandQueue::from_raw(raw));
                    }
                }
            }
        }
        let orig: ExecFn = std::mem::transmute(ORIG_EXEC.load(Ordering::Relaxed));
        orig(this, n, lists)
    }
}

#[allow(dead_code)]
fn _types(_: HWND) {}
