//! The drawing lint (design 417, plan 9.C): rules with stable codes and
//! weights over a built drawing's sheets, each finding with its points
//! and `fixes` (ready `drawing.edit` batches), and each sheet's score:
//! 100 less its findings' points, at least 0. Severities and weights come
//! from `drawing_lint.json` (warnings unless it says otherwise).
//! Whether a part is fully dimensioned isn't judged.

use super::{DIM_ROOM, SheetBuilt, annotate::DIM_GAP};
use crate::doc::drawing::DrawingFile;
use crate::doc::drawing_annotation::Kind;
use crate::doc::drawing_lint::{Prefs, Severity};
use crate::session::Session;
use ryu_draw::{Item, P};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One rule of the lint.
pub struct Rule {
    pub code: &'static str,
    pub weight: i64,
    /// What one finding counts: a pair, an annotation, a view, a hole.
    pub per: &'static str,
    pub what: &'static str,
    pub why: &'static str,
    pub fix: &'static str,
}

pub const RULES: [Rule; 11] = [
    Rule {
        code: "dwg_view_overlap",
        weight: 10,
        per: "pair of views",
        what: "Two views' boxes, their dimension room included, overlap.",
        why: "A reader can't tell whose lines and dimensions are whose.",
        fix: "Let ryucad place the later view (its at and offset taken out), or run drawing layout.",
    },
    Rule {
        code: "dwg_outside",
        weight: 5,
        per: "view or annotation",
        what: "A view or annotation lies past the drawing area or into the title block.",
        why: "It is cut off when printed, or hides the title block.",
        fix: "Let ryucad place it again (its at and offset taken out), or choose a larger sheet.",
    },
    Rule {
        code: "dwg_text_overlap",
        weight: 3,
        per: "annotation",
        what: "An annotation's text lies over other text, a drawn curve or another dimension's line.",
        why: "Text crossed by lines is misread.",
        fix: "Stand the dimension a gap further out (its offset), or move the text (its at).",
    },
    Rule {
        code: "dwg_dimension_crossing",
        weight: 3,
        per: "pair of dimensions",
        what: "A dimension or extension line crosses another dimension's.",
        why: "Crossed dimension lines are read against the wrong extension lines.",
        fix: "Stand the outer dimension a gap further out, larger dimensions outside smaller ones.",
    },
    Rule {
        code: "dwg_dimension_to_hidden",
        weight: 2,
        per: "dimension",
        what: "A dimension stands on an entity its view draws only as a hidden line.",
        why: "ASME Y14.5 and ISO 129 avoid dimensioning to hidden lines: dimension it where it is seen.",
        fix: "Dimension it in a view that sees it (a section, or another projected view).",
    },
    Rule {
        code: "dwg_duplicate",
        weight: 4,
        per: "dimension",
        what: "One size is dimensioned twice on a sheet.",
        why: "Two dimensions of one size can disagree after an edit; each size is given once.",
        fix: "Remove the later one.",
    },
    Rule {
        code: "dwg_chain_closed",
        weight: 4,
        per: "chain",
        what: "A chain of linear dimensions along one direction closes on itself.",
        why: "A closed chain over-dimensions the part: its tolerances can't all hold.",
        fix: "Make one dimension of the chain a reference dimension ({reference: true}).",
    },
    Rule {
        code: "dwg_untoleranced",
        weight: 1,
        per: "dimension",
        what: "A dimension has no tolerance, and the drawing no general tolerance.",
        why: "A size without a tolerance can't be inspected.",
        fix: "Give the drawing a general_tolerance (ISO 2768-m and the like), or the dimension a tolerance.",
    },
    Rule {
        code: "dwg_datum_missing",
        weight: 5,
        per: "datum letter",
        what: "A feature control frame names a datum no datum feature gives.",
        why: "A tolerance relative to a datum that isn't defined can't be inspected.",
        fix: "Add a datum_feature with that letter on the face the datum is.",
    },
    Rule {
        code: "dwg_hole_uncalled",
        weight: 3,
        per: "hole feature",
        what: "A hole feature the drawing shows has no hole callout and no diameter.",
        why: "Its size, depth and count are nowhere on the drawing.",
        fix: "Add a hole_callout on one of its faces in a view that shows it.",
    },
    Rule {
        code: "dwg_balloon_missing",
        weight: 2,
        per: "part",
        what: "An assembly drawing with a BOM table places a part on no instance of which a balloon stands.",
        why: "A BOM row no balloon points to can't be found on the drawing.",
        fix: "Add a balloon on one of the part's instances in a view that shows it.",
    },
];

pub fn rule(code: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.code == code)
}

/// Whether `code` is one of the lint's rules.
pub fn known(code: &str) -> bool {
    rule(code).is_some()
}

/// The project's preferences (the defaults when it has none, or when the
/// file doesn't read: doctor's `invalid_document`).
pub fn prefs(session: &Session) -> Prefs {
    crate::doc::drawing_lint::read(&session.files(), &known)
        .ok()
        .flatten()
        .unwrap_or_default()
}

pub(super) fn overlap(a: [P; 2], b: [P; 2]) -> bool {
    a[0][0] < b[1][0] && b[0][0] < a[1][0] && a[0][1] < b[1][1] && b[0][1] < a[1][1]
}

pub(super) fn grow(b: [P; 2], d: f64) -> [P; 2] {
    [[b[0][0] - d, b[0][1] - d], [b[1][0] + d, b[1][1] + d]]
}

pub(super) fn inside(b: [P; 2], area: [P; 2]) -> bool {
    b[0][0] >= area[0][0] - 1e-6
        && b[0][1] >= area[0][1] - 1e-6
        && b[1][0] <= area[1][0] + 1e-6
        && b[1][1] <= area[1][1] + 1e-6
}

/// Whether segments `a` and `b` cross (not merely touch at an end).
fn cross(a: [P; 2], b: [P; 2]) -> bool {
    let o = |p: P, q: P, r: P| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    let (d1, d2) = (o(a[0], a[1], b[0]), o(a[0], a[1], b[1]));
    let (d3, d4) = (o(b[0], b[1], a[0]), o(b[0], b[1], a[1]));
    let eps = 1e-9;
    ((d1 > eps && d2 < -eps) || (d1 < -eps && d2 > eps))
        && ((d3 > eps && d4 < -eps) || (d3 < -eps && d4 > eps))
}

/// Whether segment `s` passes through box `b`.
pub(super) fn hits(s: [P; 2], b: [P; 2]) -> bool {
    let inb = |p: P| p[0] > b[0][0] && p[0] < b[1][0] && p[1] > b[0][1] && p[1] < b[1][1];
    if inb(s[0]) || inb(s[1]) {
        return true;
    }
    let c = [b[0], [b[1][0], b[0][1]], b[1], [b[0][0], b[1][1]]];
    (0..4).any(|i| cross(s, [c[i], c[(i + 1) % 4]]))
}

fn bbox(boxes: &[[P; 2]], lines: &[[P; 2]]) -> Option<[P; 2]> {
    let mut out: Option<[P; 2]> = None;
    let mut add = |p: P| {
        out = Some(match out {
            None => [p, p],
            Some([lo, hi]) => [
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            ],
        });
    };
    for b in boxes {
        add(b[0]);
        add(b[1]);
    }
    for l in lines {
        add(l[0]);
        add(l[1]);
    }
    out
}

/// A length in sheet millimetres as a file writes it, to a thousandth.
fn mm(x: f64) -> String {
    let r = (x * 1000.0).round() / 1000.0;
    format!("{} mm", if r == 0.0 { 0.0 } else { r })
}

/// The least move that brings box `b` inside `area` and off every
/// keep-out box: up out of one, or left of it, the shorter that clears
/// them all; none where `b` can't fit.
fn shift_inside(b: [P; 2], area: [P; 2], keep_out: &[[P; 2]]) -> Option<P> {
    let clamp = |lo: f64, hi: f64, alo: f64, ahi: f64| -> Option<f64> {
        if hi - lo > ahi - alo + 1e-9 {
            None
        } else if lo < alo {
            Some(alo - lo)
        } else if hi > ahi {
            Some(ahi - hi)
        } else {
            Some(0.0)
        }
    };
    let base = [
        clamp(b[0][0], b[1][0], area[0][0], area[1][0])?,
        clamp(b[0][1], b[1][1], area[0][1], area[1][1])?,
    ];
    let at = |d: P| {
        [
            [b[0][0] + d[0], b[0][1] + d[1]],
            [b[1][0] + d[0], b[1][1] + d[1]],
        ]
    };
    let ok = |d: P| {
        let m = at(d);
        inside(m, area) && !keep_out.iter().any(|k| overlap(m, *k))
    };
    if ok(base) {
        return Some(base);
    }
    let moved = at(base);
    let mut tries: Vec<P> = Vec::new();
    for k in keep_out.iter().filter(|k| overlap(moved, **k)) {
        tries.push([base[0], base[1] + (k[1][1] - moved[0][1]) + 1.0]);
        tries.push([base[0] + (k[0][0] - moved[1][0]) - 1.0, base[1]]);
    }
    tries
        .into_iter()
        .filter(|d| ok(*d))
        .min_by(|a, b| (a[0].abs() + a[1].abs()).total_cmp(&(b[0].abs() + b[1].abs())))
}

/// Linear dimensions by view and direction: each one's ends (in
/// micrometres of the image) and its key.
type Chains = BTreeMap<(String, String), Vec<(i64, i64, String)>>;

/// Runs the lint over `sheets`, adding its findings; each sheet's score.
pub(crate) fn run(
    session: &mut Session,
    part: &crate::doc::id::Id,
    file: &DrawingFile,
    sheets: &[SheetBuilt],
    findings: &mut Vec<Value>,
) -> BTreeMap<String, i64> {
    let prefs = prefs(session);
    let mut out: Vec<Value> = Vec::new();
    let mut push = |code: &str, sheet: &str, message: String, details: Value, fixes: Vec<Value>| {
        let r = rule(code).expect("a rule");
        let severity = prefs
            .severity
            .get(code)
            .copied()
            .unwrap_or(Severity::Warning);
        if severity == Severity::Off {
            return;
        }
        let points = prefs.weights.get(code).copied().unwrap_or(r.weight);
        out.push(json!({
            "code": code, "severity": severity.as_str(), "sheet": sheet, "lint": true,
            "points": points, "message": message, "details": details, "fixes": fixes,
        }));
    };
    let set = |id: String, fields: Value| json!([{"op": "set", "id": id, "fields": fields}]);
    let called: std::collections::BTreeSet<(String, String)> = sheets
        .iter()
        .flat_map(|s| &s.annotations)
        .filter(|a| a.error.is_none())
        .filter_map(|a| a.hole.as_ref().map(|(p, h)| (p.to_string(), h.to_string())))
        .collect();
    let mut judged: std::collections::BTreeSet<(String, String)> = Default::default();
    for s in sheets {
        let sk = s.key.to_string();
        // Views over each other.
        for (i, a) in s.views.iter().enumerate() {
            for b in &s.views[i + 1..] {
                // As layout judges views apart (#644): it stands each
                // just clear of its neighbour and writes places to a
                // thousandth, so their rooms may meet by that much.
                if super::overlap(grow(a.bounds, DIM_ROOM), grow(b.bounds, DIM_ROOM)) {
                    push(
                        "dwg_view_overlap",
                        &sk,
                        format!(
                            "views {} and {} overlap, their dimension room included",
                            a.key, b.key
                        ),
                        json!({"views": [a.key.to_string(), b.key.to_string()]}),
                        vec![set(b.key.to_string(), json!({"at": null, "offset": null}))],
                    );
                }
            }
        }
        // Past the area, or into the title block.
        let off = |b: [P; 2]| !inside(b, s.area) || s.keep_out.iter().any(|k| overlap(b, *k));
        for v in &s.views {
            if off(v.bounds) {
                push(
                    "dwg_outside",
                    &sk,
                    format!(
                        "view {} lies past the drawing area or into the title block",
                        v.key
                    ),
                    json!({"view": v.key.to_string(), "box_mm": v.bounds}),
                    // Moved inside by as little as will do: taking its
                    // place out would put it back where ryucad put it.
                    // A view along its parent's row moves only along it,
                    // so it has no fix of this kind.
                    match shift_inside(v.bounds, s.area, &s.keep_out) {
                        Some(d) if !v.along => vec![json!([{"op": "move", "id": v.key.to_string(),
                            "at": [mm(v.at[0] + d[0]), mm(v.at[1] + d[1])]}])],
                        _ => Vec::new(),
                    },
                );
            }
        }
        let built: Vec<&super::annotate::Built> =
            s.annotations.iter().filter(|a| a.error.is_none()).collect();
        for a in &built {
            if let Some(b) = bbox(&a.boxes, &a.lines)
                && off(b)
            {
                push(
                    "dwg_outside",
                    &sk,
                    format!(
                        "annotation {} lies past the drawing area or into the title block",
                        a.key
                    ),
                    json!({"annotation": a.key.to_string(), "box_mm": b}),
                    // A dimension stands on the other side of what it
                    // measures; anything else moves inside by as little
                    // as will do. Taking either out would put it back.
                    match (a.offset, shift_inside(b, s.area, &s.keep_out)) {
                        (Some(o), _) if a.kind != "angular" => {
                            vec![set(a.key.to_string(), json!({"offset": mm(-o)}))]
                        }
                        (None, Some(d)) => vec![set(
                            a.key.to_string(),
                            json!({"at": [mm(a.at[0] + d[0]), mm(a.at[1] + d[1])]}),
                        )],
                        _ => Vec::new(),
                    },
                );
            }
        }
        // Text over text, lines or the views' curves.
        let curves: Vec<[P; 2]> = s
            .views
            .iter()
            .flat_map(|v| s.items[v.items.clone()].iter())
            .filter_map(|it| match it {
                Item::Path {
                    path,
                    stroke: Some(_),
                    ..
                } => Some(path.points()),
                _ => None,
            })
            .flat_map(|pts| pts.windows(2).map(|w| [w[0], w[1]]).collect::<Vec<_>>())
            .collect();
        for (i, a) in built.iter().enumerate() {
            let hit = a.boxes.iter().find_map(|tb| {
                let tb = grow(*tb, -0.2);
                for (j, b) in built.iter().enumerate() {
                    if i == j {
                        continue;
                    }
                    if b.boxes.iter().any(|x| overlap(tb, *x)) {
                        return Some(format!("{}'s text", b.key));
                    }
                    if b.lines.iter().any(|l| hits(*l, tb)) {
                        return Some(format!("{}'s lines", b.key));
                    }
                }
                curves
                    .iter()
                    .any(|l| hits(*l, tb))
                    .then(|| "the view's lines".to_owned())
            });
            if let Some(what) = hit {
                let fix = match a.offset {
                    Some(o) if a.kind != "angular" => set(
                        a.key.to_string(),
                        json!({"offset": format!("{} mm", ((o + o.signum() * DIM_GAP) * 1000.0).round() / 1000.0), "at": null}),
                    ),
                    // Text on a leader (or with no offset to stand out by)
                    // moves a gap away from its view's centre: taking its
                    // place out would put it back where it is.
                    _ => {
                        let r = ryu_math::hypot(a.at[0], a.at[1]);
                        let u = if a.view.is_some() && r > 1e-9 {
                            [a.at[0] / r, a.at[1] / r]
                        } else {
                            [0.0, 1.0]
                        };
                        set(
                            a.key.to_string(),
                            json!({"at": [mm(a.at[0] + u[0] * DIM_GAP), mm(a.at[1] + u[1] * DIM_GAP)]}),
                        )
                    }
                };
                push(
                    "dwg_text_overlap",
                    &sk,
                    format!("annotation {}'s text lies over {what}", a.key),
                    json!({"annotation": a.key.to_string(), "over": what}),
                    vec![fix],
                );
            }
        }
        // Dimension lines crossing.
        let dims: Vec<&&super::annotate::Built> =
            built.iter().filter(|a| a.measure.is_some()).collect();
        for (i, a) in dims.iter().enumerate() {
            for b in &dims[i + 1..] {
                if a.lines
                    .iter()
                    .any(|x| b.lines.iter().any(|y| cross(*x, *y)))
                {
                    let outer = if a.offset.unwrap_or(0.0).abs() >= b.offset.unwrap_or(0.0).abs() {
                        a
                    } else {
                        b
                    };
                    let fix = match outer.offset {
                        Some(o) if outer.kind != "angular" => vec![set(
                            outer.key.to_string(),
                            json!({"offset": format!("{} mm", ((o + o.signum() * DIM_GAP) * 1000.0).round() / 1000.0)}),
                        )],
                        _ => Vec::new(),
                    };
                    push(
                        "dwg_dimension_crossing",
                        &sk,
                        format!("dimensions {} and {} cross", a.key, b.key),
                        json!({"annotations": [a.key.to_string(), b.key.to_string()]}),
                        fix,
                    );
                }
            }
        }
        // On hidden lines.
        for a in &dims {
            if !a.hidden.is_empty() {
                push(
                    "dwg_dimension_to_hidden",
                    &sk,
                    format!(
                        "dimension {} stands on hidden lines: {}",
                        a.key,
                        a.hidden.join(", ")
                    ),
                    json!({"annotation": a.key.to_string(), "entities": a.hidden}),
                    Vec::new(),
                );
            }
        }
        // Twice.
        for (i, a) in dims.iter().enumerate() {
            let ma = a.measure.as_ref().expect("a dimension");
            if let Some(b) = dims[..i].iter().find(|b| {
                let mb = b.measure.as_ref().expect("a dimension");
                mb.along == ma.along && mb.refs == ma.refs
            }) {
                push(
                    "dwg_duplicate",
                    &sk,
                    format!("dimension {} measures what {} does", a.key, b.key),
                    json!({"annotation": a.key.to_string(), "as": b.key.to_string()}),
                    vec![json!([{"op": "remove", "id": a.key.to_string()}])],
                );
            }
        }
        // Closed chains: per view and direction, ends joined by
        // dimensions; one that joins two already joined closes a chain.
        let mut groups: Chains = BTreeMap::new();
        for a in &dims {
            let m = a.measure.as_ref().expect("a dimension");
            if m.free || !(m.along == "horizontal" || m.along == "vertical") {
                continue;
            }
            let q = |x: f64| (x * 1e3).round() as i64;
            groups
                .entry((
                    a.view.map(|v| v.to_string()).unwrap_or_default(),
                    m.along.clone(),
                ))
                .or_default()
                .push((q(m.ends[0]), q(m.ends[1]), a.key.to_string()));
        }
        for edges in groups.values() {
            let mut parent: BTreeMap<i64, i64> = BTreeMap::new();
            fn find(p: &mut BTreeMap<i64, i64>, x: i64) -> i64 {
                let up = *p.entry(x).or_insert(x);
                if up == x {
                    return x;
                }
                let r = find(p, up);
                p.insert(x, r);
                r
            }
            for (a, b, key) in edges {
                let (ra, rb) = (find(&mut parent, *a), find(&mut parent, *b));
                if ra == rb {
                    push(
                        "dwg_chain_closed",
                        &sk,
                        format!("dimension {key} closes a chain of dimensions"),
                        json!({"annotation": key}),
                        vec![set(key.clone(), json!({"tolerance": {"reference": true}}))],
                    );
                } else {
                    parent.insert(ra, rb);
                }
            }
        }
        // Untoleranced.
        if file.general_tolerance.is_none() {
            for a in &dims {
                let m = a.measure.as_ref().expect("a dimension");
                if !m.toleranced {
                    push(
                        "dwg_untoleranced",
                        &sk,
                        format!(
                            "dimension {} has no tolerance, and the drawing no general tolerance",
                            a.key
                        ),
                        json!({"annotation": a.key.to_string()}),
                        vec![json!([{"op": "set", "fields": {"general_tolerance": "ISO 2768-m"}}])],
                    );
                }
            }
        }
        // Holes uncalled: by a callout or a diameter of its wall anywhere
        // on the drawing, and each hole judged once, where first shown.
        for (part, hole, view, face) in &s.holes {
            let key = (part.to_string(), hole.to_string());
            if called.contains(&key) || !judged.insert(key) {
                continue;
            }
            push(
                "dwg_hole_uncalled",
                &sk,
                format!(
                    "hole {hole} of {part} is shown in view {view} with no callout or diameter"
                ),
                json!({"part": part, "hole": hole, "view": view.to_string()}),
                vec![json!([{"op": "add", "kind": "annotation",
                    "value": {"type": "hole_callout", "view": view.to_string(), "of": face}}])],
            );
        }
    }
    // Datums named but not given.
    let letters: Vec<&str> = file
        .annotations
        .values()
        .filter_map(|a| match &a.kind {
            Kind::DatumFeature { letter, .. } => Some(letter.as_str()),
            _ => None,
        })
        .collect();
    for (k, a) in &file.annotations {
        if let Kind::FeatureControlFrame { rows, .. } = &a.kind {
            // Each letter once a frame, however many rows name it.
            let mut named: Vec<&str> = Vec::new();
            for r in rows {
                for (l, _) in &r.datums {
                    if !named.contains(&l.as_str()) {
                        named.push(l.as_str());
                    }
                }
            }
            for l in named {
                if letters.contains(&l) {
                    continue;
                }
                let sheet = sheets
                    .iter()
                    .find(|s| s.annotations.iter().any(|b| b.key == *k))
                    .map(|s| s.key.to_string())
                    .unwrap_or_default();
                push(
                    "dwg_datum_missing",
                    &sheet,
                    format!("frame {k} names datum {l}, which no datum feature gives"),
                    json!({"annotation": k.to_string(), "datum": l}),
                    Vec::new(),
                );
            }
        }
    }
    // Balloons missing where a BOM table lists the instances.
    let table = file
        .annotations
        .values()
        .any(|a| matches!(a.kind, Kind::BomTable { .. }));
    if table {
        let ballooned: Vec<&str> = file
            .annotations
            .values()
            .filter_map(|a| match &a.kind {
                Kind::Balloon { of } => Some(of.as_str()),
                _ => None,
            })
            .collect();
        for s in sheets {
            let Some(v) = s.views.iter().find(|v| v.of == *part) else {
                continue;
            };
            let Ok(model) = session.model(&v.of, &v.rev) else {
                continue;
            };
            // Each part the assembly places itself, by the instances that
            // place it: a BOM row wants one balloon, on any instance of
            // its part. A pattern's copy places what its seed does, and a
            // subassembly's occurrences stand for the subassembly.
            let root = session.node(&v.of, &v.rev).ok().map(|n| n.0);
            let mut parts: Vec<(String, Vec<String>)> = Vec::new();
            for o in &model.occurrences {
                let (top, inst) = match o.path.as_slice() {
                    [c, seed, ..] if c.copy_of().is_some() => (format!("{c}/{seed}"), seed),
                    [first, ..] => (first.to_string(), first),
                    [] => continue,
                };
                let placed = root
                    .as_ref()
                    .and_then(|r| match r.features.get(inst) {
                        Some(Ok(crate::doc::files::Feature::Instance(i))) => {
                            Some(crate::doc::files::item_key(&i.part, &i.rev).to_string())
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| o.part.to_string());
                match parts.iter_mut().find(|(p, _)| *p == placed) {
                    Some((_, tops)) => {
                        if !tops.contains(&top) {
                            tops.push(top);
                        }
                    }
                    None => parts.push((placed, vec![top])),
                }
            }
            for (part, firsts) in parts {
                let named = firsts.iter().any(|f| {
                    ballooned
                        .iter()
                        .any(|b| b == f || b.starts_with(&format!("{f}/")))
                });
                if named {
                    continue;
                }
                let f = &firsts[0];
                push(
                    "dwg_balloon_missing",
                    &s.key.to_string(),
                    format!("{part} (instance {f}) has no balloon, though a BOM table lists it"),
                    json!({"part": part, "instance": f, "view": v.key.to_string()}),
                    vec![json!([{"op": "add", "kind": "annotation",
                        "value": {"type": "balloon", "view": v.key.to_string(), "of": f}}])],
                );
            }
            break;
        }
    }
    let mut scores: BTreeMap<String, i64> =
        sheets.iter().map(|s| (s.key.to_string(), 100)).collect();
    for f in &out {
        if let Some(sc) = f["sheet"].as_str().and_then(|k| scores.get_mut(k)) {
            *sc -= f["points"].as_i64().unwrap_or(0);
        }
    }
    for v in scores.values_mut() {
        *v = (*v).max(0);
    }
    findings.extend(out);
    scores
}

/// `drawing.lint --explain`: a rule's catalogue entry and its weight and
/// severity in this project.
pub fn explain(session: &Session, code: &str) -> crate::error::Result<Value> {
    let r = rule(code).ok_or_else(|| {
        crate::error::Error::new(
            crate::error::Code::InvalidParam,
            format!("{code} is no drawing lint rule"),
        )
        .with("param", "explain")
        .with("rules", RULES.iter().map(|r| r.code).collect::<Vec<_>>())
    })?;
    let p = prefs(session);
    Ok(json!({
        "code": r.code, "weight": p.weights.get(code).copied().unwrap_or(r.weight),
        "default_weight": r.weight, "per": r.per,
        "severity": p.severity.get(code).copied().unwrap_or(Severity::Warning).as_str(),
        "what": r.what, "why": r.why, "fix": r.fix,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_cross_and_boxes_are_hit() {
        assert!(cross([[0.0, 0.0], [2.0, 2.0]], [[0.0, 2.0], [2.0, 0.0]]));
        assert!(!cross([[0.0, 0.0], [1.0, 0.0]], [[1.0, 0.0], [1.0, 1.0]]));
        assert!(hits([[-1.0, 0.5], [2.0, 0.5]], [[0.0, 0.0], [1.0, 1.0]]));
        assert!(!hits([[-1.0, 2.0], [2.0, 2.0]], [[0.0, 0.0], [1.0, 1.0]]));
    }

    /// The `dwg_view_overlap` findings of a 40 × 30 × 20 mm box's front
    /// view at (100, 100) and its right view `at` mm right of it, at 1:1.
    fn view_overlaps(right_at: f64) -> usize {
        use crate::doc::id::Id;
        let (dir, mut s) = crate::testing::fresh();
        let call = crate::testing::call;
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 40, "y": 30, "z": 20}}),
        )
        .unwrap();
        let file = json!({"format": 1, "kind": "ryucad.drawing", "name": "Block",
        "sheets": {"s1": {"name": "Sheet 1", "size": "ansi_b", "scale": "1:1"}},
        "views": {
            "v1": {"type": "base", "sheet": "s1", "orient": "front", "at": ["100 mm", "100 mm"]},
            "v2": {"type": "base", "sheet": "s1", "orient": "right",
                   "at": [format!("{right_at} mm"), "100 mm"]},
        }});
        let at = dir.path().join("parts/p/drawings");
        std::fs::create_dir_all(&at).unwrap();
        crate::doc::project::write_json(&at.join("d.json"), &file).unwrap();
        s.reload().unwrap();
        let id = |x: &str| Id::parse(x, "x").unwrap();
        let b = super::super::build(&mut s, &id("p"), &id("d")).unwrap();
        b.findings
            .iter()
            .filter(|f| f["code"] == "dwg_view_overlap")
            .count()
    }

    #[test]
    fn views_meeting_by_a_written_places_rounding_are_apart() {
        // The front reaches 20 mm right of its centre and the right view
        // 15 mm left of its own, each with 12 mm of room: they meet with
        // the right view's centre at 159 mm.
        assert_eq!(view_overlaps(160.0), 0);
        assert_eq!(view_overlaps(159.0), 0);
        // Within a written place's thousandth (#644).
        assert_eq!(view_overlaps(158.9995), 0);
        assert_eq!(view_overlaps(158.99), 1);
        assert_eq!(view_overlaps(150.0), 1);
    }

    #[test]
    fn every_rule_has_its_words_and_a_dwg_code() {
        for r in &RULES {
            assert!(crate::doc::drawing_lint::code_ok(r.code), "{}", r.code);
            assert!(r.weight > 0 && !r.what.is_empty() && !r.why.is_empty() && !r.fix.is_empty());
        }
    }
}
