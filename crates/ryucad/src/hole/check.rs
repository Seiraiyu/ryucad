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
use crate::rebuild::Results;
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

/// Rows of one feature with one value of `key`, in order, as one finding's
/// holes (a pattern's hundred rows are one finding).
fn grouped<'a, K: PartialEq>(
    rows: &[&'a Row],
    key: impl Fn(&Row) -> Option<K>,
) -> Vec<(K, Vec<&'a Row>)> {
    let mut out: Vec<(K, Vec<&Row>)> = Vec::new();
    for r in rows {
        let Some(k) = key(r) else { continue };
        match out
            .iter_mut()
            .find(|(k2, g)| *k2 == k && g[0].feature == r.feature)
        {
            Some((_, g)) => g.push(r),
            None => out.push((k, vec![r])),
        }
    }
    out
}

fn refs(g: &[&Row]) -> Vec<String> {
    g.iter().map(|r| r.reference.clone()).collect()
}

/// `hole_tap_drill_nonstandard`: a tapped hole's drill that is no standard
/// drill (an override, or an engagement rule's result), with the nearest.
fn tap_drills(part: &Id, rows: &[&Row]) -> Vec<Finding> {
    grouped(rows, |r| {
        let t = r.sizes.thread.as_ref()?;
        (!r.sizes.drill_standard).then(|| (r.sizes.drill.to_nm(), t.tpi.is_some()))
    })
    .into_iter()
    .map(|((nm, inch), g)| {
        let d = ryu_standards::drill::nearest(nm, inch);
        let text = if inch {
            ryu_standards::in_decimal(d.nm).map_or_else(
                || format!("{} mm", ryu_standards::mm_decimal(d.nm)),
                |t| format!("{t} in"),
            )
        } else {
            format!("{} mm", ryu_standards::mm_decimal(d.nm))
        };
        Finding {
            code: "hole_tap_drill_nonstandard",
            holes: refs(&g),
            message: format!(
                "{}: tap drill {} is no standard drill; the nearest is {} ({})",
                g[0].feature, g[0].sizes.drill.text, d.name, text
            ),
            details: json!({"drill": g[0].sizes.drill.text, "nearest": text, "name": d.name,
                            "source": g[0].sizes.drill.source.kind()}),
            fixes: vec![edit(part, &g[0].feature, json!({"diameter": text}))],
        }
    })
    .collect()
}

/// `hole_thread_engagement`: threaded length under `k × D`.
fn engagement(part: &Id, rows: &[&Row], k: f64) -> Vec<Finding> {
    grouped(rows, |r| {
        let t = r.sizes.thread.as_ref()?;
        let d = t.major.as_ref().map_or(r.sizes.drill.si, |m| m.si);
        let start = r.sizes.thread_start();
        let end = r.depth.2.filter(|x| x.is_finite())?;
        let len = end - start;
        (len < k * d - ryukei::tol::LINEAR_RESOLUTION)
            .then(|| ((len * 1e9).round() as i64, (d * 1e9).round() as i64))
    })
    .into_iter()
    .map(|((len_nm, d_nm), g)| {
        let (len, d) = (len_nm as f64 / 1e9, d_nm as f64 / 1e9);
        let r = g[0];
        let want = r.sizes.thread_start() + k * d;
        // A blind thread may go deeper, its drill with it (3 pitches more);
        // a through one runs the material already.
        let blind =
            r.depth.0.is_finite() && r.sizes.thread.as_ref().is_some_and(|t| t.depth.is_some());
        let fixes = if blind {
            let mut f = json!({"thread_depth": mm_ceil(want)});
            let pitch = r.sizes.thread.as_ref().map_or(0.0, |t| t.pitch.si);
            if want > r.depth.0 {
                f["depth"] = json!({"blind": mm_ceil(want + 3.0 * pitch)});
            }
            vec![edit(part, &r.feature, f)]
        } else {
            Vec::new()
        };
        Finding {
            code: "hole_thread_engagement",
            holes: refs(&g),
            message: format!(
                "{}: {} of thread engages {}, under {k} x its {} diameter ({})",
                r.feature,
                crate::hole::check::mm_floor(len),
                r.spec,
                mm_floor(d),
                mm_ceil(k * d)
            ),
            details: json!({"length_m": len, "diameter_m": d, "factor": k, "needed_m": k * d}),
            fixes,
        }
    })
    .collect()
}

/// The material along a row's axis from its mouth: where the first
/// stretch of its solid (as the feature that placed it saw it) ends. None
/// when the row has no record of it; `Err` when uncertain.
fn material(cx: &Ctx, r: &Row) -> std::result::Result<Option<f64>, ()> {
    let Some(m) = &r.material else {
        return Ok(None);
    };
    let Some(Ok(o)) = cx.result.result(&m.view) else {
        return Ok(None);
    };
    let Some(b) = o.solid(&m.solid) else {
        return Ok(None);
    };
    let work = crate::work::work(cx.job);
    let s = crate::hole::exits::exits(&b.body, m.position, m.axis, &work).map_err(|_| ())?;
    Ok(crate::hole::exits::first_exit(&s))
}

/// The floor a blind hole leaves (ryucad's rule): three pitches under a
/// tapped hole, a quarter of the drill's diameter under a plain one.
fn floor(r: &Row) -> (f64, &'static str) {
    match &r.sizes.thread {
        Some(t) => (3.0 * t.pitch.si, "three pitches under a tapped hole"),
        None => (
            r.sizes.drill.si / 4.0,
            "a quarter of the drill's diameter under a plain hole",
        ),
    }
}

/// The hole feature a row carries the intent of, and its depth.
fn hole_of<'a>(cx: &'a Ctx, r: &Row) -> Option<&'a crate::doc::hole::Hole> {
    match cx.session_part.features.get(&r.feature)? {
        Ok(crate::doc::files::Feature::Hole(h)) => Some(h),
        _ => None,
    }
}

/// The blind depth fix leaving `floor` under the hole in `material`: the
/// hole's own kind of depth (to the shoulder, or to the tip).
fn blind_fix(r: &Row, h: &crate::doc::hole::Hole, material: f64) -> Value {
    use crate::doc::hole::{Bottom, Depth};
    let (f, _) = floor(r);
    let rad = r.sizes.drill.si / 2.0;
    let point = match &h.bottom {
        Bottom::Drill(a) => rad / ryukei::math::tan(a.radians() / 2.0),
        Bottom::Ball => rad,
        Bottom::Flat => 0.0,
    };
    let tip = material - f;
    match &h.depth {
        Depth::Tip(_) => json!({"tip": mm_floor(tip)}),
        _ => json!({"blind": mm_floor(tip - point)}),
    }
}

fn breaks_through(cx: &Ctx, rows: &[&Row]) -> Vec<Finding> {
    let part = &cx.session_part.id;
    let mut out = Vec::new();
    for r in rows {
        let Some(h) = hole_of(cx, r) else { continue };
        if !h.depth.is_blind()
            || ["bottom", "tip", "ball"]
                .iter()
                .any(|s| r.faces.contains_key(*s))
        {
            continue;
        }
        let m = match material(cx, r) {
            Ok(m) => m,
            Err(()) => {
                out.push(cannot(
                    r,
                    "hole_breaks_through",
                    "where its axis leaves the material is uncertain",
                ));
                continue;
            }
        };
        let mut fixes = vec![edit(part, &r.feature, json!({"depth": "through_all"}))];
        if let Some(m) = m {
            let mut f = json!({"depth": blind_fix(r, h, m)});
            if let Some(t) = &r.sizes.thread
                && let Some(td) = &t.depth
            {
                let shoulder = m - floor(r).0 - (r.depth.1 - r.depth.0);
                if td.si > shoulder - 3.0 * t.pitch.si {
                    f["thread_depth"] = json!(mm_floor(shoulder - 3.0 * t.pitch.si));
                }
            }
            fixes.push(edit(part, &r.feature, f));
        }
        out.push(Finding {
            code: "hole_breaks_through",
            holes: vec![r.reference.clone()],
            message: format!(
                "{}: blind hole {} leaves the material ({}): its bottom is gone",
                r.feature,
                r.spec,
                m.map_or("depth unknown".into(), mm_floor)
            ),
            details: json!({"material_m": m, "rule": floor(r).1}),
            fixes,
        });
    }
    out
}

fn past_material(cx: &Ctx, rows: &[&Row]) -> Vec<Finding> {
    let part = &cx.session_part.id;
    let mut out = Vec::new();
    for r in rows {
        let Some(t) = &r.sizes.thread else { continue };
        let Some(td) = &t.depth else { continue }; // a full thread runs the material
        let m = match material(cx, r) {
            Ok(Some(m)) => m,
            Ok(None) => continue,
            Err(()) => {
                out.push(cannot(
                    r,
                    "hole_thread_past_material",
                    "where its axis leaves the material is uncertain",
                ));
                continue;
            }
        };
        if td.si <= m + ryukei::tol::LINEAR_RESOLUTION {
            continue;
        }
        let short = m - 3.0 * t.pitch.si;
        out.push(Finding {
            code: "hole_thread_past_material",
            holes: vec![r.reference.clone()],
            message: format!(
                "{}: {} thread depth {} exceeds {} of material: use THRU, or {}",
                r.feature,
                t.size,
                td.text,
                mm_floor(m),
                mm_floor(short)
            ),
            details: json!({"material_m": m, "thread_m": td.si}),
            fixes: vec![
                edit(
                    part,
                    &r.feature,
                    json!({"depth": "through_all", "thread_depth": "full"}),
                ),
                edit(part, &r.feature, json!({"thread_depth": mm_floor(short)})),
            ],
        });
    }
    out
}

/// `hole_cannot_check`: a check this row couldn't decide.
fn cannot(r: &Row, check: &str, why: &str) -> Finding {
    Finding {
        code: "hole_cannot_check",
        holes: vec![r.reference.clone()],
        message: format!("{}: {check} couldn't be decided: {why}", r.reference),
        details: json!({"check": check}),
        fixes: Vec::new(),
    }
}

fn solid_findings(cx: &Ctx, rows: &[&Row]) -> Result<Vec<Finding>> {
    let mut out: Vec<Finding> = Vec::new();
    let part = &cx.session_part.id;
    out.extend(tap_drills(part, rows));
    out.extend(engagement(part, rows, cx.opts.min_engagement));
    out.extend(breaks_through(cx, rows));
    out.extend(past_material(cx, rows));
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

    #[test]
    fn tap_drills_and_engagement() {
        let (_dir, mut s) = drilled();
        // An override of 2.52 mm is no standard drill; 2.5 mm is the nearest.
        put(
            &mut s,
            "h",
            json!({"format": 23, "points": ["p1", "p2"], "form": "simple",
            "thread": {"standard": "iso_metric", "size": "M3", "depth": "2 mm"},
            "diameter": "2.52 mm", "depth": {"blind": "4 mm"}}),
        );
        let v = call(&mut s, "holes.check", json!({"part": "p"})).unwrap();
        let f = &v["findings"];
        let drill = f
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["code"] == "hole_tap_drill_nonstandard")
            .unwrap();
        assert_eq!(drill["severity"], "info");
        assert_eq!(drill["holes"], json!(["p/h/p1", "p/h/p2"]));
        assert_eq!(drill["details"]["nearest"], "2.5 mm");
        assert_eq!(drill["fixes"][0]["params"]["diameter"], "2.5 mm");
        // 2 mm of M3 is under 1 x D.
        let eng = f
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["code"] == "hole_thread_engagement")
            .unwrap();
        assert_eq!(eng["severity"], "warning");
        assert_eq!(eng["fixes"][0]["params"]["thread_depth"], "3 mm");
        // At --min-engagement 0.5 it passes.
        let v = call(
            &mut s,
            "holes.check",
            json!({"part": "p", "min_engagement": 0.5}),
        )
        .unwrap();
        assert!(
            !v["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x["code"] == "hole_thread_engagement")
        );
        // Applying the fixes clears both.
        for fix in [drill["fixes"][0].clone(), eng["fixes"][0].clone()] {
            call(
                &mut s,
                fix["method"].as_str().unwrap(),
                fix["params"].clone(),
            )
            .unwrap();
        }
        // (p2's bore is 2.74 mm from the box's end: under 1 x M3's 3 mm,
        // so the edge check is eased here.)
        let v = call(
            &mut s,
            "holes.check",
            json!({"part": "p", "edge_factor": 0.5}),
        )
        .unwrap();
        assert_eq!(v["findings"], json!([]), "{v}");
    }

    /// Plate `pl` 40 x 40 x 12 mm and sketch `t` on its top with point
    /// p1 at (20, 20).
    fn plate(s: &mut Session) {
        call(
            s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "pl",
            "params": {"x": 40, "y": 40, "z": 12}, "placement": {"origin": [100, 0, 0]}}),
        )
        .unwrap();
        call(
            s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t",
            "placement": {"origin": [100, 0, 12]}}),
        )
        .unwrap();
        call(
            s,
            "sketch.edit",
            json!({"part": "p", "sketch": "t", "ops": [
            {"op": "add", "type": "point", "at": [20, 20]},
            {"op": "add", "type": "fix", "a": "p1"}]}),
        )
        .unwrap();
    }

    #[test]
    fn a_thread_past_its_material_and_a_hole_that_broke_through() {
        let (_dir, mut s) = drilled();
        plate(&mut s);
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "hole", "id": "m6",
            "sketch": "t", "spec": "M6x1.0-6H 15 DEEP, DRILL 18 DEEP", "targets": ["pl"]}),
        )
        .unwrap();
        let v = call(&mut s, "holes.check", json!({"part": "p", "solid": "pl"})).unwrap();
        let by = |c: &str| {
            v["findings"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["code"] == c)
                .cloned()
                .unwrap_or_else(|| panic!("{c}: {v}"))
        };
        let past = by("hole_thread_past_material");
        assert_eq!(
            past["message"],
            "m6: M6 thread depth 15 mm exceeds 12 mm of material: use THRU, or 9 mm"
        );
        assert!((past["details"]["material_m"].as_f64().unwrap() - 0.012).abs() < 1e-12);
        let through = by("hole_breaks_through");
        assert_eq!(through["holes"], json!(["p/m6/p1"]));
        assert_eq!(through["fixes"][0]["params"]["depth"], "through_all");
        // Either fix leaves it clean of both.
        let mut fixed = past["fixes"][1].clone();
        assert_eq!(fixed["params"]["thread_depth"], "9 mm");
        fixed["params"]["depth"] = through["fixes"][1]["params"]["depth"].clone();
        call(&mut s, "feature.edit", fixed["params"].clone()).unwrap();
        let v = call(&mut s, "holes.check", json!({"part": "p", "solid": "pl"})).unwrap();
        assert!(
            !v["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x["code"] == "hole_thread_past_material"
                    || x["code"] == "hole_breaks_through"),
            "{v}"
        );
        call(
            &mut s,
            "feature.edit",
            through["fixes"][0]["params"].clone(),
        )
        .unwrap();
        let v = call(&mut s, "holes.check", json!({"part": "p", "solid": "pl"})).unwrap();
        assert!(
            !v["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x["code"] == "hole_thread_past_material"
                    || x["code"] == "hole_breaks_through"),
            "{v}"
        );
    }
}
