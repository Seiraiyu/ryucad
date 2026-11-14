//! Cosmetic threads (ryucad 28.B): `feature.add`, `feature.edit`, the
//! report and the rebuild step of the `cosmetic_thread` type.

use super::{BodyUse, Check, FeatureKind, Write};
use crate::doc::files::Feature;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::error::Result;
use serde_json::{Value, json};

pub(crate) struct CosmeticThreads;

/// The request fields a cosmetic thread takes.
pub(crate) const THREAD: [&str; 9] = [
    "faces",
    "thread",
    "pitch",
    "tpi",
    "class",
    "hand",
    "starts",
    "from",
    "thread_depth",
];

fn thread_of(f: &Feature) -> &crate::doc::thread::CosmeticThread {
    let Feature::CosmeticThread(c) = f else {
        unreachable!("a cosmetic thread")
    };
    c
}

impl FeatureKind for CosmeticThreads {
    fn types(&self) -> &'static [&'static str] {
        &["cosmetic_thread"]
    }

    fn fields(&self, _ty: &str) -> &'static [&'static str] {
        &THREAD
    }

    fn body(&self, _ty: &str) -> BodyUse {
        BodyUse::NONE
    }

    // One --face is sent as `face`: a cosmetic thread's `faces`.
    fn take_given(&self, _ty: &str, given: &mut super::Given) -> Result<()> {
        let f = &mut given.fields;
        if let Some(face) = f.face.take() {
            match f.faces.as_mut() {
                Some(Value::Array(a)) => a.insert(0, json!(face)),
                _ => f.faces = Some(json!([face])),
            }
        }
        Ok(())
    }

    fn from_given(
        &self,
        _ty: &str,
        current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature> {
        let mut v = match current {
            Some(Feature::CosmeticThread(c)) => c.fields(),
            _ => json!({}),
        };
        let f = &cx.given.fields;
        let part = cx.part;
        let reference = |t: &str, at: &str| -> Result<Value> {
            Ok(Reference::parse(t, part)
                .map_err(|e| e.with("pointer", at))?
                .to_json())
        };
        let o = v.as_object_mut().expect("an object");
        match &f.faces {
            None => {}
            Some(Value::Array(a)) => {
                let faces = a
                    .iter()
                    .enumerate()
                    .map(|(i, x)| match x.as_str() {
                        Some(t) => reference(t, &format!("/faces/{i}")),
                        None => Err(crate::error::Error::new(
                            crate::error::Code::InvalidParam,
                            "faces is a list of <part>/<solid>/<name>",
                        )
                        .with("param", "faces")),
                    })
                    .collect::<Result<Vec<_>>>()?;
                o.insert("faces".into(), json!(faces));
            }
            Some(Value::String(t)) => {
                o.insert("faces".into(), json!([reference(t, "/faces/0")?]));
            }
            Some(_) => {
                return Err(crate::error::Error::new(
                    crate::error::Code::InvalidParam,
                    "faces is a list of <part>/<solid>/<name>",
                )
                .with("param", "faces"));
            }
        }
        match f.from.as_deref() {
            None => {}
            Some("") => {
                o.remove("from");
            }
            Some(t) => {
                o.insert("from".into(), reference(t, "/from")?);
            }
        }
        super::hole::thread_given(o, f, false)?;
        // Its depth is the feature's own: "full" or "" is its whole length.
        match &f.thread_depth {
            None => {}
            Some(Value::String(s)) if s.is_empty() || s == "full" => {
                o.remove("depth");
            }
            Some(d) => {
                o.insert("depth".into(), d.clone());
            }
        }
        v["kind"] = json!("ryucad.feature");
        v["type"] = json!("cosmetic_thread");
        v["format"] = json!(crate::doc::thread::FORMAT);
        Feature::from_json(&v, cx.unit, cx.env)
    }

    fn report(&self, part: &Part, _unit: crate::units::Unit, f: &Feature) -> Value {
        let c = thread_of(f);
        let mut v = c.fields();
        v["faces"] = json!(c.faces.iter().map(|r| r.text(&part.id)).collect::<Vec<_>>());
        if let Some(r) = &c.from {
            v["from"] = json!(r.text(&part.id));
        }
        if let Some(q) = &c.depth {
            v["depth"] = q.report();
        }
        v
    }

    // Each face reference is a cylindrical face now; what the faces make
    // together only rebuild tells.
    fn check(&self, cx: &Check<'_>, f: &Feature) -> Result<()> {
        crate::thread::check_faces(cx.part, cx.order, cx.id, thread_of(f), cx.results)
    }

    fn rebuild(&self, cx: &super::RebuildCx<'_>, f: &Feature) -> super::Outcome {
        let r = crate::thread::build(cx.part, cx.id, thread_of(f), cx.before)
            .map(|m| crate::rebuild::Output::Thread(std::sync::Arc::new(m)));
        (r, None)
    }
}

#[cfg(test)]
mod tests {
    use crate::doc::id::Id;
    use crate::error::Code;
    use crate::testing::call;
    use serde_json::{Value, json};

    /// A reference object as a request writes it: `p/<feature>/<name>`.
    fn text(r: &Value) -> String {
        format!(
            "p/{}/{}",
            r["feature"].as_str().unwrap(),
            r["name"].as_str().unwrap()
        )
    }

    #[test]
    fn add_edit_and_report() {
        let (_dir, mut s) = crate::hole::tests::drilled();
        let (side, top) = crate::thread::tests::shaft(&mut s);
        let (side, top) = (text(&side), text(&top));
        let v = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cosmetic_thread", "id": "t",
                   "faces": [side], "thread": "iso_metric:M10"}),
        )
        .unwrap();
        let f = &v["feature"];
        assert_eq!(f["faces"], json!([side]));
        assert_eq!(
            f["thread"],
            json!({"standard": "iso_metric", "size": "M10"})
        );
        let v = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "t", "thread_depth": "12 mm", "from": top,
                   "class": "4h"}),
        )
        .unwrap();
        assert_eq!(v["feature"]["depth"]["value"], "12 mm");
        assert_eq!(v["feature"]["thread"]["class"], "4h");
        assert_eq!(v["feature"]["from"], json!(top));
        // A face that isn't a cylinder is refused when written: box b's top.
        let p = Id::parse("p", "part").unwrap();
        let b = Id::parse("b", "feature").unwrap();
        let r = s.rebuilt(&p).unwrap();
        let body = r.output(&b).unwrap().solid(&b).unwrap().clone();
        let flat = body
            .body
            .faces()
            .find(|f| {
                matches!(
                    body.body.surface(body.body.face(*f).unwrap().surface),
                    Some(ryukei::geom::Surface::Plane(_))
                )
            })
            .unwrap();
        let flat = format!("p/b/{}", body.body.face(flat).unwrap().name);
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cosmetic_thread", "id": "u",
                   "faces": [flat], "thread": "iso_metric:M10"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
        // The rebuild report carries the thread as built.
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        let t = r["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "t")
            .unwrap();
        assert_eq!(t["thread"]["designation"], "M10x1.5-4h", "{t}");
    }
}
