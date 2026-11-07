//! This machine's distributors (design 347, plan 6.C) as a request reads
//! them: each one's keys (the environment, else an owner-only file in the
//! user's config folder), the parts cache its records keep in for its
//! time to live, and how to reach it. Under the daemon's lock only kept
//! records are read; calls to a distributor are made by a long request's
//! work with the lock released (`parts::prepare`), or in process.

use crate::error::{Code, Error, Result};
use ryueda_parts::cache::Cache;
use ryueda_parts::distributor::{Clock, DISTRIBUTORS, Distributor, Live, Opened};
use ryueda_parts::fetch::{Fetch, Request, Response, unreachable};
use ryueda_parts::keys::Keys;
use ryueda_parts::rate::Limiter;
use ryueda_parts::record::Record;
use ryueda_parts::source::Call;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Reads an environment variable.
pub type Env = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// A fetcher that never connects: what `Default` distributors have.
pub struct NoNetwork;

impl Fetch for NoNetwork {
    fn get(&self, url: &str, _: &Call) -> Result<Vec<u8>> {
        Err(unreachable(url, "no network here"))
    }

    fn send(&self, req: &Request, _: &Call) -> Result<Response> {
        Err(unreachable(&req.url, "no network here"))
    }
}

#[derive(Clone)]
pub struct Distributors {
    /// The user's config folder, where keys files are; `None`: none.
    pub config: Option<PathBuf>,
    /// The parts cache; `None`: nothing kept.
    pub cache: Option<PathBuf>,
    pub env: Env,
    pub fetch: Arc<dyn Fetch>,
    /// Each distributor's API by name.
    pub bases: BTreeMap<&'static str, String>,
    pub limiter: &'static Limiter,
    pub clock: Clock,
}

impl Default for Distributors {
    /// No keys, no cache, no network.
    fn default() -> Distributors {
        Distributors {
            config: None,
            cache: None,
            env: Arc::new(|_| None),
            fetch: Arc::new(NoNetwork),
            bases: BTreeMap::new(),
            limiter: Limiter::process(),
            clock: Arc::new(crate::parts::now),
        }
    }
}

/// `parts_source_refused` for a distributor with no keys: how to get some.
pub fn not_configured(d: &Distributor) -> Error {
    Error::new(
        Code::PartsSourceRefused,
        format!("{} has no keys on this machine: {}", d.name, d.keys.how),
    )
    .with("source", d.name)
    .with("reason", "not_configured")
    .with("how", d.keys.how)
}

impl Distributors {
    /// The machine's: keys from the environment and the user's config
    /// folder, the parts cache, HTTPS, each distributor's own API.
    pub fn here() -> Distributors {
        Distributors {
            config: ryu_session::prefs::config_dir::<crate::Eda>().ok(),
            cache: crate::parts::cache_root().ok(),
            env: Arc::new(|k| std::env::var(k).ok()),
            fetch: Arc::new(crate::net::Https::new()),
            bases: DISTRIBUTORS.iter().map(|d| (d.name, (d.base)())).collect(),
            ..Distributors::default()
        }
    }

    pub fn keys(&self, d: &Distributor) -> Result<Option<Keys>> {
        let env = self.env.clone();
        ryueda_parts::keys::read(&d.keys, move |k| env(k), self.config.as_deref())
    }

    /// `d`'s source, or `None` without keys.
    pub fn open(&self, d: &Distributor) -> Result<Option<Box<dyn Live>>> {
        let Some(keys) = self.keys(d)? else {
            return Ok(None);
        };
        Ok(Some((d.open)(Opened {
            keys,
            fetch: self.fetch.clone(),
            base: self
                .bases
                .get(d.name)
                .cloned()
                .unwrap_or_else(|| (d.base)()),
            limiter: self.limiter,
            clock: self.clock.clone(),
        })))
    }

    /// `d`'s record of `key` kept on this machine and fresh at `now`.
    pub fn cached(&self, d: &Distributor, key: &str, now: u64) -> Option<Record> {
        let cache = Cache::at(self.cache.as_ref()?.clone());
        cache.get(d.name, key.trim(), now).map(|(r, _)| r)
    }

    /// Keeps `records` of `d` as its terms allow; a cache that can't be
    /// written loses them, which the next call fetches again.
    pub fn keep(&self, d: &Distributor, records: &[Record]) {
        if let Some(root) = &self.cache {
            let cache = Cache::at(root.clone());
            for r in records {
                let _ = cache.put(r, &d.terms());
            }
        }
    }

    /// `{name, kind, state, keys?, how?, attribution, ttl_s, limits,
    /// used}`: never a key.
    pub fn state(&self, d: &Distributor, now: u64) -> Value {
        let (state, keys) = match self.keys(d) {
            Ok(Some(k)) => ("configured", json!(k.origin)),
            Ok(None) => ("not_configured", Value::Null),
            Err(_) => ("keys_unreadable", Value::Null),
        };
        let mut v = json!({
            "name": d.name, "kind": "distributor", "state": state,
            "attribution": d.attribution, "ttl_s": d.ttl, "limits": d.limits,
            "used": self.limiter.used(d.name, now),
        });
        if state == "configured" {
            v["keys"] = keys;
        } else {
            v["how"] = json!(d.keys.how);
        }
        v
    }
}

/// Every distributor this ryueda has, by name.
pub fn names() -> Vec<&'static str> {
    DISTRIBUTORS.iter().map(|d| d.name).collect()
}

/// `parts login <distributor>`: `values` (one per key, in order) kept in
/// the user's config folder `dir`, owner-only. Answers where, never what.
pub fn login(dir: &std::path::Path, name: &str, values: &[String]) -> Result<Value> {
    let d = by_name(name)?;
    ryueda_parts::keys::store_in(dir, &d.keys, values)?;
    Ok(json!({"distributor": d.name, "file": dir.join(d.keys.file), "stored": true}))
}

/// `parts logout <distributor>`: its keys file removed.
pub fn logout(dir: &std::path::Path, name: &str) -> Result<Value> {
    let d = by_name(name)?;
    let removed = ryueda_parts::keys::remove_in(dir, &d.keys)?;
    Ok(json!({"distributor": d.name, "file": dir.join(d.keys.file), "removed": removed}))
}

fn by_name(name: &str) -> Result<&'static Distributor> {
    ryueda_parts::distributor::distributor(name).ok_or_else(|| {
        Error::new(
            Code::InvalidParam,
            format!("{name} isn't a distributor: {}", names().join(", ")),
        )
        .with("param", "distributor")
        .with("distributors", names())
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ryueda_parts::digikey::testing;
    use ryueda_parts::distributor::distributor;

    /// Distributors over a fake DigiKey, keys in `config` from `login`,
    /// records kept in `cache`, at a fixed time.
    pub fn fake(
        config: &std::path::Path,
        cache: &std::path::Path,
        fetch: Arc<dyn Fetch>,
        now: u64,
    ) -> Distributors {
        Distributors {
            config: Some(config.to_path_buf()),
            cache: Some(cache.to_path_buf()),
            fetch,
            bases: [("digikey", testing::BASE.to_owned())].into(),
            limiter: Box::leak(Box::new(Limiter::new())),
            clock: Arc::new(move || now),
            ..Distributors::default()
        }
    }

    pub fn logged_in(config: &std::path::Path) {
        login(
            config,
            "digikey",
            &[testing::CLIENT_ID.into(), testing::CLIENT_SECRET.into()],
        )
        .unwrap();
    }

    #[test]
    fn keys_make_a_distributor_configured_and_never_show() {
        let (cfg, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let ds = fake(cfg.path(), cache.path(), Arc::new(testing::fake()), 1_000);
        let dk = distributor("digikey").unwrap();
        let s = ds.state(dk, 1_000);
        assert_eq!(s["state"], "not_configured");
        assert!(s["how"].as_str().unwrap().contains("parts login digikey"));
        assert!(ds.open(dk).unwrap().is_none());
        let v = login(cfg.path(), "digikey", &["id".into(), "s3cr3t-value".into()]).unwrap();
        assert!(!v.to_string().contains("s3cr3t"));
        let s = ds.state(dk, 1_000);
        assert_eq!(
            (s["state"].as_str(), s["keys"].as_str()),
            (Some("configured"), Some("file"))
        );
        assert!(!s.to_string().contains("s3cr3t"));
        assert!(
            logout(cfg.path(), "digikey").unwrap()["removed"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(
            login(cfg.path(), "acme", &[]).unwrap_err().code,
            Code::InvalidParam
        );
        // The environment wins over the file.
        let ds = Distributors {
            env: Arc::new(|k| (k.starts_with("RYUEDA_DIGIKEY_")).then(|| "from-env".to_owned())),
            ..ds
        };
        assert_eq!(ds.state(dk, 1)["keys"], "environment");
    }

    #[test]
    fn records_keep_a_day() {
        let (cfg, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        logged_in(cfg.path());
        let ds = fake(cfg.path(), cache.path(), Arc::new(testing::fake()), 1_000);
        let dk = distributor("digikey").unwrap();
        let r = ds
            .open(dk)
            .unwrap()
            .unwrap()
            .get("296-1234-1-ND", &Call::default())
            .unwrap()
            .unwrap();
        ds.keep(dk, std::slice::from_ref(&r));
        assert_eq!(ds.cached(dk, "296-1234-1-ND", 1_000 + 86_400), Some(r));
        assert_eq!(ds.cached(dk, "296-1234-1-ND", 1_000 + 86_401), None);
        assert!(
            Distributors::default()
                .cached(dk, "296-1234-1-ND", 1_000)
                .is_none()
        );
    }
}
