//! AuraForge notification-area front end: starts and stops the daemon and
//! edits its settings from a tray menu. Settings live in `%APPDATA%\AuraForge`.

#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, LoadImageW, MessageBoxW, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, TrackPopupMenu, TranslateMessage, HICON,
    HMENU, MB_ICONINFORMATION, MB_OK, MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR,
    MF_STRING, MSG, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    IMAGE_ICON, LR_DEFAULTCOLOR, SM_CXSMICON, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};

use auraforge::config::Config;
use auraforge::engine::Engine;
use auraforge::{capture, govee, logging, protocol};

macro_rules! log {
    ($($arg:tt)*) => { logging::write(format_args!($($arg)*)) };
}

const WM_TRAY: u32 = WM_APP + 1;
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("AuraForge");
/// Autostart entry written before the rename to AuraForge.
const OLD_RUN_VALUE: PCWSTR = w!("TyrfingAmbilight");
/// Icon resources embedded by `build.rs`.
const ICON_RUNNING: u16 = 1;
const ICON_STOPPED: u16 = 2;

/// Menu command ids; list items add their index to the base.
const ID_TOGGLE: usize = 1;
const ID_FANS: usize = 2;
const ID_LOL: usize = 3;
const ID_DEMO: usize = 4;
const ID_LOG: usize = 5;
const ID_AUTOSTART: usize = 6;
const ID_QUIT: usize = 7;
const ID_GOVEE_OFF: usize = 8;
const ID_GOVEE_SCAN: usize = 9;
const ID_KEYBOARD: usize = 100;
const ID_THEME: usize = 200;
const ID_COLOR: usize = 300;
const ID_PALETTE: usize = 400;
const ID_SPEED: usize = 500;
const ID_BRIGHTNESS: usize = 600;
const ID_MONITOR: usize = 700;
const ID_GOVEE: usize = 800;

const KEYBOARD_MODES: [(KeyboardMode, &str); 3] = [
    (KeyboardMode::Theme, "Animated theme + LoL effects"),
    (KeyboardMode::Screen, "Follow screen (single color)"),
    (KeyboardMode::PerKey, "Follow screen (per key)"),
];
/// Labels for `protocol::THEMES`, same order.
const THEME_LABELS: [&str; 17] = [
    "Static",
    "Breathing",
    "Wave",
    "Rain",
    "Horse race",
    "Stars",
    "Reactive",
    "Water drop",
    "Cross",
    "Ripple",
    "Aurora",
    "Bomb",
    "Heatmap",
    "Spotlight",
    "Radar",
    "Snake",
    "Convolution",
];
const COLORS: [(&str, &str); 6] = [
    ("Hextech", "0AC8B9"),
    ("Gold", "C89B3C"),
    ("Rift green", "3CB44B"),
    ("Baron purple", "8C3CDC"),
    ("Red", "DC1E1E"),
    ("White", "FFFFFF"),
];
const PALETTES: [(&str, &str); 3] = [("fixed", "Chosen color"), ("rainbow", "Rainbow"), ("random", "Random")];
const SPEEDS: [u8; 5] = [0, 1, 2, 3, 4];
const BRIGHTNESS: [u8; 4] = [25, 50, 75, 100];

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum KeyboardMode {
    Theme,
    Screen,
    PerKey,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    /// Whether the daemon was running when the tray last saved; restored on launch.
    running: bool,
    monitor: u32,
    keyboard: KeyboardMode,
    theme: String,
    color: String,
    palette: String,
    speed: u8,
    brightness: u8,
    fans: bool,
    lol: bool,
    /// IP of the Govee strip to paint; off until one is picked from a scan.
    govee: Option<String>,
    /// Strips found by the last scan.
    govee_devices: Vec<GoveeDevice>,
}

#[derive(Clone, Serialize, Deserialize)]
struct GoveeDevice {
    ip: String,
    sku: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            running: true,
            monitor: 0,
            keyboard: KeyboardMode::Theme,
            theme: "aurora".into(),
            color: "0AC8B9".into(),
            palette: "fixed".into(),
            speed: 2,
            brightness: 100,
            fans: true,
            lol: true,
            govee: None,
            govee_devices: Vec::new(),
        }
    }
}

impl Settings {
    /// Command line accepted by `Config::from_args`, so the tray and the
    /// console daemon share one validated configuration path.
    fn args(&self, demo: bool) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "--monitor".into(),
            self.monitor.to_string(),
            "--brightness".into(),
            self.brightness.to_string(),
        ];
        match self.keyboard {
            KeyboardMode::Screen => {}
            KeyboardMode::PerKey => args.push("--per-key".into()),
            KeyboardMode::Theme => args.extend([
                "--keyboard-idle".into(),
                self.color.clone(),
                "--keyboard-theme".into(),
                self.theme.clone(),
                "--theme-color".into(),
                self.palette.clone(),
                "--theme-speed".into(),
                self.speed.to_string(),
            ]),
        }
        for (on, flag) in [(self.fans, "--fans"), (self.lol, "--lol"), (demo, "--lol-demo")] {
            if on {
                args.push(flag.into());
            }
        }
        if let Some(ip) = &self.govee {
            args.extend(["--govee".into(), ip.clone()]);
        }
        args
    }

    fn load(path: &PathBuf) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &PathBuf) {
        let json = serde_json::to_string_pretty(self).expect("settings serialize");
        if let Err(e) = std::fs::write(path, json) {
            log!("cannot save settings: {e}");
        }
    }
}

struct App {
    hwnd: HWND,
    settings: Settings,
    settings_path: PathBuf,
    log_path: PathBuf,
    engine: Option<Engine>,
    demo: bool,
    /// Tray icons while running and while stopped.
    icons: [HICON; 2],
    monitors: Vec<String>,
}

impl App {
    fn start(&mut self) {
        let args = self.settings.args(self.demo);
        log!("starting: {}", args.join(" "));
        match Config::from_args(args.into_iter()) {
            Ok(Some(config)) => self.engine = Some(Engine::start(config)),
            Ok(None) => {}
            Err(e) => log!("invalid settings: {e}"),
        }
    }

    fn stop(&mut self) {
        if let Some(engine) = self.engine.take() {
            engine.stop();
        }
    }

    /// Saves the settings and restarts a running daemon so they take effect.
    fn apply(&mut self) {
        if self.engine.is_some() {
            self.stop();
            self.start();
        }
        self.settings.running = self.engine.is_some();
        self.settings.save(&self.settings_path);
        self.refresh_icon(NIM_MODIFY);
    }

    fn refresh_icon(&mut self, action: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE) {
        let running = self.engine.is_some();
        let tip = if running { "AuraForge: running" } else { "AuraForge: stopped" };
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: self.icons[usize::from(!running)],
            ..Default::default()
        };
        for (dst, src) in data.szTip.iter_mut().zip(tip.encode_utf16()) {
            *dst = src;
        }
        unsafe {
            let _ = Shell_NotifyIconW(action, &data);
        }
    }

    fn build_menu(&self) -> windows::core::Result<HMENU> {
        let s = &self.settings;
        let running = self.engine.is_some();
        let theme_mode = s.keyboard == KeyboardMode::Theme;
        unsafe {
            let menu = CreatePopupMenu()?;
            let title = if running { "AuraForge — running" } else { "AuraForge — stopped" };
            item(menu, 0, title, false, false);
            item(menu, ID_TOGGLE, if running { "Stop" } else { "Start" }, false, true);
            separator(menu);

            let keyboard = CreatePopupMenu()?;
            for (i, (mode, label)) in KEYBOARD_MODES.iter().enumerate() {
                item(keyboard, ID_KEYBOARD + i, label, s.keyboard == *mode, true);
            }
            submenu(menu, keyboard, "Keyboard", true);

            let themes = CreatePopupMenu()?;
            for (i, ((name, _), label)) in protocol::THEMES.iter().zip(THEME_LABELS).enumerate() {
                item(themes, ID_THEME + i, label, s.theme == *name, true);
            }
            submenu(menu, themes, "Animated theme", theme_mode);

            let colors = CreatePopupMenu()?;
            for (i, (label, hex)) in COLORS.iter().enumerate() {
                item(colors, ID_COLOR + i, label, s.color.eq_ignore_ascii_case(hex), true);
            }
            separator(colors);
            for (i, (value, label)) in PALETTES.iter().enumerate() {
                item(colors, ID_PALETTE + i, label, s.palette == *value, true);
            }
            submenu(menu, colors, "Keyboard color", theme_mode);

            let speeds = CreatePopupMenu()?;
            for (i, speed) in SPEEDS.iter().enumerate() {
                item(speeds, ID_SPEED + i, &speed.to_string(), s.speed == *speed, true);
            }
            submenu(menu, speeds, "Animation speed", theme_mode);

            let brightness = CreatePopupMenu()?;
            for (i, level) in BRIGHTNESS.iter().enumerate() {
                item(brightness, ID_BRIGHTNESS + i, &format!("{level}%"), s.brightness == *level, true);
            }
            submenu(menu, brightness, "Keyboard brightness", true);

            let monitors = CreatePopupMenu()?;
            for (i, m) in self.monitors.iter().enumerate() {
                item(monitors, ID_MONITOR + i, &format!("{i}: {m}"), s.monitor as usize == i, true);
            }
            submenu(menu, monitors, "Captured screen", true);
            separator(menu);

            let strips = CreatePopupMenu()?;
            item(strips, ID_GOVEE_OFF, "Off", s.govee.is_none(), true);
            for (i, d) in s.govee_devices.iter().enumerate() {
                let checked = s.govee.as_deref() == Some(d.ip.as_str());
                item(strips, ID_GOVEE + i, &format!("{} — {}", d.sku, d.ip), checked, true);
            }
            separator(strips);
            item(strips, ID_GOVEE_SCAN, "Search the network for strips", false, true);

            item(menu, ID_FANS, "Fans (follow the screen)", s.fans, true);
            submenu(menu, strips, "Govee strip (follows the fans)", true);
            item(menu, ID_LOL, "League of Legends effects", s.lol, true);
            item(menu, ID_DEMO, "Effects demo (loop)", self.demo, true);
            separator(menu);
            item(menu, ID_LOG, "Open log", false, true);
            item(menu, ID_AUTOSTART, "Start with Windows", autostart_enabled(), true);
            separator(menu);
            item(menu, ID_QUIT, "Quit", false, true);
            Ok(menu)
        }
    }

    fn command(&mut self, id: usize) {
        let s = &mut self.settings;
        match id {
            ID_TOGGLE => {
                if self.engine.is_some() {
                    self.stop();
                } else {
                    self.start();
                }
            }
            ID_FANS => s.fans = !s.fans,
            ID_LOL => s.lol = !s.lol,
            ID_DEMO => self.demo = !self.demo,
            ID_GOVEE_OFF => s.govee = None,
            ID_GOVEE_SCAN => {
                self.scan_govee();
                return;
            }
            ID_LOG => {
                unsafe {
                    ShellExecuteW(
                        None,
                        w!("open"),
                        &HSTRING::from(self.log_path.as_os_str()),
                        PCWSTR::null(),
                        PCWSTR::null(),
                        SW_SHOWNORMAL,
                    );
                }
                return;
            }
            ID_AUTOSTART => {
                set_autostart(!autostart_enabled());
                return;
            }
            ID_QUIT => {
                unsafe { PostQuitMessage(0) };
                return;
            }
            _ => {
                let pick = |base: usize, len: usize| (base..base + len).contains(&id).then(|| id - base);
                if let Some(i) = pick(ID_KEYBOARD, KEYBOARD_MODES.len()) {
                    s.keyboard = KEYBOARD_MODES[i].0;
                } else if let Some(i) = pick(ID_THEME, protocol::THEMES.len()) {
                    s.theme = protocol::THEMES[i].0.into();
                } else if let Some(i) = pick(ID_COLOR, COLORS.len()) {
                    s.color = COLORS[i].1.into();
                    s.palette = "fixed".into();
                } else if let Some(i) = pick(ID_PALETTE, PALETTES.len()) {
                    s.palette = PALETTES[i].0.into();
                } else if let Some(i) = pick(ID_SPEED, SPEEDS.len()) {
                    s.speed = SPEEDS[i];
                } else if let Some(i) = pick(ID_BRIGHTNESS, BRIGHTNESS.len()) {
                    s.brightness = BRIGHTNESS[i];
                } else if let Some(i) = pick(ID_GOVEE, s.govee_devices.len()) {
                    s.govee = Some(s.govee_devices[i].ip.clone());
                } else if let Some(i) = pick(ID_MONITOR, self.monitors.len()) {
                    s.monitor = i as u32;
                } else {
                    return;
                }
            }
        }
        if id == ID_TOGGLE {
            self.settings.running = self.engine.is_some();
            self.settings.save(&self.settings_path);
            self.refresh_icon(NIM_MODIFY);
        } else {
            self.apply();
        }
    }
}

impl App {
    /// Lists the strips answering on the LAN; picking one is a separate step,
    /// so nothing is sent to a strip the user has not chosen.
    fn scan_govee(&mut self) {
        let text = match govee::scan(Duration::from_secs(3)) {
            Ok(devices) if devices.is_empty() => "No Govee strip answered.\n\n\
                Enable \"LAN Control\" for the strip in the Govee Home app and check that it is \
                on the same network as this PC."
                .to_owned(),
            Ok(devices) => {
                self.settings.govee_devices =
                    devices.iter().map(|d| GoveeDevice { ip: d.ip.to_string(), sku: d.sku.clone() }).collect();
                self.settings.save(&self.settings_path);
                let list: Vec<String> = devices.iter().map(|d| format!("{} — {}", d.sku, d.ip)).collect();
                format!("Found:\n{}\n\nPick one from the Govee strip menu.", list.join("\n"))
            }
            Err(e) => format!("Search failed: {e}"),
        };
        log!("Govee scan: {}", text.replace('\n', " "));
        unsafe {
            MessageBoxW(Some(self.hwnd), &HSTRING::from(text), w!("Govee strip"), MB_OK | MB_ICONINFORMATION);
        }
    }
}

unsafe fn item(menu: HMENU, id: usize, text: &str, checked: bool, enabled: bool) {
    let mut flags = MF_STRING;
    if checked {
        flags |= MF_CHECKED;
    }
    if !enabled {
        flags |= MF_GRAYED;
    }
    let _ = unsafe { AppendMenuW(menu, flags, id, &HSTRING::from(text)) };
}

unsafe fn submenu(menu: HMENU, sub: HMENU, text: &str, enabled: bool) {
    let flags = if enabled { MF_POPUP } else { MF_POPUP | MF_GRAYED };
    let _ = unsafe { AppendMenuW(menu, flags, sub.0 as usize, &HSTRING::from(text)) };
}

unsafe fn separator(menu: HMENU) {
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR | MENU_ITEM_FLAGS(0), 0, PCWSTR::null()) };
}

/// Embedded icon resource `id` at the notification area's size.
fn load_icon(id: u16) -> HICON {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) };
    unsafe {
        let instance = GetModuleHandleW(None).ok();
        LoadImageW(instance.map(Into::into), PCWSTR(usize::from(id) as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR)
    }
    .map(|h| HICON(h.0))
    .unwrap_or_default()
}

fn autostart_enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, None) == ERROR_SUCCESS }
}

/// Moves an autostart entry left by Tyrfing Ambilight over to this executable.
fn migrate_autostart() {
    let old = unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, OLD_RUN_VALUE, RRF_RT_REG_SZ, None, None, None) };
    if old == ERROR_SUCCESS {
        let _ = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, OLD_RUN_VALUE) };
        set_autostart(true);
    }
}

fn set_autostart(on: bool) {
    let status = if on {
        let exe = std::env::current_exe().unwrap_or_default();
        let command: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain(Some(0)).collect();
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(command.as_ptr().cast()),
                (command.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE) }
    };
    if status != ERROR_SUCCESS {
        log!("cannot change autostart: {status:?}");
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}
/// Broadcast when Explorer restarts; the icon must be added again.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|app| app.borrow_mut().as_mut().map(f))
}

fn show_menu(hwnd: HWND) {
    // The menu runs a modal loop that re-enters the window procedure, so the
    // app is not borrowed while it is open.
    let Some(Ok(menu)) = with_app(|app| app.build_menu()) else { return };
    let mut pt = POINT::default();
    let id = unsafe {
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let id = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        id.0 as usize
    };
    if id != 0 {
        with_app(|app| app.command(id));
    }
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            let event = (lparam.0 & 0xFFFF) as u32;
            if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
                show_menu(hwnd);
            }
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            with_app(|app| app.refresh_icon(NIM_ADD));
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn main() {
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    let dir = appdata.join("AuraForge");
    // Settings saved before the rename to AuraForge.
    if !dir.exists() {
        let _ = std::fs::rename(appdata.join("tyrfing-ambilight"), &dir);
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::remove_file(dir.join("ambilight.log"));
    let log_path = dir.join("auraforge.log");
    let _ = logging::to_file(&log_path);

    // Two daemons would fight over the same devices.
    let _instance = unsafe { CreateMutexW(None, false, w!("Local\\AuraForgeTray")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            MessageBoxW(None, w!("AuraForge is already running in the notification area."), w!("AuraForge"), MB_OK | MB_ICONINFORMATION);
        }
        return;
    }

    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).expect("module handle");
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("AuraForgeTray"),
            ..Default::default()
        };
        RegisterClassW(&class);
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
        // Never shown: it only receives the icon's messages.
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("AuraForgeTray"),
            w!("AuraForge"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .expect("tray window")
    };

    let settings_path = dir.join("settings.json");
    let settings = Settings::load(&settings_path);
    let monitors = capture::monitors().unwrap_or_default();
    let mut app = App {
        hwnd,
        settings,
        settings_path,
        log_path,
        engine: None,
        demo: false,
        icons: [load_icon(ICON_RUNNING), load_icon(ICON_STOPPED)],
        monitors,
    };
    migrate_autostart();
    if app.settings.running {
        app.start();
    }
    app.refresh_icon(NIM_ADD);
    APP.with(|a| *a.borrow_mut() = Some(app));

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    if let Some(mut app) = APP.with(|a| a.borrow_mut().take()) {
        // Quitting keeps `running` as it was, so the next launch resumes.
        app.stop();
        let data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            ..Default::default()
        };
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            for icon in app.icons {
                let _ = DestroyIcon(icon);
            }
        }
    }
}
