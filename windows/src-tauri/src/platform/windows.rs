// Windows: Win32 for the island window and the cursor, %APPDATA% for files.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use tauri::{AppHandle, Manager, WebviewWindow};

use ::windows::core::{BOOL, PWSTR};
use ::windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, HWND, LPARAM, LocalFree, POINT};
use ::windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use ::windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use ::windows::Win32::System::Ole::RevokeDragDrop;
use ::windows::Win32::System::SystemInformation::GetLocalTime;
use ::windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use ::windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use ::windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetClassNameW, GetCursorPos, GetWindowLongPtrW, SetWindowLongPtrW,
    GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

use super::LocalTime;
use crate::island::WINDOW_LABEL;

/// File name of the Claude Code relay.
pub const HOOK_EXE: &str = "coucou-hook.exe";

/// Environment variable holding the home directory.
pub const HOME_VAR: &str = "USERPROFILE";

/// Keeps spawned helpers from flashing a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ── Files ─────────────────────────────────────────────────────────────────────

/// %APPDATA%\Coucou — preferences.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Coucou")
}

/// %LOCALAPPDATA%\Coucou — where coucou-hook.exe, the inbox and the log live.
pub fn local_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Coucou")
}

/// %APPDATA% and %LOCALAPPDATA% are already private to the user.
pub fn ensure_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Nothing to set up before the webview starts.
pub fn prepare_environment() {}

pub fn local_time() -> LocalTime {
    let t = unsafe { GetLocalTime() };
    LocalTime {
        year: t.wYear.into(),
        month: t.wMonth.into(),
        day: t.wDay.into(),
        hour: t.wHour.into(),
        minute: t.wMinute.into(),
        second: t.wSecond.into(),
    }
}

// ── Processes ─────────────────────────────────────────────────────────────────

/// Spawned helpers must never flash a console window.
pub fn no_console(cmd: &mut Command) -> &mut Command {
    cmd.creation_flags(CREATE_NO_WINDOW)
}

pub fn open_url(url: &str) {
    let _ = no_console(Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url]))
        .spawn();
}

/// Windows Terminal when it is installed, PowerShell otherwise, started in `dir`.
pub fn open_terminal(dir: Option<&std::path::Path>) {
    let mut terminal = Command::new("wt.exe");
    if let Some(dir) = dir {
        terminal.arg("-d").arg(dir).current_dir(dir);
    }
    if terminal.spawn().is_ok() {
        return;
    }
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let mut shell = Command::new("powershell.exe");
    shell.creation_flags(CREATE_NEW_CONSOLE);
    if let Some(dir) = dir {
        shell.current_dir(dir);
    }
    let _ = shell.spawn();
}

pub fn reveal_folder(path: &str) {
    let _ = Command::new("explorer").arg(path).spawn();
}

/// Our own `where`: walks %PATH% against %PATHEXT%, no shell involved.
/// Rust quotes arguments correctly for `.cmd`/`.bat` targets since 1.77, so
/// spawning `code.cmd` directly is safe.
pub fn find_on_path(stem: &str) -> Option<PathBuf> {
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let dirs = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&dirs) {
        for ext in exts.split(';').filter(|e| !e.is_empty()) {
            let candidate = dir.join(format!("{stem}{}", ext.to_lowercase()));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

// ── Who we are ────────────────────────────────────────────────────────────────
//
// Named pipes share one machine-wide namespace, so the SID in the name is what
// keeps two accounts on the same machine from ever meeting on `coucou-*`.
// coucou-hook computes the same string (hook/src/win.rs) and additionally checks
// that the process serving the pipe really is us.

/// The SID of the account this process runs as, as `S-1-5-21-…`.
pub fn current_user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;

        // First call sizes the buffer, second fills it.
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        if needed == 0 {
            let _ = CloseHandle(token);
            return None;
        }
        let mut buf = vec![0u8; needed as usize];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .is_ok();
        let _ = CloseHandle(token);
        if !ok {
            return None;
        }

        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text).ok()?;
        let sid = text.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(text.0 as *mut _)));
        sid
    }
}

// ── Cursor ────────────────────────────────────────────────────────────────────

/// The 60 Hz poll reads the cursor and flips click-through from it.
pub const CURSOR_POLL: bool = true;

/// Cursor position in physical screen pixels.
pub fn cursor_physical() -> Option<(f64, f64)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x as f64, p.y as f64))
}

/// True while the left mouse button is held — the only signal we get that a
/// drag might be in flight before it reaches the window.
pub fn left_button_down() -> bool {
    unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 }
}

// ── Island window ─────────────────────────────────────────────────────────────

fn hwnd_of(win: &WebviewWindow) -> Option<HWND> {
    let raw = win.hwnd().ok()?.0 as isize;
    if raw == 0 {
        return None;
    }
    Some(HWND(raw as *mut _))
}

/// Lets dropped files reach the app again.
///
/// wry installs its drop target by walking the webview's child windows **once**,
/// when the webview is created. WebView2 creates `Chrome_RenderWidgetHostHWND`
/// later and registers its own target on it; being the innermost window, that one
/// wins, and since the page has no HTML5 drop handler it refuses everything — the
/// "no drop" cursor, with nothing reaching Tauri. Revoking it makes OLE fall
/// through to the target wry registered on the parent widget, which is the one
/// that feeds Tauri's drag events.
///
/// Cheap and idempotent, so it is simply re-run whenever a drag might be starting.
pub fn unblock_webview_drops(app: &AppHandle) {
    for label in [WINDOW_LABEL, "settings"] {
        let Some(win) = app.get_webview_window(label) else { continue };
        let Some(hwnd) = hwnd_of(&win) else { continue };
        unsafe {
            let _ = EnumChildWindows(Some(hwnd), Some(revoke_render_widget), LPARAM(0));
        }
    }
}

unsafe extern "system" fn revoke_render_widget(hwnd: HWND, _: LPARAM) -> BOOL {
    let mut name = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, &mut name) };
    if len > 0 {
        let class = String::from_utf16_lossy(&name[..len as usize]);
        if class == "Chrome_RenderWidgetHostHWND" {
            let _ = unsafe { RevokeDragDrop(hwnd) };
        }
    }
    true.into()
}

/// WS_EX_NOACTIVATE keeps clicks from stealing focus; WS_EX_TOOLWINDOW keeps the
/// island out of Alt-Tab.
pub fn make_non_activating(win: &WebviewWindow) {
    let Some(hwnd) = hwnd_of(win) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let want = ex | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOOLWINDOW.0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
    }
}

/// Temporarily allow activation so a text field inside the island can be typed in.
pub fn set_activating(win: &WebviewWindow, activating: bool) {
    let Some(hwnd) = hwnd_of(win) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let want = if activating {
            ex & !(WS_EX_NOACTIVATE.0 as isize)
        } else {
            ex | WS_EX_NOACTIVATE.0 as isize
        };
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
    }
}

/// Click-through here is the poll's WS_EX_TRANSPARENT toggle, not a region.
pub fn set_input_region(_win: &WebviewWindow, _rect: Option<(f64, f64, f64, f64)>) {}

// ── System controls ───────────────────────────────────────────────────────────
// What the Controls view drives: the current media session (the one the Windows
// volume flyout shows), the default speaker and microphone, and the per-user
// location switch. Every call is short and blocking; lib.rs keeps them off the
// main thread.

use ::windows::core::{w, PCWSTR};
use ::windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as MediaSession,
    GlobalSystemMediaTransportControlsSessionManager as MediaManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use ::windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use ::windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, IMMDeviceEnumerator, MMDeviceEnumerator,
};
use ::windows::Win32::System::Com::{CoCreateInstance, CoIncrementMTAUsage, CLSCTX_ALL};
use ::windows::Win32::System::Registry::{
    RegGetValueW, RegSetKeyValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_SZ,
    RRF_RT_REG_SZ,
};

use super::{MediaInfo, SystemStatus};

/// Where Windows keeps "let apps use my location": `Value` is "Allow" or "Deny".
/// HKLM holds the switch for the whole PC (admin only), HKCU the user's own.
const LOCATION_KEY: PCWSTR =
    w!(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\location");

/// Core Audio needs COM on the calling thread. Joining the multithreaded
/// apartment once covers every thread that has not chosen one itself.
fn ensure_com() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let _ = CoIncrementMTAUsage();
    });
}

fn err(e: ::windows::core::Error) -> String {
    e.message()
}

fn media_session() -> ::windows::core::Result<MediaSession> {
    MediaManager::RequestAsync()?.get()?.GetCurrentSession()
}

fn media_info() -> ::windows::core::Result<MediaInfo> {
    let session = media_session()?;
    let props = session.TryGetMediaPropertiesAsync()?.get()?;
    Ok(MediaInfo {
        title: props.Title()?.to_string(),
        artist: props.Artist()?.to_string(),
        playing: session.GetPlaybackInfo()?.PlaybackStatus()? == PlaybackStatus::Playing,
    })
}

/// The volume control of the default speaker (`eRender`) or microphone (`eCapture`).
fn endpoint(flow: EDataFlow) -> ::windows::core::Result<IAudioEndpointVolume> {
    ensure_com();
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        devices.GetDefaultAudioEndpoint(flow, eConsole)?.Activate(CLSCTX_ALL, None)
    }
}

fn read_consent(root: HKEY) -> Option<String> {
    let mut buf = [0u16; 32];
    let mut bytes = std::mem::size_of_val(&buf) as u32;
    let status = unsafe {
        RegGetValueW(
            root,
            LOCATION_KEY,
            w!("Value"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// The PC-wide switch wins: with it off, the user's own choice changes nothing.
fn location_state(machine: Option<&str>, user: Option<&str>) -> &'static str {
    match (machine, user) {
        (Some("Deny"), _) => "systemOff",
        (_, Some("Allow")) => "on",
        _ => "off",
    }
}

pub fn system_status() -> SystemStatus {
    let speaker = endpoint(eRender).ok();
    let mic = endpoint(eCapture).ok();
    unsafe {
        SystemStatus {
            supported: true,
            media: media_info().ok(),
            volume: speaker.as_ref().and_then(|s| s.GetMasterVolumeLevelScalar().ok()),
            muted: speaker.as_ref().and_then(|s| s.GetMute().ok()).is_some_and(|m| m.as_bool()),
            mic_muted: mic.as_ref().and_then(|m| m.GetMute().ok()).map(|m| m.as_bool()),
            location: location_state(
                read_consent(HKEY_LOCAL_MACHINE).as_deref(),
                read_consent(HKEY_CURRENT_USER).as_deref(),
            ),
        }
    }
}

/// `action` is "toggle", "next" or "previous".
pub fn media_command(action: &str) -> Result<(), String> {
    let session = media_session().map_err(|_| "nothing is playing".to_string())?;
    let sent = match action {
        "toggle" => session.TryTogglePlayPauseAsync(),
        "next" => session.TrySkipNextAsync(),
        "previous" => session.TrySkipPreviousAsync(),
        other => return Err(format!("unknown media action {other}")),
    };
    sent.and_then(|op| op.get()).map(|_| ()).map_err(err)
}

pub fn set_volume(level: f32) -> Result<(), String> {
    let speaker = endpoint(eRender).map_err(err)?;
    unsafe { speaker.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null()) }.map_err(err)
}

pub fn set_output_muted(muted: bool) -> Result<(), String> {
    let speaker = endpoint(eRender).map_err(err)?;
    unsafe { speaker.SetMute(muted, std::ptr::null()) }.map_err(err)
}

/// Mutes the microphone itself, for every app — not the privacy setting.
pub fn set_mic_muted(muted: bool) -> Result<(), String> {
    let mic = endpoint(eCapture).map_err(|_| "no microphone".to_string())?;
    unsafe { mic.SetMute(muted, std::ptr::null()) }.map_err(err)
}

/// Flips the user's own location switch. The PC-wide one needs an administrator.
pub fn set_location(on: bool) -> Result<(), String> {
    let value: Vec<u16> = if on { "Allow" } else { "Deny" }.encode_utf16().chain([0]).collect();
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            LOCATION_KEY,
            w!("Value"),
            REG_SZ.0,
            Some(value.as_ptr().cast()),
            (value.len() * 2) as u32,
        )
    };
    status.ok().map_err(err)
}

pub fn open_location_settings() {
    open_url("ms-settings:privacy-location");
}

#[cfg(test)]
mod system_tests {
    use super::location_state;

    #[test]
    fn the_pc_wide_location_switch_wins() {
        assert_eq!(location_state(Some("Allow"), Some("Allow")), "on");
        assert_eq!(location_state(Some("Allow"), Some("Deny")), "off");
        assert_eq!(location_state(Some("Deny"), Some("Allow")), "systemOff");
        assert_eq!(location_state(None, Some("Allow")), "on");
        assert_eq!(location_state(Some("Allow"), None), "off");
    }

    /// Flips each real control and puts it back. It touches this PC's sound and
    /// location settings, so it only runs when asked:
    /// `cargo test -p coucou -- --ignored --nocapture live_controls`
    #[test]
    #[ignore]
    fn live_controls_round_trip() {
        use super::*;
        let before = system_status();
        println!("media: {:?}", before.media.as_ref().map(|m| (&m.title, &m.artist, m.playing)));
        println!("volume {:?} muted {} mic muted {:?} location {}", before.volume, before.muted, before.mic_muted, before.location);

        if let Some(volume) = before.volume {
            let other = if volume > 0.5 { volume - 0.1 } else { volume + 0.1 };
            set_volume(other).unwrap();
            assert!((system_status().volume.unwrap() - other).abs() < 0.02);
            set_volume(volume).unwrap();
            set_output_muted(!before.muted).unwrap();
            assert_eq!(system_status().muted, !before.muted);
            set_output_muted(before.muted).unwrap();
        }
        if let Some(mic) = before.mic_muted {
            set_mic_muted(!mic).unwrap();
            assert_eq!(system_status().mic_muted, Some(!mic));
            set_mic_muted(mic).unwrap();
        }
        if before.location != "systemOff" {
            let on = before.location == "on";
            set_location(!on).unwrap();
            assert_eq!(system_status().location, if on { "off" } else { "on" });
            set_location(on).unwrap();
        }
        let after = system_status();
        assert_eq!((after.muted, after.mic_muted, after.location), (before.muted, before.mic_muted, before.location));
    }
}
