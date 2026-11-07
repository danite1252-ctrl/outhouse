//! Push to talk that works while another app (like WoW) is focused.
//!
//! Windows "low-level" keyboard and mouse hooks see every key press system
//! wide (the same approach Discord, TeamSpeak and Mumble use). The key is
//! never swallowed, so the game still gets it. Only the one key or mouse
//! button you chose is looked at, and only while you're in voice with push
//! to talk on; everything else is ignored.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;

use tauri::{AppHandle, Manager};

/// 0 = nothing. Keyboard keys are Windows virtual-key codes; mouse buttons
/// are MOUSE | 3 (middle), 4 (back) or 5 (forward).
static BINDING: AtomicU32 = AtomicU32::new(0);
static DOWN: AtomicBool = AtomicBool::new(false);
static TX: OnceLock<Sender<bool>> = OnceLock::new();
const MOUSE: u32 = 0x1_0000;

/// Sets the key from the web page's name for it (KeyboardEvent.code, or
/// MouseMiddle / Mouse4 / Mouse5). Returns false if it isn't one we know.
pub fn set_binding(code: Option<&str>) -> bool {
    let binding = code.and_then(code_to_binding).unwrap_or(0);
    let old = BINDING.swap(binding, Ordering::SeqCst);
    if old != binding {
        edge(false); // let go of any half-pressed old key
    }
    code.is_none() || binding != 0
}

fn edge(down: bool) {
    if DOWN.swap(down, Ordering::SeqCst) != down {
        if let Some(tx) = TX.get() {
            let _ = tx.send(down);
        }
    }
}

pub fn start(app: AppHandle) {
    let (tx, rx) = channel::<bool>();
    let _ = TX.set(tx);
    // Hooks must return fast, so the page is told from this thread instead.
    std::thread::spawn(move || {
        for down in rx {
            let Some(w) = app.get_webview_window("main") else {
                continue;
            };
            // While Outhouse itself is focused, the page sees the key directly
            // (and knows not to trigger while you're typing in chat).
            if down && w.is_focused().unwrap_or(false) {
                continue;
            }
            let _ = w.eval(&format!(
                "window.outhouseDesktopPtt && window.outhouseDesktopPtt({down})"
            ));
        }
    });
    #[cfg(windows)]
    win::spawn_hooks();
}

fn code_to_binding(code: &str) -> Option<u32> {
    let vk: u32 = match code {
        "MouseMiddle" => return Some(MOUSE | 3),
        "Mouse4" => return Some(MOUSE | 4),
        "Mouse5" => return Some(MOUSE | 5),
        c if c.len() == 4 && c.starts_with("Key") => c.as_bytes()[3] as u32, // KeyA..KeyZ -> 'A'..'Z'
        c if c.len() == 6 && c.starts_with("Digit") => c.as_bytes()[5] as u32, // '0'..'9'
        c if c.len() == 7 && c.starts_with("Numpad") && c.as_bytes()[6].is_ascii_digit() => {
            0x60 + (c.as_bytes()[6] - b'0') as u32
        }
        c if c.starts_with('F')
            && c[1..]
                .parse::<u32>()
                .map_or(false, |n| (1..=24).contains(&n)) =>
        {
            0x6F + c[1..].parse::<u32>().unwrap()
        }
        "Backquote" => 0xC0,
        "Minus" => 0xBD,
        "Equal" => 0xBB,
        "BracketLeft" => 0xDB,
        "BracketRight" => 0xDD,
        "Backslash" => 0xDC,
        "IntlBackslash" => 0xE2,
        "Semicolon" => 0xBA,
        "Quote" => 0xDE,
        "Comma" => 0xBC,
        "Period" => 0xBE,
        "Slash" => 0xBF,
        "Space" => 0x20,
        "Tab" => 0x09,
        "Enter" => 0x0D,
        "Backspace" => 0x08,
        "Escape" => 0x1B,
        "CapsLock" => 0x14,
        "ShiftLeft" => 0xA0,
        "ShiftRight" => 0xA1,
        "ControlLeft" => 0xA2,
        "ControlRight" => 0xA3,
        "AltLeft" => 0xA4,
        "AltRight" => 0xA5,
        "MetaLeft" => 0x5B,
        "MetaRight" => 0x5C,
        "ContextMenu" => 0x5D,
        "Insert" => 0x2D,
        "Delete" => 0x2E,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "ArrowUp" => 0x26,
        "ArrowDown" => 0x28,
        "ArrowLeft" => 0x25,
        "ArrowRight" => 0x27,
        "NumpadAdd" => 0x6B,
        "NumpadSubtract" => 0x6D,
        "NumpadMultiply" => 0x6A,
        "NumpadDivide" => 0x6F,
        "NumpadDecimal" => 0x6E,
        "NumLock" => 0x90,
        "ScrollLock" => 0x91,
        "Pause" => 0x13,
        _ => return None,
    };
    Some(vk)
}

#[cfg(windows)]
mod win {
    use std::sync::atomic::Ordering;

    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL,
        WM_KEYDOWN, WM_KEYUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
        WM_XBUTTONDOWN, WM_XBUTTONUP,
    };

    use super::{edge, BINDING, MOUSE};

    pub fn spawn_hooks() {
        std::thread::spawn(|| unsafe {
            let module = GetModuleHandleW(std::ptr::null());
            let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), module, 0);
            let ms = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), module, 0);
            // Low-level hooks are delivered through this thread's message loop.
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            UnhookWindowsHookEx(kb);
            UnhookWindowsHookEx(ms);
        });
    }

    unsafe extern "system" fn keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            let binding = BINDING.load(Ordering::Relaxed);
            if binding != 0 && binding < MOUSE {
                let info = &*(lparam as *const KBDLLHOOKSTRUCT);
                if info.vkCode == binding {
                    match wparam as u32 {
                        WM_KEYDOWN | WM_SYSKEYDOWN => edge(true),
                        WM_KEYUP | WM_SYSKEYUP => edge(false),
                        _ => {}
                    }
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }

    unsafe extern "system" fn mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            let binding = BINDING.load(Ordering::Relaxed);
            if binding > MOUSE {
                let info = &*(lparam as *const MSLLHOOKSTRUCT);
                let msg = wparam as u32;
                let event = match msg {
                    WM_MBUTTONDOWN => Some((3, true)),
                    WM_MBUTTONUP => Some((3, false)),
                    WM_XBUTTONDOWN | WM_XBUTTONUP => match (info.mouseData >> 16) & 0xFFFF {
                        1 => Some((4, msg == WM_XBUTTONDOWN)),
                        2 => Some((5, msg == WM_XBUTTONDOWN)),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some((button, down)) = event {
                    if MOUSE | button == binding {
                        edge(down);
                    }
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }
}
