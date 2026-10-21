//! Settings and preferences. A setting goes in the project (Git) only if
//! someone checking out a commit needs it to rebuild or understand the
//! model; anything that only changes how one person looks at it is a user
//! preference, kept in the OS config folder. Both have a fixed schema, and
//! every key can be read and set by agents as well as the app.

use crate::doc::canonical;
use crate::doc::project::write_atomic;
use crate::error::{Code, Error, Result};
use crate::units::Unit;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// Overrides the config folder (tests, and anyone who wants another).
pub const CONFIG_DIR_VAR: &str = "RYUCAD_CONFIG_DIR";

pub const PREFERENCES: &str = "preferences.json";
const KIND: &str = "ryucad.preferences";
const FORMAT: u64 = 1;

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
        name: "display.mesh_cache_mib",
        scope: Scope::User,
        rule: Rule::Int(16, 65536),
        default: || json!(256),
        about: "the daemon's memory for display meshes",
    },
    Key {
        name: "daemon.poll_ms",
        scope: Scope::User,
        rule: Rule::Int(50, 5000),
        default: || json!(250),
        about: "how often a watched daemon checks the files",
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
        };
        Err(Error::new(
            Code::InvalidSetting,
            format!("{} must be {expected}", self.name),
        )
        .with("key", self.name)
        .with("value", v))
    }
}

/// The folder holding the user's preferences.
pub fn config_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os(CONFIG_DIR_VAR) {
        return Ok(PathBuf::from(d));
    }
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let none = || Error::new(Code::Io, "can't find the user's config folder");
    if cfg!(windows) {
        return env("APPDATA").map(|d| d.join("ryucad")).ok_or_else(none);
    }
    let home = env("HOME");
    if cfg!(target_os = "macos") {
        return home
            .map(|h| h.join("Library/Application Support/ryucad"))
            .ok_or_else(none);
    }
    env("XDG_CONFIG_HOME")
        .or_else(|| home.map(|h| h.join(".config")))
        .map(|d| d.join("ryucad"))
        .ok_or_else(none)
}

/// The user's preferences as stored: only the keys set, and why the file
/// couldn't be used, if it couldn't.
pub struct Preferences {
    pub path: PathBuf,
    pub values: Map<String, Value>,
    pub problem: Option<String>,
}

impl Preferences {
    /// Reads the preferences. A missing file is no preferences; a file
    /// that can't be used is reported and treated the same, never thrown
    /// away.
    pub fn load() -> Result<Preferences> {
        let path = config_dir()?.join(PREFERENCES);
        let (values, problem) = match std::fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Map::new(), None),
            Err(e) => (Map::new(), Some(format!("{}: {e}", path.display()))),
            Ok(text) => match parse(&text) {
                Ok(values) => (values, None),
                Err(why) => (
                    Map::new(),
                    Some(format!("{}: {why}; using defaults", path.display())),
                ),
            },
        };
        Ok(Preferences {
            path,
            values,
            problem,
        })
    }

    /// Sets a key and writes the file. A file that couldn't be used is
    /// kept as `preferences.json.bak` first.
    pub fn set(&mut self, name: &str, value: Value) -> Result<()> {
        if self.problem.is_some() && self.path.is_file() {
            let bak = self.path.with_extension("json.bak");
            std::fs::rename(&self.path, &bak).map_err(|e| Error::io(&self.path, &e))?;
            self.problem = None;
        }
        // A preference set to its default is no longer one.
        if key(name).is_ok_and(|k| (k.default)() == value) {
            self.values.remove(name);
        } else {
            self.values.insert(name.to_owned(), value);
        }
        let doc = json!({"format": FORMAT, "kind": KIND, "values": self.values});
        write_atomic(&self.path, canonical::to_string(&doc))
    }
}

/// The `values` of a preferences file, keeping only known keys with valid
/// values.
pub fn parse(text: &str) -> core::result::Result<Map<String, Value>, String> {
    let v = canonical::parse(text).map_err(|e| e.message)?;
    if v["kind"] != KIND {
        return Err(format!("not a {KIND} file"));
    }
    match v["format"].as_u64() {
        Some(f) if f > FORMAT => return Err(format!("format {f} is newer than this ryucad")),
        Some(_) => {}
        None => return Err("no format".to_owned()),
    }
    let values = v["values"].as_object().ok_or("no values")?;
    Ok(values
        .iter()
        .filter(|(k, v)| key(k).is_ok_and(|k| k.scope == Scope::User && k.check(v).is_ok()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect())
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
/// daemons of every project share the file, and without it one daemon's
/// write can lose another's. The lock is a file of its own, so replacing
/// `preferences.json` (a rename) never drops it.
fn locked<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, &e))?;
    let path = dir.join("preferences.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| Error::io(&path, &e))?;
    file.lock().map_err(|e| Error::io(&path, &e))?;
    let r = f();
    let _ = file.unlock();
    r
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

/// A project revision: a hash of the manifest's bytes.
pub fn project_revision(root: &Path) -> Result<String> {
    let path = root.join(crate::doc::project::MANIFEST);
    let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, &e))?;
    Ok(blake3::hash(&bytes).to_hex()[..32].to_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

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
