//! A product's user preferences: one canonical JSON file in the OS config
//! folder, shared by every project's daemon. Which keys exist, what values
//! they take and their defaults are the product's ([`Preferences`]); this
//! is where the file lives, how it is read and written, and the lock that
//! keeps daemons from losing each other's writes.

use crate::Product;
use ryu_doc::canonical;
use ryu_doc::error::{Code, Error, Result};
use ryu_doc::source::write_atomic;
use serde_json::{Map, Value, json};
use std::path::PathBuf;

/// The preferences file, in the config folder.
pub const PREFERENCES: &str = "preferences.json";
const FORMAT: u64 = 1;

/// A product's preference keys: which a file may hold, and their defaults.
pub trait Schema: Product {
    /// The variable that overrides the config folder (tests, and anyone who
    /// wants another): `RYUCAD_CONFIG_DIR`.
    const CONFIG_DIR_VAR: &'static str;
    /// Whether a file may hold `value` for `name`: a user preference that
    /// takes it.
    fn accepts(name: &str, value: &Value) -> bool;
    /// The default of preference `name`, if it is one.
    fn default_of(name: &str) -> Option<Value>;
}

/// The file's kind: `ryucad.preferences`.
fn kind<P: Product>() -> String {
    format!("{}.preferences", P::NAME)
}

/// The folder holding the user's preferences: the variable's, else the
/// OS's config folder for `P` (`%APPDATA%\ryucad`, `~/Library/Application
/// Support/ryucad`, `$XDG_CONFIG_HOME/ryucad`).
pub fn config_dir<P: Schema>() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os(P::CONFIG_DIR_VAR) {
        return Ok(PathBuf::from(d));
    }
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let none = || Error::new(Code::Io, "can't find the user's config folder");
    if cfg!(windows) {
        return env("APPDATA").map(|d| d.join(P::NAME)).ok_or_else(none);
    }
    let home = env("HOME");
    if cfg!(target_os = "macos") {
        return home
            .map(|h| h.join("Library/Application Support").join(P::NAME))
            .ok_or_else(none);
    }
    env("XDG_CONFIG_HOME")
        .or_else(|| home.map(|h| h.join(".config")))
        .map(|d| d.join(P::NAME))
        .ok_or_else(none)
}

/// The user's preferences as stored: only the keys set, and why the file
/// couldn't be used, if it couldn't.
pub struct Preferences<P> {
    pub path: PathBuf,
    pub values: Map<String, Value>,
    pub problem: Option<String>,
    product: std::marker::PhantomData<fn() -> P>,
}

impl<P: Schema> Preferences<P> {
    /// Reads the preferences. A missing file is no preferences; a file
    /// that can't be used is reported and treated the same, never thrown
    /// away.
    pub fn load() -> Result<Preferences<P>> {
        let path = config_dir::<P>()?.join(PREFERENCES);
        let (values, problem) = match std::fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Map::new(), None),
            Err(e) => (Map::new(), Some(format!("{}: {e}", path.display()))),
            Ok(text) => match parse::<P>(&text) {
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
            product: std::marker::PhantomData,
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
        if P::default_of(name).is_some_and(|d| d == value) {
            self.values.remove(name);
        } else {
            self.values.insert(name.to_owned(), value);
        }
        let doc = json!({"format": FORMAT, "kind": kind::<P>(), "values": self.values});
        write_atomic(&self.path, canonical::to_string(&doc))
    }
}

/// The `values` of a preferences file, keeping only keys the schema
/// accepts with the values it accepts.
pub fn parse<P: Schema>(text: &str) -> core::result::Result<Map<String, Value>, String> {
    let v = canonical::parse(text).map_err(|e| e.message)?;
    let kind = kind::<P>();
    if v["kind"] != kind.as_str() {
        return Err(format!("not a {kind} file"));
    }
    match v["format"].as_u64() {
        Some(f) if f > FORMAT => {
            return Err(format!("format {f} is newer than this {}", P::NAME));
        }
        Some(_) => {}
        None => return Err("no format".to_owned()),
    }
    let values = v["values"].as_object().ok_or("no values")?;
    Ok(values
        .iter()
        .filter(|(k, v)| P::accepts(k, v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect())
}

/// Runs a read-modify-write of the preferences holding their lock: the
/// daemons of every project share the file, and without it one daemon's
/// write can lose another's. The lock is a file of its own, so replacing
/// `preferences.json` (a rename) never drops it.
pub fn locked<P: Schema, T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    let dir = config_dir::<P>()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Test;

    impl Schema for Test {
        const CONFIG_DIR_VAR: &'static str = "RYUTEST_CONFIG_DIR";

        fn accepts(name: &str, value: &Value) -> bool {
            name == "display.quality" && value.is_string()
        }

        fn default_of(name: &str) -> Option<Value> {
            (name == "display.quality").then(|| json!("normal"))
        }
    }

    #[test]
    fn preferences_keep_what_isnt_their_default() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: the only test in this binary that sets the variable.
        unsafe { std::env::set_var(Test::CONFIG_DIR_VAR, dir.path()) };
        assert_eq!(config_dir::<Test>().unwrap(), dir.path());
        let mut p = Preferences::<Test>::load().unwrap();
        assert!(p.values.is_empty() && p.problem.is_none());
        locked::<Test, _>(|| p.set("display.quality", json!("fine"))).unwrap();
        let text = std::fs::read_to_string(dir.path().join(PREFERENCES)).unwrap();
        assert_eq!(
            parse::<Test>(&text).unwrap(),
            Map::from_iter([("display.quality".to_owned(), json!("fine"))])
        );
        assert!(text.contains("ryutest.preferences"));
        p.set("display.quality", json!("normal")).unwrap();
        assert!(Preferences::<Test>::load().unwrap().values.is_empty());
        // A file that can't be used is reported, then kept aside.
        std::fs::write(dir.path().join(PREFERENCES), "{").unwrap();
        let mut p = Preferences::<Test>::load().unwrap();
        assert!(p.problem.as_ref().unwrap().contains("using defaults"));
        p.set("display.quality", json!("coarse")).unwrap();
        assert!(dir.path().join("preferences.json.bak").is_file());
    }

    #[test]
    fn files_of_other_products_and_formats_are_refused() {
        let doc = |kind: &str, format: u64| {
            canonical::to_string(&json!({"format": format, "kind": kind, "values": {}}))
        };
        assert!(parse::<Test>(&doc("ryutest.preferences", 1)).is_ok());
        assert_eq!(
            parse::<Test>(&doc("ryucad.preferences", 1)).unwrap_err(),
            "not a ryutest.preferences file"
        );
        assert_eq!(
            parse::<Test>(&doc("ryutest.preferences", 2)).unwrap_err(),
            "format 2 is newer than this ryutest"
        );
    }
}
