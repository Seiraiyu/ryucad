//! Settings and preferences. A setting goes in the project (Git) only if
//! someone checking out a commit needs it to rebuild or understand the
//! model; anything that only changes how one person looks at it is a user
//! preference, kept in the OS config folder. Both have a fixed schema, and
//! every key can be read and set by agents as well as the app.

use crate::Cad;
use crate::error::{Code, Error, Result};
use crate::units::Unit;
use ryu_session::prefs::{self, Schema};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

pub use ryu_session::prefs::PREFERENCES;

/// Overrides the config folder (tests, and anyone who wants another).
pub const CONFIG_DIR_VAR: &str = Cad::CONFIG_DIR_VAR;

impl Schema for Cad {
    const CONFIG_DIR_VAR: &'static str = "RYUCAD_CONFIG_DIR";

    fn accepts(name: &str, value: &Value) -> bool {
        key(name).is_ok_and(|k| k.scope == Scope::User && k.check(value).is_ok())
    }

    fn default_of(name: &str) -> Option<Value> {
        key(name).ok().map(|k| (k.default)())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// In `ryucad.json`, in Git.
    Project,
    /// In the user's preferences file.
    User,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::User => "user",
        }
    }

    pub fn parse(s: &str) -> Result<Scope> {
        match s {
            "project" => Ok(Scope::Project),
            "user" => Ok(Scope::User),
            _ => Err(Error::new(
                Code::InvalidSetting,
                format!("unknown scope {s:?}; use project or user"),
            )),
        }
    }
}

/// What values a key takes.
enum Rule {
    OneOf(&'static [&'static str]),
    Bool,
    Int(i64, i64),
    Paths(usize),
    Object,
    Url,
    /// A URL as `Url` takes, or `""` for none.
    UrlOrEmpty,
    /// `{command id: [chord, …]}`: [`command_id_ok`] ids, at most
    /// [`MAX_CHORDS`] [`chord_ok`] chords each.
    Keys,
    /// A saved tile layout: an object with a whole `format` of at least 1.
    /// The app reads the rest, and keeps a newer format's as its default.
    Layout,
}

struct Key {
    name: &'static str,
    scope: Scope,
    rule: Rule,
    default: fn() -> Value,
    about: &'static str,
}

const KEYS: &[Key] = &[
    Key {
        name: "default_unit",
        scope: Scope::Project,
        rule: Rule::OneOf(&["m", "cm", "mm", "um", "in", "ft"]),
        default: || json!("mm"),
        about: "the unit bare numbers take",
    },
    Key {
        name: "ui.theme",
        scope: Scope::User,
        rule: Rule::OneOf(&["system", "light", "dark"]),
        default: || json!("system"),
        about: "the app's colours",
    },
    Key {
        name: "ui.style",
        scope: Scope::User,
        rule: Rule::OneOf(&["drafting", "graphite", "studio"]),
        default: || json!("drafting"),
        about: "the app's look: palette, corners and spacing",
    },
    Key {
        name: "ui.navigation",
        scope: Scope::User,
        rule: Rule::OneOf(&["ryucad", "solidworks", "onshape"]),
        default: || json!("ryucad"),
        about: "mouse and keys for orbit, pan and zoom",
    },
    Key {
        name: "ui.keys",
        scope: Scope::User,
        rule: Rule::Keys,
        default: || json!({}),
        about: "the app's key bindings over its defaults: {command id: [chord, …]}, [] unbinds",
    },
    Key {
        name: "ui.layout",
        scope: Scope::User,
        rule: Rule::Layout,
        default: || json!({}),
        about: "the app's tiles: which are open, where, and how big ({} is the default layout)",
    },
    Key {
        name: "display.quality",
        scope: Scope::User,
        rule: Rule::OneOf(&["coarse", "normal", "fine"]),
        default: || json!("normal"),
        about: "how finely the model is meshed for display",
    },
    Key {
        name: "display.edges",
        scope: Scope::User,
        rule: Rule::Bool,
        default: || json!(true),
        about: "whether edges are drawn",
    },
    Key {
        name: "display.sketches",
        scope: Scope::User,
        rule: Rule::Bool,
        default: || json!(true),
        about: "whether sketches are drawn",
    },
    Key {
        name: "display.nav_cube",
        scope: Scope::User,
        rule: Rule::Bool,
        default: || json!(true),
        about: "whether the navigation cube is drawn in the viewport's corner",
    },
    Key {
        name: "display.triad",
        scope: Scope::User,
        rule: Rule::Bool,
        default: || json!(true),
        about: "whether the axis triad is drawn in the viewport's corner",
    },
    Key {
        name: "display.origin",
        scope: Scope::User,
        rule: Rule::Bool,
        default: || json!(false),
        about: "whether the shown model's origin axes and planes are drawn",
    },
    Key {
        name: "display.gpu_memory",
        scope: Scope::User,
        rule: Rule::Int(1 << 26, 1 << 40),
        default: || json!(2u64 << 30),
        about: "how many bytes of meshes a window keeps on its graphics adapter",
    },
    Key {
        name: "display.mesh_cache_mib",
        scope: Scope::User,
        rule: Rule::Int(16, 65536),
        default: || json!(256),
        about: "the daemon's memory for display meshes",
    },
    Key {
        name: "cache.max_bytes",
        scope: Scope::User,
        rule: Rule::Int(1 << 26, 1 << 44),
        default: || json!(crate::cache::DEFAULT_MAX_BYTES),
        about: "how much the disk cache of results and meshes may hold",
    },
    Key {
        name: "cache.remote",
        scope: Scope::User,
        rule: Rule::UrlOrEmpty,
        default: || json!(""),
        about: "the shared cache server a released build reads through and writes behind \
                (`ryucad.cache.1`); empty for none",
    },
    Key {
        name: "cache.remote_write",
        scope: Scope::User,
        rule: Rule::Bool,
        default: || json!(true),
        about: "whether this machine writes what it builds to the shared cache (its token \
                must allow it)",
    },
    Key {
        name: "cache.remote_timeout_ms",
        scope: Scope::User,
        rule: Rule::Int(100, 60_000),
        default: || json!(2000),
        about: "how long a request to the shared cache may take before it is skipped",
    },
    Key {
        name: "daemon.poll_ms",
        scope: Scope::User,
        rule: Rule::Int(50, 5000),
        default: || json!(250),
        about: "how often a watched daemon checks the files",
    },
    Key {
        name: "session.max_memory",
        scope: Scope::User,
        rule: Rule::Int(1 << 29, 1 << 44),
        default: || json!(crate::budget::default_max()),
        about: "how many bytes of parts' rebuilds and display meshes a daemon holds between \
                requests (a quarter of the machine's memory, at most 8 GiB, by default)",
    },
    Key {
        name: "daemon.rescan_s",
        scope: Scope::User,
        rule: Rule::Int(5, 3600),
        default: || json!(60),
        about: "how often a daemon checks every file, though file events say what changed",
    },
    Key {
        name: "review.url",
        scope: Scope::User,
        rule: Rule::Url,
        default: || json!("https://review.seiraiyu.com"),
        about: "the hosted review service `ryucad review` talks to",
    },
    Key {
        name: "app.recent_projects",
        scope: Scope::User,
        rule: Rule::Paths(20),
        default: || json!([]),
        about: "recently opened projects, newest first",
    },
    Key {
        name: "app.window",
        scope: Scope::User,
        rule: Rule::Object,
        default: || json!({}),
        about: "the app's window geometry and panel layout",
    },
];

fn key(name: &str) -> Result<&'static Key> {
    KEYS.iter().find(|k| k.name == name).ok_or_else(|| {
        Error::new(Code::UnknownSetting, format!("no setting {name:?}"))
            .with("key", name)
            .with("known", KEYS.iter().map(|k| k.name).collect::<Vec<_>>())
    })
}

/// The scope a key belongs to.
pub fn scope_of(name: &str) -> Result<Scope> {
    key(name).map(|k| k.scope)
}

impl Key {
    fn check(&self, v: &Value) -> Result<()> {
        let ok = match &self.rule {
            Rule::OneOf(allowed) => v.as_str().is_some_and(|s| allowed.contains(&s)),
            Rule::Bool => v.is_boolean(),
            Rule::Int(lo, hi) => v.as_i64().is_some_and(|n| (*lo..=*hi).contains(&n)),
            Rule::Paths(max) => v.as_array().is_some_and(|a| {
                a.len() <= *max && a.iter().all(|p| p.as_str().is_some_and(|s| !s.is_empty()))
            }),
            Rule::Object => v.is_object(),
            Rule::Url => v.as_str().is_some_and(url_ok),
            Rule::UrlOrEmpty => v.as_str().is_some_and(|s| s.is_empty() || url_ok(s)),
            Rule::Keys => v.as_object().is_some_and(|m| {
                m.iter().all(|(id, chords)| {
                    command_id_ok(id)
                        && chords.as_array().is_some_and(|a| {
                            a.len() <= MAX_CHORDS
                                && a.iter().all(|c| c.as_str().is_some_and(chord_ok))
                        })
                })
            }),
            Rule::Layout => v.as_object().is_some_and(|m| {
                m.is_empty()
                    || m.get("format")
                        .and_then(Value::as_u64)
                        .is_some_and(|f| f >= 1)
            }),
        };
        if ok {
            return Ok(());
        }
        let expected = match &self.rule {
            Rule::OneOf(allowed) => format!("one of {}", allowed.join(", ")),
            Rule::Bool => "true or false".to_owned(),
            Rule::Int(lo, hi) => format!("an integer from {lo} to {hi}"),
            Rule::Paths(max) => format!("a list of at most {max} paths"),
            Rule::Object => "a JSON object".to_owned(),
            Rule::Url => "an https:// URL (http:// only for this machine)".to_owned(),
            Rule::UrlOrEmpty => {
                "an https:// URL (http:// only for this machine), or \"\" for none".to_owned()
            }
            Rule::Keys => format!(
                "an object of command ids (`edit.undo`) to lists of at most {MAX_CHORDS} chords \
                 (`Mod+Shift+Z`: Mod, Ctrl, Alt and Shift, then one key)"
            ),
            Rule::Layout => "a JSON object with a whole format of at least 1, or {}".to_owned(),
        };
        Err(Error::new(
            Code::InvalidSetting,
            format!("{} must be {expected}", self.name),
        )
        .with("key", self.name)
        .with("value", v))
    }
}

/// The most chords `ui.keys` binds to one command.
pub const MAX_CHORDS: usize = 8;

/// The modifiers a chord may name, in the order the app writes them: `Mod`
/// is Cmd on macOS and Ctrl elsewhere, `Ctrl` is Ctrl everywhere.
pub const MODIFIERS: [&str; 4] = ["Mod", "Ctrl", "Alt", "Shift"];

/// The keys named by more than one character (egui's names for them).
pub const NAMED_KEYS: [&str; 26] = [
    "Up",
    "Down",
    "Left",
    "Right",
    "Escape",
    "Tab",
    "Backspace",
    "Enter",
    "Insert",
    "Delete",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Space",
    "Comma",
    "Minus",
    "Period",
    "Plus",
    "Equals",
    "Semicolon",
    "Backslash",
    "Slash",
    "OpenBracket",
    "CloseBracket",
    "Backtick",
];

/// Every key a chord may end in: A to Z, 0 to 9, F1 to F24 and
/// [`NAMED_KEYS`].
pub fn key_names() -> Vec<String> {
    let mut out: Vec<String> = ('A'..='Z').chain('0'..='9').map(String::from).collect();
    out.extend((1..=24).map(|n| format!("F{n}")));
    out.extend(NAMED_KEYS.iter().map(|k| (*k).to_owned()));
    out
}

/// A chord's text: modifiers from [`MODIFIERS`], each once and not `Mod`
/// with `Ctrl` (they are one key off macOS), then one of [`key_names`],
/// joined by `+`.
pub fn chord_ok(s: &str) -> bool {
    let parts: Vec<&str> = s.split('+').collect();
    let Some((key, mods)) = parts.split_last() else {
        return false;
    };
    let mut seen = [false; 4];
    for m in mods {
        let Some(i) = MODIFIERS.iter().position(|x| x == m) else {
            return false;
        };
        if std::mem::replace(&mut seen[i], true) {
            return false;
        }
    }
    !(seen[0] && seen[1]) && key_names().iter().any(|k| k == key)
}

/// A command id: dotted lower-case words, at least two (`edit.undo`,
/// `view.front_top_right`), at most 64 characters.
pub fn command_id_ok(id: &str) -> bool {
    id.len() <= 64
        && id.split('.').count() >= 2
        && id.split('.').all(|w| {
            w.starts_with(|c: char| c.is_ascii_lowercase())
                && w.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// An https URL, or an http one to this machine (for tests and a local
/// service): a host, no user, no spaces or control characters.
fn url_ok(s: &str) -> bool {
    let (secure, rest) = match (s.strip_prefix("https://"), s.strip_prefix("http://")) {
        (Some(r), _) => (true, r),
        (_, Some(r)) => (false, r),
        _ => return false,
    };
    let authority = &rest[..rest.find('/').unwrap_or(rest.len())];
    if authority.is_empty()
        || authority.contains(['@', '?', '#'])
        || s.contains(' ')
        || s.chars().any(char::is_control)
    {
        return false;
    }
    secure
        || ["localhost", "127.0.0.1", "[::1]"].iter().any(|h| {
            authority == *h
                || authority
                    .strip_prefix(h)
                    .is_some_and(|p| p.starts_with(':'))
        })
}

/// The folder holding the user's preferences.
pub fn config_dir() -> Result<PathBuf> {
    prefs::config_dir::<Cad>()
}

/// The user's preferences as stored: only the keys set, and why the file
/// couldn't be used, if it couldn't.
pub type Preferences = prefs::Preferences<Cad>;

/// The `values` of a preferences file, keeping only known keys with valid
/// values.
pub fn parse(text: &str) -> core::result::Result<Map<String, Value>, String> {
    prefs::parse::<Cad>(text)
}

/// One setting as reported: its value, scope, and whether it is the
/// default.
fn report(k: &Key, value: Option<&Value>) -> Value {
    json!({
        "key": k.name,
        "scope": k.scope.as_str(),
        "value": value.cloned().unwrap_or_else(k.default),
        "default": value.is_none(),
        "about": k.about,
    })
}

/// The project's settings from its manifest.
fn project_values(unit: Unit) -> Map<String, Value> {
    [("default_unit".to_owned(), json!(unit))]
        .into_iter()
        .collect()
}

/// `config.get`: one key, or every key of `scope` (both when `None`).
pub fn get(unit: Unit, name: Option<&str>, scope: Option<Scope>) -> Result<Value> {
    let prefs = Preferences::load()?;
    let project = project_values(unit);
    let value_of = |k: &Key| match k.scope {
        Scope::Project => project.get(k.name),
        Scope::User => prefs.values.get(k.name),
    };
    let warnings: Vec<String> = prefs.problem.iter().cloned().collect();
    if let Some(name) = name {
        let k = key(name)?;
        if scope.is_some_and(|s| s != k.scope) {
            return Err(wrong_scope(k));
        }
        let mut v = report(k, value_of(k));
        v["warnings"] = json!(warnings);
        return Ok(v);
    }
    let settings: Vec<Value> = KEYS
        .iter()
        .filter(|k| scope.is_none_or(|s| s == k.scope))
        .map(|k| report(k, value_of(k)))
        .collect();
    Ok(json!({"settings": settings, "warnings": warnings}))
}

fn wrong_scope(k: &Key) -> Error {
    Error::new(
        Code::InvalidSetting,
        format!("{} is a {} setting", k.name, k.scope.as_str()),
    )
    .with("key", k.name)
    .with("scope", k.scope.as_str())
}

/// Checks `value` for `name` (in `scope`, if given) and returns its key's
/// scope.
pub fn validate(name: &str, value: &Value, scope: Option<Scope>) -> Result<Scope> {
    let k = key(name)?;
    if scope.is_some_and(|s| s != k.scope) {
        return Err(wrong_scope(k));
    }
    k.check(value)?;
    Ok(k.scope)
}

/// Sets a user preference.
pub fn set_user(name: &str, value: Value) -> Result<Value> {
    validate(name, &value, Some(Scope::User))?;
    locked(|| {
        let mut prefs = Preferences::load()?;
        prefs.set(name, value)?;
        Ok(report(key(name)?, prefs.values.get(name)))
    })
}

/// Runs a read-modify-write of the preferences holding their lock: the
/// daemons of every project share the file.
fn locked<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    prefs::locked::<Cad, T>(f)
}

/// Puts the project at `root` first among the recent projects: a human
/// opened it in the app.
pub fn remember_project(root: &Path) -> Result<()> {
    locked(|| {
        let mut prefs = Preferences::load()?;
        let max = match key("app.recent_projects")?.rule {
            Rule::Paths(n) => n,
            _ => unreachable!("a list of paths"),
        };
        let root = root.to_string_lossy();
        let mut list = vec![json!(root)];
        let old = prefs
            .values
            .get("app.recent_projects")
            .and_then(Value::as_array);
        list.extend(
            old.into_iter()
                .flatten()
                .filter(|p| p.as_str() != Some(&*root))
                .cloned(),
        );
        list.truncate(max);
        prefs.set("app.recent_projects", json!(list))
    })
}

/// The value of a user preference, or its default.
pub fn user(name: &str) -> Value {
    let k = key(name).expect("a known key");
    Preferences::load()
        .ok()
        .and_then(|p| p.values.get(name).cloned())
        .unwrap_or_else(k.default)
}

/// Refuses to write the project's own settings when the caller's view of
/// them is out of date; returns the revision they are at.
pub fn check_project_revision(root: &Path, expected: Option<&str>) -> Result<String> {
    let rev = project_revision(root)?;
    match expected {
        Some(e) if e != rev => Err(Error::new(
            Code::Conflict,
            format!("the project changed since revision {e}; read it again and retry"),
        )
        .with("expected", e)
        .with("actual", rev)),
        _ => Ok(rev),
    }
}

/// A project revision: a hash of the files that hold the project's own
/// settings, the manifest, ryupdm's settings, the material library and
/// the lock if there are any. Each piece is length-prefixed, and the optional ones
/// are tagged.
pub fn project_revision(root: &Path) -> Result<String> {
    use crate::doc::materials::MATERIALS;
    use crate::doc::project::{MANIFEST, PDM_CONFIG};
    let read = |name: &str| {
        let path = root.join(name);
        match std::fs::read(&path) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && name != MANIFEST => Ok(None),
            Err(e) => Err(Error::io(&path, &e)),
        }
    };
    let mut h = blake3::Hasher::new();
    let mut piece = |bytes: &[u8]| {
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    };
    piece(&read(MANIFEST)?.unwrap_or_default());
    if let Some(pdm) = read(PDM_CONFIG)? {
        piece(PDM_CONFIG.as_bytes());
        piece(&pdm);
    }
    if let Some(m) = read(MATERIALS)? {
        piece(MATERIALS.as_bytes());
        piece(&m);
    }
    if let Some(l) = read(crate::doc::lock::LOCK)? {
        piece(crate::doc::lock::LOCK.as_bytes());
        piece(&l);
    }
    Ok(h.finalize().to_hex()[..32].to_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc::canonical;

    #[test]
    fn the_remote_cache_is_a_url_or_none() {
        let k = key("cache.remote").unwrap();
        assert_eq!((k.default)(), json!(""));
        for ok in [
            "",
            "https://cache.example.com",
            "http://127.0.0.1:9000/team",
        ] {
            assert!(k.check(&json!(ok)).is_ok(), "{ok}");
        }
        for bad in ["http://cache.example.com", "ftp://x", " ", "https://"] {
            assert!(k.check(&json!(bad)).is_err(), "{bad}");
        }
        assert!(
            key("cache.remote_write")
                .unwrap()
                .check(&json!(false))
                .is_ok()
        );
        let t = key("cache.remote_timeout_ms").unwrap();
        assert_eq!((t.default)(), json!(2000));
        assert!(t.check(&json!(50)).is_err());
    }

    #[test]
    fn review_url_takes_https_or_loopback_http() {
        let k = key("review.url").unwrap();
        assert_eq!((k.default)(), json!("https://review.seiraiyu.com"));
        for ok in [
            "https://review.seiraiyu.com",
            "https://x.example/sub",
            "http://127.0.0.1:8080",
            "http://localhost",
            "http://[::1]:9",
        ] {
            assert!(k.check(&json!(ok)).is_ok(), "{ok}");
        }
        for bad in [
            "http://review.seiraiyu.com",
            "ftp://x",
            "https://",
            "review.seiraiyu.com",
            "https://a b",
            "http://localhost.evil.com",
            "https://u@x",
        ] {
            assert_eq!(
                k.check(&json!(bad)).unwrap_err().code,
                Code::InvalidSetting,
                "{bad}"
            );
        }
    }

    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Runs `f` with the config folder set to a fresh temporary one.
    pub(crate) fn with_config<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _g = ENV.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: tests that touch this variable hold ENV.
        unsafe { std::env::set_var(CONFIG_DIR_VAR, dir.path()) };
        let r = f(dir.path());
        unsafe { std::env::remove_var(CONFIG_DIR_VAR) };
        r
    }

    #[test]
    fn defaults_then_set_values() {
        with_config(|dir| {
            let v = get(Unit::Mm, Some("ui.theme"), None).unwrap();
            assert_eq!(
                (&v["value"], &v["default"]),
                (&json!("system"), &json!(true))
            );
            set_user("ui.theme", json!("dark")).unwrap();
            set_user("display.mesh_cache_mib", json!(512)).unwrap();
            let v = get(Unit::Mm, Some("ui.theme"), None).unwrap();
            assert_eq!(
                (&v["value"], &v["default"]),
                (&json!("dark"), &json!(false))
            );
            assert_eq!(user("display.mesh_cache_mib"), 512);
            let text = std::fs::read_to_string(dir.join(PREFERENCES)).unwrap();
            assert!(canonical::is_canonical(&text));
            let all = get(Unit::In, None, None).unwrap();
            assert_eq!(all["settings"][0]["value"], "in");
            assert_eq!(all["settings"].as_array().unwrap().len(), KEYS.len());
            let user_only = get(Unit::In, None, Some(Scope::User)).unwrap();
            assert_eq!(
                user_only["settings"].as_array().unwrap().len(),
                KEYS.len() - 1
            );
        });
    }

    #[test]
    fn daemons_writing_at_once_lose_nothing() {
        with_config(|_| {
            let roots: Vec<String> = (0..8).map(|i| format!("/projects/{i}")).collect();
            std::thread::scope(|s| {
                for r in &roots {
                    s.spawn(move || remember_project(Path::new(r)).unwrap());
                }
                s.spawn(|| set_user("ui.theme", json!("dark")).unwrap());
            });
            let list = user("app.recent_projects");
            for r in &roots {
                assert!(
                    list.as_array().unwrap().contains(&json!(r)),
                    "{r} in {list}"
                );
            }
            assert_eq!(user("ui.theme"), "dark");
            remember_project(Path::new("/projects/3")).unwrap();
            assert_eq!(user("app.recent_projects")[0], "/projects/3");
            assert_eq!(user("app.recent_projects").as_array().unwrap().len(), 8);
        });
    }

    #[test]
    fn the_schema_is_enforced() {
        with_config(|_| {
            let e = set_user("ui.colour", json!("x")).unwrap_err();
            assert_eq!(e.code, Code::UnknownSetting);
            for (k, v) in [
                ("ui.theme", json!("neon")),
                ("display.edges", json!("yes")),
                ("daemon.poll_ms", json!(10)),
                ("app.recent_projects", json!([""])),
                ("app.window", json!([])),
                ("default_unit", json!("mm")),
            ] {
                assert_eq!(
                    set_user(k, v).unwrap_err().code,
                    Code::InvalidSetting,
                    "{k}"
                );
            }
            assert_eq!(
                validate("default_unit", &json!("parsec"), None)
                    .unwrap_err()
                    .code,
                Code::InvalidSetting
            );
            assert_eq!(
                validate("default_unit", &json!("in"), None).unwrap(),
                Scope::Project
            );
        });
    }

    #[test]
    fn chords_and_command_ids() {
        for ok in [
            "Mod+K",
            "Mod+Shift+Z",
            "F",
            "1",
            "Shift+F6",
            "Ctrl+Alt+Delete",
            "Space",
            "F24",
        ] {
            assert!(chord_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "Mod+",
            "+K",
            "Mod+Mod+K",
            "Mod+Ctrl+K",
            "Hyper+K",
            "k",
            "Mod+k",
            "F25",
            "Shift",
            "Mod+Shift",
        ] {
            assert!(!chord_ok(bad), "{bad}");
        }
        for ok in [
            "edit.undo",
            "view.front_top_right",
            "view.panel.activity",
            "measure.tool",
        ] {
            assert!(command_id_ok(ok), "{ok}");
        }
        for bad in [
            "undo",
            "Edit.undo",
            "edit.",
            ".undo",
            "edit.1x",
            "edit.un-do",
        ] {
            assert!(!command_id_ok(bad), "{bad}");
        }
    }

    #[test]
    fn keys_and_layouts_are_preferences() {
        with_config(|_| {
            assert_eq!(user("ui.keys"), json!({}));
            set_user(
                "ui.keys",
                json!({"edit.undo": ["Mod+Z", "Alt+Backspace"], "view.fit": []}),
            )
            .unwrap();
            assert_eq!(user("ui.keys")["view.fit"], json!([]));
            set_user(
                "ui.layout",
                json!({"format": 1, "root": {"tile": "viewport"}}),
            )
            .unwrap();
            set_user("ui.layout", json!({"format": 7, "anything": true})).unwrap();
            for (k, v) in [
                ("ui.keys", json!({"edit.undo": "Mod+Z"})),
                ("ui.keys", json!({"edit.undo": ["Mod+Zed"]})),
                ("ui.keys", json!({"undo": ["Mod+Z"]})),
                ("ui.keys", json!([])),
                ("ui.layout", json!({"root": {}})),
                ("ui.layout", json!({"format": 0})),
            ] {
                assert_eq!(
                    set_user(k, v).unwrap_err().code,
                    Code::InvalidSetting,
                    "{k}"
                );
            }
        });
    }

    #[test]
    fn the_project_revision_covers_ryupdm_settings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(crate::doc::project::MANIFEST), "{}").unwrap();
        let r0 = project_revision(root).unwrap();
        std::fs::write(root.join(crate::doc::project::PDM_CONFIG), "{}").unwrap();
        let r1 = project_revision(root).unwrap();
        assert_ne!(r1, r0);
        std::fs::write(root.join(crate::doc::project::PDM_CONFIG), "{ }").unwrap();
        assert_ne!(project_revision(root).unwrap(), r1);
        std::fs::remove_file(root.join(crate::doc::project::MANIFEST)).unwrap();
        assert_eq!(project_revision(root).unwrap_err().code, Code::Io);
    }

    #[test]
    fn the_project_revision_covers_the_material_library() {
        use crate::doc::materials::MATERIALS;
        use crate::doc::project::{MANIFEST, PDM_CONFIG};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(MANIFEST), "{}").unwrap();
        let r0 = project_revision(root).unwrap();
        std::fs::write(root.join(MATERIALS), "{}").unwrap();
        let r1 = project_revision(root).unwrap();
        assert_ne!(r1, r0);
        // Tagged: the same bytes as ryupdm's settings hash otherwise.
        std::fs::remove_file(root.join(MATERIALS)).unwrap();
        std::fs::write(root.join(PDM_CONFIG), "{}").unwrap();
        assert_ne!(project_revision(root).unwrap(), r1);
    }

    #[test]
    fn the_viewports_overlays_are_preferences() {
        for (key, default) in [
            ("display.nav_cube", true),
            ("display.triad", true),
            ("display.origin", false),
        ] {
            assert_eq!(scope_of(key).unwrap(), Scope::User, "{key}");
            assert_eq!(<Cad as Schema>::default_of(key), Some(json!(default)));
            assert!(<Cad as Schema>::accepts(key, &json!(!default)));
            assert!(!<Cad as Schema>::accepts(key, &json!("yes")));
        }
    }

    #[test]
    fn a_bad_file_is_reported_and_kept() {
        with_config(|dir| {
            let path = dir.join(PREFERENCES);
            std::fs::write(&path, "{not json").unwrap();
            let v = get(Unit::Mm, Some("ui.theme"), None).unwrap();
            assert_eq!(v["value"], "system");
            assert!(
                v["warnings"][0]
                    .as_str()
                    .unwrap()
                    .contains("using defaults")
            );
            set_user("ui.theme", json!("light")).unwrap();
            assert_eq!(
                std::fs::read_to_string(dir.join("preferences.json.bak")).unwrap(),
                "{not json"
            );
            assert_eq!(user("ui.theme"), "light");
            // A newer format is not ours to read.
            std::fs::write(
                &path,
                r#"{"format":2,"kind":"ryucad.preferences","values":{}}"#,
            )
            .unwrap();
            let v = get(Unit::Mm, None, None).unwrap();
            assert!(v["warnings"][0].as_str().unwrap().contains("newer"));
        });
    }
}
