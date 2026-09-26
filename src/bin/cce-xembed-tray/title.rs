//! What to call a tray icon.
//!
//! Icon windows are almost always untitled, and their WM_CLASS names the
//! toolkit rather than the app: every Proton program's is `steam_proton`.
//! Worse, a Wine icon is not even the app's window — Wine's tray lives in
//! the prefix's `explorer.exe`, which creates the icon windows for every
//! program in that prefix. So the name comes from the APP's own top-level
//! windows: for a Wine icon, those of the other programs sharing its
//! `WINEPREFIX` (Wine's own `C:\windows\…` processes excluded); for a native
//! icon, those of its own process. Ubisoft Connect titles its windows
//! "Ubisoft Connect" even while it sits hidden in the tray.

use std::collections::HashMap;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

use crate::x11::Atoms;

/// A resolved name, and whether it is final. A fallback (the process or
/// class name) is not: the app may simply not have titled a window yet,
/// so the caller asks again a little later.
pub struct Title {
    pub text: String,
    pub settled: bool,
}

/// Wine windows that exist in every Wine process and name nothing.
const PLUMBING_TITLES: &[&str] = &["Default IME", "MSCTFIME UI"];

pub fn resolve(conn: &RustConnection, root: Window, atoms: &Atoms, icon: Window) -> Title {
    if let Some(own) = window_title(conn, atoms, icon) {
        return Title { text: own, settled: true };
    }
    let pid = cardinal(conn, icon, atoms._NET_WM_PID);
    let app = pid.map(app_pids).unwrap_or_default();
    if !app.is_empty() {
        let names = toplevel_titles(conn, root, atoms, &app);
        if let Some(name) = choose_title(&names) {
            return Title { text: name, settled: true };
        }
    }
    let fallback = app
        .first()
        .and_then(|&p| process_name(p))
        .or_else(|| wm_class(conn, icon))
        .unwrap_or_else(|| "X11 tray icon".to_string());
    Title { text: fallback, settled: false }
}

fn text_property(conn: &RustConnection, window: Window, atom: u32, kind: u32) -> Option<String> {
    let reply = conn.get_property(false, window, atom, kind, 0, 256).ok()?.reply().ok()?;
    let s = String::from_utf8_lossy(&reply.value).trim_end_matches('\0').trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn window_title(conn: &RustConnection, atoms: &Atoms, window: Window) -> Option<String> {
    text_property(conn, window, atoms._NET_WM_NAME, atoms.UTF8_STRING)
        .or_else(|| text_property(conn, window, AtomEnum::WM_NAME.into(), AtomEnum::ANY.into()))
}

fn wm_class(conn: &RustConnection, window: Window) -> Option<String> {
    text_property(conn, window, AtomEnum::WM_CLASS.into(), AtomEnum::STRING.into())
        .and_then(|c| c.split('\0').nth(1).map(str::to_string))
        .filter(|c| !c.is_empty())
}

fn cardinal(conn: &RustConnection, window: Window, atom: u32) -> Option<u32> {
    let reply = conn.get_property(false, window, atom, AtomEnum::CARDINAL, 0, 1).ok()?.reply().ok()?;
    reply.value32().and_then(|mut v| v.next())
}

/// Titles of every top-level window owned by one of `pids`. The
/// `_NET_WM_PID` lookups are pipelined: one round trip for all of them.
fn toplevel_titles(conn: &RustConnection, root: Window, atoms: &Atoms, pids: &[u32]) -> Vec<String> {
    let Some(tree) = conn.query_tree(root).ok().and_then(|c| c.reply().ok()) else {
        return Vec::new();
    };
    let cookies: Vec<_> = tree
        .children
        .iter()
        .map(|&w| (w, conn.get_property(false, w, atoms._NET_WM_PID, AtomEnum::CARDINAL, 0, 1)))
        .collect();
    let mut names = Vec::new();
    for (w, cookie) in cookies {
        let owner = cookie
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next()));
        if owner.is_some_and(|p| pids.contains(&p)) {
            names.extend(window_title(conn, atoms, w));
        }
    }
    let _ = conn.flush();
    names
}

/// The name an app's windows agree on: the most common title, the shorter
/// on a tie ("Ubisoft Connect" over "Ubisoft Connect Notification").
pub fn choose_title(names: &[String]) -> Option<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for n in names {
        let n = n.trim();
        if !n.is_empty() && !PLUMBING_TITLES.contains(&n) {
            *counts.entry(n).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by(|(a, ca), (b, cb)| ca.cmp(cb).then(b.len().cmp(&a.len())).then(b.cmp(a)))
        .map(|(n, _)| n.to_string())
}

/// The processes that are "the app" for an icon window owned by `pid`.
fn app_pids(pid: u32) -> Vec<u32> {
    let Some(prefix) = environ_var(pid, "WINEPREFIX") else {
        return vec![pid];
    };
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut pids: Vec<u32> = dir
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&p| environ_var(p, "WINEPREFIX").as_deref() == Some(prefix.as_str()))
        .filter(|&p| argv0(p).is_some_and(|a| is_wine_program(&a)))
        .collect();
    pids.sort_unstable();
    pids
}

/// A Windows program run by Wine that is not part of Wine itself: a
/// drive-letter path outside `C:\windows\`. Excludes the prefix's plumbing
/// (explorer, services, winedevice, Proton's `steam.exe` shim, xalia) and
/// every Unix process that merely carries the prefix in its environment
/// (wineserver, Proton's python, pressure-vessel).
pub fn is_wine_program(argv0: &str) -> bool {
    let a = argv0.trim().replace('/', "\\").to_ascii_lowercase();
    let a = a.strip_prefix("\\\\?\\").unwrap_or(&a);
    let b = a.as_bytes();
    let drive_path = b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    drive_path && !a[1..].starts_with(":\\windows\\") && !a.contains("\\xalia\\")
}

/// A process's name for the fallback: a Windows exe's file stem, or the
/// kernel's `comm` for anything else.
fn process_name(pid: u32) -> Option<String> {
    if let Some(a) = argv0(pid).filter(|a| is_wine_program(a)) {
        let file = a.trim().rsplit(['\\', '/']).next()?.to_string();
        let stem = file.rsplit_once('.').map_or(file.as_str(), |(s, _)| s).to_string();
        return (!stem.is_empty()).then_some(stem);
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let comm = comm.trim();
    (!comm.is_empty()).then(|| comm.to_string())
}

fn argv0(pid: u32) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let first = raw.split(|&b| b == 0).next()?;
    let s = String::from_utf8_lossy(first).into_owned();
    (!s.is_empty()).then_some(s)
}

fn environ_var(pid: u32, name: &str) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let key = format!("{name}=");
    raw.split(|&b| b == 0)
        .find(|kv| kv.starts_with(key.as_bytes()))
        .map(|kv| String::from_utf8_lossy(&kv[key.len()..]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::{choose_title, is_wine_program};

    #[test]
    fn app_windows_outvote_plumbing_and_the_notification() {
        let names: Vec<String> = [
            "Default IME",
            "Ubisoft Connect",
            "Default IME",
            "Ubisoft Connect",
            "Ubisoft Connect Notification",
            "Default IME",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(choose_title(&names).as_deref(), Some("Ubisoft Connect"));
        assert_eq!(choose_title(&["Default IME".to_string()]), None);
        assert_eq!(choose_title(&[]), None);
    }

    #[test]
    fn a_tie_goes_to_the_shorter_name() {
        let names = vec!["Launcher Notification".to_string(), "Launcher".to_string()];
        assert_eq!(choose_title(&names).as_deref(), Some("Launcher"));
    }

    #[test]
    fn only_the_apps_own_exes_count() {
        assert!(is_wine_program(r"C:\Program Files (x86)\Ubisoft\Ubisoft Game Launcher\upc.exe"));
        assert!(is_wine_program("D:/Games/thing.exe"));
        assert!(!is_wine_program(r"C:\windows\system32\explorer.exe"));
        assert!(!is_wine_program(r"c:\windows\system32\steam.exe"));
        assert!(!is_wine_program(
            r"\\?\Z:\home\u\.local\share\Steam\steamapps\common\Proton - Experimental\files\share\wine/../xalia/xalia.exe"
        ));
        assert!(!is_wine_program("/usr/bin/python3"));
        assert!(!is_wine_program("python3"));
    }
}
