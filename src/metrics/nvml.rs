//! NVIDIA GPUs through NVML, loaded at runtime from the driver (no SDK, nothing to ship).

use std::ffi::c_void;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{PCSTR, w};

type Device = *mut c_void;
type FnInit = unsafe extern "C" fn() -> i32;
type FnHandle = unsafe extern "C" fn(u32, *mut Device) -> i32;
type FnU32 = unsafe extern "C" fn(Device, *mut u32) -> i32;
type FnU32Arg = unsafe extern "C" fn(Device, u32, *mut u32) -> i32;
type FnUtil = unsafe extern "C" fn(Device, *mut Utilization) -> i32;
type FnMem = unsafe extern "C" fn(Device, *mut Memory) -> i32;
type FnName = unsafe extern "C" fn(Device, *mut u8, u32) -> i32;

#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: u32,
    memory: u32,
}

#[repr(C)]
#[derive(Default)]
struct Memory {
    total: u64,
    free: u64,
    used: u64,
}

pub struct Nvml {
    device: Device,
    utilization: FnUtil,
    temperature: FnU32Arg,
    clock: FnU32Arg,
    memory: FnMem,
    power: FnU32,
    fan: FnU32,
    pub name: String,
}

unsafe impl Send for Nvml {}

#[derive(Default)]
pub struct GpuReading {
    pub usage: Option<f64>,
    pub temp: Option<f64>,
    pub clock: Option<f64>,
    pub mem_clock: Option<f64>,
    pub vram_used_mb: Option<f64>,
    pub vram_total_mb: Option<f64>,
    pub power_w: Option<f64>,
    pub fan: Option<f64>,
}

unsafe fn sym<T>(lib: HMODULE, name: &[u8]) -> Option<T> {
    let p = unsafe { GetProcAddress(lib, PCSTR(name.as_ptr())) }?;
    Some(unsafe { std::mem::transmute_copy(&p) })
}

impl Nvml {
    pub fn load(index: u32) -> Option<Nvml> {
        unsafe {
            let lib = LoadLibraryW(w!("nvml.dll"))
                .or_else(|_| LoadLibraryW(w!("C:\\Program Files\\NVIDIA Corporation\\NVSMI\\nvml.dll")))
                .ok()?;
            let init: FnInit = sym(lib, b"nvmlInit_v2\0")?;
            if init() != 0 {
                return None;
            }
            let by_index: FnHandle = sym(lib, b"nvmlDeviceGetHandleByIndex_v2\0")?;
            let mut device: Device = std::ptr::null_mut();
            if by_index(index, &mut device) != 0 {
                return None;
            }
            let get_name: FnName = sym(lib, b"nvmlDeviceGetName\0")?;
            let mut buf = [0u8; 96];
            let name = if get_name(device, buf.as_mut_ptr(), buf.len() as u32) == 0 {
                let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                String::from_utf8_lossy(&buf[..end]).into_owned()
            } else {
                "NVIDIA GPU".into()
            };
            Some(Nvml {
                device,
                utilization: sym(lib, b"nvmlDeviceGetUtilizationRates\0")?,
                temperature: sym(lib, b"nvmlDeviceGetTemperature\0")?,
                clock: sym(lib, b"nvmlDeviceGetClockInfo\0")?,
                memory: sym(lib, b"nvmlDeviceGetMemoryInfo\0")?,
                power: sym(lib, b"nvmlDeviceGetPowerUsage\0")?,
                fan: sym(lib, b"nvmlDeviceGetFanSpeed\0")?,
                name,
            })
        }
    }

    pub fn read(&self) -> GpuReading {
        let mut r = GpuReading::default();
        unsafe {
            let mut u = Utilization::default();
            if (self.utilization)(self.device, &mut u) == 0 {
                r.usage = Some(u.gpu as f64);
            }
            let mut v = 0u32;
            if (self.temperature)(self.device, 0, &mut v) == 0 {
                r.temp = Some(v as f64);
            }
            if (self.clock)(self.device, 0, &mut v) == 0 {
                r.clock = Some(v as f64);
            }
            if (self.clock)(self.device, 2, &mut v) == 0 {
                r.mem_clock = Some(v as f64);
            }
            let mut m = Memory::default();
            if (self.memory)(self.device, &mut m) == 0 {
                r.vram_used_mb = Some(m.used as f64 / 1048576.0);
                r.vram_total_mb = Some(m.total as f64 / 1048576.0);
            }
            if (self.power)(self.device, &mut v) == 0 {
                r.power_w = Some(v as f64 / 1000.0);
            }
            if (self.fan)(self.device, &mut v) == 0 {
                r.fan = Some(v as f64);
            }
        }
        r
    }
}
