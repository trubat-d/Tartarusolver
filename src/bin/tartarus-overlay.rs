//! Tartarus overlay: watches the game log, reads the game window when Tartarus changes, asks the Solver and draws a
//! box around the best choice. It only reads the screen and the log: it never touches the game.
//!
//! The console says at start what's found and what isn't, then a block per decision. Type `help` there for the
//! commands (Rumor, objective, instruments). What the screen can't show is saved after every change, so the
//! overlay can be closed and started again mid-run.

// The lite build (`--features lite`, tartarus-overlay-lite.exe): no console, nothing written to disk, only the boxes.
#![cfg_attr(all(windows, feature = "lite"), windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("tartarus-overlay runs on Windows, beside the game.");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    if let Err(e) = win::run() {
        if cfg!(feature = "lite") {
            win::message(&format!("Tartarus overlay can't start: {e}"));
            std::process::exit(1);
        }
        eprintln!("{e}");
        eprintln!("Press Enter to close.");
        let _ = std::io::stdin().read_line(&mut String::new());
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod win {
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use tartarus::advisor::{advise, Advice, Popup, Rect, RunState};
    use tartarus::instruments::Instrument;
    use tartarus::reader::Frame;
    use tartarus::view::View;

    /// The UI scale in a log line ("PageViewSetUIScale ... PageScale = 1 0.640 0.64": the last number).
    fn ui_scale_in(line: &str) -> Option<f32> {
        if !line.contains("PageViewSetUIScale") {
            return None;
        }
        line.split("PageScale =")
            .nth(1)?
            .split_whitespace()
            .last()?
            .parse()
            .ok()
    }
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
        MOD_SHIFT, MOD_WIN,
    };
    use tartarus::settings::Settings;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const VERSION: &str = env!("CARGO_PKG_VERSION");
    const LOG_IN_GAME: &str =
        r"steamapps\common\Torchlight Infinite\UE_game\TorchLight\Saved\Logs\UE_game.log";
    /// How long after a log line to read the screen, so its animations have settled.
    /// First look after a click: soon, since the screen is read again until it holds still.
    const SETTLE: Duration = Duration::from_millis(300);
    /// Looks again this soon after a picture, until two in a row agree.
    const RECHECK: Duration = Duration::from_millis(350);
    /// Futures per option: about a second of work on a usual PC.
    const FUTURES: usize = 2000;
    /// A look's time budget (about half a second), and the fewest futures a look plays.
    const LOOK_MS: f64 = 450.0;
    const MIN_FUTURES: usize = 300;
    /// Looks pooled at one position before it's no longer solved again.
    const SETTLED_LOOKS: f64 = 6.0;
    const HK_BOXES: i32 = 1;
    const HK_REDO: i32 = 2;
    const HK_QUIT: i32 = 3;
    /// The tray icon's messages to the window.
    const WM_TRAY: u32 = WM_APP + 1;
    const MENU_BOXES: usize = 1;
    const MENU_QUIT: usize = 2;
    const MENU_SETTINGS: usize = 3;
    const MENU_TERMS: usize = 4;
    static TRAY_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

    /// The icon in the taskbar's corner (notification area): right- or left-click for Boxes on/off and Quit.
    unsafe fn add_tray(hwnd: HWND, instance: HINSTANCE) {
        use windows::Win32::UI::Shell::*;
        let icon = LoadIconW(instance, PCWSTR(1 as *const u16))
            .or_else(|_| LoadIconW(HINSTANCE::default(), IDI_APPLICATION))
            .unwrap_or_default();
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: icon,
            ..Default::default()
        };
        for (k, c) in "Tartarus overlay (click: boxes on/off, quit)"
            .encode_utf16()
            .take(nid.szTip.len() - 1)
            .enumerate()
        {
            nid.szTip[k] = c;
        }
        if Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            TRAY_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Quits, taking the tray icon away first (else it stays until the mouse passes over it).
    fn quit() -> ! {
        let h = TRAY_HWND.load(std::sync::atomic::Ordering::SeqCst);
        if h != 0 {
            unsafe {
                use windows::Win32::UI::Shell::*;
                let nid = NOTIFYICONDATAW {
                    cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                    hWnd: HWND(h as *mut core::ffi::c_void),
                    uID: 1,
                    ..Default::default()
                };
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
        }
        std::process::exit(0)
    }

    /// The hotkeys from the settings, registered on the overlay window (again after a change).
    unsafe fn register_hotkeys(hwnd: HWND) {
        let st = settings().lock().unwrap().clone();
        for (id, key) in [(HK_BOXES, &st.boxes_key), (HK_REDO, &st.redo_key), (HK_QUIT, &st.quit_key)] {
            let _ = UnregisterHotKey(hwnd, id);
            let Some((ctrl, alt, shift, win, vk)) = tartarus::settings::parse_hotkey(key) else {
                continue;
            };
            let mut m = MOD_NOREPEAT.0;
            if ctrl {
                m |= MOD_CONTROL.0;
            }
            if alt {
                m |= MOD_ALT.0;
            }
            if shift {
                m |= MOD_SHIFT.0;
            }
            if win {
                m |= MOD_WIN.0;
            }
            if RegisterHotKey(hwnd, id, HOT_KEY_MODIFIERS(m), vk).is_err() {
                say_line(format!("  [!] hotkey {key} is taken by another program (change it in Settings, from the tray icon)"));
            }
        }
    }

    // ---------- the settings window (from the tray icon) ----------

    const ID_LOG: i32 = 101;
    const ID_BOXES: i32 = 102;
    const ID_REDO: i32 = 103;
    const ID_QUIT: i32 = 104;
    const ID_COLOR: i32 = 105;
    const ID_BROWSE: i32 = 110;
    const ID_AUTO: i32 = 111;
    const ID_PICK: i32 = 112;
    const ID_SAVE: i32 = 113;
    const ID_CANCEL: i32 = 114;
    static SETTINGS_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

    thread_local! {
        static CONTROLS: std::cell::RefCell<std::collections::HashMap<i32, isize>> = Default::default();
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn ctrl(id: i32) -> HWND {
        HWND(CONTROLS.with(|c| c.borrow().get(&id).copied().unwrap_or(0)) as *mut core::ffi::c_void)
    }

    unsafe fn text_of(h: HWND) -> String {
        let n = GetWindowTextLengthW(h).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(h, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..got.min(n)])
    }

    unsafe fn set_text(h: HWND, t: &str) {
        let w = wide(t);
        let _ = SetWindowTextW(h, PCWSTR(w.as_ptr()));
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn add_control(parent: HWND, class: PCWSTR, text: &str, style: u32, edit: bool, x: i32, y: i32, w: i32, h: i32, id: i32) {
        let t = wide(text);
        let instance: HINSTANCE = GetModuleHandleW(None).map(|m| m.into()).unwrap_or_default();
        let c = CreateWindowExW(
            if edit { WS_EX_CLIENTEDGE } else { WINDOW_EX_STYLE(0) },
            class,
            PCWSTR(t.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
            x,
            y,
            w,
            h,
            parent,
            HMENU(id as isize as *mut core::ffi::c_void),
            instance,
            None,
        )
        .unwrap_or_default();
        let font = GetStockObject(DEFAULT_GUI_FONT);
        SendMessageW(c, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
        CONTROLS.with(|m| m.borrow_mut().insert(id, c.0 as isize));
    }

    fn open_settings() {
        unsafe {
            let open = SETTINGS_HWND.load(std::sync::atomic::Ordering::SeqCst);
            if open != 0 {
                let _ = SetForegroundWindow(HWND(open as *mut core::ffi::c_void));
                return;
            }
            let instance: HINSTANCE = GetModuleHandleW(None).map(|m| m.into()).unwrap_or_default();
            static CLASS: OnceLock<()> = OnceLock::new();
            CLASS.get_or_init(|| {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(settings_proc),
                    hInstance: instance,
                    lpszClassName: w!("TartarusSettings"),
                    hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as isize as *mut core::ffi::c_void),
                    hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
                    ..Default::default()
                };
                RegisterClassW(&wc);
            });
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOPMOST,
                w!("TartarusSettings"),
                w!("Tartarus overlay settings"),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                500,
                400,
                HWND::default(),
                HMENU::default(),
                instance,
                None,
            ) else {
                return;
            };
            SETTINGS_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
            dark_title(hwnd);
            let st = settings().lock().unwrap().clone();
            let (label, edit, button) = (w!("STATIC"), w!("EDIT"), w!("BUTTON"));
            let (es, tab) = (0x80 | 0x0001_0000, 0x0001_0000); // ES_AUTOHSCROLL | WS_TABSTOP, WS_TABSTOP
            add_control(hwnd, label, "Game log (leave empty: found through Steam)", 0, false, 16, 66, 440, 18, 0);
            add_control(hwnd, edit, st.log.as_deref().unwrap_or(""), es, true, 16, 86, 300, 24, ID_LOG);
            add_control(hwnd, button, "Browse...", tab, false, 322, 86, 76, 24, ID_BROWSE);
            add_control(hwnd, button, "Auto", tab, false, 404, 86, 60, 24, ID_AUTO);
            for (k, (name, value, id)) in [
                ("Boxes on/off", &st.boxes_key, ID_BOXES),
                ("Read the screen again", &st.redo_key, ID_REDO),
                ("Quit", &st.quit_key, ID_QUIT),
            ]
            .into_iter()
            .enumerate()
            {
                let y = 128 + 32 * k as i32;
                add_control(hwnd, label, name, 0, false, 16, y + 4, 150, 18, 0);
                add_control(hwnd, edit, value, es, true, 170, y, 150, 24, id);
            }
            add_control(hwnd, label, "Box color (#RRGGBB)", 0, false, 16, 228, 150, 18, 0);
            add_control(hwnd, edit, &tartarus::settings::format_color(st.color), es, true, 170, 224, 100, 24, ID_COLOR);
            add_control(hwnd, button, "Pick...", tab, false, 276, 224, 70, 24, ID_PICK);
            add_control(hwnd, label, "Hotkeys: Ctrl, Alt, Shift or Win + a letter, a digit or F1-F24 (e.g. Ctrl+Alt+B, F8)", 0, false, 16, 264, 460, 18, 0);
            add_control(hwnd, button, "Save", tab, false, 290, 304, 84, 28, ID_SAVE);
            add_control(hwnd, button, "Cancel", tab, false, 382, 304, 84, 28, ID_CANCEL);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }

    /// The dark window's brushes, made once: 0 the background, 1 the entry fields.
    unsafe fn dark_brush(k: usize) -> HBRUSH {
        static B: [std::sync::atomic::AtomicIsize; 2] =
            [std::sync::atomic::AtomicIsize::new(0), std::sync::atomic::AtomicIsize::new(0)];
        let colors = [0x0020_1A18u32, 0x0034_2E2Bu32];
        if B[k].load(std::sync::atomic::Ordering::Relaxed) == 0 {
            B[k].store(CreateSolidBrush(COLORREF(colors[k])).0 as isize, std::sync::atomic::Ordering::Relaxed);
        }
        HBRUSH(B[k].load(std::sync::atomic::Ordering::Relaxed) as *mut core::ffi::c_void)
    }

    unsafe fn browse_log(owner: HWND) -> Option<String> {
        use windows::Win32::UI::Controls::Dialogs::*;
        let filter: Vec<u16> = "Game log (UE_game.log)\0UE_game.log\0Log files (*.log)\0*.log\0All files\0*.*\0\0"
            .encode_utf16()
            .collect();
        let mut file = [0u16; 1024];
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: owner,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: windows::core::PWSTR(file.as_mut_ptr()),
            nMaxFile: file.len() as u32,
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        if GetOpenFileNameW(&mut ofn).as_bool() {
            let n = file.iter().position(|&c| c == 0).unwrap_or(file.len());
            Some(String::from_utf16_lossy(&file[..n]))
        } else {
            None
        }
    }

    unsafe fn pick_color(owner: HWND) {
        use windows::Win32::UI::Controls::Dialogs::*;
        let c = tartarus::settings::parse_color(&text_of(ctrl(ID_COLOR)))
            .unwrap_or(settings().lock().unwrap().color);
        let mut custom = [COLORREF(0x00FF_FFFF); 16];
        let mut cc = CHOOSECOLORW {
            lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: owner,
            rgbResult: COLORREF(c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16),
            lpCustColors: custom.as_mut_ptr(),
            Flags: CC_RGBINIT | CC_FULLOPEN,
            ..Default::default()
        };
        if ChooseColorW(&mut cc).as_bool() {
            let v = cc.rgbResult.0;
            set_text(ctrl(ID_COLOR), &tartarus::settings::format_color([v as u8, (v >> 8) as u8, (v >> 16) as u8]));
        }
    }

    unsafe fn save_from_window(hwnd: HWND) {
        let keys = [text_of(ctrl(ID_BOXES)), text_of(ctrl(ID_REDO)), text_of(ctrl(ID_QUIT))];
        if let Some(bad) = keys.iter().find(|k| tartarus::settings::parse_hotkey(k).is_none()) {
            let t = wide(&format!("\"{bad}\" isn't a hotkey I can use: Ctrl, Alt, Shift or Win + a letter, a digit or F1-F24 (e.g. Ctrl+Alt+B)."));
            MessageBoxW(hwnd, PCWSTR(t.as_ptr()), w!("Tartarus overlay"), MB_OK | MB_ICONWARNING);
            return;
        }
        let Some(color) = tartarus::settings::parse_color(&text_of(ctrl(ID_COLOR))) else {
            MessageBoxW(hwnd, w!("The box color is written #RRGGBB, e.g. #28FF5A (or use Pick...)."), w!("Tartarus overlay"), MB_OK | MB_ICONWARNING);
            return;
        };
        let log = text_of(ctrl(ID_LOG)).trim().to_string();
        if !log.is_empty() && !Path::new(&log).is_file() {
            MessageBoxW(hwnd, w!("That game log doesn't exist: pick UE_game.log with Browse... (or Auto)."), w!("Tartarus overlay"), MB_OK | MB_ICONWARNING);
            return;
        }
        let cur = settings().lock().unwrap().clone();
        let new = Settings {
            hide_notice: cur.hide_notice,
            accepted: cur.accepted.clone(),
            log: (!log.is_empty()).then_some(log),
            boxes_key: keys[0].trim().to_string(),
            redo_key: keys[1].trim().to_string(),
            quit_key: keys[2].trim().to_string(),
            color,
        };
        let old = std::mem::replace(&mut *settings().lock().unwrap(), new.clone());
        save_settings(&new);
        let overlay = TRAY_HWND.load(std::sync::atomic::Ordering::SeqCst);
        if overlay != 0 {
            register_hotkeys(HWND(overlay as *mut core::ffi::c_void));
        }
        if old.log != new.log {
            if let Some(p) = new.log.clone().map(PathBuf::from).or_else(find_log) {
                let _ = TO_WORKER.get().map(|t| t.send(Msg::Log(p)));
            }
        }
        shared().lock().unwrap().changed = true;
        let _ = DestroyWindow(hwnd);
    }

    unsafe extern "system" fn settings_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_COMMAND if (wp.0 >> 16) & 0xFFFF == 0 => {
                match (wp.0 & 0xFFFF) as i32 {
                    ID_BROWSE => {
                        if let Some(p) = browse_log(hwnd) {
                            set_text(ctrl(ID_LOG), &p);
                        }
                    }
                    ID_AUTO => set_text(ctrl(ID_LOG), ""),
                    ID_PICK => pick_color(hwnd),
                    ID_SAVE => save_from_window(hwnd),
                    ID_CANCEL => {
                        let _ = DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_PAINT => {
                // the notice's look: dark, a green bar along the top, a title
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                FillRect(hdc, &rc, dark_brush(0));
                let accent = CreateSolidBrush(COLORREF(0x005A_FF28));
                FillRect(hdc, &RECT { left: 0, top: 0, right: rc.right, bottom: 5 }, accent);
                let _ = DeleteObject(accent);
                let mut lf = LOGFONTW { lfHeight: -24, lfWeight: 600, ..Default::default() };
                for (k, c) in "Segoe UI".encode_utf16().enumerate() {
                    lf.lfFaceName[k] = c;
                }
                let f = CreateFontIndirectW(&lf);
                let old = SelectObject(hdc, f);
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, COLORREF(0x00F0_F0F0));
                let mut t: Vec<u16> = "Settings".encode_utf16().collect();
                let mut r = RECT { left: 16, top: 14, right: rc.right - 16, bottom: 50 };
                DrawTextW(hdc, &mut t, &mut r, DT_LEFT | DT_NOPREFIX | DT_SINGLELINE);
                SelectObject(hdc, old);
                let _ = DeleteObject(f);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
                // labels on the dark background, entry fields a shade lighter, light text
                let hdc = HDC(wp.0 as *mut core::ffi::c_void);
                let field = msg == WM_CTLCOLOREDIT;
                SetTextColor(hdc, COLORREF(if field { 0x00F0_F0F0 } else { 0x00DC_DCDC }));
                SetBkColor(hdc, COLORREF(if field { 0x0034_2E2B } else { 0x0020_1A18 }));
                LRESULT(dark_brush(field as usize).0 as isize)
            }
            WM_DESTROY => {
                SETTINGS_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
                CONTROLS.with(|c| c.borrow_mut().clear());
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    // ---------- the notice at start ----------

    const ID_AGREE: i32 = 201;
    const ID_LEAVE: i32 = 202;
    const ID_HIDE: i32 = 203;
    const ID_START: i32 = 204;
    /// The notice opened from the tray icon, to read only.
    static NOTICE_VIEW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    /// The notice's page: 0 the notice, 1 how to use it (after I accept).
    static NOTICE_PAGE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

    fn ready_text() -> Vec<(bool, String)> {
        let st = settings().lock().unwrap().clone();
        vec![
            (true, "While you play".into()),
            (false, "A box marks the best choice: where to go, Consume / Replace / Hunt / End, which Inmate to eat, which instrument or treasure to take. In the bottom-left corner you see what that choice is expected to bring (Chromatics, and your chance of reaching 10), with a bar showing the time until the next box.".into()),
            (true, "Shortcuts".into()),
            (false, format!("{}   boxes on / off\n{}   read the screen again\n{}   quit", st.boxes_key, st.redo_key, st.quit_key)),
            (true, "The icon next to the clock".into()),
            (false, "Click it for Boxes on/off, Settings... (game log, shortcuts, box color) and Quit. It may be hidden under the ^ arrow. The overlay also quits on its own a couple of minutes after the game closes.".into()),
        ]
    }
    /// 0 while the notice is up, 1 understood, 2 quit.
    static NOTICE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

    fn notice_text() -> Vec<(bool, String)> {
        let saved = if LITE {
            "Nothing is sent anywhere. Only your settings are kept on this computer."
        } else {
            "Nothing is sent anywhere. This full version keeps screenshots and a journal of your runs in Documents\\Tartarus overlay on this computer, to improve the advice; delete that folder whenever you like."
        };
        vec![
            (true, "What it does".into()),
            (false, "Tartarus overlay reads the game's picture on your screen and its log file, plays thousands of possible futures for each choice in a simulation of the minigame's rules, and draws a box around the choice that ends best on average.".into()),
            (true, "Purely an overlay".into()),
            (false, "It only looks and shows. The boxes are drawn in a separate, see-through window on top of the game, like a note held over your screen. It never sends a click, a key press or any other input to the game, never plays or automates a move, and never reads or changes the game's memory, files or network traffic. Every decision and every click is yours.".into()),
            (true, "Not affiliated".into()),
            (false, "An independent fan project. It is not affiliated with, endorsed, sponsored or approved by the developers or publishers of Torchlight: Infinite. Torchlight: Infinite and its names and images belong to their respective owners.".into()),
            (true, "Processor use".into()),
            (false, "For about half a second after each choice it calculates on all of your processor's cores, at low priority so the game comes first. Your fans may spin up briefly: that is expected.".into()),
            (true, "No guarantee".into()),
            (false, format!("The advice comes from a simulation of the rules as they are known, and can be wrong; a good choice can still end badly. The program is provided as is, without warranty of any kind, and you use it at your own risk. Its author is not liable for any loss or damage, in the game or otherwise, nor for any action the game's publisher may take on your account. Whether a tool like this is allowed is set by the game's terms of service, which you should check yourself. {saved}")),
        ]
    }

    unsafe extern "system" fn notice_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let bg = COLORREF(0x0020_1A18); // a dark warm grey (BGR)
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let brush = CreateSolidBrush(bg);
                FillRect(hdc, &rc, brush);
                let _ = DeleteObject(brush);
                // a green bar along the top, the overlay's color
                let accent = CreateSolidBrush(COLORREF(0x005A_FF28));
                let bar = RECT { left: 0, top: 0, right: rc.right, bottom: 5 };
                FillRect(hdc, &bar, accent);
                let _ = DeleteObject(accent);
                let font = |height: i32, weight: i32| {
                    let mut lf = LOGFONTW { lfHeight: height, lfWeight: weight, ..Default::default() };
                    for (k, c) in "Segoe UI".encode_utf16().enumerate() {
                        lf.lfFaceName[k] = c;
                    }
                    CreateFontIndirectW(&lf)
                };
                let (title, head, body) = (font(-28, 600), font(-17, 600), font(-15, 400));
                SetBkMode(hdc, TRANSPARENT);
                let mut y = 22;
                let draw = |f: HFONT, color: u32, text: &str, y: &mut i32, gap: i32| {
                    let old = SelectObject(hdc, f);
                    SetTextColor(hdc, COLORREF(color));
                    let mut t: Vec<u16> = text.encode_utf16().collect();
                    let mut r = RECT { left: 28, top: *y, right: rc.right - 28, bottom: rc.bottom };
                    let h = DrawTextW(hdc, &mut t, &mut r, DT_WORDBREAK | DT_LEFT | DT_NOPREFIX);
                    SelectObject(hdc, old);
                    *y += h + gap;
                };
                let ready = NOTICE_PAGE.load(std::sync::atomic::Ordering::SeqCst) == 1;
                if ready {
                    draw(title, 0x00F0_F0F0, "You're all set", &mut y, 2);
                    draw(body, 0x00A0_A0A0, "Start Tartarus Prison: the boxes appear on their own.", &mut y, 14);
                } else {
                    draw(title, 0x00F0_F0F0, "Tartarus overlay", &mut y, 2);
                    draw(body, 0x00A0_A0A0, &format!("Version {VERSION} - advice for the Tartarus Prison minigame"), &mut y, 14);
                }
                for (is_head, text) in if ready { ready_text() } else { notice_text() } {
                    if is_head {
                        draw(head, 0x005A_FF28, &text, &mut y, 2);
                    } else {
                        draw(body, 0x00DC_DCDC, &text, &mut y, 12);
                    }
                }
                for f in [title, head, body] {
                    let _ = DeleteObject(f);
                }
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_CTLCOLORSTATIC => {
                // the check box on the dark background
                let hdc = HDC(wp.0 as *mut core::ffi::c_void);
                SetTextColor(hdc, COLORREF(0x00DC_DCDC));
                SetBkColor(hdc, bg);
                static BRUSH: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
                if BRUSH.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                    BRUSH.store(CreateSolidBrush(bg).0 as isize, std::sync::atomic::Ordering::Relaxed);
                }
                LRESULT(BRUSH.load(std::sync::atomic::Ordering::Relaxed))
            }
            WM_COMMAND if (wp.0 >> 16) & 0xFFFF == 0 => {
                match (wp.0 & 0xFFFF) as i32 {
                    ID_AGREE => {
                        let hide = SendMessageW(ctrl(ID_HIDE), BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == 1;
                        {
                            let mut st = settings().lock().unwrap();
                            st.hide_notice = hide;
                            st.accepted = Some(VERSION.to_string());
                            save_settings(&st);
                        }
                        // the same window turns to how to use it
                        for id in [ID_AGREE, ID_LEAVE, ID_HIDE] {
                            let _ = DestroyWindow(ctrl(id));
                        }
                        NOTICE_PAGE.store(1, std::sync::atomic::Ordering::SeqCst);
                        let mut rc = RECT::default();
                        let _ = GetClientRect(hwnd, &mut rc);
                        add_control(hwnd, w!("BUTTON"), "Start", 0x0001_0001, false, rc.right - 150, rc.bottom - 62, 120, 36, ID_START);
                        let _ = InvalidateRect(hwnd, None, true);
                    }
                    ID_START => {
                        if !NOTICE_VIEW.load(std::sync::atomic::Ordering::SeqCst) {
                            NOTICE.store(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        let _ = DestroyWindow(hwnd);
                    }
                    ID_LEAVE => {
                        NOTICE.store(2, std::sync::atomic::Ordering::SeqCst);
                        let _ = DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                // closing the notice quits; closing the how-to page (already accepted) starts
                // (read from the tray icon: just closes)
                if !NOTICE_VIEW.load(std::sync::atomic::Ordering::SeqCst) {
                    let accepted = NOTICE_PAGE.load(std::sync::atomic::Ordering::SeqCst) == 1;
                    NOTICE.store(if accepted { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
                }
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    /// A dark title bar (Windows 10 20H1 and later; ignored elsewhere), to match the dark window.
    unsafe fn dark_title(hwnd: HWND) {
        let on: i32 = 1;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(20),
            &on as *const i32 as *const core::ffi::c_void,
            4,
        );
    }

    /// The notice at start (unless hidden in the settings): true to go on, false to quit.
    /// The notice window: for accepting at start, or to read again from the tray icon (`view`: a Close button only).
    unsafe fn create_notice(view: bool) -> Option<HWND> {
        let instance: HINSTANCE = GetModuleHandleW(None).map(|m| m.into()).unwrap_or_default();
        static CLASS: OnceLock<()> = OnceLock::new();
        CLASS.get_or_init(|| {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(notice_proc),
                hInstance: instance,
                lpszClassName: w!("TartarusNotice"),
                hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
                hIcon: LoadIconW(instance, PCWSTR(1 as *const u16)).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        NOTICE_VIEW.store(view, std::sync::atomic::Ordering::SeqCst);
        NOTICE_PAGE.store(0, std::sync::atomic::Ordering::SeqCst);
        let (w, h) = (680, 820.min(GetSystemMetrics(SM_CYSCREEN) - 60).max(600));
        let x = (GetSystemMetrics(SM_CXSCREEN) - w) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - h) / 2;
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST,
            w!("TartarusNotice"),
            w!("Tartarus overlay"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            x.max(0),
            y.max(0),
            w,
            h,
            HWND::default(),
            HMENU::default(),
            instance,
            None,
        )
        .ok()?;
        dark_title(hwnd);
        let button = w!("BUTTON");
        if view {
            add_control(hwnd, button, "Close", 0x0001_0001, false, w - 150, h - 116, 110, 34, ID_START);
        } else {
            add_control(hwnd, button, "Don't show this again for this version", 3 | 0x0001_0000, false, 28, h - 110, 320, 24, ID_HIDE);
            add_control(hwnd, button, "I accept", 0x0001_0001, false, w - 290, h - 116, 130, 34, ID_AGREE);
            add_control(hwnd, button, "Quit", 0x0001_0000, false, w - 150, h - 116, 110, 34, ID_LEAVE);
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        Some(hwnd)
    }

    /// The notice at start: shown unless this version's terms were accepted with "Don't show this again" ticked (a
    /// new version asks again). True to go on, false to quit.
    fn show_notice() -> bool {
        {
            let st = settings().lock().unwrap();
            if st.hide_notice && st.accepted.as_deref() == Some(VERSION) {
                return true;
            }
        }
        unsafe {
            if create_notice(false).is_none() {
                return true;
            }
            let mut msg = MSG::default();
            while NOTICE.load(std::sync::atomic::Ordering::SeqCst) == 0
                && GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool()
            {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        NOTICE.load(std::sync::atomic::Ordering::SeqCst) == 1
    }

    unsafe fn tray_menu(hwnd: HWND) {
        let Ok(m) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(m, MF_STRING, MENU_BOXES, w!("Boxes on/off"));
        let _ = AppendMenuW(m, MF_STRING, MENU_SETTINGS, w!("Settings..."));
        let _ = AppendMenuW(m, MF_STRING, MENU_TERMS, w!("Terms of use..."));
        let _ = AppendMenuW(m, MF_STRING, MENU_QUIT, w!("Quit Tartarus overlay"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // the menu closes when clicking elsewhere only if the window is in front
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            m,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            0,
            hwnd,
            None,
        );
        let _ = DestroyMenu(m);
        match cmd.0 as usize {
            MENU_QUIT => quit(),
            MENU_SETTINGS => open_settings(),
            MENU_TERMS => {
                let _ = create_notice(true);
            }
            MENU_BOXES => {
                let _ = TO_WORKER
                    .get()
                    .map(|t| t.send(Msg::Command("boxes".into())));
            }
            _ => {}
        }
    }

    enum Msg {
        Command(String),
        Redo,
        /// Read this game log from now on (chosen in the settings).
        Log(PathBuf),
    }

    static TO_WORKER: OnceLock<Sender<Msg>> = OnceLock::new();

    /// What the window draws: the boxes (game picture coordinates) and where the game's picture is on screen.
    #[derive(Default)]
    struct Shared {
        boxes: Vec<Rect>,
        game: Option<(i32, i32, i32, i32)>,
        show: bool,
        changed: bool,
        /// The line drawn at the top: the boxed option's expected Chromatics and chance of 10.
        line: Option<String>,
        /// Solving a new screen: the boxes are hidden and a progress bar shows instead.
        busy: bool,
        /// When the solving started and how long it should take (ms), for a bar that moves with time.
        busy_since: Option<Instant>,
        busy_ms: f64,
    }

    static SHARED: OnceLock<Arc<Mutex<Shared>>> = OnceLock::new();

    fn shared() -> &'static Arc<Mutex<Shared>> {
        SHARED.get_or_init(|| {
            Arc::new(Mutex::new(Shared {
                show: true,
                ..Default::default()
            }))
        })
    }

    fn set_boxes(b: Vec<Rect>) {
        let mut s = shared().lock().unwrap();
        if s.boxes != b {
            s.boxes = b;
            s.changed = true;
        }
    }

    fn set_busy(b: bool) {
        let mut s = shared().lock().unwrap();
        if s.busy != b {
            s.busy = b;
            s.busy_since = b.then(Instant::now);
            s.changed = true;
        }
    }

    /// How long the solving now going on should take (the bar's pace).
    fn set_busy_ms(ms: f64) {
        shared().lock().unwrap().busy_ms = ms;
    }

    /// The bar's fill: steady with time up to 90% at the expected end, then creeping on (never quite full) if it runs
    /// late.
    fn bar_fill(since: Option<Instant>, ms: f64) -> f64 {
        let t = since.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0) / ms.max(50.0);
        if t < 1.0 {
            0.9 * t
        } else {
            0.9 + 0.1 * (1.0 - (-(t - 1.0) * 1.5).exp())
        }
    }

    fn set_line(l: Option<String>) {
        let mut s = shared().lock().unwrap();
        if s.line != l {
            s.line = l;
            s.changed = true;
        }
    }

    /// The solver's threads, all at below-normal priority so the game comes first: every thread for the first looks
    /// after a click (the box appears fast), a quarter of them (env TARTARUS_THREADS) for the later looks that only
    /// sharpen the estimates while the screen stays up.
    fn pool(fast: bool) -> &'static rayon::ThreadPool {
        static FAST: OnceLock<rayon::ThreadPool> = OnceLock::new();
        static SLOW: OnceLock<rayon::ThreadPool> = OnceLock::new();
        let all = std::thread::available_parallelism().map_or(4, |n| n.get());
        let build = |n: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n.max(2))
                .start_handler(|_| unsafe {
                    use windows::Win32::System::Threading::*;
                    let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
                })
                .build()
                .expect("solver threads")
        };
        if fast {
            FAST.get_or_init(|| build(all))
        } else {
            SLOW.get_or_init(|| {
                build(
                    std::env::var("TARTARUS_THREADS")
                        .ok()
                        .and_then(|t| t.parse().ok())
                        .unwrap_or(all / 4),
                )
            })
        }
    }

    // ---------- the console and its log file ----------

    static DIAG: Mutex<Option<std::fs::File>> = Mutex::new(None);

    /// The lite build: no console, no files, only the boxes.
    pub const LITE: bool = cfg!(feature = "lite");

    /// A message box (the lite build has no console to say things in).
    pub fn message(text: &str) {
        let t: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            MessageBoxW(
                HWND::default(),
                PCWSTR(t.as_ptr()),
                w!("Tartarus overlay"),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }

    macro_rules! say {
        ($($t:tt)*) => {{
            let line = format!($($t)*);
            if !LITE {
                println!("{line}");
            }
            if let Ok(mut g) = DIAG.lock() {
                if let Some(f) = g.as_mut() {
                    let _ = writeln!(f, "{line}");
                    let _ = f.flush();
                }
            }
        }};
    }

    /// `say!` for code written above the macro.
    fn say_line(line: String) {
        say!("{line}");
    }

    fn now_parts() -> (i32, u32, u32, u32, u32, u32) {
        let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        (
            st.wYear as i32,
            st.wMonth as u32,
            st.wDay as u32,
            st.wHour as u32,
            st.wMinute as u32,
            st.wSecond as u32,
        )
    }

    fn clock() -> String {
        let (_, _, _, h, mi, s) = now_parts();
        format!("{h:02}:{mi:02}:{s:02}")
    }

    fn quick_edit_off() {
        use windows::Win32::System::Console::*;
        unsafe {
            if let Ok(h) = GetStdHandle(STD_INPUT_HANDLE) {
                let mut mode = CONSOLE_MODE(0);
                if GetConsoleMode(h, &mut mode).is_ok() {
                    let _ = SetConsoleMode(
                        h,
                        CONSOLE_MODE(
                            (mode.0 & !ENABLE_QUICK_EDIT_MODE.0) | ENABLE_EXTENDED_FLAGS.0,
                        ),
                    );
                }
            }
        }
    }

    // ---------- finding things ----------

    fn reg_string(root: HKEY, key: PCWSTR, value: PCWSTR) -> Option<String> {
        let mut buf = [0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let err = unsafe {
            RegGetValueW(
                root,
                key,
                value,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut _),
                Some(&mut len),
            )
        };
        if err.is_err() {
            return None;
        }
        Some(String::from_utf16_lossy(
            &buf[..(len as usize / 2).saturating_sub(1)],
        ))
    }

    /// The newest UE_game.log among the Steam libraries.
    fn find_log() -> Option<PathBuf> {
        let mut roots: Vec<PathBuf> = Vec::new();
        for (root, key, value) in [
            (
                HKEY_CURRENT_USER,
                w!(r"Software\Valve\Steam"),
                w!("SteamPath"),
            ),
            (
                HKEY_LOCAL_MACHINE,
                w!(r"SOFTWARE\WOW6432Node\Valve\Steam"),
                w!("InstallPath"),
            ),
        ] {
            if let Some(p) = reg_string(root, key, value) {
                roots.push(PathBuf::from(p.replace('/', "\\")));
            }
        }
        roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
        let mut libraries = roots.clone();
        for r in &roots {
            if let Ok(vdf) = std::fs::read_to_string(r.join("steamapps").join("libraryfolders.vdf"))
            {
                for line in vdf.lines() {
                    if let Some(rest) = line.trim().strip_prefix("\"path\"") {
                        libraries.push(PathBuf::from(
                            rest.trim().trim_matches('"').replace("\\\\", "\\"),
                        ));
                    }
                }
            }
        }
        libraries
            .into_iter()
            .map(|l| l.join(LOG_IN_GAME))
            .filter_map(|p| {
                std::fs::metadata(&p)
                    .ok()
                    .filter(|m| m.is_file())
                    .map(|m| (m.modified().ok(), p))
            })
            .max_by_key(|(t, _)| *t)
            .map(|(_, p)| p)
    }

    fn find_game() -> Option<HWND> {
        unsafe extern "system" fn each(h: HWND, found: LPARAM) -> BOOL {
            let found = &mut *(found.0 as *mut Option<HWND>);
            if !IsWindowVisible(h).as_bool() {
                return true.into();
            }
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(h, &mut buf);
            if n > 0
                && String::from_utf16_lossy(&buf[..n as usize]).trim() == "Torchlight: Infinite"
            {
                *found = Some(h);
                return false.into();
            }
            true.into()
        }
        let mut found: Option<HWND> = None;
        unsafe {
            let _ = EnumWindows(Some(each), LPARAM(&mut found as *mut _ as isize));
        }
        found
    }

    /// The game's picture on screen: x, y, width, height.
    fn game_rect(hwnd: HWND) -> Option<(i32, i32, i32, i32)> {
        unsafe {
            if IsIconic(hwnd).as_bool() {
                return None;
            }
            let mut r = RECT::default();
            GetClientRect(hwnd, &mut r).ok()?;
            let mut p = POINT { x: 0, y: 0 };
            let _ = ClientToScreen(hwnd, &mut p);
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            (w > 0 && h > 0).then_some((p.x, p.y, w, h))
        }
    }

    fn capture() -> Result<Frame, String> {
        let hwnd = find_game().ok_or("no window titled Torchlight: Infinite")?;
        let (x, y, w, h) = game_rect(hwnd).ok_or("the game is minimized")?;
        shared().lock().unwrap().game = Some((x, y, w, h));
        unsafe {
            let screen = GetDC(HWND::default());
            let mem = CreateCompatibleDC(screen);
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let result = match CreateDIBSection(mem, &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bmp) => {
                    let old = SelectObject(mem, bmp);
                    // the overlay's own boxes are hidden from the copy: it's a layered window, which BitBlt skips
                    // unless CAPTUREBLT is asked
                    let copied = BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY);
                    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
                    if copied.is_ok() {
                        let src =
                            std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
                        for px in src.chunks_exact(4) {
                            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
                        }
                    }
                    SelectObject(mem, old);
                    let _ = DeleteObject(bmp);
                    copied
                        .map(|_| Frame {
                            w: w as usize,
                            h: h as usize,
                            rgb,
                        })
                        .map_err(|e| e.to_string())
                }
                Err(e) => Err(e.to_string()),
            };
            let _ = DeleteDC(mem);
            ReleaseDC(HWND::default(), screen);
            result
        }
    }

    fn save_png(path: &Path, f: &Frame) {
        let Ok(file) = std::fs::File::create(path) else {
            return;
        };
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), f.w as u32, f.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        if let Ok(mut wr) = enc.write_header() {
            let _ = wr.write_image_data(&f.rgb);
        }
    }

    /// Follows the end of a growing file.
    struct Tail {
        path: PathBuf,
        pos: u64,
        rest: String,
    }

    impl Tail {
        /// Starts `back` bytes before the end, to pick up where a run stands.
        fn open(path: &Path, back: u64) -> Result<Tail, String> {
            let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let len = f.metadata().map_err(|e| e.to_string())?.len();
            Ok(Tail {
                path: path.to_path_buf(),
                pos: len.saturating_sub(back),
                rest: String::new(),
            })
        }

        fn lines(&mut self) -> Vec<String> {
            let Ok(mut f) = std::fs::File::open(&self.path) else {
                return Vec::new();
            };
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if len < self.pos {
                self.pos = 0;
                self.rest.clear();
            }
            if len == self.pos || f.seek(SeekFrom::Start(self.pos)).is_err() {
                return Vec::new();
            }
            let mut buf = Vec::new();
            let _ = (&mut f)
                .take((len - self.pos).min(16 << 20))
                .read_to_end(&mut buf);
            self.pos += buf.len() as u64;
            self.rest.push_str(&String::from_utf8_lossy(&buf));
            let mut out: Vec<String> = self.rest.split('\n').map(String::from).collect();
            self.rest = out.pop().unwrap_or_default();
            out
        }
    }

    // ---------- the run's state on disk ----------

    fn data_dir() -> PathBuf {
        let base = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        let d = base.join("TartarusSolver");
        let _ = std::fs::create_dir_all(&d);
        d
    }

    /// The settings in use (settings.json in the data folder).
    fn settings() -> &'static Mutex<Settings> {
        static S: OnceLock<Mutex<Settings>> = OnceLock::new();
        S.get_or_init(|| {
            Mutex::new(
                std::fs::read_to_string(data_dir().join("settings.json"))
                    .ok()
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                    .map(|v| Settings::from_json(&v))
                    .unwrap_or_default(),
            )
        })
    }

    fn save_settings(s: &Settings) {
        let _ = std::fs::write(
            data_dir().join("settings.json"),
            serde_json::to_string_pretty(&s.to_json()).unwrap_or_default(),
        );
    }

    fn load_state() -> RunState {
        if LITE {
            return RunState::default();
        }
        std::fs::read_to_string(data_dir().join("state.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .map(|v| RunState::from_json(&v))
            .unwrap_or_default()
    }

    fn save_state(st: &RunState) {
        if LITE {
            return;
        }
        let _ = std::fs::write(
            data_dir().join("state.json"),
            serde_json::to_string_pretty(&st.to_json()).unwrap_or_default(),
        );
    }

    fn print_state(st: &RunState) {
        say!("  Rumor:       {}", st.rumor.as_deref().unwrap_or("none"));
        say!(
            "  objective:   {}{}",
            st.objective.as_deref().unwrap_or("not set"),
            match st.objective.as_deref() {
                Some("prison-walk") => format!(" ({} tiles)", st.walked),
                Some("undercurrents") => format!(" ({} of 2 incidents)", st.triggered),
                Some("behind-cell-doors") => format!(" ({} of 10 cells opened)", st.opened),
                Some("survival-of-the-fittest") => format!(" ({} of 4 consumes)", st.consumed),
                _ => String::new(),
            }
        );
        say!(
            "  instruments: {}",
            if st.instruments.is_empty() {
                "none".into()
            } else {
                st.instruments.join(", ")
            }
        );
        if let Some((cur, max)) = st.day {
            say!(
                "  day:         {cur} of {max} ({} until the riot)",
                max.saturating_sub(cur)
            );
        }
        for g in st.gaps() {
            say!("  [!] {g}");
        }
    }

    // ---------- the log ----------

    /// What a log line means for the overlay.
    enum Event {
        Day(u32, u32),
        Popup(Popup),
        /// A pop-up closed (the next one can open before the last one's close is logged).
        PopupClosed(Popup),
        InstrumentSelected,
        InstrumentConfirmed,
        TreasureConfirmed,
        /// The eye on a pop-up: the cards hide (the pop-up closes in the log) to show the board.
        ViewBoard,
        /// The eye again: back to the cards (the pop-up opens again in the log).
        ViewReturn,
        Click,
    }

    fn event(line: &str) -> Option<Event> {
        if let Some(i) = line.find("LastDay] check cur=") {
            let rest = &line[i + 19..];
            let cur: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            let max: String = rest
                .split("max=")
                .nth(1)
                .unwrap_or("20")
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            let max: u32 = max.parse().unwrap_or(20);
            // before a run starts the game checks with max=0: not a day
            return (max > 0)
                .then(|| cur.parse().ok().map(|c| Event::Day(c, max)))
                .flatten();
        }
        if line.contains("normal-click S16ViewBoardBtn_") {
            return Some(Event::ViewBoard);
        }
        if line.contains("normal-click Btn_S16Board_ViewReturn") {
            return Some(Event::ViewReturn);
        }
        for (key, p) in [
            ("BoardRelicPop", Popup::Instrument),
            ("BoardColoredPop", Popup::Treasure),
            ("BoardEventPop", Popup::Incident),
            ("BoardThemePop", Popup::Rumor),
            ("BoardTaskPop", Popup::Objective),
            ("GamePlayCheckOut", Popup::End),
        ] {
            if line.contains(&format!("{key} Run")) {
                return Some(Event::Popup(p));
            }
            if line.contains(&format!("{key} Destory")) {
                return Some(Event::PopupClosed(p));
            }
        }
        if line.contains("normal-click Btn_") && line.contains("Board_") {
            if line.contains("RelicOptCell") {
                return Some(Event::InstrumentSelected);
            }
            if line.contains("RelicPop_Confirm") {
                return Some(Event::InstrumentConfirmed);
            }
            if line.contains("ColoredPop_Confirm") {
                return Some(Event::TreasureConfirmed);
            }
            return Some(Event::Click);
        }
        if line.contains("BoardMain Run") || line.contains("Prison_Relock") {
            return Some(Event::Click);
        }
        None
    }

    // ---------- the worker ----------

    struct Worker {
        st: RunState,
        popup: Popup,
        due: Option<Instant>,
        last_key: String,
        frames: PathBuf,
        count: u32,
        /// The board is shown through a pop-up's eye: the pop-up's close and reopen in the log aren't real.
        viewing: bool,
        /// How many pictures in a row gave the same advice.
        stable: u8,
        /// When the log last showed something happening.
        last_event: Instant,
        /// The picture's mapping onto the reference (any window size and UI scale), and the UI scale it was found at.
        view: Option<View>,
        view_ui: Option<f32>,
        /// The UI scale the game's log reports.
        ui_scale: Option<f32>,
        /// The screen boxed and the option the box is on (kept while it's within the margin of the best).
        shown_screen: String,
        shown_best: Option<String>,
        /// Where the boxes of the kept option are (put back after a click cleared them).
        shown_boxes: Vec<tartarus::advisor::Rect>,
        /// The looks at one position pooled: option -> (sum of estimates, sum of squared margins, sum of chances
        /// of 10, looks).
        pooled: std::collections::HashMap<String, (f64, f64, f64, f64)>,
        pooled_at: String,
        /// Whether the game log tells the pop-ups (else they're told from the picture).
        log_popups: bool,
        guessed_popup: Popup,
        /// Whether the last look boxed something (a Tartarus screen with a choice).
        last_had_advice: bool,
        /// Looks pooled at this position.
        pooled_looks: u32,
        /// Milliseconds per future on this computer, [slow pool, fast pool] (0: not measured yet).
        ms_per_future: [f64; 2],
        /// The screen as last solved, coarsely (see `fingerprint`).
        last_print: Vec<u8>,
        /// Where each option that came out best in a look is drawn.
        boxes_of: std::collections::HashMap<String, Vec<tartarus::advisor::Rect>>,
    }

    impl Worker {
        /// How the game picture maps onto the reference: kept once found on the board, found again when the window's
        /// size or the UI scale changes.
        fn view_for(&mut self, f: &Frame) -> View {
            if let Some(v) = self.view {
                if v.w == f.w && v.h == f.h && self.view_ui == self.ui_scale {
                    return v;
                }
            }
            match tartarus::view::calibrate(f) {
                Some((v, m)) => {
                    if !v.is_reference() {
                        say!("{}  (window {}x{}: interface at {:.3} times the reference, match {:.2})", clock(), f.w, f.h, v.k, m);
                    }
                    self.view = Some(v);
                    self.view_ui = self.ui_scale;
                    v
                }
                // not on the board: the window's height and UI scale tell it, until the board shows
                None => View::new(f.w, f.h, self.ui_scale),
            }
        }

        /// A Consume confirmed on the target screen: counts for Survival of the Fittest.
        fn consumed(&mut self) {
            if !self.last_key.starts_with("choose what to Consume") {
                return;
            }
            self.st.consumed += 1;
            if self.st.objective.as_deref() == Some("survival-of-the-fittest") {
                say!(
                    "{}  noted: Consume {} of 4 for Survival of the Fittest",
                    clock(),
                    self.st.consumed
                );
                if self.st.consumed >= 4 {
                    self.st.objective = Some("done".into());
                }
            }
            save_state(&self.st);
        }

        fn handle(&mut self, ev: Event, live: bool) {
            self.last_event = Instant::now();
            let card_click = matches!(ev, Event::InstrumentSelected);
            match ev {
                Event::Day(cur, max) => {
                    // a new run: the day went back or the length changed (day 0 right after the Rumor/objective cards,
                    // which already started it, isn't one: what was typed on those cards stays)
                    // (a length that changes isn't a new run: the Iron Law Rumor makes it 30 days, and the Rumor and
                    // objective already read stay)
                    let new_run = match self.st.day {
                        None => cur == 0,
                        Some((c, _)) => cur < c,
                    };
                    if new_run {
                        self.st.new_run(max);
                        if live {
                            say!("");
                            say!("{}  ===== a new run ({max} days) =====", clock());
                            say!(
                                "  If it has a Rumor, type `rumor opulence`, `rumor cage-fight`, `rumor tyranny` or `rumor iron-law`."
                            );
                            say!("  Set the objective: `objective prison-walk 0`, `objective king`, `objective undercurrents` or `objective <name>`.");
                            say!("  The Torture Instruments you take are noted by themselves (`add <name>` if one is missed).");
                        }
                    }
                    self.st.day = Some((cur, max));
                    self.popup = Popup::None;
                    save_state(&self.st);
                    self.due = Some(Instant::now() + SETTLE);
                }
                Event::Popup(p) => {
                    // the Rumor and objective cards open a run: once the last one is over, start the new one now, so
                    // the Rumor and objective typed while they show are kept
                    if matches!(p, Popup::Rumor | Popup::Objective) && live {
                        if let Some((c, m)) = self.st.day {
                            if c >= m && m > 0 {
                                self.st.new_run(m);
                                say!("");
                                say!("{}  ===== a new run ({m} days) =====", clock());
                                say!("  If it has a Rumor, type `rumor opulence`, `rumor cage-fight`, `rumor tyranny` or `rumor iron-law`.");
                                say!("  Set the objective if it isn't read: `objective prison-walk 0`, `objective king`, `objective undercurrents`.");
                                save_state(&self.st);
                            }
                        }
                    }
                    if p == Popup::Incident
                        && self.popup != Popup::Incident
                        && !self.viewing
                        && live
                    {
                        self.st.incident_triggered();
                        save_state(&self.st);
                    }
                    self.popup = p;
                    if live {
                        self.log_popups = true;
                    }
                    self.due = Some(Instant::now() + SETTLE);
                }
                Event::PopupClosed(_) if self.viewing => {
                    self.due = Some(Instant::now() + Duration::from_millis(500))
                }
                Event::PopupClosed(p) if p != self.popup => {}
                Event::PopupClosed(_) => {
                    self.popup = Popup::None;
                    self.due = Some(Instant::now() + SETTLE);
                }
                Event::ViewBoard => {
                    self.viewing = true;
                    self.due = Some(Instant::now() + Duration::from_millis(500));
                }
                Event::ViewReturn => {
                    self.viewing = false;
                    self.due = Some(Instant::now() + Duration::from_millis(500));
                }
                Event::InstrumentSelected => {
                    self.due = Some(Instant::now() + Duration::from_millis(300))
                }
                Event::InstrumentConfirmed => {
                    let guess = std::mem::take(&mut self.st.selected_guess);
                    match self.st.selected_instrument.take() {
                        Some(name) => {
                            if !self.st.instruments.contains(&name) {
                                self.st.instruments.push(name.clone());
                            }
                            if live && guess {
                                say!("{}  [!] I didn't see which card was selected: noted {name} (the boxed one). If you took another, type `remove {name}` then `add <its name>`", clock());
                            } else if live {
                                say!("{}  noted: you took {name}", clock());
                            }
                        }
                        None if live => say!("{}  [!] I couldn't tell which instrument you took: type `add <its name>`", clock()),
                        None => {}
                    }
                    save_state(&self.st);
                    self.due = Some(Instant::now() + SETTLE);
                }
                Event::TreasureConfirmed => {
                    self.st.picks_done += 1;
                    self.viewing = false;
                    // the pick changes the board: look again before the next one
                    self.st.board_fresh = false;
                    self.due = Some(Instant::now() + SETTLE);
                }
                Event::Click => self.due = Some(Instant::now() + SETTLE),
            }
            // a card click (selecting an instrument) doesn't change the screen: the boxes stay
            if live && !card_click {
                set_boxes(Vec::new());
            }
        }

        fn command(&mut self, line: &str) {
            let line = line.trim();
            let (cmd, arg) = line
                .split_once(' ')
                .map(|(a, b)| (a, b.trim()))
                .unwrap_or((line, ""));
            match cmd.to_ascii_lowercase().as_str() {
                "" => return,
                "help" | "?" => {
                    say!("commands:");
                    say!("  rumor <opulence|cage-fight|none>     the run's Rumor");
                    say!("  objective prison-walk <tiles>        or: objective king / undercurrents / doors <n> / survival <n> / done");
                    say!("  add <instrument>                     an instrument you hold (part of its name is enough)");
                    say!("  remove <instrument>");
                    say!("  state                                what I know about this run");
                    say!("  new                                  forget this run (instruments, Rumor, objective)");
                    say!("  redo                                 read the screen again now (also Ctrl+Alt+R)");
                    say!("  boxes                                hide or show the boxes (also Ctrl+Alt+B)");
                    return;
                }
                "rumor" => {
                    self.st.rumor = if arg.is_empty() || arg.eq_ignore_ascii_case("none") {
                        None
                    } else {
                        Some(tartarus::instruments::slug(arg))
                    };
                }
                "objective" => {
                    let mut p = arg.split_whitespace();
                    let name = tartarus::instruments::slug(p.next().unwrap_or(""));
                    self.st.objective = match name.as_str() {
                        "" => None,
                        "prison-walk" | "walk" => {
                            self.st.walked = p.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                            Some("prison-walk".into())
                        }
                        "king" | "king-of-the-caged" => Some("king-of-the-caged".into()),
                        "doors" | "behind-cell-doors" => {
                            if let Some(n) = p.next().and_then(|n| n.parse().ok()) {
                                self.st.opened = n;
                            }
                            Some("behind-cell-doors".into())
                        }
                        "survival" | "survival-of-the-fittest" => {
                            if let Some(n) = p.next().and_then(|n| n.parse().ok()) {
                                self.st.consumed = n;
                            }
                            Some("survival-of-the-fittest".into())
                        }
                        other => Some(other.into()),
                    };
                }
                "add" => match Instrument::find(arg) {
                    Some(i) => {
                        if !self
                            .st
                            .instruments
                            .iter()
                            .any(|n| Instrument::find(n) == Some(i))
                        {
                            self.st.instruments.push(i.name().into());
                        }
                    }
                    None => {
                        say!("  [!] I don't know an instrument called \"{arg}\"");
                        return;
                    }
                },
                "remove" => {
                    let i = Instrument::find(arg);
                    self.st.instruments.retain(|n| Instrument::find(n) != i);
                }
                "new" => {
                    let max = self.st.day.map(|d| d.1).unwrap_or(20);
                    let day = self.st.day;
                    self.st.new_run(max);
                    self.st.day = day;
                }
                "state" => {}
                "redo" => {}
                "boxes" => {
                    let mut s = shared().lock().unwrap();
                    s.show = !s.show;
                    s.changed = true;
                    say!("  boxes {}", if s.show { "on" } else { "off" });
                    return;
                }
                _ => {
                    say!("  [!] unknown command \"{cmd}\": type `help`");
                    return;
                }
            }
            save_state(&self.st);
            print_state(&self.st);
            self.last_key.clear();
            self.due = Some(Instant::now());
        }

        /// The picture's gray on a coarse grid (every 20 pixels), to tell a screen that changed from one that didn't.
        fn fingerprint(f: &Frame) -> Vec<u8> {
            let mut v = Vec::with_capacity(96 * 54);
            for y in (10..1080).step_by(20) {
                for x in (10..1920).step_by(20) {
                    v.push(f.gray(x, y).clamp(0.0, 255.0) as u8);
                }
            }
            v
        }

        /// Like `fingerprint`, on the picture as captured (any size): a 96x54 grid.
        fn raw_print(f: &Frame) -> Vec<u8> {
            let mut v = Vec::with_capacity(96 * 54);
            for j in 0..54 {
                for i in 0..96 {
                    let (x, y) = ((2 * i + 1) * f.w / 192, (2 * j + 1) * f.h / 108);
                    v.push(f.gray(x, y).clamp(0.0, 255.0) as u8);
                }
            }
            v
        }

        fn changed_spots(a: &[u8], b: &[u8]) -> usize {
            if a.len() != b.len() {
                return usize::MAX;
            }
            a.iter().zip(b).filter(|(x, y)| x.abs_diff(**y) > 25).count()
        }

        fn analyse(&mut self) {
            let shot = match capture() {
                Ok(f) => f,
                Err(e) => {
                    say!("{}  (no picture: {e})", clock());
                    return;
                }
            };
            // any window size and UI scale: map the picture onto the reader's 1920x1080 reference
            let view = self.view_for(&shot);
            let f = view.to_reference(&shot);
            let to_screen = |boxes: &[tartarus::advisor::Rect]| -> Vec<tartarus::advisor::Rect> {
                boxes
                    .iter()
                    .map(|b| {
                        let (x0, y0, x1, y1) =
                            view.rect_to_screen(b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32);
                        tartarus::advisor::Rect {
                            x0: x0 as i32,
                            y0: y0 as i32,
                            x1: x1 as i32,
                            y1: y1 as i32,
                        }
                    })
                    .collect()
            };
            let t = Instant::now();
            // one full look (no quick first guess: a box that appears a little later but doesn't hop)
            // a screen already solved enough times that hasn't changed since isn't solved again: the computer's
            // fans get a rest while a card screen stays up
            let print = Self::fingerprint(&f);
            let settled = self.pooled.values().map(|e| e.3).fold(0.0, f64::max) >= SETTLED_LOOKS
                || self.pooled_looks >= 12;
            if settled && Self::changed_spots(&print, &self.last_print) <= 6 {
                self.due = Some(Instant::now() + RECHECK);
                return;
            }
            // a new screen (clearly different from the last one solved): the old boxes go now, a bar shows the work
            // (only after a screen with advice, or on a pop-up: elsewhere in the game nothing shows)
            // (not over the instrument or treasure cards: hovering a card lights it up, which isn't a new screen)
            let on_cards = matches!(self.popup, Popup::Instrument | Popup::Treasure)
                || matches!(self.guessed_popup, Popup::Instrument | Popup::Treasure);
            let new_screen = !on_cards && Self::changed_spots(&print, &self.last_print) > 60;
            if new_screen {
                set_boxes(Vec::new());
                self.shown_boxes.clear();
            }
            // the bar shows whenever the boxes are down (a click took them, or a new screen) while solving, after a
            // screen that had advice: in the rest of the game nothing shows
            let boxes_down = shared().lock().unwrap().boxes.is_empty();
            set_busy(boxes_down && (self.last_had_advice || self.popup != Popup::None));
            self.last_print = print;
            let fast = self.last_event.elapsed() < Duration::from_secs(3) || self.pooled.is_empty();
            // a game log without the pop-up lines (seen on another computer): the instrument offer and the treasure are
            // told from the picture, and leaving the offer counts as taking the selected (or boxed) instrument
            let mut popup = self.popup;
            if !self.log_popups && popup == Popup::None {
                let names = tartarus::reader::read_instrument_offer(&f)
                    .iter()
                    .filter(|n| n.is_some())
                    .count();
                popup = if names >= 2 {
                    Popup::Instrument
                } else if tartarus::reader::treasure_header(&f) {
                    Popup::Treasure
                } else if self.guessed_popup == Popup::Treasure && tartarus::reader::eye_label(&f) {
                    // the eye clicked: the board with "Return to Selection", still the treasure's
                    Popup::Treasure
                } else {
                    Popup::None
                };
                if self.guessed_popup == Popup::Instrument && popup != Popup::Instrument {
                    self.guessed_popup = popup;
                    self.handle(Event::InstrumentConfirmed, true);
                }
                self.guessed_popup = popup;
            }
            let st = &mut self.st;
            // as many futures as fit in about half a second on this computer (measured on the last looks), so a slower
            // computer or a busy game gets a box as soon, a little rougher, and later looks sharpen it
            let speed = self.ms_per_future[fast as usize];
            let futures = if speed > 0.0 {
                ((LOOK_MS / speed) as usize).clamp(MIN_FUTURES, FUTURES)
            } else {
                FUTURES / 2
            };
            tartarus::advisor::set_look(self.pooled_looks as u64);
            set_busy_ms(if speed > 0.0 { speed * futures as f64 * 1.15 } else { LOOK_MS });
            let solved = Instant::now();
            let mut a = pool(fast).install(|| advise(&f, popup, st, futures));
            if !a.options.is_empty() {
                let per = solved.elapsed().as_secs_f64() * 1000.0 / futures as f64;
                let m = &mut self.ms_per_future[fast as usize];
                *m = if *m > 0.0 { 0.5 * *m + 0.5 * per } else { per };
            }
            set_busy(false);
            self.last_had_advice = !a.boxes.is_empty();
            let weight = futures as f64 / FUTURES as f64;
            save_state(&self.st);
            // the same screen: same kind and the warden in the same cell (a cell misread between two looks doesn't
            // make it a new screen)
            let screen = format!(
                "{}|{}",
                a.screen,
                a.board
                    .as_deref()
                    .and_then(|b| b.split(" |").next())
                    .unwrap_or("")
            );
            let same_screen = screen == self.shown_screen;
            // every look at the very same position adds its futures to the earlier ones', so the estimates get
            // sharper the longer a screen stays up instead of jumping with each look's luck
            let position = format!("{}|{:?}", a.screen, a.board);
            if position != self.pooled_at {
                self.pooled_at = position;
                self.pooled.clear();
                self.pooled_looks = 0;
            }
            self.pooled_looks += 1;
            // each look weighs by its futures (a full look = 1)
            for o in &a.options {
                let e = self.pooled.entry(o.0.clone()).or_default();
                e.0 += o.1 * weight;
                e.1 += (o.2 * weight) * (o.2 * weight);
                e.2 += o.3 * weight;
                e.3 += weight;
            }
            if let Some(b) = a.options.first() {
                self.boxes_of.insert(b.0.clone(), to_screen(&a.boxes));
            }
            for o in a.options.iter_mut() {
                if let Some(&(sum, var, full, n)) = self.pooled.get(&o.0) {
                    if n > 0.0 {
                        *o = (o.0.clone(), sum / n, var.sqrt() / n, full / n);
                    }
                }
            }
            a.options
                .sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
            // the box moves only when another option is clearly better than the boxed one: by twice their
            // margins together (and at least 0.05 Chromatics); a toss-up keeps the box where it is
            let best = a.options.first().cloned();
            let kept = same_screen
                && self.shown_best.as_ref().is_some_and(|prev| {
                    let Some(b) = best.as_ref() else { return false };
                    a.options
                        .iter()
                        .any(|o| &o.0 == prev && b.1 - o.1 <= (2.0 * (o.2 + b.2)).max(0.05))
                });
            if !kept {
                let boxes = best
                    .as_ref()
                    .and_then(|b| self.boxes_of.get(&b.0).cloned())
                    .unwrap_or_else(|| to_screen(&a.boxes));
                set_boxes(boxes.clone());
                self.shown_boxes = boxes;
                self.shown_best = best.as_ref().map(|b| b.0.clone());
            } else {
                // the same option stays boxed, drawn where it is now (the latest look's place for it: the eye before
                // the board is seen, the card after), also after the boxes were taken down meanwhile
                let boxes = self
                    .shown_best
                    .as_ref()
                    .and_then(|p| self.boxes_of.get(p).cloned())
                    .filter(|b| !b.is_empty())
                    .unwrap_or_else(|| self.shown_boxes.clone());
                if !boxes.is_empty() {
                    set_boxes(boxes.clone());
                    self.shown_boxes = boxes;
                }
            }
            if !same_screen {
                self.boxes_of
                    .retain(|k, _| a.options.iter().any(|o| &o.0 == k));
            }
            if kept {
                if let Some(k) = a
                    .options
                    .iter()
                    .position(|o| Some(&o.0) == self.shown_best.as_ref())
                {
                    // the console shows the boxed option first, as a toss-up with the others
                    let o = a.options.remove(k);
                    a.options.insert(0, o);
                }
            }
            // the prediction stays up between decisions (updated at each choice), and goes away off Tartarus
            if let Some(o) = a.options.first().filter(|_| !self.shown_boxes.is_empty()) {
                set_line(Some(tartarus::font::chance_line(a.unit, o.1, o.3)));
            } else if a.screen == "not the board" || self.popup == Popup::End {
                set_line(None);
            }
            self.shown_screen = screen;
            let key = format!("{}|{:?}|{:?}", a.screen, a.board, self.shown_best);
            // look again soon until the screen holds still (the first look may catch the end of an animation);
            // the eye's clicks and some card clicks aren't in the log, so keep looking while a choice is open
            self.stable = if key == self.last_key {
                self.stable + 1
            } else {
                0
            };
            // a screen with nothing to box yet (between screens, cards sliding in) is looked at again until it
            // gives advice, for a while after the last click
            let waiting = a.boxes.is_empty() && self.last_event.elapsed() < Duration::from_secs(15);
            if self.stable < 2
                || waiting
                || matches!(self.popup, Popup::Treasure | Popup::Instrument)
            {
                self.due = Some(Instant::now() + RECHECK);
            }
            if key == self.last_key {
                return;
            }
            self.last_key = key;
            self.count += 1;
            let name = format!(
                "{:04}-{}.png",
                self.count,
                a.screen.replace([' ', '\'', '(', ')', '?'], "-")
            );
            // what the game did, kept across runs for measuring its odds (Documents\Tartarus overlay\observations.jsonl)
            if !a.observations.is_empty() && !LITE {
                if let Some(dir) = self.frames.parent() {
                    use std::io::Write;
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(dir.join("observations.jsonl"))
                    {
                        let folder = self
                            .frames
                            .file_name()
                            .map(|x| x.to_string_lossy().to_string())
                            .unwrap_or_default();
                        for o in &a.observations {
                            let mut o = o.clone();
                            o["frame"] = serde_json::json!(format!("{folder}/{name}"));
                            let _ = writeln!(file, "{o}");
                        }
                    }
                }
            }
            if !LITE {
                save_png(&self.frames.join(&name), &f);
            }
            print_advice(&a, t.elapsed(), &name);
        }
    }

    fn print_advice(a: &Advice, took: Duration, frame: &str) {
        if a.options.is_empty() && a.notes.is_empty() && a.board.is_none() {
            return;
        }
        say!("");
        say!("{}  {}   [{}]", clock(), a.screen.to_uppercase(), frame);
        if let Some(b) = &a.board {
            say!("  read: {b}");
        }
        let unit = if a.unit.is_empty() {
            "Chromatics"
        } else {
            a.unit
        };
        for (k, (d, c, m, full)) in a.options.iter().enumerate() {
            say!(
                "  {} {:<46} {:>5.2} ± {:.2} {}  {:>3.0}% reach 10",
                if k == 0 { "=>" } else { "  " },
                d,
                c,
                m,
                unit,
                full * 100.0
            );
        }
        for n in &a.notes {
            say!("  [!] {n}");
        }
        if !a.options.is_empty() {
            say!("  ({} ms)", took.as_millis());
        }
    }

    fn worker(rx: Receiver<Msg>, log: PathBuf, frames: PathBuf) {
        let mut w = Worker {
            st: load_state(),
            popup: Popup::None,
            due: Some(Instant::now()),
            last_key: String::new(),
            frames,
            count: 0,
            viewing: false,
            stable: 0,
            last_event: Instant::now(),
            view: None,
            view_ui: None,
            ui_scale: None,
            shown_screen: String::new(),
            shown_best: None,
            shown_boxes: Vec::new(),
            pooled: Default::default(),
            pooled_at: String::new(),
            log_popups: false,
            guessed_popup: Popup::None,
            pooled_looks: 0,
            last_had_advice: false,
            ms_per_future: [0.0; 2],
            last_print: Vec::new(),
            boxes_of: Default::default(),
        };
        // catch up on the end of the log: the day, an open pop-up, a new run since the last time
        let mut tail = match Tail::open(&log, 16 << 20) {
            Ok(t) => t,
            Err(e) => {
                say!("[!] {e}");
                return;
            }
        };
        // only the day and the open pop-up are taken from the past: what was noted in the saved state stays, unless
        // the log shows a new run since
        let mut last_day: Option<(u32, u32)> = None;
        // Prison Incidents since the run's start, when the log reaches back to it
        let mut seen_start = false;
        let mut incidents = 0u32;
        // the Rumor/objective cards showing since the last day: a new run is opening
        let mut opening = false;
        for line in tail.lines() {
            if let Some(u) = ui_scale_in(&line) {
                w.ui_scale = Some(u);
            }
            match event(&line) {
                Some(Event::Day(c, m)) => {
                    if c == 0 || last_day.is_some_and(|(lc, lm)| c < lc || m != lm) {
                        seen_start = true;
                        incidents = 0;
                    }
                    last_day = Some((c, m));
                    w.popup = Popup::None;
                    opening = false;
                }
                Some(Event::Popup(p)) => {
                    if matches!(p, Popup::Rumor | Popup::Objective) {
                        opening = true;
                    }
                    if p == Popup::Incident && w.popup != Popup::Incident {
                        incidents += 1;
                    }
                    w.popup = p
                }
                Some(Event::PopupClosed(p)) if p == w.popup => w.popup = Popup::None,
                _ => {}
            }
        }
        if let Some((c, m)) = last_day {
            let same_run = w.st.day.is_some_and(|(sc, sm)| sm == m && c >= sc);
            if !same_run {
                say!("");
                say!("(the log shows another run than the one saved: starting this one fresh)");
                w.st.new_run(m);
            }
            w.st.day = Some((c, m));
            if opening && c >= m && m > 0 {
                say!("");
                say!("(a new run is opening: starting it now, so the Rumor and objective you type are kept)");
                w.st.new_run(m);
            }
            if seen_start && incidents > w.st.triggered {
                for _ in w.st.triggered..incidents {
                    w.st.incident_triggered();
                }
            }
        }
        save_state(&w.st);
        say!("");
        say!("This run, as I know it:");
        print_state(&w.st);
        say!("");
        say!("Watching. Type `help` for the commands.");
        let mut last_growth = Instant::now();
        // the lite build has no window to close: it quits a while after the game does
        let mut game_seen: Option<Instant> = None;
        let mut game_checked = Instant::now();
        let mut wait_frame: Option<String> = None;
        let mut warned = false;
        let mut last_poll = Instant::now();
        let mut poll_print: Vec<u8> = Vec::new();
        loop {
            let lines = tail.lines();
            if !lines.is_empty() {
                last_growth = Instant::now();
                warned = false;
            } else if !warned && last_growth.elapsed() > Duration::from_secs(120) {
                say!(
                    "{}  (the game log hasn't changed for 2 minutes: is the game running?)",
                    clock()
                );
                warned = true;
            }
            for line in lines {
                if let Some(u) = ui_scale_in(&line) {
                    w.ui_scale = Some(u);
                }
                // a Consume: the target's click goes with a request to the server in the same frame (a click on a
                // cell that can't be eaten doesn't)
                let frame_tag = line.split(']').nth(1).map(|t| t.to_string());
                if line.contains("NetGame ShowWaiting") {
                    wait_frame = frame_tag.clone();
                }
                if line.trim_end().ends_with("normal-click Btn_S16Board_Cell")
                    && frame_tag.is_some()
                    && frame_tag == wait_frame
                {
                    w.consumed();
                }
                if let Some(ev) = event(&line) {
                    w.handle(ev, true);
                }
            }
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Command(c) => w.command(&c),
                    Msg::Redo => {
                        w.last_key.clear();
                        w.due = Some(Instant::now());
                    }
                    Msg::Log(p) => match Tail::open(&p, 0) {
                        Ok(t) => {
                            tail = t;
                            say!("{}  game log now: {}", clock(), p.display());
                        }
                        Err(e) => say!("{}  [!] {e}", clock()),
                    },
                }
            }
            if LITE && game_checked.elapsed() > Duration::from_secs(5) {
                game_checked = Instant::now();
                if find_game().is_some() {
                    game_seen = Some(Instant::now());
                } else if game_seen.is_some_and(|t| t.elapsed() > Duration::from_secs(120)) {
                    quit();
                }
            }
            // when the log is quiet (some computers write it late, or it's another install's log) or doesn't tell the
            // pop-ups, the screen itself is watched: a big change (a new screen) gets a look, as a log line would
            if w.due.is_none()
                && (last_growth.elapsed() > Duration::from_secs(3) || !w.log_popups)
                && last_poll.elapsed() > Duration::from_millis(700)
            {
                last_poll = Instant::now();
                if let Ok(shot) = capture() {
                    let p = Worker::raw_print(&shot);
                    let changed = Worker::changed_spots(&p, &poll_print);
                    poll_print = p;
                    if changed > 150 {
                        w.due = Some(Instant::now() + SETTLE);
                    }
                }
            }
            if w.due.is_some_and(|t| Instant::now() >= t) {
                w.due = None;
                w.analyse();
            }
            std::thread::sleep(Duration::from_millis(80));
        }
    }

    // ---------- the window that draws the boxes ----------

    fn draw(hwnd: HWND) {
        let (boxes, game, show, line, busy, fill, left_ms) = {
            let mut s = shared().lock().unwrap();
            s.changed = false;
            let fill = bar_fill(s.busy_since, s.busy_ms);
            let left = s.busy_ms - s.busy_since.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
            (s.boxes.clone(), s.game, s.show, s.line.clone(), s.busy, fill, left)
        };
        let color = settings().lock().unwrap().color;
        unsafe {
            let Some((gx, gy, gw, gh)) =
                game.filter(|_| show && (!boxes.is_empty() || busy || line.is_some()))
            else {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return;
            };
            // the boxes are already in the window's pixels (mapped from the reference picture by the view)
            let (sx, sy) = (1.0f32, 1.0f32);
            let mut px = vec![0u8; (gw * gh * 4) as usize];
            let mut put = |x: i32, y: i32, c: [u8; 4]| {
                if x >= 0 && y >= 0 && x < gw && y < gh {
                    let i = ((y * gw + x) * 4) as usize;
                    px[i..i + 4].copy_from_slice(&c);
                }
            };
            for b in &boxes {
                let (x0, y0, x1, y1) = (
                    (b.x0 as f32 * sx) as i32,
                    (b.y0 as f32 * sy) as i32,
                    (b.x1 as f32 * sx) as i32,
                    (b.y1 as f32 * sy) as i32,
                );
                for t in -2..7 {
                    // BGRA, premultiplied: a dark rim around a bright green frame
                    let c = if t < 0 || t > 4 {
                        [0, 0, 0, 200]
                    } else {
                        [color[2], color[1], color[0], 255]
                    };
                    for x in x0 - t..=x1 + t {
                        put(x, y0 - t, c);
                        put(x, y1 + t, c);
                    }
                    for y in y0 - t..=y1 + t {
                        put(x0 - t, y, c);
                        put(x1 + t, y, c);
                    }
                }
            }
            // the line and the bar in the bottom-left corner (nothing of the game's there): white text on a dark band,
            // the bar under it
            let scale = (gh / 400).max(2);
            let (lx, bar_y) = (14 * scale, gh - 8 * scale);
            let line_y = bar_y - 6 * scale - 7 * scale;
            if let Some(line) = &line {
                let scale = scale as usize;
                let (tw, th, mask) = tartarus::font::render(&line, scale);
                let pad = 4 * scale as i32;
                let (tx, ty) = (lx, line_y);
                for y in ty - pad..ty + th as i32 + pad {
                    for x in tx - pad..tx + tw as i32 + pad {
                        put(x, y, [0, 0, 0, 190]);
                    }
                }
                for (i, &on) in mask.iter().enumerate() {
                    if on {
                        put(tx + (i % tw) as i32, ty + (i / tw) as i32, [120, 255, 160, 255]);
                    }
                }
            }
            // the progress bar under the line: always there, full when the advice is up, filling while solving
            if line.is_some() || busy {
                let (bw, bh) = (120 * scale, 3 * scale);
                let (bx, by) = (lx, bar_y);
                let fill = if busy { fill } else { 1.0 };
                let done = (fill * bw as f64) as i32;
                for y in by - 2..by + bh + 2 {
                    for x in bx - 2..bx + bw + 2 {
                        put(x, y, [0, 0, 0, 190]);
                    }
                }
                for y in by..by + bh {
                    for x in bx..bx + done {
                        put(x, y, [120, 255, 160, 255]);
                    }
                    for x in bx + done..bx + bw {
                        put(x, y, [40, 70, 50, 220]);
                    }
                }
                // beside the bar: the time before the next box
                if busy {
                    let label = if left_ms > 50.0 {
                        format!("Next box in {:.1}s", left_ms / 1000.0)
                    } else {
                        "Next box: almost".to_string()
                    };
                    let (tw, th, mask) = tartarus::font::render(&label, scale as usize);
                    let (tx, ty) = (bx + bw + 6 * scale, by + bh / 2 - th as i32 / 2);
                    for y in ty - 2 * scale..ty + th as i32 + 2 * scale {
                        for x in tx - 3 * scale..tx + tw as i32 + 3 * scale {
                            put(x, y, [0, 0, 0, 190]);
                        }
                    }
                    for (i, &on) in mask.iter().enumerate() {
                        if on {
                            put(tx + (i % tw) as i32, ty + (i / tw) as i32, [120, 255, 160, 255]);
                        }
                    }
                }
            }
            let screen = GetDC(HWND::default());
            let mem = CreateCompatibleDC(screen);
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: gw,
                    biHeight: -gh,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            if let Ok(bmp) = CreateDIBSection(mem, &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
                std::slice::from_raw_parts_mut(bits as *mut u8, px.len()).copy_from_slice(&px);
                let old = SelectObject(mem, bmp);
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                let (pt, size, src) = (
                    POINT { x: gx, y: gy },
                    SIZE { cx: gw, cy: gh },
                    POINT { x: 0, y: 0 },
                );
                let _ = UpdateLayeredWindow(
                    hwnd,
                    screen,
                    Some(&pt),
                    Some(&size),
                    mem,
                    Some(&src),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                );
                SelectObject(mem, old);
                let _ = DeleteObject(bmp);
            }
            let _ = DeleteDC(mem);
            ReleaseDC(HWND::default(), screen);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_TIMER => {
                // follow the game window, and hide the boxes while it's minimized
                let rect = find_game().and_then(game_rect);
                let changed = {
                    let mut s = shared().lock().unwrap();
                    if s.game != rect {
                        s.game = rect;
                        s.changed = true;
                    }
                    // the progress bar moves while solving
                    s.changed || s.busy
                };
                if changed {
                    draw(hwnd);
                }
                LRESULT(0)
            }
            WM_HOTKEY => {
                match wp.0 as i32 {
                    HK_BOXES => {
                        let _ = TO_WORKER
                            .get()
                            .map(|t| t.send(Msg::Command("boxes".into())));
                    }
                    HK_REDO => {
                        let _ = TO_WORKER.get().map(|t| t.send(Msg::Redo));
                    }
                    HK_QUIT => quit(),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_TRAY => {
                let ev = (lp.0 as u32) & 0xFFFF;
                if ev == WM_RBUTTONUP || ev == WM_LBUTTONUP {
                    tray_menu(hwnd);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    // ---------- start ----------

    pub fn run() -> Result<(), String> {
        unsafe {
            let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
        quick_edit_off();
        if !show_notice() {
            std::process::exit(0);
        }
        let home = std::env::var("USERPROFILE").map_err(|_| "no USERPROFILE")?;
        let (y, mo, d, h, mi, s) = now_parts();
        let frames = PathBuf::from(home)
            .join("Documents")
            .join("Tartarus overlay")
            .join(format!("{y}-{mo:02}-{d:02} {h:02}.{mi:02}.{s:02}"));
        if !LITE {
            let _ = std::fs::create_dir_all(&frames);
            if let Ok(f) = std::fs::File::create(frames.join("console.log")) {
                *DIAG.lock().unwrap() = Some(f);
            }
        }
        say!("Tartarus overlay {VERSION}: advice only, it never touches the game.");
        say!("");
        say!("Checks:");
        let log = std::env::args()
            .skip(1)
            .collect::<Vec<_>>()
            .windows(2)
            .find(|a| a[0] == "--log")
            .map(|a| PathBuf::from(&a[1]))
            .or_else(|| {
                settings()
                    .lock()
                    .unwrap()
                    .log
                    .clone()
                    .map(PathBuf::from)
                    .filter(|p| p.is_file())
            })
            .or_else(find_log);
        let Some(log) = log else {
            say!("  [!!] game log: not found through Steam. Start me with --log \"<path to UE_game.log>\"");
            return Err("no game log".into());
        };
        let age = std::fs::metadata(&log)
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().unwrap_or_default().as_secs() / 60)
            .unwrap_or(0);
        say!(
            "  [ok] game log: {} (written {} min ago){}",
            log.display(),
            age,
            if age > 10 {
                "  [!] old: if the game is running, this may be the wrong log"
            } else {
                ""
            }
        );
        match find_game().and_then(game_rect) {
            Some((x, y, w, h)) if w == 1920 && h == 1080 => say!("  [ok] game window: 1920x1080 at {x},{y}"),
            Some((x, y, w, h)) => say!("  [ok] game window: {w}x{h} at {x},{y} (the interface's scale is found on the first board seen)"),
            None => say!("  [!] game window: not found yet (start the game, I'll look again)"),
        }
        say!("  [ok] reader: 4 Codes, 5 rarities, the warden, destinations, Chromatics, the Consume screen");
        say!(
            "       instrument cards it can read ({}): {}",
            tartarus::reader::known_titles().len(),
            tartarus::reader::known_titles().join(", ")
        );
        say!("       others show as \"can't read\": tell me their names and send the picture from the frames folder");
        say!("  [ok] solver: 30 of 31 instruments, Opulence, Cage Fight, Tyranny, Iron Law, Prison Walk, King of the Caged");
        say!("  [..] not played yet: Squirming Iron Box, other Rumors/objectives");
        say!("  [..] screens with guessed box places: Instigate/Bestow, two destination cards (Forged Key etc.)");
        say!("  frames and this console: {}", frames.display());
        say!(
            "  run state (survives a restart): {}",
            data_dir().join("state.json").display()
        );
        say!("  hotkeys: Ctrl+Alt+B boxes on/off, Ctrl+Alt+R read again, Ctrl+Alt+Q quit");

        let (tx, rx) = channel();
        let _ = TO_WORKER.set(tx.clone());
        if !LITE {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let stdin = std::io::stdin();
                let mut line = String::new();
                while stdin.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
                    let _ = tx.send(Msg::Command(line.trim().to_string()));
                    line.clear();
                }
            });
        }
        std::thread::spawn(move || worker(rx, log, frames));

        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None).map_err(|e| e.to_string())?.into();
            let class = w!("TartarusOverlay");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE,
                class,
                w!("Tartarus overlay"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                instance,
                None,
            )
            .map_err(|e| e.to_string())?;
            add_tray(hwnd, instance);
            // keep the boxes out of screen captures (Windows 10 2004 and later): on some computers a capture otherwise
            // includes them, and the reader would see its own boxes over the game
            let _ = SetWindowDisplayAffinity(hwnd, WINDOW_DISPLAY_AFFINITY(0x11));
            register_hotkeys(hwnd);
            // often enough for the progress bar to move smoothly
            SetTimer(hwnd, 1, 40, None);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }
}
