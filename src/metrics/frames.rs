//! Per-process frame timing without injecting anything: a real-time ETW session listening to
//! the DXGI (D3D10/11/12) and D3D9 runtime `Present` events, like PresentMon does.
//! Starting an ETW session needs administrator rights (or the Performance Log Users group).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Diagnostics::Etw::*;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::core::{GUID, PCWSTR, PWSTR};

const SESSION: &str = "DeltaOverlayFrames";
const DXGI: GUID = GUID::from_u128(0xCA11C036_0102_4A2D_A6AD_F03CFED5D3C9);
const D3D9: GUID = GUID::from_u128(0x783ACA0A_790E_4D7F_8451_AA850511C6B9);
const DXGI_PRESENT_START: u16 = 42;
const D3D9_PRESENT_START: u16 = 1;
const EVENT_TRACE_USE_MS_FLUSH_TIMER: u32 = 0x10;
const DXGI_PRESENT_TEST: u32 = 1;

/// Seconds of frames kept per process (for the 1% / 0.1% lows).
const KEEP_SECS: f64 = 10.0;

struct ProcFrames {
    /// Which API we count for this process (0 = DXGI, 1 = D3D9), so nothing is counted twice.
    source: u8,
    last_ts: i64,
    /// (qpc timestamp, frame time in ms)
    frames: VecDeque<(i64, f32)>,
}

struct State {
    procs: HashMap<u32, ProcFrames>,
    qpc_freq: f64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Default, Debug)]
pub struct FrameStats {
    pub fps: f64,
    pub frametime: f64,
    pub low1: Option<f64>,
    pub low01: Option<f64>,
    /// Most recent frame times (ms), oldest first.
    pub recent: Vec<f32>,
    /// Frames presented in the last second (for RTSS-compatible counters).
    pub frames_1s: u32,
}

pub fn running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

fn now_qpc() -> i64 {
    let mut t = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut t);
    }
    t
}

/// Frame stats of `pid`, or None if it hasn't presented a frame recently.
pub fn stats(pid: u32, recent: usize) -> Option<FrameStats> {
    let guard = STATE.lock().ok()?;
    let st = guard.as_ref()?;
    let p = st.procs.get(&pid)?;
    let now = now_qpc();
    let freq = st.qpc_freq;
    if (now - p.last_ts) as f64 / freq > 1.5 {
        return None;
    }
    let one_sec = now - freq as i64;
    let last_sec: Vec<f32> = p.frames.iter().filter(|(t, _)| *t >= one_sec).map(|(_, ms)| *ms).collect();
    if last_sec.is_empty() {
        return None;
    }
    let avg = last_sec.iter().map(|&m| m as f64).sum::<f64>() / last_sec.len() as f64;
    let mut all: Vec<f32> = p.frames.iter().map(|(_, ms)| *ms).collect();
    let n = all.len();
    let start = n.saturating_sub(recent);
    let recent_frames = all[start..].to_vec();
    let (low1, low01) = if n >= 20 {
        all.sort_by(|a, b| b.total_cmp(a)); // slowest first
        let pick = |pct: f64| {
            let idx = ((n as f64) * pct / 100.0).floor() as usize;
            1000.0 / all[idx.min(n - 1)].max(0.01) as f64
        };
        (Some(pick(1.0)), (n >= 200).then(|| pick(0.1)))
    } else {
        (None, None)
    };
    Some(FrameStats {
        fps: if avg > 0.0 { 1000.0 / avg } else { 0.0 },
        frametime: avg,
        low1,
        low01,
        recent: recent_frames,
        frames_1s: last_sec.len() as u32,
    })
}

/// Processes that presented frames in the last couple of seconds.
pub fn active_pids() -> Vec<u32> {
    let Ok(guard) = STATE.lock() else { return vec![] };
    let Some(st) = guard.as_ref() else { return vec![] };
    let cutoff = now_qpc() - (2.0 * st.qpc_freq) as i64;
    st.procs.iter().filter(|(_, p)| p.last_ts > cutoff).map(|(pid, _)| *pid).collect()
}

fn record(pid: u32, ts: i64, source: u8) {
    let Ok(mut guard) = STATE.lock() else { return };
    let Some(st) = guard.as_mut() else { return };
    let freq = st.qpc_freq;
    let keep = (KEEP_SECS * freq) as i64;
    let p = st.procs.entry(pid).or_insert(ProcFrames { source, last_ts: 0, frames: VecDeque::with_capacity(2048) });
    if p.source != source {
        // Only switch API if the other one has gone quiet.
        if ((ts - p.last_ts) as f64) < 2.0 * freq {
            return;
        }
        p.source = source;
        p.last_ts = 0;
        p.frames.clear();
    }
    if p.last_ts != 0 && ts > p.last_ts {
        let ms = ((ts - p.last_ts) as f64 * 1000.0 / freq) as f32;
        if ms < 5000.0 {
            p.frames.push_back((ts, ms));
        }
    }
    p.last_ts = ts;
    while p.frames.front().is_some_and(|(t, _)| ts - *t > keep) {
        p.frames.pop_front();
    }
    // Forget processes that stopped rendering a while ago.
    if p.frames.len() == 1 {
        let stale = ts - 30 * freq as i64;
        st.procs.retain(|_, q| q.last_ts > stale);
    }
}

unsafe extern "system" fn on_event(rec: *mut EVENT_RECORD) {
    let r = unsafe { &*rec };
    let h = &r.EventHeader;
    let id = h.EventDescriptor.Id;
    let source = if id == DXGI_PRESENT_START && h.ProviderId == DXGI {
        // Present_Start payload: pIDXGISwapChain (pointer), Flags (u32), SyncInterval (i32)
        let ptr_size = if h.Flags as u32 & EVENT_HEADER_FLAG_32_BIT_HEADER != 0 { 4 } else { 8 };
        if r.UserDataLength as usize >= ptr_size + 4 && !r.UserData.is_null() {
            let flags = unsafe { std::ptr::read_unaligned((r.UserData as *const u8).add(ptr_size) as *const u32) };
            if flags & DXGI_PRESENT_TEST != 0 {
                return;
            }
        }
        0
    } else if id == D3D9_PRESENT_START && h.ProviderId == D3D9 {
        1
    } else {
        return;
    };
    record(h.ProcessId, h.TimeStamp, source);
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn properties(name_len: usize) -> Vec<u64> {
    let base = std::mem::size_of::<EVENT_TRACE_PROPERTIES>();
    let total = base + (name_len + 1) * 2 + 64;
    let mut buf = vec![0u64; total.div_ceil(8)];
    let p = buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
    unsafe {
        (*p).Wnode.BufferSize = total as u32;
        (*p).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        (*p).Wnode.ClientContext = 1; // QPC timestamps
        (*p).BufferSize = 16; // KB
        (*p).MinimumBuffers = 4;
        (*p).MaximumBuffers = 32;
        (*p).LogFileMode = EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_USE_MS_FLUSH_TIMER;
        (*p).FlushTimer = 100; // ms: keeps the frametime graph smooth
        (*p).LoggerNameOffset = base as u32;
    }
    buf
}

/// Starts the session and the consumer thread. Err describes why FPS isn't available.
pub fn start() -> Result<(), String> {
    if running() {
        return Ok(());
    }
    let mut freq = 0i64;
    unsafe {
        let _ = QueryPerformanceFrequency(&mut freq);
    }
    *STATE.lock().unwrap() = Some(State { procs: HashMap::new(), qpc_freq: freq as f64 });

    let name = wide(SESSION);
    unsafe {
        // A session left behind by a previous run (e.g. after a crash) would block ours.
        let mut stop = properties(name.len());
        let _ = ControlTraceW(CONTROLTRACE_HANDLE::default(), PCWSTR(name.as_ptr()), stop.as_mut_ptr() as *mut _, EVENT_TRACE_CONTROL_STOP);

        let mut props = properties(name.len());
        let mut handle = CONTROLTRACE_HANDLE::default();
        let err = StartTraceW(&mut handle, PCWSTR(name.as_ptr()), props.as_mut_ptr() as *mut _);
        if err != ERROR_SUCCESS {
            return Err(if err.0 == 5 { "FPS needs Delta to run as administrator".into() } else { format!("couldn't start frame tracing (error {})", err.0) });
        }
        for guid in [DXGI, D3D9] {
            let e = EnableTraceEx2(handle, &guid, EVENT_CONTROL_CODE_ENABLE_PROVIDER.0, TRACE_LEVEL_INFORMATION as u8, 0, 0, 0, None);
            if e != ERROR_SUCCESS {
                eprintln!("frames: enabling provider failed: {}", e.0);
            }
        }

        let mut logger_name = name.clone();
        let mut logfile = EVENT_TRACE_LOGFILEW::default();
        logfile.LoggerName = PWSTR(logger_name.as_mut_ptr());
        logfile.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
        logfile.Anonymous2.EventRecordCallback = Some(on_event);
        let trace = OpenTraceW(&mut logfile);
        if trace.Value == u64::MAX {
            stop_session();
            return Err("couldn't open the frame trace".into());
        }
        RUNNING.store(true, Ordering::Relaxed);
        std::thread::Builder::new()
            .name("etw-frames".into())
            .spawn(move || {
                let _keep = logger_name;
                let _ = ProcessTrace(&[trace], None, None);
                let _ = CloseTrace(trace);
                RUNNING.store(false, Ordering::Relaxed);
            })
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn stop_session() {
    let name = wide(SESSION);
    let mut props = properties(name.len());
    unsafe {
        let _ = ControlTraceW(CONTROLTRACE_HANDLE::default(), PCWSTR(name.as_ptr()), props.as_mut_ptr() as *mut _, EVENT_TRACE_CONTROL_STOP);
    }
}
