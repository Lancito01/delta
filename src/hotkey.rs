//! Global hotkeys from strings like "Ctrl+Shift+F10".

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

pub fn parse(s: &str) -> Result<(HOT_KEY_MODIFIERS, u32), String> {
    let mut mods = MOD_NOREPEAT;
    let mut key = None;
    for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" | "super" => mods |= MOD_WIN,
            k => key = Some(vk(k).ok_or_else(|| format!("unknown key '{part}' in hotkey '{s}'"))?),
        }
    }
    key.map(|k| (mods, k)).ok_or_else(|| format!("hotkey '{s}' has no key"))
}

fn vk(k: &str) -> Option<u32> {
    let named = match k {
        "home" => VK_HOME,
        "end" => VK_END,
        "insert" | "ins" => VK_INSERT,
        "delete" | "del" => VK_DELETE,
        "pageup" | "pgup" => VK_PRIOR,
        "pagedown" | "pgdn" => VK_NEXT,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "space" => VK_SPACE,
        "tab" => VK_TAB,
        "pause" => VK_PAUSE,
        "scrolllock" => VK_SCROLL,
        "backquote" | "`" => VK_OEM_3,
        _ => VIRTUAL_KEY(0),
    };
    if named.0 != 0 {
        return Some(named.0 as u32);
    }
    if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(VK_F1.0 as u32 + n - 1);
        }
    }
    if let Some(n) = k.strip_prefix("num").and_then(|n| n.parse::<u32>().ok()) {
        if n <= 9 {
            return Some(VK_NUMPAD0.0 as u32 + n);
        }
    }
    let mut chars = k.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase() as u32),
        _ => None,
    }
}

/// Registers hotkeys (id, text); returns messages for the ones that failed.
pub fn register(hwnd: HWND, keys: &[(i32, &str)]) -> Vec<String> {
    let mut errors = vec![];
    for (id, text) in keys {
        unsafe {
            let _ = UnregisterHotKey(Some(hwnd), *id);
        }
        if text.trim().is_empty() {
            continue;
        }
        match parse(text) {
            Ok((mods, key)) => {
                if unsafe { RegisterHotKey(Some(hwnd), *id, mods, key) }.is_err() {
                    errors.push(format!("hotkey {text} is already used by another app"));
                }
            }
            Err(e) => errors.push(e),
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses() {
        let (m, k) = parse("Ctrl+Shift+F10").unwrap();
        assert_eq!(k, VK_F10.0 as u32);
        assert!(m.contains(MOD_CONTROL) && m.contains(MOD_SHIFT));
        assert_eq!(parse("alt+o").unwrap().1, 'O' as u32);
        assert!(parse("Ctrl+Nope").is_err());
        assert!(parse("Ctrl").is_err());
    }
}
