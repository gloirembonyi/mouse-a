// Mouse AI Desktop - Computer-use input synthesis
//
// Lets the agent physically drive the machine: move+click the mouse, type text,
// and press key combos - via the Win32 SendInput API. This is what makes the
// agent able to fill a form in any app (browser included), not just read the
// screen. All coordinates are GLOBAL virtual-desktop pixels, matching the rest
// of the app and GetCursorPos.

#[cfg(windows)]
mod win {
    use std::mem::{size_of, zeroed};
    use winapi::ctypes::c_int;
    use winapi::um::winuser::{
        SendInput, SetCursorPos, VkKeyScanW, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
        KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, VK_BACK, VK_CONTROL, VK_DELETE,
        VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_LWIN, VK_MENU, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_TAB, VK_UP,
    };

    fn send(inputs: &mut [INPUT]) {
        unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_mut_ptr(),
                size_of::<INPUT>() as c_int,
            );
            crate::shield::OUR_LAST_INPUT_TICK.store(winapi::um::sysinfoapi::GetTickCount64(), std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn key_input(vk: u16, flags: u32) -> INPUT {
        let mut input: INPUT = unsafe { zeroed() };
        input.type_ = INPUT_KEYBOARD;
        unsafe {
            let ki = input.u.ki_mut();
            *ki = KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            };
        }
        input
    }

    fn unicode_input(ch: u16, key_up: bool) -> INPUT {
        let mut input: INPUT = unsafe { zeroed() };
        input.type_ = INPUT_KEYBOARD;
        unsafe {
            let ki = input.u.ki_mut();
            *ki = KEYBDINPUT {
                wVk: 0,
                wScan: ch,
                dwFlags: KEYEVENTF_UNICODE | if key_up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            };
        }
        input
    }

    pub fn move_and_click(x: i32, y: i32, right: bool) {
        unsafe { SetCursorPos(x, y) };
        crate::shield::OUR_LAST_INPUT_TICK.store(unsafe { winapi::um::sysinfoapi::GetTickCount64() }, std::sync::atomic::Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(40));
        let (down, up) = if right {
            (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
        } else {
            (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
        };
        let mut inputs = [mouse_input(down), mouse_input(up)];
        send(&mut inputs);
    }

    pub fn move_and_double_click(x: i32, y: i32) {
        unsafe { SetCursorPos(x, y) };
        crate::shield::OUR_LAST_INPUT_TICK.store(unsafe { winapi::um::sysinfoapi::GetTickCount64() }, std::sync::atomic::Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(40));
        let mut inputs = [
            mouse_input(MOUSEEVENTF_LEFTDOWN),
            mouse_input(MOUSEEVENTF_LEFTUP),
            mouse_input(MOUSEEVENTF_LEFTDOWN),
            mouse_input(MOUSEEVENTF_LEFTUP),
        ];
        send(&mut inputs);
    }

    /// Scroll the wheel by `clicks` notches (positive = up, negative = down)
    /// at the given point.
    pub fn scroll(x: i32, y: i32, clicks: i32) {
        unsafe { SetCursorPos(x, y) };
        crate::shield::OUR_LAST_INPUT_TICK.store(unsafe { winapi::um::sysinfoapi::GetTickCount64() }, std::sync::atomic::Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(30));
        let mut input = mouse_input(MOUSEEVENTF_WHEEL);
        unsafe {
            let mi = input.u.mi_mut();
            // One wheel notch = WHEEL_DELTA (120).
            mi.mouseData = (clicks * 120) as u32;
        }
        let mut inputs = [input];
        send(&mut inputs);
    }

    fn mouse_input(flags: u32) -> INPUT {
        let mut input: INPUT = unsafe { zeroed() };
        input.type_ = INPUT_MOUSE;
        unsafe {
            let mi = input.u.mi_mut();
            *mi = MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            };
        }
        input
    }

    pub fn type_text(text: &str) {
        // Send each UTF-16 code unit as a synthetic Unicode keypress. Works
        // regardless of keyboard layout and covers characters with no VK.
        for unit in text.encode_utf16() {
            let mut inputs = [unicode_input(unit, false), unicode_input(unit, true)];
            send(&mut inputs);
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
    }

    /// Keystroke typing for the agent: newlines press Enter and tabs press Tab (a
    /// Unicode U+000A keypress inserts nothing in most apps), and ordinary runs go
    /// out in batches of up to 32 code units per SendInput call with one short pause
    /// per batch - about 30x faster than one unit + 3 ms each, which made a 2000-char
    /// paste take six seconds.
    pub fn type_text_keys(text: &str) {
        let mut batch: Vec<INPUT> = Vec::with_capacity(64);
        let flush = |batch: &mut Vec<INPUT>| {
            if batch.is_empty() {
                return;
            }
            send(batch);
            batch.clear();
            std::thread::sleep(std::time::Duration::from_millis(3));
        };
        for ch in text.chars() {
            match ch {
                '\r' => continue,
                '\n' | '\t' => {
                    flush(&mut batch);
                    let vk = if ch == '\n' { VK_RETURN } else { VK_TAB } as u16;
                    let mut keys = [key_input(vk, 0), key_input(vk, KEYEVENTF_KEYUP)];
                    send(&mut keys);
                    std::thread::sleep(std::time::Duration::from_millis(12));
                }
                _ => {
                    let mut buf = [0u16; 2];
                    for unit in ch.encode_utf16(&mut buf).iter() {
                        batch.push(unicode_input(*unit, false));
                        batch.push(unicode_input(*unit, true));
                    }
                    if batch.len() >= 64 {
                        flush(&mut batch);
                    }
                }
            }
        }
        flush(&mut batch);
    }

    /// Map a friendly key name to a Win32 virtual-key code. Returns None for
    /// single printable characters (handled via VkKeyScanW by the caller).
    fn named_vk(name: &str) -> Option<i32> {
        let vk = match name.to_lowercase().as_str() {
            "enter" | "return" => VK_RETURN,
            "tab" => VK_TAB,
            "esc" | "escape" => VK_ESCAPE,
            "backspace" => VK_BACK,
            "delete" | "del" => VK_DELETE,
            "home" => VK_HOME,
            "end" => VK_END,
            "up" => VK_UP,
            "down" => VK_DOWN,
            "left" => VK_LEFT,
            "right" => VK_RIGHT,
            "ctrl" | "control" => VK_CONTROL,
            "alt" => VK_MENU,
            "shift" => VK_SHIFT,
            "win" | "super" | "meta" => VK_LWIN,
            "space" => 0x20,
            "insert" | "ins" => 0x2D,
            "pageup" | "pgup" => 0x21,
            "pagedown" | "pgdn" => 0x22,
            "f1" => 0x70, "f2" => 0x71, "f3" => 0x72, "f4" => 0x73, "f5" => 0x74, "f6" => 0x75,
            "f7" => 0x76, "f8" => 0x77, "f9" => 0x78, "f10" => 0x79, "f11" => 0x7A, "f12" => 0x7B,
            _ => return None,
        };
        Some(vk)
    }

    fn resolve_vk(token: &str) -> Option<u16> {
        if let Some(vk) = named_vk(token) {
            return Some(vk as u16);
        }
        let mut chars = token.chars();
        let ch = chars.next()?;
        if chars.next().is_some() {
            return None; // multi-char, non-named token - unsupported
        }
        let scan = unsafe { VkKeyScanW(ch as u16) };
        if scan == -1 {
            return None;
        }
        Some((scan as u16) & 0xFF)
    }

    /// Press a combo like "ctrl+c", "alt+tab", "ctrl+shift+t", or a single key
    /// like "enter". Modifiers are held while the final key is tapped.
    pub fn press_combo(combo: &str) -> Result<(), String> {
        let tokens: Vec<&str> = combo.split('+').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if tokens.is_empty() {
            return Err("empty key combo".to_string());
        }
        let vks: Vec<u16> = tokens
            .iter()
            .map(|t| resolve_vk(t).ok_or_else(|| format!("unknown key: {}", t)))
            .collect::<Result<_, _>>()?;

        let mut downs: Vec<INPUT> = vks.iter().map(|&vk| key_input(vk, 0)).collect();
        send(&mut downs);
        std::thread::sleep(std::time::Duration::from_millis(20));
        // Release in reverse order, mirroring how a human lifts keys.
        let mut ups: Vec<INPUT> = vks.iter().rev().map(|&vk| key_input(vk, KEYEVENTF_KEYUP)).collect();
        send(&mut ups);
        Ok(())
    }
}

#[cfg(windows)]
pub fn click(x: i32, y: i32, right: bool) {
    win::move_and_click(x, y, right);
}

#[cfg(windows)]
pub fn double_click(x: i32, y: i32) {
    win::move_and_double_click(x, y);
}

#[cfg(windows)]
pub fn scroll(x: i32, y: i32, clicks: i32) {
    win::scroll(x, y, clicks);
}

#[cfg(windows)]
pub fn type_text(text: &str) {
    win::type_text(text);
}

/// Agent typing: batched keystrokes, `\n` = Enter, `\t` = Tab. See `win::type_text_keys`.
#[cfg(windows)]
pub fn type_text_keys(text: &str) {
    win::type_text_keys(text);
}

/// Put `text` on the clipboard, press Ctrl+V into the focused app, then restore what
/// was on the clipboard (text only - a non-text clipboard is left as the pasted text,
/// which the caller reports). Moved here from copilot.rs so the agent's type_text can
/// paste code without the auto-indent/auto-close corruption of per-key typing.
#[cfg(windows)]
pub fn paste_via_clipboard(text: &str) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("Clipboard unavailable: {}", e))?;
    let previous = cb.get_text().ok();
    cb.set_text(text.to_string()).map_err(|e| format!("Clipboard unavailable: {}", e))?;
    std::thread::sleep(std::time::Duration::from_millis(60));
    press_combo("ctrl+v")?;
    // Give the target app time to read the clipboard before restoring it.
    std::thread::sleep(std::time::Duration::from_millis(450));
    if let Some(prev) = previous {
        let _ = cb.set_text(prev);
    }
    Ok(())
}

#[cfg(windows)]
pub fn press_combo(combo: &str) -> Result<(), String> {
    win::press_combo(combo)
}

#[cfg(not(windows))]
pub fn click(_x: i32, _y: i32, _right: bool) {}
#[cfg(not(windows))]
pub fn double_click(_x: i32, _y: i32) {}
#[cfg(not(windows))]
pub fn scroll(_x: i32, _y: i32, _clicks: i32) {}
#[cfg(not(windows))]
pub fn type_text(_text: &str) {}
#[cfg(not(windows))]
pub fn type_text_keys(_text: &str) {}
#[cfg(not(windows))]
pub fn paste_via_clipboard(_text: &str) -> Result<(), String> {
    Err("computer-use input is only supported on Windows".to_string())
}
#[cfg(not(windows))]
pub fn press_combo(_combo: &str) -> Result<(), String> {
    Err("computer-use input is only supported on Windows".to_string())
}
