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
        RegisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    };
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
    /// A look's time budget, and the fewest futures a look plays.
    const LOOK_MS: f64 = 900.0;
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

    unsafe fn tray_menu(hwnd: HWND) {
        let Ok(m) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(m, MF_STRING, MENU_BOXES, w!("Boxes on/off"));
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
            s.changed = true;
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
                set_line(None);
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
            let new_screen = Self::changed_spots(&print, &self.last_print) > 60;
            let had_advice = !self.shown_boxes.is_empty() || self.popup != Popup::None;
            if new_screen {
                set_boxes(Vec::new());
                set_line(None);
                self.shown_boxes.clear();
            }
            set_busy(new_screen && had_advice);
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
            // as many futures as fit in about a second on this computer (measured on the last looks), so a slower
            // computer or a busy game gets a box as soon, a little rougher, and later looks sharpen it
            let speed = self.ms_per_future[fast as usize];
            let futures = if speed > 0.0 {
                ((LOOK_MS / speed) as usize).clamp(MIN_FUTURES, FUTURES)
            } else {
                FUTURES / 2
            };
            tartarus::advisor::set_look(self.pooled_looks as u64);
            let solved = Instant::now();
            let mut a = pool(fast).install(|| advise(&f, popup, st, futures));
            if !a.options.is_empty() {
                let per = solved.elapsed().as_secs_f64() * 1000.0 / futures as f64;
                let m = &mut self.ms_per_future[fast as usize];
                *m = if *m > 0.0 { 0.5 * *m + 0.5 * per } else { per };
            }
            set_busy(false);
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
            } else if !self.shown_boxes.is_empty() {
                set_boxes(self.shown_boxes.clone());
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
            set_line(
                a.options
                    .first()
                    .filter(|_| !self.shown_boxes.is_empty())
                    .map(|o| tartarus::font::chance_line(a.unit, o.1, o.3)),
            );
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
        let (boxes, game, show, line, busy) = {
            let mut s = shared().lock().unwrap();
            s.changed = false;
            (s.boxes.clone(), s.game, s.show, s.line.clone(), s.busy)
        };
        unsafe {
            let Some((gx, gy, gw, gh)) = game.filter(|_| show && (!boxes.is_empty() || busy)) else {
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
                        [90, 255, 40, 255]
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
            // the line at the top centre: white text on a dark band
            if let Some(line) = line {
                let scale = (gh / 400).max(2) as usize;
                let (tw, th, mask) = tartarus::font::render(&line, scale);
                let pad = 4 * scale as i32;
                let (tx, ty) = (gw / 2 - tw as i32 / 2, 6 * scale as i32);
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
            // the progress bar, under the line's place: how much of the solving is done
            if busy {
                let scale = (gh / 400).max(2);
                let (bw, bh) = (120 * scale, 3 * scale);
                let (bx, by) = (gw / 2 - bw / 2, 6 * scale + 7 * scale + 10 * scale);
                let done = (tartarus::solver::progress() * bw as f64) as i32;
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
        if LITE {
            std::thread::spawn(|| {
                message("Tartarus overlay is running: it draws a green box around the best choice in Tartarus Prison.\n\nCtrl+Alt+B: boxes on/off\nCtrl+Alt+R: read the screen again\nCtrl+Alt+Q: quit")
            });
        }

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
            for (id, vk) in [(HK_BOXES, 'B'), (HK_REDO, 'R'), (HK_QUIT, 'Q')] {
                if RegisterHotKey(
                    hwnd,
                    id,
                    HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0 | MOD_NOREPEAT.0),
                    vk as u32,
                )
                .is_err()
                {
                    say!("  [!] hotkey Ctrl+Alt+{vk} is taken by another program");
                }
            }
            SetTimer(hwnd, 1, 150, None);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }
}
