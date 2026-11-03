//! `sweep.get`: what a sweep or a loft is built from, drawn in model space
//! for the app (design 323, phase 6). A sweep gives its profile as one
//! section and the path it follows; a loft gives each of its sections in
//! order, turned to face along the loft as it built, each loop from its
//! start with where that start is. Polylines are cut to a display
//! quality's chord for the feature's own size, as `path.get` cuts paths
//! ([`crate::render::path_polyline`]).

use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::geometry::At;
use crate::rebuild::Results;
use ryukei::geom::{Frame, Point3};
use ryukei::sweep::{Segment, ValidLoop, ValidProfile};
use serde_json::{Value, json};

/// A model point as JSON.
fn at(p: Point3) -> [f64; 3] {
    [p.x, p.y, p.z]
}

/// The diagonal of the box of `points`, at least resolution: the size a
/// display quality's chord is a fraction of.
fn span(points: impl Iterator<Item = [f64; 3]>) -> f64 {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let d = [0, 1, 2].map(|k| (hi[k] - lo[k]).max(0.0));
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2])
        .sqrt()
        .max(ryukei::tol::LINEAR_RESOLUTION)
}

/// One segment's points in model space after its start (its start is the
/// previous segment's end), the curve halved until each piece bows less
/// than `chord` from its chord and turns less than `angle`, at most 16
/// times; a line is its end alone.
fn segment_points(frame: &Frame, g: &Segment, chord: f64, angle: f64, out: &mut Vec<[f64; 3]>) {
    let lift = |t: f64| {
        let p = g.curve.point(t);
        frame.to_world(p.x, p.y, 0.0)
    };
    let first = if matches!(g.curve, ryukei::geom::Curve2::Line2(_)) {
        1
    } else {
        8
    };
    let (t0, t1) = (g.t0, g.t1);
    for i in 0..first {
        let mut stack = vec![(
            t0 + (t1 - t0) * f64::from(i) / f64::from(first),
            t0 + (t1 - t0) * f64::from(i + 1) / f64::from(first),
            0u32,
        )];
        while let Some((u, w, depth)) = stack.pop() {
            let m = 0.5 * (u + w);
            let (pu, pw, pm) = (lift(u), lift(w), lift(m));
            let bow = pm.distance(pu + (pw - pu) * 0.5);
            let turn = match (g.curve.tangent(u), g.curve.tangent(w)) {
                (Some(x), Some(y)) => {
                    ryukei::math::atan2(x.x * y.y - x.y * y.x, x.x * y.x + x.y * y.y).abs()
                }
                _ => 0.0,
            };
            if depth < 16 && (bow > chord || turn > angle) {
                stack.push((m, w, depth + 1));
                stack.push((u, m, depth + 1));
            } else {
                out.push(at(pw));
            }
        }
    }
}

/// A loop's polyline in model space, from its first segment's start back
/// to it.
fn loop_polyline(frame: &Frame, l: &ValidLoop, chord: f64, angle: f64) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    if let Some(v) = l.vertices.first() {
        out.push(at(frame.to_world(v.x, v.y, 0.0)));
    }
    for g in &l.segments {
        segment_points(frame, g, chord, angle, &mut out);
    }
    out
}

/// Every loop of a profile: each lump's outer loop, then its holes.
fn loops_of(v: &ValidProfile) -> impl Iterator<Item = &ValidLoop> {
    v.lumps()
        .iter()
        .flat_map(|l| std::iter::once(&l.outer).chain(&l.holes))
}

/// Where a profile's loops' vertices are, for its size.
fn vertices(v: &ValidProfile) -> Vec<[f64; 3]> {
    let f = v.frame();
    loops_of(v)
        .flat_map(|l| l.vertices.iter().map(|p| at(f.to_world(p.x, p.y, 0.0))))
        .collect()
}

/// `sweep.get` of `feature` in `part` at `quality`: `{feature, type,
/// revision, quality, sections: [{index, source, turned, centroid_m,
/// loops: [{anchor, start?, start_m?, polyline}]}], path?: {closed,
/// polyline}}`. A feature that failed answers its own error; one that
/// isn't a sweep or a loft is `reference_wrong_kind`.
pub fn get(
    session: &mut crate::session::Session,
    part: &str,
    feature: &str,
    quality: crate::mesh::Quality,
) -> Result<Value> {
    let (pid, fid) = (
        crate::server::part_id(part)?,
        crate::server::feature_id(feature)?,
    );
    let p: Part = session.project().part(&pid)?.clone();
    let revision = p.revision.clone();
    let file = match p.features.get(&fid) {
        Some(Ok(f)) => f.clone(),
        Some(Err(e)) => return Err(e.clone()),
        None => {
            return Err(
                Error::new(Code::NotFound, format!("{pid} has no feature {fid}"))
                    .with("part", &pid)
                    .with("feature", &fid),
            );
        }
    };
    let built = session.rebuilt(&pid)?;
    if let Some((_, Err(e))) = built.features.iter().find(|(id, _)| *id == fid) {
        return Err(e.clone());
    }
    let mut v = match &file {
        Feature::Sweep(w) => sweep(&p, &fid, w, built, quality)?,
        Feature::Loft(l) => loft(&p, &fid, l, built, quality)?,
        _ => {
            return Err(Error::new(
                Code::ReferenceWrongKind,
                format!("{pid}/{fid} is not a sweep or a loft"),
            )
            .with("part", &pid)
            .with("feature", &fid));
        }
    };
    v["feature"] = json!(fid);
    v["revision"] = json!(revision);
    v["quality"] = json!(quality.as_str());
    Ok(v)
}

/// A sweep's profile as section 0 and its path's polyline.
fn sweep(
    part: &Part,
    id: &Id,
    w: &crate::doc::sweep::Sweep,
    results: &dyn Results,
    quality: crate::mesh::Quality,
) -> Result<Value> {
    let profile = crate::along::profile(part, id, &w.profile, results)?;
    let at_ = At {
        part,
        order: part.order(),
        owner: id,
        results,
    };
    let path = crate::path::of_curve(&at_, "path", &w.path)?;
    let chain = path.chain();
    let size = span(vertices(&profile.valid).into_iter()).max(crate::render::chain_span(&chain));
    let params = quality.params(size);
    let frame = profile.valid.frame();
    let loops: Vec<Value> = loops_of(&profile.valid)
        .map(|l| {
            json!({
                "anchor": profile.keys.loops[&l.key.0],
                "polyline": loop_polyline(frame, l, params.chord, params.angle),
            })
        })
        .collect();
    let mut source = profile.source.clone();
    if let Some(g) = source.get("face").cloned()
        && let Ok(g) = crate::doc::geometry::GeomRef::from_json(&g, "/face")
    {
        source["face"] = json!(g.text(&part.id));
    }
    Ok(json!({
        "type": "sweep",
        "sections": [{
            "index": 0,
            "source": source,
            "turned": false,
            "centroid_m": at(frame.origin()),
            "loops": loops,
        }],
        "path": {
            "closed": path.closed,
            "polyline": crate::render::path_polyline(&chain, params.chord, params.angle),
        },
    }))
}

/// A loft's sections in order, each as it built: turned or not, each loop
/// from its start, with where that start is.
fn loft(
    part: &Part,
    id: &Id,
    l: &crate::doc::sweep::Loft,
    results: &dyn Results,
    quality: crate::mesh::Quality,
) -> Result<Value> {
    let s = crate::loft::sections(part, id, l, results)?;
    let size = span(s.iter().flat_map(|r| vertices(&r.valid)));
    let params = quality.params(size);
    let sections: Vec<Value> = s
        .iter()
        .enumerate()
        .map(|(k, r)| {
            let frame = r.valid.frame();
            let loops: Vec<Value> = r
                .kernel_loops(&s[0])
                .into_iter()
                .zip(&r.loops)
                .map(|(kl, sl)| {
                    json!({
                        "anchor": sl.anchor,
                        "start": sl.start,
                        "start_m": at(sl.start_at()),
                        "polyline": loop_polyline(frame, kl, params.chord, params.angle),
                    })
                })
                .collect();
            let mut source = r.source.clone();
            if let Some(g) = source.get("face").cloned()
                && let Ok(g) = crate::doc::geometry::GeomRef::from_json(&g, "/face")
            {
                source["face"] = json!(g.text(&part.id));
            }
            json!({
                "index": k,
                "source": source,
                "turned": r.turned,
                "centroid_m": at(r.centroid()),
                "loops": loops,
            })
        })
        .collect();
    Ok(json!({"type": "loft", "sections": sections, "closed": l.closed}))
}

#[cfg(test)]
mod tests;
