//! Viewport placement (docs/EDITOR.md E7.1): put the game's window next to the editor's.
//!
//! The editor and the game stay two processes with two windows; nothing is embedded. After the
//! game starts, the editor looks the game window up by the title it passed in `EFE_WINDOW_TITLE`
//! and moves it to the right of its own window. This is best effort and optional: on Windows
//! (PowerShell and user32) and X11 (`xdotool`) it works in principle; on Wayland a client may
//! not place windows, so nothing is done and the reason is returned for the log. Every failure
//! is an `Err(String)`, never a panic.
//!
//! The geometry is the pure [`game_rect`]; the platform calls are thin shell-outs around it.

use std::process::Command;

/// A window rectangle in screen pixels (origin top-left).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Gap between the editor and the game window, in pixels.
pub const GAP: i32 = 8;

/// Where the game window goes: to the right of `editor`, top edges aligned. When the game does
/// not fit on the right it goes to the left of the editor, and when it fits on neither side it
/// is clamped to the right screen edge (overlapping the editor). The game is shrunk to the
/// screen if it is larger, and it never leaves the screen vertically.
pub fn game_rect(editor: Rect, screen: (i32, i32), game: (i32, i32)) -> Rect {
    let (sw, sh) = (screen.0.max(1), screen.1.max(1));
    let w = game.0.clamp(1, sw);
    let h = game.1.clamp(1, sh);
    let right = editor.x + editor.w + GAP;
    let left = editor.x - GAP - w;
    let x = if right + w <= sw {
        right
    } else if left >= 0 {
        left
    } else {
        sw - w
    };
    let y = editor.y.min(sh - h).max(0);
    Rect { x, y, w, h }
}

/// What the display session allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    Windows,
    X11,
    /// Wayland (or no display variable at all): clients cannot place windows.
    Unsupported,
}

/// Decide the session from `cfg!(windows)` and the Linux display variables (`WAYLAND_DISPLAY`,
/// `DISPLAY`). Wayland wins when both are set (a Wayland client cannot be placed even though an
/// XWayland server exists).
pub fn session_kind(windows: bool, wayland_display: Option<&str>, display: Option<&str>) -> Session {
    let set = |v: Option<&str>| v.is_some_and(|s| !s.is_empty());
    if windows {
        Session::Windows
    } else if set(wayland_display) {
        Session::Unsupported
    } else if set(display) {
        Session::X11
    } else {
        Session::Unsupported
    }
}

pub fn current_session() -> Session {
    session_kind(
        cfg!(windows),
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        std::env::var("DISPLAY").ok().as_deref(),
    )
}

/// The message logged when placement cannot be done.
pub const UNSUPPORTED: &str = "placement not supported on this session";

/// The unique window title the game is asked to use for this editor process.
pub fn game_window_title(editor_pid: u32) -> String {
    format!("efe-game-{editor_pid}")
}

/// Parse `xdotool getwindowgeometry --shell` output (`X=..`, `Y=..`, `WIDTH=..`, `HEIGHT=..`).
pub fn parse_geometry_shell(out: &str) -> Option<Rect> {
    let get = |key: &str| out.lines().find_map(|l| l.trim().strip_prefix(key)?.strip_prefix('=')?.trim().parse::<i32>().ok());
    Some(Rect { x: get("X")?, y: get("Y")?, w: get("WIDTH")?, h: get("HEIGHT")? })
}

/// Parse `xdotool getdisplaygeometry` output (`1920 1080`).
pub fn parse_display_geometry(out: &str) -> Option<(i32, i32)> {
    let mut it = out.split_whitespace().map(|p| p.parse::<i32>());
    match (it.next(), it.next()) {
        (Some(Ok(w)), Some(Ok(h))) if w > 0 && h > 0 => Some((w, h)),
        _ => None,
    }
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program).args(args).output().map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{program} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn first_id(out: &str) -> Option<String> {
    out.split_whitespace().next().map(str::to_string)
}

/// Move and size the window titled `game_title` to sit next to the editor window (the window
/// owned by process `editor_pid`). Returns the rectangle used.
pub fn place_game(editor_pid: u32, game_title: &str) -> Result<Rect, String> {
    match current_session() {
        Session::Unsupported => Err(UNSUPPORTED.into()),
        Session::X11 => place_x11(editor_pid, game_title),
        Session::Windows => place_windows(editor_pid, game_title),
    }
}

fn place_x11(editor_pid: u32, game_title: &str) -> Result<Rect, String> {
    let pid = editor_pid.to_string();
    let editor_id = first_id(&run("xdotool", &["search", "--onlyvisible", "--pid", &pid])?).ok_or("editor window not found")?;
    let game_id = first_id(&run("xdotool", &["search", "--onlyvisible", "--name", &format!("^{}$", game_title)])?).ok_or("game window not found (yet)")?;
    let editor = parse_geometry_shell(&run("xdotool", &["getwindowgeometry", "--shell", &editor_id])?).ok_or("cannot read the editor window geometry")?;
    let game = parse_geometry_shell(&run("xdotool", &["getwindowgeometry", "--shell", &game_id])?).ok_or("cannot read the game window geometry")?;
    let screen = parse_display_geometry(&run("xdotool", &["getdisplaygeometry"])?).ok_or("cannot read the screen size")?;
    let r = game_rect(editor, screen, (game.w, game.h));
    run("xdotool", &["windowsize", &game_id, &r.w.to_string(), &r.h.to_string()])?;
    run("xdotool", &["windowmove", &game_id, &r.x.to_string(), &r.y.to_string()])?;
    Ok(r)
}

/// Windows: a short PowerShell script over user32 (`GetWindowRect`, `MoveWindow`) so the editor
/// needs no extra crate. Not exercised in this repository's CI (Linux only).
fn place_windows(editor_pid: u32, game_title: &str) -> Result<Rect, String> {
    if !game_title.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("unsafe window title".into());
    }
    let script = format!(
        r#"Add-Type @"
using System; using System.Runtime.InteropServices;
public class W {{
 [StructLayout(LayoutKind.Sequential)] public struct R {{ public int L, T, R, B; }}
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
 [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool repaint);
 [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
}}
"@
$e = (Get-Process -Id {editor_pid}).MainWindowHandle
$g = (Get-Process | Where-Object {{ $_.MainWindowTitle -eq '{game_title}' }} | Select-Object -First 1).MainWindowHandle
if ($e -eq 0 -or -not $g) {{ Write-Error 'window not found'; exit 1 }}
$er = New-Object W+R; $gr = New-Object W+R
[void][W]::GetWindowRect($e, [ref]$er); [void][W]::GetWindowRect($g, [ref]$gr)
Write-Output "$($er.L) $($er.T) $($er.R - $er.L) $($er.B - $er.T) $($gr.R - $gr.L) $($gr.B - $gr.T) $([W]::GetSystemMetrics(0)) $([W]::GetSystemMetrics(1))"
"#
    );
    let out = run("powershell", &["-NoProfile", "-Command", &script])?;
    let n: Vec<i32> = out.split_whitespace().filter_map(|p| p.parse().ok()).collect();
    let [ex, ey, ew, eh, gw, gh, sw, sh] = n[..] else { return Err("cannot read window geometry".into()) };
    let r = game_rect(Rect { x: ex, y: ey, w: ew, h: eh }, (sw, sh), (gw, gh));
    let mv = format!(
        r#"Add-Type -MemberDefinition '[DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h,int x,int y,int w,int hh,bool r);' -Name M -Namespace N
$g = (Get-Process | Where-Object {{ $_.MainWindowTitle -eq '{game_title}' }} | Select-Object -First 1).MainWindowHandle
[void][N.M]::MoveWindow($g, {}, {}, {}, {}, $true)"#,
        r.x, r.y, r.w, r.h
    );
    run("powershell", &["-NoProfile", "-Command", &mv])?;
    Ok(r)
}

/// Retry [`place_game`] until the game window exists (it appears a moment after the process
/// starts) or `attempts` run out; the last error is returned. Sleeps `every` between tries.
/// An unsupported session returns at once. Runs on the caller's thread: call it from a worker.
pub fn place_with_retry(editor_pid: u32, game_title: &str, attempts: u32, every: std::time::Duration) -> Result<Rect, String> {
    let mut last = String::from("no attempt made");
    for i in 0..attempts {
        match place_game(editor_pid, game_title) {
            Ok(r) => return Ok(r),
            Err(e) if e == UNSUPPORTED => return Err(e),
            Err(e) => last = e,
        }
        if i + 1 < attempts {
            std::thread::sleep(every);
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (i32, i32) = (1920, 1080);
    fn editor(x: i32, y: i32) -> Rect {
        Rect { x, y, w: 800, h: 600 }
    }

    #[test]
    fn goes_right_of_the_editor_with_a_gap() {
        let r = game_rect(editor(100, 50), SCREEN, (640, 480));
        assert_eq!(r, Rect { x: 100 + 800 + GAP, y: 50, w: 640, h: 480 });
    }

    #[test]
    fn falls_back_to_the_left_then_the_screen_edge() {
        // No room on the right (editor at 1300..2100 is itself off-screen-ish), room on the left.
        let r = game_rect(Rect { x: 1100, y: 0, w: 800, h: 600 }, SCREEN, (640, 480));
        assert_eq!(r.x, 1100 - GAP - 640);
        // No room on either side: clamp to the right screen edge.
        let r = game_rect(Rect { x: 200, y: 0, w: 1500, h: 600 }, SCREEN, (640, 480));
        assert_eq!(r.x, 1920 - 640);
    }

    #[test]
    fn stays_on_screen_vertically_and_shrinks_to_fit() {
        let r = game_rect(editor(0, 900), SCREEN, (640, 480));
        assert_eq!(r.y, 1080 - 480);
        let r = game_rect(editor(-50, -20), SCREEN, (640, 480));
        assert_eq!(r.y, 0);
        let big = game_rect(editor(0, 0), (1280, 720), (4000, 3000));
        assert_eq!((big.w, big.h), (1280, 720));
        assert!(big.x >= 0 && big.x + big.w <= 1280 && big.y >= 0 && big.y + big.h <= 720);
    }

    #[test]
    fn session_detection() {
        assert_eq!(session_kind(true, None, None), Session::Windows);
        assert_eq!(session_kind(false, Some("wayland-0"), Some(":0")), Session::Unsupported);
        assert_eq!(session_kind(false, Some(""), Some(":0")), Session::X11);
        assert_eq!(session_kind(false, None, Some(":1")), Session::X11);
        assert_eq!(session_kind(false, None, None), Session::Unsupported);
    }

    #[test]
    fn parses_xdotool_output() {
        let g = "WINDOW=123\nX=10\nY=20\nWIDTH=640\nHEIGHT=480\nSCREEN=0\n";
        assert_eq!(parse_geometry_shell(g), Some(Rect { x: 10, y: 20, w: 640, h: 480 }));
        assert_eq!(parse_geometry_shell("X=1\nY=2\n"), None);
        assert_eq!(parse_display_geometry("1920 1080\n"), Some((1920, 1080)));
        assert_eq!(parse_display_geometry("oops"), None);
    }

    #[test]
    fn title_is_per_editor_process() {
        assert_eq!(game_window_title(42), "efe-game-42");
    }
}
