//! Updates from GitHub releases.
//!
//! Delta asks GitHub for the latest release, and if it's newer, downloads `delta.exe`, checks its
//! SHA-256 against the one GitHub publishes for it, and puts it in place of the running exe
//! (Windows lets a running exe be renamed, not overwritten: the old one moves aside to
//! `delta.exe.old` and is deleted on the next start). The new version runs from the next start;
//! Delta offers to restart, and never does it on its own (a game may be running).

use std::path::PathBuf;
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::core::{HSTRING, PCWSTR, w};

pub const REPO: &str = "Lancito01/delta";
const ASSET: &str = "delta.exe";

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    /// A newer version is downloaded and in place; it runs after a restart.
    Ready(String),
    Failed(String),
}

struct Handle(*mut core::ffi::c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// HTTPS GET (follows redirects, which GitHub's downloads use). Returns the body.
fn get(url: &str, accept: &str) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or("only https is allowed")?;
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    unsafe {
        let session = Handle(WinHttpOpen(w!("Delta-updater"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if session.0.is_null() {
            return Err("network unavailable".into());
        }
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 60_000);
        let conn = Handle(WinHttpConnect(session.0, &HSTRING::from(host), 443, 0));
        if conn.0.is_null() {
            return Err(format!("can't reach {host}"));
        }
        let req = Handle(WinHttpOpenRequest(conn.0, w!("GET"), &HSTRING::from(path.as_str()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE));
        if req.0.is_null() {
            return Err("can't create the request".into());
        }
        let headers: Vec<u16> = format!("Accept: {accept}\r\n").encode_utf16().collect();
        WinHttpSendRequest(req.0, Some(&headers), None, 0, 0, 0).map_err(|e| format!("request failed: {}", e.message()))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|e| format!("no response: {}", e.message()))?;
        let mut status = 0u32;
        let mut len = 4u32;
        let mut index = 0u32;
        WinHttpQueryHeaders(req.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut status as *mut _ as *mut _), &mut len, &mut index)
            .map_err(|e| e.message().to_string())?;
        if status != 200 {
            return Err(format!("{host} answered {status}"));
        }
        let mut body = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(req.0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut read).map_err(|e| format!("download interrupted: {}", e.message()))?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&buf[..read as usize]);
            if body.len() > 64 * 1024 * 1024 {
                return Err("download too large".into());
            }
        }
        Ok(body)
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut out = [0u8; 32];
    unsafe {
        let _ = BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out);
    }
    out.iter().map(|b| format!("{b:02x}")).collect()
}

/// "v1.2.3" or "1.2.3" -> [1, 2, 3]
fn parse_version(v: &str) -> Vec<u64> {
    v.trim().trim_start_matches(['v', 'V']).split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect()
}

pub fn is_newer(remote: &str, local: &str) -> bool {
    parse_version(remote) > parse_version(local)
}

pub struct Release {
    pub version: String,
    url: String,
    size: u64,
    sha256: Option<String>,
    sha256_url: Option<String>,
}

pub fn latest() -> Result<Release, String> {
    let body = get(&format!("https://api.github.com/repos/{REPO}/releases/latest"), "application/vnd.github+json")?;
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| format!("unexpected answer from GitHub: {e}"))?;
    let version = json["tag_name"].as_str().ok_or("the release has no version")?.trim_start_matches('v').to_string();
    let assets = json["assets"].as_array().ok_or("the release has no files")?;
    let exe = assets.iter().find(|a| a["name"].as_str() == Some(ASSET)).ok_or("the release has no delta.exe")?;
    let sum = assets.iter().find(|a| a["name"].as_str() == Some("delta.exe.sha256"));
    Ok(Release {
        version,
        url: exe["browser_download_url"].as_str().ok_or("no download link")?.to_string(),
        size: exe["size"].as_u64().unwrap_or(0),
        sha256: exe["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_ascii_lowercase),
        sha256_url: sum.and_then(|s| s["browser_download_url"].as_str()).map(str::to_string),
    })
}

fn exe_path() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| e.to_string())
}

/// Downloads the release, verifies it, and swaps it in for the running exe.
pub fn install(r: &Release) -> Result<(), String> {
    let expected = match (&r.sha256, &r.sha256_url) {
        (Some(s), _) => s.clone(),
        (None, Some(u)) => String::from_utf8_lossy(&get(u, "application/octet-stream")?).split_whitespace().next().unwrap_or("").to_ascii_lowercase(),
        _ => return Err("the release has no checksum, so it can't be verified".into()),
    };
    let data = get(&r.url, "application/octet-stream")?;
    if r.size != 0 && data.len() as u64 != r.size {
        return Err("the download is incomplete".into());
    }
    if sha256_hex(&data) != expected {
        return Err("the download doesn't match its checksum".into());
    }
    if data.len() < 2 || &data[..2] != b"MZ" {
        return Err("the download isn't a Windows program".into());
    }
    let exe = exe_path()?;
    let new = exe.with_extension("exe.download");
    let old = exe.with_extension("exe.old");
    std::fs::write(&new, &data).map_err(|e| format!("can't write next to delta.exe ({e}). Move Delta to a folder you can write to, or update by hand"))?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("can't move the old version aside: {e}"))?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe); // put things back
        return Err(format!("can't put the new version in place: {e}"));
    }
    Ok(())
}

/// Checks, and installs a newer version if there is one. `current` is this build's version.
pub fn check_and_install(current: &str) -> State {
    match latest() {
        Ok(r) if is_newer(&r.version, current) => match install(&r) {
            Ok(()) => State::Ready(r.version),
            Err(e) => State::Failed(e),
        },
        Ok(_) => State::UpToDate,
        Err(e) => State::Failed(e),
    }
}

/// Removes the previous version left by an update.
pub fn cleanup() {
    if let Ok(exe) = exe_path() {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
        let _ = std::fs::remove_file(exe.with_extension("exe.download"));
    }
}

/// Starts the (updated) exe, which waits for this one to exit.
pub fn restart() -> Result<(), String> {
    let exe = exe_path()?;
    std::process::Command::new(exe).arg("--wait").spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("0.10.0", "0.9.0"));
    }
    #[test]
    fn hashes() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
