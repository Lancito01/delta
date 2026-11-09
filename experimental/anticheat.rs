//! Spots anti-cheat before Delta loads anything into a game.
//!
//! Anti-cheat systems watch for modules loaded into their game and may kick or ban for it, even for
//! a harmless overlay. Delta looks at the game four ways and errs on the side of staying out:
//! known games, files next to the game, modules inside it, and anti-cheat services running now.

use std::path::Path;
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError};
use windows::Win32::System::Diagnostics::ToolHelp::*;

/// Lowercase markers (file, folder, module or process names) and the anti-cheat they belong to.
const MARKERS: &[(&str, &str)] = &[
    ("easyanticheat", "Easy Anti-Cheat"),
    ("start_protected_game", "Easy Anti-Cheat"),
    ("beclient", "BattlEye"),
    ("beservice", "BattlEye"),
    ("battleye", "BattlEye"),
    ("eaanticheat", "EA Javelin Anti-Cheat"),
    ("gameguard", "nProtect GameGuard"),
    ("gamemon", "nProtect GameGuard"),
    ("npgg", "nProtect GameGuard"),
    ("xigncode", "XIGNCODE3"),
    ("x3.xem", "XIGNCODE3"),
    ("xhunter", "XIGNCODE3"),
    ("wellbia", "XIGNCODE3"),
    ("pnkbstr", "PunkBuster"),
    ("punkbuster", "PunkBuster"),
    ("mhyprot", "miHoYo anti-cheat"),
    ("equ8", "EQU8"),
    ("ace-base", "Anti-Cheat Expert"),
    ("anticheatexpert", "Anti-Cheat Expert"),
    ("sguard", "Anti-Cheat Expert"),
    ("faceit", "FACEIT"),
    ("vanguard", "Riot Vanguard"),
    ("randgrid", "Ricochet"),
    ("denuvo-anti-cheat", "Denuvo Anti-Cheat"),
    ("anti-cheat", "an anti-cheat"),
    ("anticheat", "an anti-cheat"),
];

/// Games known to use anti-cheat (exe name, lowercase).
const KNOWN: &[(&str, &str)] = &[
    ("valorant-win64-shipping.exe", "Riot Vanguard"),
    ("valorant.exe", "Riot Vanguard"),
    ("league of legends.exe", "Riot Vanguard"),
    ("cs2.exe", "Valve Anti-Cheat"),
    ("csgo.exe", "Valve Anti-Cheat"),
    ("dota2.exe", "Valve Anti-Cheat"),
    ("deadlock.exe", "Valve Anti-Cheat"),
    ("project8.exe", "Valve Anti-Cheat"),
    ("tf_win64.exe", "Valve Anti-Cheat"),
    ("fortniteclient-win64-shipping.exe", "Easy Anti-Cheat"),
    ("r5apex.exe", "Easy Anti-Cheat"),
    ("r5apex_dx12.exe", "Easy Anti-Cheat"),
    ("rustclient.exe", "Easy Anti-Cheat"),
    ("eldenring.exe", "Easy Anti-Cheat"),
    ("deadbydaylight-win64-shipping.exe", "Easy Anti-Cheat"),
    ("narakabladepoint.exe", "Easy Anti-Cheat"),
    ("discovery.exe", "Easy Anti-Cheat"),
    ("lostark.exe", "Easy Anti-Cheat"),
    ("newworld.exe", "Easy Anti-Cheat"),
    ("vrchat.exe", "Easy Anti-Cheat"),
    ("haloinfinite.exe", "Easy Anti-Cheat"),
    ("huntgame.exe", "Easy Anti-Cheat"),
    ("aces.exe", "Easy Anti-Cheat"),
    ("rainbowsix.exe", "BattlEye"),
    ("rainbowsix_be.exe", "BattlEye"),
    ("rainbowsix_vulkan.exe", "BattlEye"),
    ("tslgame.exe", "BattlEye"),
    ("escapefromtarkov.exe", "BattlEye"),
    ("destiny2.exe", "BattlEye"),
    ("dayz_x64.exe", "BattlEye"),
    ("arma3_x64.exe", "BattlEye"),
    ("gta5.exe", "BattlEye"),
    ("gta5_enhanced.exe", "BattlEye"),
    ("cod.exe", "Ricochet"),
    ("cod22-cod.exe", "Ricochet"),
    ("cod23-cod.exe", "Ricochet"),
    ("cod24-cod.exe", "Ricochet"),
    ("modernwarfare.exe", "Ricochet"),
    ("blackopscoldwar.exe", "Ricochet"),
    ("bf2042.exe", "EA Javelin Anti-Cheat"),
    ("bf6.exe", "EA Javelin Anti-Cheat"),
    ("overwatch.exe", "Overwatch's anti-cheat"),
    ("genshinimpact.exe", "miHoYo anti-cheat"),
    ("yuanshen.exe", "miHoYo anti-cheat"),
    ("starrail.exe", "miHoYo anti-cheat"),
    ("zenlesszonezero.exe", "miHoYo anti-cheat"),
    ("robloxplayerbeta.exe", "Hyperion (Byfron)"),
    ("marvel-win64-shipping.exe", "NetEase anti-cheat"),
    ("fivem.exe", "FiveM's anti-cheat"),
    ("blackdesert64.exe", "XIGNCODE3"),
];

/// Anti-cheat services that only run while a protected game does.
const SERVICES: &[(&str, &str)] = &[
    ("easyanticheat.exe", "Easy Anti-Cheat"),
    ("easyanticheat_eos.exe", "Easy Anti-Cheat"),
    ("beservice.exe", "BattlEye"),
    ("beservice_x64.exe", "BattlEye"),
    ("eaanticheat.gameservice.exe", "EA Javelin Anti-Cheat"),
    ("pnkbstra.exe", "PunkBuster"),
    ("gamemon.des", "nProtect GameGuard"),
    ("gamemon64.des", "nProtect GameGuard"),
];

fn marker(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    MARKERS.iter().find(|(m, _)| n.contains(m)).map(|(_, ac)| *ac)
}

pub enum Verdict {
    Clean,
    Found(String),
}

/// Checks a game. `deep` = Delta may look inside it (same or higher rights than the game).
pub fn check(pid: u32, exe_path: &Path, deep: bool) -> Verdict {
    let exe = exe_path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if let Some((_, ac)) = KNOWN.iter().find(|(n, _)| *n == exe) {
        return Verdict::Found(ac.to_string());
    }
    // Files and folders next to the game (and a few levels up: games often live in a "bin" folder).
    let mut dir = exe_path.parent();
    for _ in 0..4 {
        let Some(d) = dir else { break };
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten().take(400) {
                if let Some(ac) = marker(&e.file_name().to_string_lossy()) {
                    return Verdict::Found(ac.to_string());
                }
            }
        }
        // Stop at library roots ("...\common", "...\Games") so neighbors don't count.
        let name = d.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if matches!(name.as_str(), "common" | "games" | "steamapps" | "epic games" | "program files" | "program files (x86)") {
            break;
        }
        dir = d.parent();
    }
    unsafe {
        // Modules loaded in the game.
        if let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) {
            let mut m = MODULEENTRY32W { dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32, ..Default::default() };
            let mut found = None;
            if Module32FirstW(snap, &mut m).is_ok() {
                loop {
                    let n = String::from_utf16_lossy(&m.szModule[..m.szModule.iter().position(|&c| c == 0).unwrap_or(0)]);
                    if let Some(ac) = marker(&n) {
                        found = Some(ac);
                        break;
                    }
                    if Module32NextW(snap, &mut m).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
            if let Some(ac) = found {
                return Verdict::Found(ac.to_string());
            }
        } else if deep && GetLastError() == ERROR_ACCESS_DENIED {
            // Even with the rights to, Delta can't look inside: the game protects itself.
            return Verdict::Found("a protection that blocks other apps from looking inside it (usually anti-cheat)".into());
        }
        // Anti-cheat services running right now.
        if let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut found = None;
            if Process32FirstW(snap, &mut e).is_ok() {
                loop {
                    let n = String::from_utf16_lossy(&e.szExeFile[..e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)]).to_ascii_lowercase();
                    if let Some((_, ac)) = SERVICES.iter().find(|(s, _)| *s == n) {
                        found = Some(*ac);
                        break;
                    }
                    if Process32NextW(snap, &mut e).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
            if let Some(ac) = found {
                return Verdict::Found(ac.to_string());
            }
        }
    }
    Verdict::Clean
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markers() {
        assert_eq!(marker("EasyAntiCheat_EOS_Setup.exe"), Some("Easy Anti-Cheat"));
        assert_eq!(marker("BEClient_x64.dll"), Some("BattlEye"));
        assert_eq!(marker("d3d11.dll"), None);
        let dir = std::env::temp_dir().join("delta-ac-test");
        let _ = std::fs::create_dir_all(dir.join("EasyAntiCheat"));
        assert!(matches!(check(0, &dir.join("game.exe"), false), Verdict::Found(_)));
        assert!(matches!(check(0, Path::new("C:\\x\\cs2.exe"), false), Verdict::Found(ac) if ac.contains("Valve")));
    }
}
