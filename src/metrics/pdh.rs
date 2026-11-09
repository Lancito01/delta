//! Windows performance counters (PDH): CPU per-core usage and clocks, disk, network, and a
//! vendor-neutral GPU fallback. English counter names are used so it works on any Windows language.

use windows::Win32::System::Performance::*;
use windows::core::{HSTRING, PCWSTR};

const PDH_MORE_DATA: u32 = 0x800007D2;
/// Don't cap percentages at 100 (we clamp ourselves where it matters).
const FMT: PDH_FMT = PDH_FMT(PDH_FMT_DOUBLE.0 | 0x0000_8000);

pub struct Counter {
    handle: PDH_HCOUNTER,
}

pub struct Query {
    query: PDH_HQUERY,
}

// PDH handles are used from the one sampler thread only.
unsafe impl Send for Query {}
unsafe impl Send for Counter {}

impl Query {
    pub fn new() -> Option<Query> {
        let mut q = PDH_HQUERY::default();
        let r = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut q) };
        (r == 0).then_some(Query { query: q })
    }

    pub fn add(&self, path: &str) -> Option<Counter> {
        let mut c = PDH_HCOUNTER::default();
        let r = unsafe { PdhAddEnglishCounterW(self.query, &HSTRING::from(path), 0, &mut c) };
        (r == 0).then_some(Counter { handle: c })
    }

    pub fn collect(&self) {
        unsafe {
            PdhCollectQueryData(self.query);
        }
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

impl Counter {
    pub fn value(&self) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        let r = unsafe { PdhGetFormattedCounterValue(self.handle, FMT, None, &mut v) };
        if r != 0 || (v.CStatus != 0 && v.CStatus != 1) {
            return None;
        }
        Some(unsafe { v.Anonymous.doubleValue })
    }

    /// All instances of a wildcard counter as (instance name, value).
    pub fn array(&self) -> Vec<(String, f64)> {
        unsafe {
            let mut size = 0u32;
            let mut count = 0u32;
            let r = PdhGetFormattedCounterArrayW(self.handle, FMT, &mut size, &mut count, None);
            if r != PDH_MORE_DATA || size == 0 {
                return vec![];
            }
            // u64-aligned buffer; the strings live after the items.
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            let r = PdhGetFormattedCounterArrayW(self.handle, FMT, &mut size, &mut count, Some(items));
            if r != 0 {
                return vec![];
            }
            let items = std::slice::from_raw_parts(items, count as usize);
            items
                .iter()
                .filter(|i| i.FmtValue.CStatus == 0 || i.FmtValue.CStatus == 1)
                .map(|i| (i.szName.to_string().unwrap_or_default(), i.FmtValue.Anonymous.doubleValue))
                .collect()
        }
    }
}

/// Everything the sampler reads through PDH.
pub struct Counters {
    query: Query,
    cpu_util: Option<Counter>,
    cpu_perf: Option<Counter>,
    cpu_freq: Option<Counter>,
    disk_read: Option<Counter>,
    disk_write: Option<Counter>,
    disk_idle: Option<Counter>,
    net_down: Option<Counter>,
    net_up: Option<Counter>,
    gpu_engine: Option<Counter>,
    gpu_dedicated: Option<Counter>,
}

#[derive(Default)]
pub struct Readings {
    pub cpu_total: Option<f64>,
    pub cores: Vec<f64>,
    pub core_clocks: Vec<f64>,
    pub cpu_clock: Option<f64>,
    pub disk_read: Option<f64>,
    pub disk_write: Option<f64>,
    pub disk_active: Option<f64>,
    pub net_down: Option<f64>,
    pub net_up: Option<f64>,
    pub gpu_usage: Option<f64>,
    pub gpu_dedicated: Option<f64>,
}

impl Counters {
    /// `gpu_fallback`: also read GPU usage/VRAM through PDH (used when no vendor library is found;
    /// the per-process GPU counters are comparatively expensive, so they're skipped otherwise).
    pub fn new(gpu_fallback: bool) -> Option<Counters> {
        let query = Query::new()?;
        let c = Counters {
            cpu_util: query.add(r"\Processor Information(*)\% Processor Utility"),
            cpu_perf: query.add(r"\Processor Information(*)\% Processor Performance"),
            cpu_freq: query.add(r"\Processor Information(_Total)\Processor Frequency"),
            disk_read: query.add(r"\PhysicalDisk(_Total)\Disk Read Bytes/sec"),
            disk_write: query.add(r"\PhysicalDisk(_Total)\Disk Write Bytes/sec"),
            disk_idle: query.add(r"\PhysicalDisk(_Total)\% Idle Time"),
            net_down: query.add(r"\Network Interface(*)\Bytes Received/sec"),
            net_up: query.add(r"\Network Interface(*)\Bytes Sent/sec"),
            gpu_engine: if gpu_fallback { query.add(r"\GPU Engine(*engtype_3D)\Utilization Percentage") } else { None },
            gpu_dedicated: if gpu_fallback { query.add(r"\GPU Adapter Memory(*)\Dedicated Usage") } else { None },
            query,
        };
        c.query.collect(); // rate counters need two samples
        Some(c)
    }

    pub fn read(&self) -> Readings {
        self.query.collect();
        let mut r = Readings::default();

        if let Some(c) = &self.cpu_util {
            let all = c.array();
            r.cpu_total = all.iter().find(|(n, _)| n == "_Total").map(|(_, v)| v.clamp(0.0, 100.0));
            r.cores = per_core(&all).into_iter().map(|v| v.clamp(0.0, 100.0)).collect();
        }
        let base = self.cpu_freq.as_ref().and_then(Counter::value);
        if let (Some(c), Some(base)) = (&self.cpu_perf, base) {
            let all = c.array();
            r.cpu_clock = all.iter().find(|(n, _)| n == "_Total").map(|(_, p)| base * p / 100.0);
            r.core_clocks = per_core(&all).into_iter().map(|p| base * p / 100.0).collect();
        }
        r.disk_read = self.disk_read.as_ref().and_then(Counter::value);
        r.disk_write = self.disk_write.as_ref().and_then(Counter::value);
        r.disk_active = self.disk_idle.as_ref().and_then(Counter::value).map(|idle| (100.0 - idle).clamp(0.0, 100.0));
        r.net_down = self.net_down.as_ref().map(|c| c.array().iter().map(|(_, v)| v).sum());
        r.net_up = self.net_up.as_ref().map(|c| c.array().iter().map(|(_, v)| v).sum());

        if let Some(c) = &self.gpu_engine {
            // Sum the 3D engine time of every process per adapter; report the busiest adapter.
            let mut per_adapter: Vec<(String, f64)> = vec![];
            for (name, v) in c.array() {
                let luid = name.split("_phys").next().and_then(|s| s.split("luid_").nth(1)).unwrap_or("").to_string();
                match per_adapter.iter_mut().find(|(l, _)| *l == luid) {
                    Some(e) => e.1 += v,
                    None => per_adapter.push((luid, v)),
                }
            }
            r.gpu_usage = per_adapter.iter().map(|(_, v)| v.min(100.0)).reduce(f64::max);
        }
        if let Some(c) = &self.gpu_dedicated {
            r.gpu_dedicated = c.array().iter().map(|(_, v)| *v).reduce(f64::max);
        }
        r
    }
}

/// Per logical processor values from a `Processor Information(*)` array ("group,index" instances),
/// in processor order. Skips the `_Total` aggregates.
fn per_core(all: &[(String, f64)]) -> Vec<f64> {
    let mut cores: Vec<((u32, u32), f64)> = all
        .iter()
        .filter_map(|(n, v)| {
            let (g, i) = n.split_once(',')?;
            Some(((g.parse().ok()?, i.parse().ok()?), *v))
        })
        .collect();
    cores.sort_by_key(|(k, _)| *k);
    cores.into_iter().map(|(_, v)| v).collect()
}
