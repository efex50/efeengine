//! The editor's per-user settings (docs/EDITOR.md E9.6): a JSON file in the user config
//! directory. Loading and saving never panic; problems come back as messages.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub const MAX_RECENT: usize = 10;
pub const DEFAULT_PORT: u16 = 7878;
const FILE: &str = "config.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Where new projects are created and listed; `None` means [`default_projects_dir_from_env`].
    pub projects_dir: Option<PathBuf>,
    pub default_port: u16,
    pub place_game_window: bool,
    pub cargo_release: bool,
    /// `.efproj` files, most recent first, at most [`MAX_RECENT`].
    pub recent: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self { projects_dir: None, default_port: DEFAULT_PORT, place_game_window: true, cargo_release: false, recent: Vec::new() }
    }
}

/// The editor's config folder under the user's config base: `$XDG_CONFIG_HOME`, else
/// `<home>/.config`; on Windows `%APPDATA%`. Values are passed in so tests need no environment.
pub fn config_dir(windows: bool, xdg: Option<&str>, home: Option<&str>, appdata: Option<&str>) -> Option<PathBuf> {
    let non_empty = |s: Option<&str>| s.filter(|s| !s.is_empty()).map(PathBuf::from);
    let base = if windows {
        non_empty(appdata)?
    } else {
        non_empty(xdg).or_else(|| non_empty(home).map(|h| h.join(".config")))?
    };
    Some(base.join("efeengine-editor"))
}

/// [`config_dir`] for the running process.
pub fn config_dir_from_env() -> Option<PathBuf> {
    let get = |k: &str| std::env::var(k).ok();
    let (x, h, a) = (get("XDG_CONFIG_HOME"), get("HOME"), get("APPDATA"));
    config_dir(cfg!(windows), x.as_deref(), h.as_deref(), a.as_deref())
}

/// `<home>/efeengine-projects`; on Windows `%USERPROFILE%` is the home. Values are passed in so
/// tests need no environment.
pub fn default_projects_dir(windows: bool, home: Option<&str>, userprofile: Option<&str>) -> Option<PathBuf> {
    let base = if windows { userprofile } else { home };
    base.filter(|s| !s.is_empty()).map(|h| PathBuf::from(h).join("efeengine-projects"))
}

/// [`default_projects_dir`] for the running process.
pub fn default_projects_dir_from_env() -> Option<PathBuf> {
    let get = |k: &str| std::env::var(k).ok();
    let (h, u) = (get("HOME"), get("USERPROFILE"));
    default_projects_dir(cfg!(windows), h.as_deref(), u.as_deref())
}

impl Config {
    /// The folder projects live in: the configured one, else the default.
    pub fn projects_dir(&self) -> Option<PathBuf> {
        self.projects_dir.clone().or_else(default_projects_dir_from_env)
    }
}

/// A port from a text field: 1 to 65535.
pub fn validate_port(s: &str) -> Result<u16, String> {
    match s.trim().parse::<u32>() {
        Ok(p) if (1..=65535).contains(&p) => Ok(p as u16),
        _ => Err(format!("port `{}` must be a number from 1 to 65535", s.trim())),
    }
}

impl Config {
    pub fn to_json(&self) -> Value {
        json!({
            "projects_dir": self.projects_dir.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "default_port": self.default_port,
            "place_game_window": self.place_game_window,
            "cargo_release": self.cargo_release,
            "recent": self.recent.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        })
    }

    /// Missing keys keep their defaults; a key of the wrong type is an error.
    pub fn from_json(v: &Value) -> Result<Config, String> {
        let obj = v.as_object().ok_or("the config is not a JSON object")?;
        let mut c = Config::default();
        for (key, val) in obj {
            match key.as_str() {
                "projects_dir" => match val {
                    Value::Null => c.projects_dir = None,
                    Value::String(s) => c.projects_dir = Some(PathBuf::from(s)),
                    _ => return Err("`projects_dir` must be a string or null".into()),
                },
                "default_port" => {
                    let p = val.as_u64().filter(|p| (1..=65535).contains(p)).ok_or("`default_port` must be a number from 1 to 65535")?;
                    c.default_port = p as u16;
                }
                "place_game_window" => c.place_game_window = val.as_bool().ok_or("`place_game_window` must be true or false")?,
                "cargo_release" => c.cargo_release = val.as_bool().ok_or("`cargo_release` must be true or false")?,
                "recent" => {
                    let a = val.as_array().ok_or("`recent` must be a list of strings")?;
                    c.recent = a
                        .iter()
                        .map(|x| x.as_str().map(PathBuf::from).ok_or("`recent` must be a list of strings"))
                        .collect::<Result<_, _>>()?;
                    c.recent.truncate(MAX_RECENT);
                }
                _ => {}
            }
        }
        Ok(c)
    }

    /// Put `p` first; an earlier entry for it is dropped; the list is capped.
    pub fn add_recent(&mut self, p: &Path) {
        self.recent.retain(|r| r != p);
        self.recent.insert(0, p.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }

    pub fn remove_recent(&mut self, p: &Path) {
        self.recent.retain(|r| r != p);
    }
}

/// Load `<dir>/config.json`. A missing file gives defaults and no message. A file that is not
/// valid is moved to `config.json.bad`, defaults are written in its place, and the message says so.
pub fn load(dir: &Path) -> (Config, Option<String>) {
    let file = dir.join(FILE);
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Config::default(), None),
        Err(e) => return replace_bad(dir, &file, format!("cannot read: {e}")),
    };
    match serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()).and_then(|v| Config::from_json(&v)) {
        Ok(c) => (c, None),
        Err(why) => replace_bad(dir, &file, why),
    }
}

fn replace_bad(dir: &Path, file: &Path, why: String) -> (Config, Option<String>) {
    let bad = dir.join(format!("{FILE}.bad"));
    let moved = std::fs::rename(file, &bad);
    let c = Config::default();
    let saved = save(dir, &c);
    let mut msg = format!("config file {} is not valid ({why}); using defaults", file.display());
    match moved {
        Ok(()) => msg.push_str(&format!("; the old file is kept as {}", bad.display())),
        Err(e) => msg.push_str(&format!("; could not keep a copy ({e})")),
    }
    if let Err(e) = saved {
        msg.push_str(&format!("; {e}"));
    }
    (c, Some(msg))
}

/// Write `<dir>/config.json` (through a temporary file).
pub fn save(dir: &Path, c: &Config) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(&c.to_json()).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    let file = dir.join(FILE);
    std::fs::write(&tmp, text + "\n").map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &file).map_err(|e| format!("cannot write {}: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("efe-editor-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_file_gives_defaults() {
        let d = temp("missing");
        let (c, n) = load(&d.join("nothing-here"));
        assert_eq!(c, Config::default());
        assert!(n.is_none());
        assert_eq!((c.default_port, c.place_game_window, c.cargo_release), (7878, true, false));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bad_file_is_moved_aside() {
        for (i, text) in ["not json {", "{\"default_port\": \"x\"}", "{\"default_port\": 0}", "[1]", "{\"recent\": [1]}"].iter().enumerate() {
            let d = temp(&format!("bad{i}"));
            std::fs::write(d.join("config.json"), text).unwrap();
            let (c, n) = load(&d);
            assert_eq!(c, Config::default(), "{text}");
            assert!(n.unwrap().contains("not valid"));
            assert_eq!(std::fs::read_to_string(d.join("config.json.bad")).unwrap(), *text);
            // Defaults were written, so the next load is clean.
            let (c2, n2) = load(&d);
            assert_eq!((c2, n2), (Config::default(), None));
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn round_trip() {
        let d = temp("round");
        let c = Config {
            projects_dir: Some("/opt/efe".into()),
            default_port: 9000,
            place_game_window: false,
            cargo_release: true,
            recent: vec!["/a/a.efproj".into(), "/b/b.efproj".into()],
        };
        save(&d.join("sub"), &c).unwrap();
        assert_eq!(load(&d.join("sub")), (c, None));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn old_engine_path_is_ignored_without_a_notice() {
        let d = temp("oldengine");
        std::fs::write(d.join("config.json"), r#"{"engine_path": "/old/engine", "default_port": 9001}"#).unwrap();
        let (c, n) = load(&d);
        assert!(n.is_none(), "{n:?}");
        assert_eq!(c.default_port, 9001);
        assert!(!d.join("config.json.bad").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn projects_dir_default() {
        assert_eq!(default_projects_dir(false, Some("/h"), Some("/u")), Some("/h/efeengine-projects".into()));
        assert_eq!(default_projects_dir(true, Some("/h"), Some("/u")), Some("/u/efeengine-projects".into()));
        assert_eq!(default_projects_dir(false, Some(""), None), None);
        let c = Config { projects_dir: Some("/p".into()), ..Config::default() };
        assert_eq!(c.projects_dir(), Some("/p".into()));
    }

    #[test]
    fn save_failure_is_an_error_not_a_panic() {
        let d = temp("savefail");
        std::fs::write(d.join("file"), "x").unwrap();
        assert!(save(&d.join("file").join("under"), &Config::default()).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn recent_is_capped_and_ordered() {
        let mut c = Config::default();
        for i in 0..15 {
            c.add_recent(Path::new(&format!("/p{i}.efproj")));
        }
        assert_eq!(c.recent.len(), MAX_RECENT);
        assert_eq!(c.recent[0], PathBuf::from("/p14.efproj"));
        assert_eq!(c.recent[9], PathBuf::from("/p5.efproj"));
        c.add_recent(Path::new("/p10.efproj"));
        assert_eq!(c.recent[0], PathBuf::from("/p10.efproj"));
        assert_eq!(c.recent.len(), MAX_RECENT);
        assert_eq!(c.recent.iter().filter(|p| **p == PathBuf::from("/p10.efproj")).count(), 1);
        c.remove_recent(Path::new("/p10.efproj"));
        assert_eq!(c.recent.len(), MAX_RECENT - 1);
    }

    #[test]
    fn port_validation() {
        assert_eq!(validate_port("7878"), Ok(7878));
        assert_eq!(validate_port(" 1 "), Ok(1));
        assert_eq!(validate_port("65535"), Ok(65535));
        for bad in ["0", "65536", "-1", "", "abc", "80.5"] {
            assert!(validate_port(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn config_dir_rules() {
        assert_eq!(config_dir(false, Some("/x"), Some("/h"), None), Some("/x/efeengine-editor".into()));
        assert_eq!(config_dir(false, None, Some("/h"), None), Some("/h/.config/efeengine-editor".into()));
        assert_eq!(config_dir(false, Some(""), Some("/h"), None), Some("/h/.config/efeengine-editor".into()));
        assert_eq!(config_dir(true, Some("/x"), Some("/h"), Some("/ap")), Some("/ap/efeengine-editor".into()));
        assert_eq!(config_dir(false, None, None, None), None);
    }
}
