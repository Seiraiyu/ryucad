//! Hole checks (design 533, Hole checks), run only when asked: `ryucad
//! holes --check` (`holes.check`) and `ryucad check --holes`; never by a
//! rebuild, an import, a re-import or a plain check (the owner's rule).
//! Each finding is `{code, severity, holes, message, details, fixes}`,
//! `fixes` being requests an agent can send as they are. Checks read only
//! the rebuilt part, run per solid in parallel, and come out in a fixed
//! order (the rows' order, then the codes' order below). A check that
//! can't be decided in budget is `hole_cannot_check`, never passed.

use crate::doc::id::Id;
use crate::error::Result;
use crate::holes::{Holes, Row};
use crate::session::Session;
use rayon::prelude::*;
use serde_json::{Value, json};

/// Every code and its severity, in the order findings come out.
pub const CODES: [(&str, &str); 7] = [
    ("hole_breakout", "error"),
    ("hole_breaks_through", "error"),
    ("hole_thread_past_material", "error"),
    ("hole_edge_distance", "warning"),
    ("hole_thread_engagement", "warning"),
    ("hole_cannot_check", "warning"),
    ("hole_tap_drill_nonstandard", "info"),
];

/// How many times the checks ran (a test holds that nothing runs them
/// unasked).
#[doc(hidden)]
pub static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// What the checks take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// A bore's wall is at least this times its diameter from any other face.
    pub edge_factor: f64,
    /// A thread engages at least this times its diameter.
    pub min_engagement: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            edge_factor: 1.0,
            min_engagement: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub code: &'static str,
    pub holes: Vec<String>,
    pub message: String,
    pub details: Value,
    pub fixes: Vec<Value>,
}

impl Finding {
    pub fn severity(&self) -> &'static str {
        CODES
            .iter()
            .find(|(c, _)| *c == self.code)
            .map_or("error", |(_, s)| s)
    }

    fn order(&self) -> usize {
        CODES
            .iter()
            .position(|(c, _)| *c == self.code)
            .unwrap_or(CODES.len())
    }

    pub fn to_json(&self) -> Value {
        json!({"code": self.code, "severity": self.severity(), "holes": self.holes,
               "message": self.message, "details": self.details, "fixes": self.fixes})
    }
}

/// A `feature.edit` of `feature` in `part` with `fields`, as a fix.
pub(crate) fn edit(part: &Id, feature: &Id, fields: Value) -> Value {
    let mut params = json!({"part": part, "id": feature});
    if let Value::Object(f) = fields {
        for (k, v) in f {
            params[k] = v;
        }
    }
    json!({"method": "feature.edit", "params": params})
}

/// A length for a fix: millimetres, floored to 0.01 mm (a value within
/// 1e-11 m under a step counts as on it).
pub(crate) fn mm_floor(m: f64) -> String {
    hundredths((m * 1e5 + 1e-6).floor() as i64)
}

/// A length for a fix, rounded up to 0.01 mm (within 1e-11 m over a step
/// counts as on it).
pub(crate) fn mm_ceil(m: f64) -> String {
    hundredths((m * 1e5 - 1e-6).ceil() as i64)
}

fn hundredths(h: i64) -> String {
    let t = format!("{}.{:02}", h / 100, h % 100);
    let t = t.trim_end_matches('0').trim_end_matches('.');
    format!("{t} mm")
}

/// What one solid's rows give: each check in turn, in the rows' order.
pub(crate) struct Ctx<'a> {
    pub session_part: &'a crate::doc::project::Part,
    pub result: &'a crate::rebuild::PartResult,
    pub holes: &'a Holes,
    pub opts: Options,
    pub job: &'a ryu_session::job::Job,
}

fn solid_findings(cx: &Ctx, rows: &[&Row]) -> Result<Vec<Finding>> {
    let mut out: Vec<Finding> = Vec::new();
    // Tasks 28 to 31 add their checks here, each `out.extend(…)`.
    let _ = (cx, rows);
    Ok(out)
}

/// Every finding of `part` (one solid's with `solid`).
pub fn check(
    session: &mut Session,
    part: &Id,
    solid: Option<&str>,
    opts: Options,
    rev: Option<&str>,
) -> Result<(Vec<Finding>, usize)> {
    RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let holes = crate::holes::list(session, part, rev)?;
    let (_, at) = session.at(rev)?;
    let p = at.project().part(part)?.clone();
    let result = at.rebuilt(part)?.clone();
    let job = at.job.clone();
    let cx = Ctx {
        session_part: &p,
        result: &result,
        holes: &holes,
        opts,
        job: &job,
    };
    // Rows by solid, in their order.
    let mut solids: Vec<(&Id, Vec<&Row>)> = Vec::new();
    for r in holes
        .rows
        .iter()
        .filter(|r| solid.is_none_or(|s| r.solid.as_str() == s))
    {
        match solids.iter_mut().find(|(s, _)| *s == &r.solid) {
            Some((_, v)) => v.push(r),
            None => solids.push((&r.solid, vec![r])),
        }
    }
    let checked = solids.iter().map(|(_, v)| v.len()).sum();
    let found: Vec<Result<Vec<Finding>>> = solids
        .par_iter()
        .map(|(_, rows)| solid_findings(&cx, rows))
        .collect();
    let mut out = Vec::new();
    for f in found {
        let mut f = f?;
        f.sort_by_key(Finding::order);
        out.extend(f);
    }
    Ok((out, checked))
}

/// `holes.check`.
pub fn request(
    session: &mut Session,
    part: &str,
    solid: Option<&str>,
    edge_factor: Option<f64>,
    min_engagement: Option<f64>,
    rev: Option<&str>,
) -> Result<Value> {
    let pid = Id::parse(part, "part")?;
    let d = Options::default();
    let opts = Options {
        edge_factor: edge_factor.unwrap_or(d.edge_factor),
        min_engagement: min_engagement.unwrap_or(d.min_engagement),
    };
    for (k, x) in [
        ("edge_factor", opts.edge_factor),
        ("min_engagement", opts.min_engagement),
    ] {
        if !(x > 0.0 && x.is_finite()) {
            return Err(crate::error::Error::new(
                crate::error::Code::InvalidParam,
                format!("{k} is a number above 0, not {x}"),
            )
            .with("param", k));
        }
    }
    let (findings, checked) = check(session, &pid, solid, opts, rev)?;
    let passed = !findings.iter().any(|f| f.severity() == "error");
    Ok(json!({
        "findings": findings.iter().map(Finding::to_json).collect::<Vec<_>>(),
        "checked": checked,
        "passed": passed,
        "options": {"edge_factor": opts.edge_factor, "min_engagement": opts.min_engagement},
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::hole::tests::{drilled, put};
    use crate::testing::call;
    use serde_json::json;

    #[test]
    fn a_clean_part_has_no_findings() {
        let (_dir, mut s) = drilled();
        put(
            &mut s,
            "h",
            json!({"points": ["p1"], "form": "simple", "diameter": 2,
            "depth": "through_all"}),
        );
        let v = call(&mut s, "holes.check", json!({"part": "p"})).unwrap();
        assert_eq!(v["findings"], json!([]));
        assert_eq!(v["checked"], 1);
        assert_eq!(v["passed"], true);
        assert_eq!(
            v["options"],
            json!({"edge_factor": 1.0, "min_engagement": 1.0})
        );
    }
}
