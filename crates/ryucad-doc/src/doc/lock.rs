//! `ryucad.lock`: which commit each label of another project that the
//! working files reach resolved to, committed beside the manifest so
//! every clone and the service build the same thing. The type is
//! ryupdm's lock (`ryu_doc::lock`), shared with other products; this is
//! its kind as ryucad writes it.

use crate::error::Result;
pub use ryu_doc::lock::{FORMAT, Lock, Locked, LockedLabel};
use serde_json::Value;

/// The lock's file name, beside the manifest.
pub const LOCK: &str = "ryucad.lock";

pub const KIND: &str = "ryucad.lock";

/// ryucad's lock as JSON and back, kind `ryucad.lock`: the calls ryucad
/// made before the type moved to ryu-doc.
pub trait CadLock: Sized {
    fn to_json(&self) -> Value;
    fn from_json(v: &Value) -> Result<Self>;
}

impl CadLock for Lock {
    fn to_json(&self) -> Value {
        self.to_json_as(KIND)
    }

    fn from_json(v: &Value) -> Result<Lock> {
        Lock::from_json_as(v, KIND)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::id::{Id, ProjectId};
    use serde_json::json;
    use std::collections::BTreeMap;
    use crate::error::Code;

    fn lock() -> Lock {
        let (a, b) = (
            ProjectId::from_bytes([7; 16]),
            ProjectId::from_bytes([9; 16]),
        );
        let mut labels = BTreeMap::new();
        labels.insert(
            (Id::parse("m6-bolt", "part").unwrap(), "B".to_owned()),
            LockedLabel {
                commit: "1".repeat(40),
                content: "2".repeat(64),
            },
        );
        let mut projects = BTreeMap::new();
        projects.insert(
            a.clone(),
            Locked {
                url: "https://github.com/acme/fasteners.git".into(),
                path: "".into(),
                release_branch: "main".into(),
                via: vec![],
                labels: labels.clone(),
            },
        );
        projects.insert(
            b,
            Locked {
                url: "file:///srv/lib".into(),
                path: "lib".into(),
                release_branch: "release".into(),
                via: vec![a],
                labels,
            },
        );
        Lock { projects }
    }

    #[test]
    fn a_lock_reads_and_writes_back() {
        let l = lock();
        let v = l.to_json();
        let a = ProjectId::from_bytes([7; 16]);
        assert_eq!(v["kind"], "ryucad.lock");
        assert_eq!(
            v["projects"][a.as_str()]["labels"]["m6-bolt/B"],
            json!({"commit": "1".repeat(40), "content": "2".repeat(64)})
        );
        assert_eq!(Lock::from_json(&v).unwrap(), l);
        let bolt = Id::parse("m6-bolt", "part").unwrap();
        assert_eq!(l.label(&a, &bolt, "B").unwrap().commit, "1".repeat(40));
        assert!(l.label(&a, &bolt, "C").is_none());
    }

    #[test]
    fn a_lock_is_checked() {
        let a = ProjectId::from_bytes([7; 16]).to_string();
        let good = lock().to_json();
        let refused = |change: &dyn Fn(&mut Value)| {
            let mut v = good.clone();
            change(&mut v);
            Lock::from_json(&v).unwrap_err()
        };
        let e = refused(&|v| v["projects"] = json!([]));
        assert_eq!(e.details["pointer"], "/projects");
        let e = refused(&|v| v["projects"]["x"] = json!({}));
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::InvalidId, &json!("/projects/x"))
        );
        let e = refused(&|v| v["projects"][&a]["labels"]["m6-bolt/B"]["commit"] = json!("HEAD"));
        assert_eq!(
            e.details["pointer"],
            format!("/projects/{a}/labels/m6-bolt~1B/commit")
        );
        let e = refused(&|v| v["projects"][&a]["labels"]["bolt"] = json!({}));
        assert_eq!(e.details["pointer"], format!("/projects/{a}/labels/bolt"));
        let e = refused(&|v| v["projects"][&a]["via"] = json!(["x"]));
        assert_eq!(e.details["pointer"], format!("/projects/{a}/via/0"));
        let e = refused(&|v| v["projects"][&a]["extra"] = json!(1));
        assert_eq!(e.details["pointer"], format!("/projects/{a}/extra"));
        let e = refused(&|v| v["format"] = json!(2));
        assert_eq!(e.code, Code::FormatTooNew);
    }
}
