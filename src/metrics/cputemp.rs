//! CPU temperature, read from the processor's own sensor through the PawnIO driver.
//!
//! The sensor sits behind registers only kernel code can read (model-specific registers on Intel,
//! the SMN bus on AMD). PawnIO runs small signed modules that expose exactly such reads; Delta
//! embeds the two official ones it needs, unmodified, and only ever calls their read functions.
//!
//! The driver is opened for each reading and closed right after (about once a second), so Delta
//! never keeps it busy: it can be paused or removed at any time. If the driver is missing, paused
//! or blocked, the only effect is that `cpu.temp` has no value; see `crate::pawnio`.

use crate::pawnio;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::w;

/// PawnIO.Modules 0.2.11, as released and signed by PawnIO's author (LGPL-2.1, see assets/pawnio).
const INTEL_MSR: &[u8] = include_bytes!("../../assets/pawnio/IntelMSR.bin");
const AMD_FAMILY_17: &[u8] = include_bytes!("../../assets/pawnio/AMDFamily17.bin");

// CTL_CODE(0xA1B2, function, METHOD_BUFFERED, FILE_ANY_ACCESS)
const IOCTL_LOAD_BINARY: u32 = (0xA1B2 << 16) | (0x821 << 2);
const IOCTL_EXECUTE_FN: u32 = (0xA1B2 << 16) | (0x841 << 2);

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    /// Not looked yet.
    #[default]
    Unknown,
    NotInstalled,
    /// Installed but switched off by the user.
    Paused,
    /// The driver only answers administrators, and Delta isn't one.
    NeedsAdmin,
    /// Installed and switched on, but it can't be reached (the reason, for the log and settings).
    Blocked(String),
    /// A processor Delta has no reading for.
    Unsupported,
    Working,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cpu {
    Intel,
    /// AMD family 17h and later (Ryzen, Threadripper, EPYC).
    Zen,
    Other,
}

fn cpu() -> Cpu {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::__cpuid;
        #[allow(unused_unsafe)]
        let (v, f) = unsafe { (__cpuid(0), __cpuid(1)) };
        let vendor: Vec<u8> = [v.ebx, v.edx, v.ecx].iter().flat_map(|r| r.to_le_bytes()).collect();
        let base = (f.eax >> 8) & 0xF;
        let family = if base == 0xF { base + ((f.eax >> 20) & 0xFF) } else { base };
        match vendor.as_slice() {
            b"GenuineIntel" => Cpu::Intel,
            b"AuthenticAMD" if family >= 0x17 => Cpu::Zen,
            _ => Cpu::Other,
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    Cpu::Other
}

// ---------------------------------------------------------------- the readings

const IA32_THERM_STATUS: u64 = 0x19C;
const MSR_TEMPERATURE_TARGET: u64 = 0x1A2;
const IA32_PACKAGE_THERM_STATUS: u64 = 0x1B1;
const ZEN_REPORTED_TEMP_CTRL: u64 = 0x0005_9800;

/// Believable temperatures; anything else is a misread, and no value beats a wrong one.
fn plausible(t: f64) -> Option<f64> {
    (t > 0.0 && t <= 125.0).then_some(t)
}

/// Intel reports how far below the throttling point (TjMax) the processor is.
/// `target` is MSR 0x1A2 (TjMax in bits 23:16), `status` a thermal status MSR (0x1B1 or 0x19C).
fn intel_temp(target: u64, status: u64) -> Option<f64> {
    if status & (1 << 31) == 0 {
        return None; // "reading valid" isn't set
    }
    let tjmax = match (target >> 16) & 0xFF {
        0 => 100, // not reported (some older and low-power chips): the usual value
        t => t,
    };
    let below = (status >> 16) & 0x7F;
    plausible(tjmax as f64 - below as f64)
}

/// AMD Zen: the control temperature register, as Linux's k10temp reads it.
fn zen_temp(raw: u64, cpu_name: &str) -> Option<f64> {
    let raw = raw as u32;
    let mut t = (raw >> 21) as f64 * 0.125;
    // Some models report on a scale shifted up by 49 degrees, flagged in the register.
    if raw & 0x8_0000 != 0 || raw & 0x3_0000 == 0x3_0000 {
        t -= 49.0;
    }
    // And a few report a fixed amount above the real temperature, to steer their fans.
    let n = cpu_name;
    if ["1600X", "1700X", "1800X"].iter().any(|m| n.contains(m)) {
        t -= 20.0;
    } else if n.contains("2700X") {
        t -= 10.0;
    } else if n.contains("Threadripper 19") || n.contains("Threadripper 29") {
        t -= 27.0;
    }
    plausible(t)
}

// ---------------------------------------------------------------- talking to the driver

struct Driver(HANDLE);

impl Drop for Driver {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

impl Driver {
    fn open() -> windows::core::Result<Driver> {
        unsafe {
            CreateFileW(
                w!("\\\\?\\GLOBALROOT\\Device\\PawnIO"),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
            .map(Driver)
        }
    }

    /// Hands the driver a signed module. It checks the signature itself and refuses anything else.
    fn load(&self, module: &[u8]) -> windows::core::Result<()> {
        unsafe { DeviceIoControl(self.0, IOCTL_LOAD_BINARY, Some(module.as_ptr() as *const _), module.len() as u32, None, 0, None, None) }
    }

    /// Calls a module function that takes one number and returns one.
    fn call(&self, name: &str, arg: u64) -> Option<u64> {
        // 32 bytes of zero-padded name, then the arguments.
        let mut input = [0u8; 40];
        input[..name.len()].copy_from_slice(name.as_bytes());
        input[32..].copy_from_slice(&arg.to_le_bytes());
        let mut out = 0u64;
        let mut got = 0u32;
        unsafe {
            DeviceIoControl(self.0, IOCTL_EXECUTE_FN, Some(input.as_ptr() as *const _), input.len() as u32, Some(&mut out as *mut u64 as *mut _), 8, Some(&mut got), None).ok()?;
        }
        (got == 8).then_some(out)
    }
}

/// Other sensor tools serialize their access to the PCI bus with this mutex; so do we.
fn with_pci_lock<T>(f: impl FnOnce() -> T) -> T {
    use windows::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
    unsafe {
        let m = CreateMutexW(None, false, w!("Global\\Access_PCI")).ok();
        let got = m.is_some_and(|m| matches!(WaitForSingleObject(m, 100), WAIT_OBJECT_0 | WAIT_ABANDONED));
        let r = f();
        if let Some(m) = m {
            if got {
                let _ = ReleaseMutex(m);
            }
            let _ = CloseHandle(m);
        }
        r
    }
}

fn read_through(d: &Driver, cpu: Cpu, cpu_name: &str) -> Option<f64> {
    match cpu {
        Cpu::Intel => {
            let msr = |n| d.call("ioctl_read_msr", n);
            let target = msr(MSR_TEMPERATURE_TARGET).unwrap_or(0);
            // The whole package if the chip reports it, else the core this thread is on.
            msr(IA32_PACKAGE_THERM_STATUS).and_then(|s| intel_temp(target, s)).or_else(|| msr(IA32_THERM_STATUS).and_then(|s| intel_temp(target, s)))
        }
        Cpu::Zen => with_pci_lock(|| d.call("ioctl_read_smn", ZEN_REPORTED_TEMP_CTRL)).and_then(|raw| zen_temp(raw, cpu_name)),
        Cpu::Other => None,
    }
}

// ---------------------------------------------------------------- the reader

pub struct Reader {
    cpu: Cpu,
    cpu_name: String,
    elevated: bool,
    status: Status,
    value: Option<f64>,
    next: Instant,
    epoch: u32,
}

impl Reader {
    pub fn new(cpu_name: &str) -> Reader {
        Reader { cpu: cpu(), cpu_name: cpu_name.to_string(), elevated: super::system::is_elevated(), status: Status::Unknown, value: None, next: Instant::now(), epoch: pawnio::epoch() }
    }

    /// The temperature in °C, and why there isn't one when there isn't.
    pub fn read(&mut self) -> (Option<f64>, Status) {
        if self.cpu == Cpu::Other {
            return (None, Status::Unsupported);
        }
        if pawnio::held() {
            // The driver is being installed, paused or removed: stay out of the way.
            return (None, self.status.clone());
        }
        let epoch = pawnio::epoch();
        if epoch != self.epoch {
            self.epoch = epoch;
            self.next = Instant::now(); // something changed: look now
        }
        if Instant::now() < self.next {
            return (self.value, self.status.clone());
        }
        let (value, status) = self.sample();
        if status != self.status {
            crate::log!("cpu temperature: {status:?}");
        }
        // A temperature moves slowly; once a second is plenty. And don't nag a driver that isn't there.
        self.next = Instant::now() + Duration::from_millis(if status == Status::Working { 900 } else { 3000 });
        self.value = value;
        self.status = status.clone();
        (value, status)
    }

    fn sample(&self) -> (Option<f64>, Status) {
        let _busy = pawnio::GATE.lock();
        if pawnio::held() {
            return (None, self.status.clone());
        }
        let driver = match Driver::open() {
            Ok(d) => d,
            Err(e) => return (None, self.why_not(&e)),
        };
        let module = if self.cpu == Cpu::Intel { INTEL_MSR } else { AMD_FAMILY_17 };
        if let Err(e) = driver.load(module) {
            return (None, Status::Blocked(format!("it refused Delta's sensor module ({})", e.message())));
        }
        match read_through(&driver, self.cpu, &self.cpu_name) {
            Some(t) => (Some(t), Status::Working),
            None => (None, Status::Blocked("the processor gave no valid reading".into())),
        }
    }

    /// Why the driver couldn't be opened.
    fn why_not(&self, e: &windows::core::Error) -> Status {
        if !pawnio::installed() {
            return Status::NotInstalled;
        }
        match pawnio::device() {
            pawnio::Device::Disabled => Status::Paused,
            _ if !self.elevated => Status::NeedsAdmin,
            _ if e.code() == ERROR_ACCESS_DENIED.to_hresult() => Status::Blocked("Windows denied access to it".into()),
            pawnio::Device::Missing => Status::Blocked("its device is missing".into()),
            pawnio::Device::Problem(0) | pawnio::Device::Started => Status::Blocked("it isn't running".into()),
            pawnio::Device::Problem(n) => Status::Blocked(format!("Windows stopped it (Device Manager code {n})")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_codes() {
        // The values PawnIO's own library (PawnIOLib) uses.
        assert_eq!(IOCTL_LOAD_BINARY, 0xA1B2_2084);
        assert_eq!(IOCTL_EXECUTE_FN, 0xA1B2_2104);
    }

    #[test]
    fn modules_are_the_signed_releases() {
        for (blob, sha) in [
            (INTEL_MSR, "d6ed85d65ab17a22f813ef98207d6d537155ee2ded5976a21cb48413c9b92e5f"),
            (AMD_FAMILY_17, "dae74615761b78bdf064dfb3e136252ddcc6fc727d88f14738d0e5800d427a91"),
        ] {
            assert_eq!(crate::update::sha256_hex(blob), sha);
            // [signature length][signature][program]
            assert_eq!(u32::from_le_bytes(blob[..4].try_into().unwrap()), 512);
        }
    }

    #[test]
    fn intel() {
        let valid = 1u64 << 31;
        // TjMax 100, 42 below it.
        assert_eq!(intel_temp(100 << 16, valid | (42 << 16)), Some(58.0));
        // TjMax 115 (11900K-style), other status bits set.
        assert_eq!(intel_temp((115 << 16) | 0xFF00_00FF, valid | (30 << 16) | 0x8FF), Some(85.0));
        // TjMax not reported: 100 is assumed.
        assert_eq!(intel_temp(0, valid | (20 << 16)), Some(80.0));
        // No valid reading.
        assert_eq!(intel_temp(100 << 16, 42 << 16), None);
        // Nonsense (at or below zero) is dropped.
        assert_eq!(intel_temp(90 << 16, valid | (127 << 16)), None);
    }

    #[test]
    fn zen() {
        let raw = |t: f64| ((t / 0.125) as u64) << 21;
        assert_eq!(zen_temp(raw(61.5), "AMD Ryzen 7 5800X3D 8-Core Processor"), Some(61.5));
        // Shifted scale.
        assert_eq!(zen_temp(raw(110.0) | 0x8_0000, "AMD Ryzen 9 7950X 16-Core Processor"), Some(61.0));
        assert_eq!(zen_temp(raw(110.0) | 0x3_0000, "AMD Ryzen 9 7950X 16-Core Processor"), Some(61.0));
        // Models that report high.
        assert_eq!(zen_temp(raw(80.0), "AMD Ryzen 7 1800X Eight-Core Processor"), Some(60.0));
        assert_eq!(zen_temp(raw(70.0), "AMD Ryzen 7 2700X Eight-Core Processor"), Some(60.0));
        assert_eq!(zen_temp(raw(87.0), "AMD Ryzen Threadripper 2950X 16-Core Processor"), Some(60.0));
        assert_eq!(zen_temp(0, "AMD Ryzen 5 5600X 6-Core Processor"), None);
    }

    #[test]
    fn knows_this_processor() {
        // Whatever this machine is, detection mustn't panic; x64 PCs are Intel or AMD.
        println!("{:?}", cpu());
    }
}
