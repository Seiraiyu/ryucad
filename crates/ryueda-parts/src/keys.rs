//! A live source's keys (design 347): from the environment, else from an
//! owner-only file beside the user's preferences that `ryueda parts login`
//! writes and `parts logout` removes. Never in the preferences or a
//! project, never logged, never in an error: `Debug` and every message
//! name a key's field, not its value.

use crate::error::{Code, Error, Result};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;

pub const KIND: &str = "ryueda.keys";
pub const FORMAT: u64 = 1;

/// One key a source needs, and the variable that gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    pub var: &'static str,
}

/// What keys a source needs, the file they are kept in, and where a
/// person gets them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeySpec {
    pub fields: &'static [Field],
    /// Its name in the user's config folder.
    pub file: &'static str,
    pub how: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Environment,
    File,
}

/// A source's keys.
#[derive(Clone, PartialEq, Eq)]
pub struct Keys {
    values: BTreeMap<&'static str, String>,
    pub origin: Origin,
}

impl fmt::Debug for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.values.keys().copied().collect();
        write!(f, "Keys {{ fields: {names:?}, origin: {:?} }}", self.origin)
    }
}

impl Keys {
    /// For tests and a source's own fakes.
    pub fn of(pairs: &[(&'static str, &str)], origin: Origin) -> Keys {
        Keys {
            values: pairs.iter().map(|(k, v)| (*k, (*v).to_owned())).collect(),
            origin,
        }
    }

    /// The value of `field` (`""` for one it doesn't hold).
    pub fn get(&self, field: &str) -> &str {
        self.values.get(field).map_or("", String::as_str)
    }
}

/// Whether `v` can be a key: 1 to 4,096 visible ASCII characters, since
/// keys travel in headers and forms.
pub fn valid(v: &str) -> bool {
    (1..=4096).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_graphic())
}

fn refused(spec: &KeySpec, why: String) -> Error {
    Error::new(Code::PartsSourceRefused, why)
        .with("reason", "keys")
        .with("file", spec.file)
}

/// `spec`'s keys: every variable set (and none empty), else the file in
/// `dir`, else `None`. A file that doesn't read is `parts_source_refused`
/// naming the file, never what it holds.
pub fn read(
    spec: &KeySpec,
    env: impl Fn(&str) -> Option<String>,
    dir: Option<&Path>,
) -> Result<Option<Keys>> {
    let from_env: Vec<(&'static str, String)> = spec
        .fields
        .iter()
        .filter_map(|f| env(f.var).filter(|v| !v.is_empty()).map(|v| (f.name, v)))
        .collect();
    if from_env.len() == spec.fields.len() {
        if let Some((name, _)) = from_env.iter().find(|(_, v)| !valid(v)) {
            return Err(refused(
                spec,
                format!("the variable for {name} isn't a key"),
            ));
        }
        return Ok(Some(Keys {
            values: from_env.into_iter().collect(),
            origin: Origin::Environment,
        }));
    }
    let Some(dir) = dir else { return Ok(None) };
    let path = dir.join(spec.file);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io(&path, &e)),
    };
    let bad = || {
        refused(
            spec,
            format!(
                "{} doesn't read as keys; run ryueda parts login again",
                spec.file
            ),
        )
    };
    let Ok(Value::Object(o)) = serde_json::from_str::<Value>(&text) else {
        return Err(bad());
    };
    if o.get("kind").and_then(Value::as_str) != Some(KIND)
        || o.get("format").and_then(Value::as_u64) != Some(FORMAT)
    {
        return Err(bad());
    }
    let mut values = BTreeMap::new();
    for f in spec.fields {
        match o.get(f.name).and_then(Value::as_str) {
            Some(v) if valid(v) => {
                values.insert(f.name, v.to_owned());
            }
            _ => return Err(bad()),
        }
    }
    Ok(Some(Keys {
        values,
        origin: Origin::File,
    }))
}

/// Writes `values` (one per field, in order) as `spec`'s file in `dir`:
/// through a temporary file readable by its owner alone, then renamed.
pub fn store_in(dir: &Path, spec: &KeySpec, values: &[String]) -> Result<()> {
    if values.len() != spec.fields.len() {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "give {} ({})",
                spec.fields.len(),
                spec.fields
                    .iter()
                    .map(|f| f.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    let mut o = Map::new();
    o.insert("kind".into(), KIND.into());
    o.insert("format".into(), FORMAT.into());
    for (f, v) in spec.fields.iter().zip(values) {
        let v = v.trim();
        if !valid(v) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{} is 1 to 4096 visible ASCII characters", f.name),
            )
            .with("field", f.name));
        }
        o.insert(f.name.into(), v.into());
    }
    fs::create_dir_all(dir).map_err(|e| Error::io(dir, &e))?;
    let path = dir.join(spec.file);
    let tmp = dir.join(format!("{}.{}.tmp", spec.file, std::process::id()));
    let written = (|| {
        use std::io::Write;
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&tmp)?;
        f.write_all(ryu_doc::canonical::to_string(&Value::Object(o)).as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, &path)
    })();
    written.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        Error::io(&path, &e)
    })
}

/// Removes `spec`'s file from `dir`; whether there was one.
pub fn remove_in(dir: &Path, spec: &KeySpec) -> Result<bool> {
    let path = dir.join(spec.file);
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Error::io(&path, &e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: KeySpec = KeySpec {
        fields: &[
            Field {
                name: "client_id",
                var: "T_ID",
            },
            Field {
                name: "client_secret",
                var: "T_SECRET",
            },
        ],
        file: "parts-test.keys",
        how: "make some",
    };

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn keys_come_from_the_environment_then_the_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(&SPEC, env(&[]), Some(dir.path())).unwrap(), None);
        store_in(dir.path(), &SPEC, &["id-1".into(), " s3cret ".into()]).unwrap();
        let k = read(&SPEC, env(&[("T_ID", "only-one")]), Some(dir.path()))
            .unwrap()
            .unwrap();
        assert_eq!(
            (k.get("client_id"), k.get("client_secret"), k.origin),
            ("id-1", "s3cret", Origin::File)
        );
        let k = read(
            &SPEC,
            env(&[("T_ID", "e"), ("T_SECRET", "f")]),
            Some(dir.path()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            (k.get("client_secret"), k.origin),
            ("f", Origin::Environment)
        );
        // No value ever shows in Debug.
        assert_eq!(
            format!("{k:?}"),
            "Keys { fields: [\"client_id\", \"client_secret\"], origin: Environment }"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join(SPEC.file))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(remove_in(dir.path(), &SPEC).unwrap());
        assert!(!remove_in(dir.path(), &SPEC).unwrap());
        assert_eq!(read(&SPEC, env(&[]), None).unwrap(), None);
    }

    #[test]
    fn a_broken_file_is_refused_without_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(SPEC.file),
            "{\"client_secret\": \"leak-me\"",
        )
        .unwrap();
        let e = read(&SPEC, env(&[]), Some(dir.path())).unwrap_err();
        assert_eq!(
            (e.code, e.details["reason"].as_str()),
            (Code::PartsSourceRefused, Some("keys"))
        );
        assert!(!e.to_string().contains("leak-me") && !e.details.to_string().contains("leak-me"));
        let e = store_in(dir.path(), &SPEC, &["id".into(), "two words".into()]).unwrap_err();
        assert_eq!(
            (e.code, e.details["field"].as_str()),
            (Code::InvalidParam, Some("client_secret"))
        );
        assert!(!e.to_string().contains("two words"));
        assert_eq!(
            store_in(dir.path(), &SPEC, &["id".into()])
                .unwrap_err()
                .code,
            Code::InvalidParam
        );
    }
}
