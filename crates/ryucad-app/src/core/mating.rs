//! The Mate dialog (design 374, phase 5): an entity picked on each of two
//! instances, a form, a distance and an angle where the form takes them,
//! and Flip. Every change previews the solve (`assembly.solve` with
//! `mates`, writing nothing); Add sends `feature.add` of the mate. The
//! daemon judges what a connector takes and whether it holds.

use ryucad::proto::Request;
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    A,
    B,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Form {
    #[default]
    Fastened,
    Revolute,
    Slider,
    Cylindrical,
    Planar,
    Ball,
}

impl Form {
    pub const ALL: [Form; 6] = [
        Form::Fastened,
        Form::Revolute,
        Form::Slider,
        Form::Cylindrical,
        Form::Planar,
        Form::Ball,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Form::Fastened => "fastened",
            Form::Revolute => "revolute",
            Form::Slider => "slider",
            Form::Cylindrical => "cylindrical",
            Form::Planar => "planar",
            Form::Ball => "ball",
        }
    }

    /// Whether it takes an angle and a distance (as the mate file does).
    pub fn takes(self) -> (bool, bool) {
        match self {
            Form::Revolute => (true, false),
            Form::Slider => (false, true),
            Form::Cylindrical | Form::Planar => (true, true),
            Form::Fastened | Form::Ball => (false, false),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mating {
    pub part: String,
    /// The instance the dialog was opened from, which the first pick must
    /// be on when given.
    pub from: Option<String>,
    pub a: Option<String>,
    pub b: Option<String>,
    pub form: Form,
    pub angle: String,
    pub distance: String,
    pub flip: bool,
    /// The last preview: whether it held, and the daemon's refusal.
    pub held: Option<bool>,
    pub refusal: Option<String>,
    pub waiting: bool,
}

/// The instance an occurrence's entity reference is on: its first step.
fn instance_of<'r>(part: &str, entity: &'r str) -> Option<&'r str> {
    let rest = entity.strip_prefix(part)?.strip_prefix('/')?;
    rest.split('/').next().filter(|s| !s.is_empty())
}

impl Mating {
    pub fn new(part: &str, from: Option<&str>) -> Mating {
        Mating {
            part: part.to_owned(),
            from: from.map(str::to_owned),
            ..Mating::default()
        }
    }

    /// The side the next pick fills.
    pub fn next(&self) -> Option<Side> {
        match (&self.a, &self.b) {
            (None, _) => Some(Side::A),
            (Some(_), None) => Some(Side::B),
            _ => None,
        }
    }

    /// Takes a picked entity for the next side, or says why not.
    pub fn pick(&mut self, entity: &str) -> Result<Side, String> {
        let on = instance_of(&self.part, entity)
            .ok_or_else(|| format!("pick on an instance of {}", self.part))?;
        let side = self.next().unwrap_or(Side::B);
        if side == Side::B
            && self.a.as_deref().and_then(|a| instance_of(&self.part, a)) == Some(on)
        {
            return Err("pick on another instance: a mate joins two".into());
        }
        match side {
            Side::A => self.a = Some(entity.to_owned()),
            Side::B => self.b = Some(entity.to_owned()),
        }
        self.held = None;
        self.refusal = None;
        Ok(side)
    }

    /// The mate's fields as `feature.add` and `assembly.solve` take them.
    pub fn fields(&self) -> Value {
        let mut f = Map::new();
        f.insert("form".into(), json!(self.form.name()));
        if let Some(a) = &self.a {
            f.insert("a".into(), json!(a));
        }
        if let Some(b) = &self.b {
            f.insert("b".into(), json!(b));
        }
        let (angle, distance) = self.form.takes();
        if angle && !self.angle.trim().is_empty() {
            f.insert("angle".into(), json!(self.angle.trim()));
        }
        if distance && !self.distance.trim().is_empty() {
            f.insert("distance".into(), json!(self.distance.trim()));
        }
        if self.flip {
            f.insert("b_offset".into(), json!({"flip": true}));
        }
        Value::Object(f)
    }

    /// The preview's `assembly.solve` at `rev`, once both sides are picked.
    pub fn preview(&self, rev: Option<String>) -> Option<Request> {
        self.b.as_ref()?;
        let Value::Object(fields) = self.fields() else {
            return None;
        };
        Some(Request::AssemblySolve {
            part: self.part.clone(),
            moves: Vec::new(),
            component: false,
            rev,
            mates: vec![fields],
        })
    }

    /// `feature.add` of the mate, against the assembly's revision.
    pub fn add(&self, expected_revision: Option<String>) -> Option<Request> {
        self.b.as_ref()?;
        let mut params = json!({"part": self.part, "type": "mate"});
        for (k, v) in self.fields().as_object()? {
            params[k] = v.clone();
        }
        if let Some(r) = expected_revision {
            params["expected_revision"] = json!(r);
        }
        Request::from_rpc("feature.add", Some(params)).ok()
    }

    /// Takes a preview's answer: whether it held, or the refusal.
    pub fn previewed(&mut self, answer: Result<bool, (String, String)>) {
        self.waiting = false;
        match answer {
            Ok(held) => {
                self.held = Some(held);
                self.refusal = None;
            }
            Err((_, message)) => {
                self.held = None;
                self.refusal = Some(message);
            }
        }
    }

    /// Whether Add is enabled: both picked and the last preview solved.
    pub fn addable(&self) -> bool {
        self.b.is_some() && self.refusal.is_none() && self.held.is_some() && !self.waiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_fill_a_then_b_on_two_instances() {
        let mut d = Mating::new("frame", Some("pin"));
        assert_eq!(d.next(), Some(Side::A));
        assert_eq!(d.pick("frame/pin/shank/aa").unwrap(), Side::A);
        assert_eq!(
            d.pick("frame/pin/shank/bb").unwrap_err(),
            "pick on another instance: a mate joins two"
        );
        assert_eq!(d.pick("frame/base/slab/cc").unwrap(), Side::B);
        assert_eq!(d.next(), None);
        assert!(d.pick("other/x/y/z").is_err(), "another assembly's");
    }

    #[test]
    fn a_form_takes_only_its_quantities() {
        let mut d = Mating::new("frame", None);
        d.a = Some("frame/pin/shank/aa".into());
        d.b = Some("frame/base/slab/cc".into());
        d.form = Form::Revolute;
        d.angle = "30".into();
        d.distance = "5".into();
        d.flip = true;
        assert_eq!(
            d.fields(),
            json!({"form": "revolute", "a": "frame/pin/shank/aa", "b": "frame/base/slab/cc",
                   "angle": "30", "b_offset": {"flip": true}})
        );
        d.form = Form::Slider;
        assert_eq!(d.fields()["distance"], "5");
        assert!(d.fields().get("angle").is_none());
        d.form = Form::Fastened;
        let f = d.fields();
        assert!(f.get("angle").is_none() && f.get("distance").is_none());
    }

    #[test]
    fn the_preview_and_the_add_are_the_same_mate() {
        let mut d = Mating::new("frame", None);
        d.a = Some("frame/pin/shank/aa".into());
        d.b = Some("frame/base/slab/cc".into());
        let p = d.preview(Some("draft:dave".into())).unwrap().params();
        assert_eq!(p["mates"][0], d.fields());
        assert_eq!(p["rev"], "draft:dave");
        let a = d.add(Some("r9".into())).unwrap();
        assert_eq!(a.method(), "feature.add");
        let a = a.params();
        assert_eq!((&a["type"], &a["form"], &a["expected_revision"]), (&json!("mate"), &json!("fastened"), &json!("r9")));
        d.b = None;
        assert!(d.preview(None).is_none(), "nothing until both are picked");
    }

    #[test]
    fn a_refused_preview_disables_add_and_says_why() {
        let mut d = Mating::new("frame", None);
        d.previewed(Err(("connector_unsupported".into(), "a torus face can't be a connector".into())));
        assert!(!d.addable());
        assert_eq!(d.refusal.as_deref(), Some("a torus face can't be a connector"));
    }
}
