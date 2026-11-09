//! Sensor sampling, history and formatting.

pub mod cputemp;
pub mod frames;
pub mod nvml;
pub mod pdh;
pub mod system;

use crate::config::TempUnit;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default, Clone)]
pub struct Sample {
    pub values: HashMap<String, f64>,
    pub cores: Vec<f64>,
    pub core_clocks: Vec<f64>,
    pub history: HashMap<String, VecDeque<f64>>,
    pub app: String,
    pub gpu_name: String,
    pub cpu_name: String,
    pub gpu_source: &'static str,
    /// Whether the CPU temperature can be read, and why not.
    pub cpu_temp: cputemp::Status,
    /// Bumped on every sensor read, so the UI knows when there's something new to draw.
    pub generation: u64,
}

pub struct Sampler {
    pub shared: Arc<Mutex<Sample>>,
    pub poll_ms: Arc<AtomicU32>,
    pub history: Arc<AtomicU32>,
    /// Per-core metrics (`cpu.core.N`, `cpu.core_clock.N`) some row graphs. Only these get a
    /// history; keeping one for every core of every machine would be copied on each redraw.
    pub graphed_cores: Arc<Mutex<Vec<String>>>,
}

impl Sampler {
    pub fn start(poll_ms: u32, history: usize, gpu_index: u32) -> Sampler {
        let shared = Arc::new(Mutex::new(Sample::default()));
        let poll = Arc::new(AtomicU32::new(poll_ms));
        let hist = Arc::new(AtomicU32::new(history as u32));
        let graphed_cores = Arc::new(Mutex::new(Vec::new()));
        let (s, p, h, g) = (shared.clone(), poll.clone(), hist.clone(), graphed_cores.clone());
        std::thread::Builder::new()
            .name("sampler".into())
            .spawn(move || run(s, p, h, g, gpu_index))
            .expect("sampler thread");
        Sampler { shared, poll_ms: poll, history: hist, graphed_cores }
    }
}

fn run(shared: Arc<Mutex<Sample>>, poll: Arc<AtomicU32>, hist: Arc<AtomicU32>, graphed_cores: Arc<Mutex<Vec<String>>>, gpu_index: u32) {
    let nvml = nvml::Nvml::load(gpu_index);
    let counters = pdh::Counters::new(nvml.is_none());
    let cpu_name = system::cpu_name();
    let mut cpu_temp = cputemp::Reader::new(&cpu_name);
    {
        let mut s = shared.lock().unwrap();
        s.cpu_name = cpu_name;
        s.gpu_name = nvml.as_ref().map(|n| n.name.clone()).unwrap_or_else(|| "GPU".into());
        s.gpu_source = if nvml.is_some() { "NVML" } else { "Windows counters" };
    }
    // First PDH rates need an interval; a short one so the overlay fills in quickly.
    std::thread::sleep(Duration::from_millis(250));
    loop {
        let started = Instant::now();
        let mut v: HashMap<String, f64> = HashMap::new();
        let mut put = |k: &str, x: Option<f64>| {
            if let Some(x) = x.filter(|x| x.is_finite()) {
                v.insert(k.to_string(), x);
            }
        };

        let mut cores = vec![];
        let mut core_clocks = vec![];
        if let Some(c) = &counters {
            let r = c.read();
            put("cpu.usage", r.cpu_total);
            put("cpu.clock", r.cpu_clock);
            put("disk.read", r.disk_read);
            put("disk.write", r.disk_write);
            put("disk.active", r.disk_active);
            put("net.down", r.net_down);
            put("net.up", r.net_up);
            if nvml.is_none() {
                put("gpu.usage", r.gpu_usage);
                put("gpu.vram", r.gpu_dedicated.map(|b| b / 1048576.0));
            }
            for (i, c) in r.cores.iter().enumerate() {
                put(&format!("cpu.core.{i}"), Some(*c));
            }
            for (i, c) in r.core_clocks.iter().enumerate() {
                put(&format!("cpu.core_clock.{i}"), Some(*c));
            }
            cores = r.cores;
            core_clocks = r.core_clocks;
        }
        if let Some(n) = &nvml {
            let g = n.read();
            put("gpu.usage", g.usage);
            put("gpu.temp", g.temp);
            put("gpu.clock", g.clock);
            put("gpu.mem_clock", g.mem_clock);
            put("gpu.power", g.power_w);
            put("gpu.fan", g.fan);
            put("gpu.vram", g.vram_used_mb);
            if let (Some(u), Some(t)) = (g.vram_used_mb, g.vram_total_mb) {
                put("gpu.vram_pct", Some(u / t * 100.0));
            }
        }
        if let Some(m) = system::memory() {
            put("ram.used", Some(m.used_gb));
            put("ram.total", Some(m.total_gb));
            put("ram.pct", Some(m.pct));
        }
        let (temp, temp_status) = cpu_temp.read();
        put("cpu.temp", temp);
        let fg = system::foreground();
        put("app.ram", system::process_ram_gb(fg.pid));

        {
            let wanted = graphed_cores.lock().unwrap().clone();
            let mut s = shared.lock().unwrap();
            let cap = hist.load(Ordering::Relaxed) as usize;
            s.history.retain(|k, _| !k.starts_with("cpu.core") || wanted.contains(k));
            for (k, x) in &v {
                if k.starts_with("cpu.core") && !wanted.contains(k) {
                    continue;
                }
                let h = s.history.entry(k.clone()).or_default();
                h.push_back(*x);
                while h.len() > cap {
                    h.pop_front();
                }
            }
            s.values = v;
            s.cores = cores;
            s.core_clocks = core_clocks;
            s.cpu_temp = temp_status;
            s.app = fg.exe.trim_end_matches(".exe").to_string();
            s.generation = s.generation.wrapping_add(1);
        }

        let wait = Duration::from_millis(poll.load(Ordering::Relaxed) as u64).saturating_sub(started.elapsed());
        std::thread::sleep(wait.max(Duration::from_millis(50)));
    }
}

/// Every metric the settings window offers: (id, name, category, description).
pub const CATALOG: &[(&str, &str, &str, &str)] = &[
    ("fps", "Frame rate", "Frames", "Frames per second of the app in front"),
    ("frametime", "Frame time", "Frames", "Milliseconds per frame (a graph of it shows stutter)"),
    ("fps.low1", "1% low", "Frames", "Frame rate of the slowest 1% of frames"),
    ("fps.low01", "0.1% low", "Frames", "Frame rate of the slowest 0.1% of frames"),
    ("gpu.usage", "GPU usage", "Graphics card", "How busy the GPU is"),
    ("gpu.temp", "GPU temperature", "Graphics card", "Core temperature"),
    ("gpu.clock", "GPU clock", "Graphics card", "Core clock speed"),
    ("gpu.mem_clock", "Memory clock", "Graphics card", "Video memory clock speed"),
    ("gpu.power", "GPU power", "Graphics card", "Board power draw in watts"),
    ("gpu.fan", "GPU fan", "Graphics card", "Fan speed in percent"),
    ("gpu.vram", "VRAM used", "Graphics card", "Video memory in use"),
    ("gpu.vram_pct", "VRAM %", "Graphics card", "Video memory in use, in percent"),
    ("cpu.usage", "CPU usage", "Processor", "Total processor load"),
    ("cpu.temp", "CPU temperature", "Processor", "Package temperature (set it up on the Overview page)"),
    ("cpu.clock", "CPU clock", "Processor", "Average effective clock speed"),
    ("cpu.cores", "Per-core load", "Processor", "A small bar for every core"),
    ("ram.used", "RAM used", "Memory", "System memory in use"),
    ("ram.total", "RAM total", "Memory", "Total physical system memory"),
    ("ram.pct", "RAM %", "Memory", "System memory in use, in percent"),
    ("app.ram", "App memory", "Memory", "Memory used by the app in front"),
    ("disk.read", "Disk read", "Disk & network", "Bytes read per second, all disks"),
    ("disk.write", "Disk write", "Disk & network", "Bytes written per second, all disks"),
    ("disk.active", "Disk activity", "Disk & network", "How busy the disks are"),
    ("net.down", "Download", "Disk & network", "Network receive speed"),
    ("net.up", "Upload", "Disk & network", "Network send speed"),
    ("app", "App name", "Other", "Name of the app in front"),
    ("time", "Clock", "Other", "The current time"),
];

/// The per-core metrics a config graphs (see `Sampler::graphed_cores`).
pub fn graphed_cores(rows: &[crate::config::Row]) -> Vec<String> {
    let mut ids: Vec<String> = vec![];
    for row in rows.iter().filter(|r| r.display != crate::config::Display::Text) {
        let fallback = if row.graph.is_empty() { row.metrics.first() } else { None };
        for id in row.graph.iter().chain(fallback) {
            if id.starts_with("cpu.core") && !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    ids
}

pub fn display_name(id: &str) -> String {
    if let Some((_, n, _, _)) = CATALOG.iter().find(|(i, ..)| *i == id) {
        return n.to_string();
    }
    if let Some(n) = id.strip_prefix("cpu.core.") {
        return n.parse::<usize>().map_or_else(|_| format!("Core {n} load"), |n| format!("Core {} load", n + 1));
    }
    if let Some(n) = id.strip_prefix("cpu.core_clock.") {
        return n.parse::<usize>().map_or_else(|_| format!("Core {n} clock"), |n| format!("Core {} clock", n + 1));
    }
    id.to_string()
}

/// The color group of a metric (drives accent colors).
pub fn group_of(id: &str) -> &'static str {
    let head = id.split('.').next().unwrap_or("");
    match head {
        "fps" | "frametime" => "fps",
        "gpu" if id.starts_with("gpu.vram") => "vram",
        "gpu" => "gpu",
        "cpu" => "cpu",
        "ram" => "ram",
        "app" if id == "app.ram" => "ram",
        "app" => "app",
        "disk" => "disk",
        "net" => "net",
        _ => "misc",
    }
}

/// Fixed graph range for metrics that have one.
pub fn graph_max(id: &str) -> Option<f64> {
    if id.ends_with("usage") || id.ends_with("pct") || id.ends_with("active") || id.ends_with(".fan") || id.starts_with("cpu.core.") {
        Some(100.0)
    } else {
        None
    }
}

pub struct Cell {
    pub value: String,
    pub unit: String,
}

/// The widest normal reading of a metric and its widest unit. Fixed spacing reserves this much
/// room so values don't shift as digits come and go. Digits are tabular, so "0" stands for any digit.
pub fn template(id: &str) -> (&'static str, &'static str) {
    match id {
        "fps" | "fps.low1" | "fps.low01" => ("000", "FPS"),
        "frametime" => ("00.0", "ms"),
        "gpu.clock" | "gpu.mem_clock" => ("0,000", "MHz"),
        "gpu.power" => ("000", "W"),
        "gpu.vram" => ("00.0", "GB"),
        "cpu.clock" => ("0.00", "GHz"),
        "ram.used" | "ram.total" => ("00.0", "GB"),
        "app.ram" => ("000", "MB"),
        "disk.read" | "disk.write" | "net.down" | "net.up" => ("000", "MB/s"),
        "app" => ("", ""),
        "time" => ("00:00", ""),
        _ if id.ends_with(".temp") => ("00", "°C"),
        _ if id.starts_with("cpu.core_clock.") => ("0.0", "GHz"),
        _ if graph_max(id) == Some(100.0) => ("100", "%"),
        _ => ("000", ""),
    }
}

fn cell(value: String, unit: &str) -> Cell {
    Cell { value, unit: unit.to_string() }
}

fn bytes_rate(b: f64) -> Cell {
    let kb = b / 1024.0;
    if kb < 1000.0 {
        cell(format!("{kb:.0}"), "KB/s")
    } else if kb < 1024.0 * 100.0 {
        cell(format!("{:.1}", kb / 1024.0), "MB/s")
    } else {
        cell(format!("{:.0}", kb / 1024.0), "MB/s")
    }
}

fn thousands(v: f64) -> String {
    let n = v.round() as i64;
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// Formats a numeric metric value with its unit.
pub fn format(id: &str, v: f64, temp: TempUnit) -> Cell {
    match id {
        "fps" => cell(format!("{v:.0}"), "FPS"),
        "fps.low1" => cell(format!("{v:.0}"), "1%"),
        "fps.low01" => cell(format!("{v:.0}"), "0.1%"),
        "frametime" => cell(format!("{v:.1}"), "ms"),
        "gpu.clock" | "gpu.mem_clock" => cell(thousands(v), "MHz"),
        "gpu.power" => cell(format!("{v:.0}"), "W"),
        "gpu.vram" => {
            if v >= 1024.0 {
                cell(format!("{:.1}", v / 1024.0), "GB")
            } else {
                cell(format!("{v:.0}"), "MB")
            }
        }
        "cpu.clock" => cell(format!("{:.2}", v / 1000.0), "GHz"),
        "ram.used" | "ram.total" => cell(format!("{v:.1}"), "GB"),
        "app.ram" => {
            if v >= 1.0 {
                cell(format!("{v:.2}"), "GB")
            } else {
                cell(format!("{:.0}", v * 1024.0), "MB")
            }
        }
        "disk.read" | "disk.write" | "net.down" | "net.up" => bytes_rate(v),
        _ if id.ends_with(".temp") => match temp {
            TempUnit::C => cell(format!("{v:.0}"), "°C"),
            TempUnit::F => cell(format!("{:.0}", v * 9.0 / 5.0 + 32.0), "°F"),
        },
        _ if id.starts_with("cpu.core_clock.") => cell(format!("{:.1}", v / 1000.0), "GHz"),
        _ if graph_max(id) == Some(100.0) => cell(format!("{v:.0}"), "%"),
        _ => cell(format!("{v:.1}"), ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats() {
        assert_eq!(format("gpu.clock", 1860.0, TempUnit::C).value, "1,860");
        assert_eq!(format("gpu.temp", 100.0, TempUnit::F).value, "212");
        assert_eq!(format("cpu.core.3", 42.4, TempUnit::C).unit, "%");
        assert_eq!(format("net.down", 2.5 * 1048576.0, TempUnit::C).unit, "MB/s");
        assert_eq!(group_of("gpu.vram_pct"), "vram");
    }
}
