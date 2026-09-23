//! The Windows side: a notification-area icon, its menu, and running the CLI.
//!
//! Everything a click *does* goes through `desktop-switcher.exe`, exactly as
//! a hotkey does, so the tray has no switching logic of its own and cannot
//! drift from the rules the CLI enforces.

use std::cell::Cell;
use std::ffi::c_void;
use std::mem::size_of;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command as Process;

use switcher_core::config::{self, Config, ConfigError};
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics,
    MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, TrackPopupMenu, TranslateMessage, HICON, HMENU, ICONINFO, MB_ICONERROR,
    MB_OK, MENU_ITEM_FLAGS, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, SM_CXSMICON,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WM_APP, WM_DESTROY,
    WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
};

use crate::icon;
use crate::menu::{self, Command, Entry};
use crate::notice::{self, Notice};

/// The icon reports mouse activity with this message.
const WM_TRAY: u32 = WM_APP + 1;
/// A worker thread hands a notice back to the window thread with this one.
const WM_NOTICE: u32 = WM_APP + 2;
const TRAY_ID: u32 = 1;
/// `CREATE_NO_WINDOW`: run the console CLI without flashing a console.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

thread_local! {
    static ICON: Cell<HICON> = const { Cell::new(HICON(std::ptr::null_mut())) };
    /// Explorer broadcasts this after restarting, and every tray icon
    /// vanishes with the old taskbar unless it adds itself again.
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

pub fn run() -> i32 {
    match start() {
        Ok(code) => code,
        Err(e) => {
            let text = wide(&format!("The tray could not start: {e}"));
            unsafe {
                MessageBoxW(
                    None,
                    PCWSTR(text.as_ptr()),
                    w!("Desktop Switcher"),
                    MB_OK | MB_ICONERROR,
                );
            }
            1
        }
    }
}

fn start() -> windows::core::Result<i32> {
    unsafe {
        // One tray is enough. A second launch, from the Start Menu or a pinned
        // shortcut, quietly leaves the first one in charge.
        let _single = CreateMutexW(None, true, w!("Local\\DesktopSwitcherTray"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(0);
        }

        // Without this the icon is drawn at 100% and stretched, blurry, at
        // any other scaling. Ignored if something already set it.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let instance = HINSTANCE(GetModuleHandleW(None)?.0);
        let class = w!("DesktopSwitcherTray");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        // A hidden top-level window rather than a message-only one: only a
        // top-level window hears Explorer's TaskbarCreated broadcast.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Desktop Switcher"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )?;

        TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));
        let size = GetSystemMetrics(SM_CXSMICON).max(16) as u32;
        ICON.set(make_icon(size)?);
        add_icon(hwnd);

        let mut msg = MSG::default();
        loop {
            let got = GetMessageW(&mut msg, None, 0, 0);
            if got.0 == 0 || got.0 == -1 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = DestroyIcon(ICON.get());
        Ok(0)
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAY => {
            let event = (lparam.0 & 0xFFFF) as u32;
            if event == WM_LBUTTONUP || event == WM_RBUTTONUP {
                show_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_NOTICE => {
            // Handed over by `post_notice`, which gave up ownership.
            let notice = unsafe { Box::from_raw(lparam.0 as *mut Notice) };
            show_notice(hwnd, &notice);
            LRESULT(0)
        }
        WM_DESTROY => {
            remove_icon(hwnd);
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.get() => {
            add_icon(hwnd);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        ..Default::default()
    }
}

fn add_icon(hwnd: HWND) {
    let mut data = icon_data(hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = ICON.get();
    fill(&mut data.szTip, "Desktop Switcher");
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
    }
}

fn remove_icon(hwnd: HWND) {
    let data = icon_data(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

fn show_notice(hwnd: HWND, notice: &Notice) {
    let mut data = icon_data(hwnd);
    data.uFlags = NIF_INFO;
    data.dwInfoFlags = NIIF_WARNING;
    fill(&mut data.szInfoTitle, &notice.title);
    fill(&mut data.szInfo, &notice.body);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

/// Copy text into a fixed Windows buffer, truncated, always terminated.
fn fill(buffer: &mut [u16], text: &str) {
    let room = buffer.len().saturating_sub(1);
    let mut n = 0;
    for unit in text.encode_utf16().take(room) {
        buffer[n] = unit;
        n += 1;
    }
    buffer[n] = 0;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The configuration, read fresh on every click so the menu is never stale.
fn load_config() -> Result<Config, String> {
    let path = config::default_config_path().map_err(|e| e.to_string())?;
    match Config::load(&path) {
        Ok(c) => Ok(c),
        Err(ConfigError::Missing(_)) => Err("No configuration yet".into()),
        Err(e) => {
            let text = format!("Configuration problem: {e}");
            Err(text.chars().take(80).collect())
        }
    }
}

fn show_menu(hwnd: HWND) {
    let config = load_config();
    let entries = match &config {
        Ok(c) => menu::build(Ok(c)),
        Err(why) => menu::build(Err(why)),
    };

    let chosen = unsafe {
        let Ok(popup) = CreatePopupMenu() else {
            return;
        };
        append(popup, &entries);
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        // Without the foreground dance the menu never closes when you click
        // elsewhere, a long-standing quirk of notification-area menus.
        let _ = SetForegroundWindow(hwnd);
        let picked = TrackPopupMenu(
            popup,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            at.x,
            at.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(popup);
        picked.0 as u32
    };

    if chosen != 0 {
        if let Some(command) = menu::find(&entries, chosen).cloned() {
            perform(hwnd, command);
        }
    }
}

/// Build a native menu from the model. Submenus are destroyed with the parent.
fn append(menu: HMENU, entries: &[Entry]) {
    for entry in entries {
        match entry {
            Entry::Item { id, text, .. } => add(menu, MF_STRING, *id as usize, text),
            Entry::Note { text } => add(menu, MF_STRING | MF_GRAYED, 0, text),
            Entry::Separator => unsafe {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            },
            Entry::Submenu { text, entries } => unsafe {
                if let Ok(sub) = CreatePopupMenu() {
                    append(sub, entries);
                    add(menu, MF_STRING | MF_POPUP, sub.0 as usize, text);
                }
            },
        }
    }
}

fn add(menu: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str) {
    let text = wide(text);
    unsafe {
        let _ = AppendMenuW(menu, flags, id, PCWSTR(text.as_ptr()));
    }
}

fn perform(hwnd: HWND, command: Command) {
    match command {
        Command::Quit => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        Command::OpenSettings => {
            let started = sibling("desktop-switcher-gui.exe")
                .ok_or_else(|| "desktop-switcher-gui.exe was not found".to_string())
                .and_then(|gui| Process::new(gui).spawn().map_err(|e| e.to_string()));
            if let Err(why) = started {
                show_notice(hwnd, &notice::cannot_start("Settings", &why));
            }
        }
        Command::Cli { args, describe } => {
            let Some(cli) = sibling("desktop-switcher.exe") else {
                show_notice(
                    hwnd,
                    &notice::cannot_start(&describe, "desktop-switcher.exe was not found"),
                );
                return;
            };
            // Off the window thread: a switch can take a few seconds, and the
            // icon must stay responsive meanwhile.
            let window = hwnd.0 as isize;
            std::thread::spawn(move || {
                let run = Process::new(cli)
                    .args(&args)
                    .creation_flags(CREATE_NO_WINDOW)
                    .output();
                let found = match run {
                    Ok(out) => {
                        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                        text.push('\n');
                        text.push_str(&String::from_utf8_lossy(&out.stderr));
                        notice::for_run(&describe, out.status.code(), &text)
                    }
                    Err(e) => Some(notice::cannot_start(&describe, &e.to_string())),
                };
                if let Some(found) = found {
                    post_notice(window, found);
                }
            });
        }
    }
}

/// Give a notice to the window thread, which shows it and frees it.
fn post_notice(window: isize, found: Notice) {
    let raw = Box::into_raw(Box::new(found));
    let posted = unsafe {
        PostMessageW(
            Some(HWND(window as *mut c_void)),
            WM_NOTICE,
            WPARAM(0),
            LPARAM(raw as isize),
        )
    };
    if posted.is_err() {
        // Nobody will receive it, so take it back rather than leak it.
        drop(unsafe { Box::from_raw(raw) });
    }
}

/// A program installed next to this one, or failing that, on PATH.
fn sibling(name: &str) -> Option<PathBuf> {
    if let Ok(mut here) = std::env::current_exe() {
        here.pop();
        let candidate = here.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// An icon from pixels drawn in code: a 32-bit colour bitmap with alpha, and
/// the monochrome mask Windows still insists on.
fn make_icon(size: u32) -> windows::core::Result<HICON> {
    let rgba = icon::pixels(size);
    unsafe {
        let header = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size as i32,
            // Negative height: rows run top to bottom, as the pixels do.
            biHeight: -(size as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let info = BITMAPINFO {
            bmiHeader: header,
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let colour = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let target = std::slice::from_raw_parts_mut(bits as *mut u8, rgba.len());
        for (to, from) in target.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
            to[0] = from[2];
            to[1] = from[1];
            to[2] = from[0];
            to[3] = from[3];
        }

        // Rows of a monochrome bitmap are padded to 16 bits.
        let mask_bits = vec![0u8; (size.div_ceil(16) * 2 * size) as usize];
        let mask = CreateBitmap(
            size as i32,
            size as i32,
            1,
            1,
            Some(mask_bits.as_ptr() as *const c_void),
        );

        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: BOOL::from(true),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: colour,
        });
        let _ = DeleteObject(colour.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}
