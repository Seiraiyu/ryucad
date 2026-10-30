//! A check of a `BrepArray` prim against the omniSolid schema, written
//! from the schema alone (its equations, its packing rules) and usd-brep's
//! validator's rule names, so the writer is checked by something other
//! than itself: sizes, faceuse pairs, radial cycles, loops that chain, and
//! every curve where the schema's equations put it.

use ryuformats::usd::layer::{Prim, Value};
use ryukei::math;
use std::collections::BTreeMap;

/// What a sound BrepArray holds, for tests to compare.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub regions: Vec<String>,
    pub shells: usize,
    pub faces: usize,
    pub loops: usize,
    pub edgeuses: usize,
    pub edges: usize,
    pub vertices: usize,
    pub surfaces: BTreeMap<String, usize>,
    pub curves: BTreeMap<String, usize>,
    pub rectangular: usize,
    pub tolerance: f64,
    /// Loops whose edgeuses have no UV curves ((0, 0) records): loops
    /// through a pole.
    pub uvless: usize,
}

struct A<'a>(BTreeMap<&'a str, &'a Value>);

impl A<'_> {
    fn get(&self, name: &str) -> &Value {
        self.0
            .get(name)
            .unwrap_or_else(|| panic!("no attribute {name}"))
    }
    fn uints(&self, name: &str) -> Vec<usize> {
        match self.get(name) {
            Value::UIntArray(v) => v.iter().map(|&x| x as usize).collect(),
            v => panic!("{name} is {}", v.type_name()),
        }
    }
    fn doubles(&self, name: &str) -> Vec<f64> {
        match self.get(name) {
            Value::DoubleArray(v) => v.clone(),
            v => panic!("{name} is {}", v.type_name()),
        }
    }
    fn tokens(&self, name: &str) -> Vec<String> {
        match self.get(name) {
            Value::TokenArray(v) | Value::StringArray(v) => v.clone(),
            v => panic!("{name} is {}", v.type_name()),
        }
    }
    fn d2(&self, name: &str) -> Vec<[f64; 2]> {
        match self.get(name) {
            Value::Double2Array(v) => v.clone(),
            v => panic!("{name} is {}", v.type_name()),
        }
    }
    fn d3(&self, name: &str) -> Vec<[f64; 3]> {
        match self.get(name) {
            Value::Double3Array(v) | Value::Point3dArray(v) | Value::Vector3dArray(v) => v.clone(),
            v => panic!("{name} is {}", v.type_name()),
        }
    }
    fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
}

type P3 = [f64; 3];

/// A UV curve: control points, weights, knots.
type Uv = (Vec<[f64; 2]>, Vec<f64>, Vec<f64>);

fn add(a: P3, b: P3) -> P3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: P3, k: f64) -> P3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn cross(a: P3, b: P3) -> P3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dist(a: P3, b: P3) -> f64 {
    math::sqrt((0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum())
}

/// A NURBS (control points of any dimension as `Vec<f64>`) at `t`, by
/// Cox–de Boor's basis functions.
fn basis(knots: &[f64], p: usize, n: usize, t: f64) -> Vec<f64> {
    let (lo, hi) = (knots[p], knots[n]);
    let t = t.clamp(lo, hi);
    // The span holding t; at the end, the last non-empty one.
    let mut span = p;
    for i in p..n {
        if knots[i] <= t && t < knots[i + 1] {
            span = i;
        }
        if t == hi && knots[i] < knots[i + 1] {
            span = i;
        }
    }
    let mut b = vec![0.0; n + p + 1];
    b[span] = 1.0;
    for d in 1..=p {
        let mut next = vec![0.0; n + p + 1];
        for i in 0..n + p - d {
            let mut v = 0.0;
            let l = knots[i + d] - knots[i];
            if l > 0.0 {
                v += (t - knots[i]) / l * b[i];
            }
            let r = knots[i + d + 1] - knots[i + 1];
            if r > 0.0 {
                v += (knots[i + d + 1] - t) / r * b[i + 1];
            }
            next[i] = v;
        }
        b = next;
    }
    b.truncate(n);
    b
}

/// A rational curve's point.
fn nurbs_point<const D: usize>(cv: &[[f64; D]], w: &[f64], knots: &[f64], t: f64) -> [f64; D] {
    let n = cv.len();
    let p = knots.len() - n - 1;
    let b = basis(knots, p, n, t);
    let mut num = [0.0; D];
    let mut den = 0.0;
    for i in 0..n {
        for k in 0..D {
            num[k] += b[i] * w[i] * cv[i][k];
        }
        den += b[i] * w[i];
    }
    num.map(|x| x / den)
}

/// A rational surface's point: control points row-major, U as rows.
fn surface_point(
    cv: &[P3],
    w: &[f64],
    [nu, nv]: [usize; 2],
    [ku, kv]: [&[f64]; 2],
    u: f64,
    v: f64,
) -> P3 {
    let bu = basis(ku, ku.len() - nu - 1, nu, u);
    let bv = basis(kv, kv.len() - nv - 1, nv, v);
    let mut num = [0.0; 3];
    let mut den = 0.0;
    for i in 0..nu {
        for j in 0..nv {
            let c = bu[i] * bv[j] * w[i * nv + j];
            num = add(num, mul(cv[i * nv + j], c));
            den += c;
        }
    }
    mul(num, 1.0 / den)
}

/// A face's or an edge's geometry, as the schema's equations give it.
enum Shape3 {
    Plane(P3, P3, P3),
    Cylinder(P3, P3, P3, f64),
    Cone(P3, P3, P3, f64, f64),
    Sphere(P3, P3, P3, f64),
    Torus(P3, P3, P3, f64, f64),
    Nurbs(Vec<P3>, Vec<f64>, [usize; 2], [Vec<f64>; 2]),
}

impl Shape3 {
    fn point(&self, u: f64, v: f64) -> P3 {
        let radial = |z: P3, x: P3, a: f64| {
            let y = cross(z, x);
            add(mul(x, math::cos(a)), mul(y, math::sin(a)))
        };
        match self {
            Shape3::Plane(o, z, x) => add(add(*o, mul(*x, u)), mul(cross(*z, *x), v)),
            Shape3::Cylinder(o, z, x, r) => add(add(*o, mul(radial(*z, *x, u), *r)), mul(*z, v)),
            Shape3::Cone(o, z, x, r, a) => {
                let rr = r + v * math::tan(*a);
                add(add(*o, mul(radial(*z, *x, u), rr)), mul(*z, v))
            }
            Shape3::Sphere(c, z, x, r) => add(
                *c,
                mul(
                    add(mul(radial(*z, *x, u), math::cos(v)), mul(*z, math::sin(v))),
                    *r,
                ),
            ),
            Shape3::Torus(o, z, x, big, r) => add(
                add(*o, mul(radial(*z, *x, u), big + r * math::cos(v))),
                mul(*z, r * math::sin(v)),
            ),
            Shape3::Nurbs(cv, w, n, [ku, kv]) => surface_point(cv, w, *n, [ku, kv], u, v),
        }
    }
}

enum Curve3 {
    Line(P3, P3),
    Circle(P3, P3, P3, f64, f64),
    Nurbs(Vec<P3>, Vec<f64>, Vec<f64>),
}

impl Curve3 {
    fn point(&self, t: f64) -> P3 {
        match self {
            Curve3::Line(o, d) => add(*o, mul(*d, t)),
            Curve3::Circle(c, z, x, a, b) => add(
                add(*c, mul(*x, a * math::cos(t))),
                mul(cross(*z, *x), b * math::sin(t)),
            ),
            Curve3::Nurbs(cv, w, k) => nurbs_point(cv, w, k, t),
        }
    }
}

/// Checks `p` and gives what it holds; panics naming the first rule it
/// breaks.
pub fn check(p: &Prim) -> Summary {
    assert_eq!(p.type_name.as_deref(), Some("BrepArray"));
    let a = A(p
        .attributes
        .iter()
        .map(|x| (x.name.as_str(), &x.value))
        .collect());
    for x in &p.attributes {
        assert!(x.uniform || x.name == "extent", "{} isn't uniform", x.name);
    }
    let apis = &p.meta.api_schemas;
    let tol = a.doubles("brep:intersectTol3d");
    assert_eq!(tol.len(), 1, "one brep");
    let tol = tol[0];
    assert!(tol > 0.0);
    // Sizes (the validator's BA_065 to BA_325).
    let regions = a.uints("brep:regionCount");
    assert_eq!(regions.len(), 1);
    let rtypes = a.tokens("region:type");
    assert_eq!(rtypes.len(), regions[0]);
    assert_eq!(
        rtypes[0], "voidRegion",
        "the first region is the infinite void"
    );
    let shell_counts = a.uints("region:shellCount");
    assert_eq!(shell_counts.len(), regions[0]);
    let shells: usize = shell_counts.iter().sum();
    let fu_counts = a.uints("shell:faceuseCount");
    assert_eq!(fu_counts.len(), shells);
    assert_eq!(a.uints("shell:wireEdgeCount"), vec![0; shells]);
    assert_eq!(a.tokens("shell:pointType"), vec!["none"; shells]);
    let fu: usize = fu_counts.iter().sum();
    let fu_faces = a.uints("faceuse:faceIndex");
    let fu_types = a.tokens("faceuse:orientationType");
    assert_eq!((fu_faces.len(), fu_types.len()), (fu, fu));
    assert_eq!(fu % 2, 0);
    let faces = fu / 2;
    let loop_counts = a.uints("face:loopCount");
    let stypes = a.tokens("face:surfaceType");
    let trims = a.tokens("face:trimType");
    let ranges = a.d2("face:range");
    assert_eq!(
        (loop_counts.len(), stypes.len(), trims.len(), ranges.len()),
        (faces, faces, faces, 2 * faces)
    );
    let loops: usize = loop_counts.iter().sum();
    let eu_counts = a.uints("loop:edgeuseCount");
    let loop_vertices = a.uints("loop:vertexIndex");
    assert_eq!((eu_counts.len(), loop_vertices.len()), (loops, loops));
    let eus: usize = eu_counts.iter().sum();
    let eu_edges = a.uints("edgeuse:edgeIndex");
    let eu_types = a.tokens("edgeuse:orientationType");
    let eu_next = a.uints("edgeuse:nextRadialEUIndex");
    let eu_entries = a.tokens("edgeuse:thisRadialEntryType");
    for n in [
        eu_edges.len(),
        eu_types.len(),
        eu_next.len(),
        eu_entries.len(),
    ] {
        assert_eq!(n, eus);
    }
    let ctypes = a.tokens("edge:curveType");
    let edges = ctypes.len();
    let eranges = a.doubles("edge:range");
    assert_eq!(eranges.len(), 2 * edges);
    let evs = match a.get("edge:vertexIndices") {
        Value::Int2Array(v) => v.clone(),
        _ => panic!("edge:vertexIndices"),
    };
    assert_eq!(evs.len(), edges);
    assert!(a.tokens("wireEdge:curveType").is_empty());
    let vertices = a.tokens("vertex:pointType").len();
    let points = if vertices > 0 {
        a.d3("brep:vertexPoint:point:position")
    } else {
        Vec::new()
    };
    assert_eq!(points.len(), vertices);
    for name in ["ryucad:faceName", "ryucad:edgeName", "ryucad:vertexName"] {
        let n = a.tokens(name).len();
        let want = match name {
            "ryucad:faceName" => faces,
            "ryucad:edgeName" => edges,
            _ => vertices,
        };
        assert_eq!(n, want, "{name}");
    }
    // Faceuse pairs (BA_580): each face twice, once each way, one in a
    // solid region and one in a void.
    let mut shell_region = Vec::new();
    for (r, &n) in shell_counts.iter().enumerate() {
        shell_region.extend(std::iter::repeat_n(r, n));
    }
    let mut fu_region = Vec::new();
    for (s, &n) in fu_counts.iter().enumerate() {
        fu_region.extend(std::iter::repeat_n(shell_region[s], n));
    }
    for f in 0..faces {
        let uses: Vec<usize> = (0..fu).filter(|&i| fu_faces[i] == f).collect();
        assert_eq!(uses.len(), 2, "face {f}'s faceuses");
        let kinds: Vec<&str> = uses.iter().map(|&i| fu_types[i].as_str()).collect();
        assert!(kinds.contains(&"same") && kinds.contains(&"opposite"));
        let sides: Vec<&str> = uses
            .iter()
            .map(|&i| rtypes[fu_region[i]].as_str())
            .collect();
        assert!(sides.contains(&"solidRegion") && sides.contains(&"voidRegion"));
    }
    // Radial cycles (BA_581, BA_582): every edge used, each cycle of
    // uses of one edge, entering from the bottom exactly when `same`.
    for (e, _) in ctypes.iter().enumerate() {
        assert!(eu_edges.contains(&e), "edge {e} has no edgeuse");
    }
    for u in 0..eus {
        assert!(eu_edges[u] < edges);
        assert_eq!(
            eu_entries[u] == "bottomEntry",
            eu_types[u] == "same",
            "edgeuse {u}"
        );
        let mut x = eu_next[u];
        let mut steps = 1;
        while x != u {
            assert_eq!(eu_edges[x], eu_edges[u], "a cycle leaves its edge");
            x = eu_next[x];
            steps += 1;
            assert!(steps <= eus);
        }
        let all = eu_edges.iter().filter(|&&e| e == eu_edges[u]).count();
        assert_eq!(steps, all, "edgeuse {u}'s cycle misses uses");
    }
    // Geometry, per kind in face and edge order.
    let mut surfaces = BTreeMap::new();
    let mut shapes = Vec::new();
    let mut nurbs_at = (0, 0, 0);
    for t in &stypes {
        let k = *surfaces.get(t.as_str()).unwrap_or(&0);
        *surfaces.entry(t.clone()).or_insert(0) += 1;
        assert!(apis.contains(t), "{t} isn't applied");
        let get = |kind: &str, attr: &str| a.d3(&format!("brep:surface:{kind}:{attr}"))[k];
        let num = |kind: &str, attr: &str| a.doubles(&format!("brep:surface:{kind}:{attr}"))[k];
        shapes.push(match t.as_str() {
            "BrepSurfacePlaneAPI" => Shape3::Plane(
                get("plane", "origin"),
                get("plane", "axis"),
                get("plane", "refDirection"),
            ),
            "BrepSurfaceCylinderAPI" => Shape3::Cylinder(
                get("cylinder", "origin"),
                get("cylinder", "axis"),
                get("cylinder", "refDirection"),
                num("cylinder", "radius"),
            ),
            "BrepSurfaceConeAPI" => Shape3::Cone(
                get("cone", "origin"),
                get("cone", "axis"),
                get("cone", "refDirection"),
                num("cone", "radius"),
                num("cone", "semiAngle"),
            ),
            "BrepSurfaceSphereAPI" => Shape3::Sphere(
                get("sphere", "center"),
                get("sphere", "axis"),
                get("sphere", "refDirection"),
                num("sphere", "radius"),
            ),
            "BrepSurfaceTorusAPI" => Shape3::Torus(
                get("torus", "origin"),
                get("torus", "axis"),
                get("torus", "refDirection"),
                num("torus", "majorRadius"),
                num("torus", "minorRadius"),
            ),
            "BrepSurfaceNurbAPI" => {
                let at = "brep:surface:nurb";
                let nu = a.uints(&format!("{at}:uVertexCount"))[k];
                let nv = a.uints(&format!("{at}:vVertexCount"))[k];
                let ou = a.uints(&format!("{at}:uOrder"))[k];
                let ov = a.uints(&format!("{at}:vOrder"))[k];
                let (c0, ku0, kv0) = nurbs_at;
                let cv = a.d3(&format!("{at}:controlVertices"))[c0..c0 + nu * nv].to_vec();
                let w = a.doubles(&format!("{at}:weights"))[c0..c0 + nu * nv].to_vec();
                let ku = a.doubles(&format!("{at}:uKnots"))[ku0..ku0 + nu + ou].to_vec();
                let kv = a.doubles(&format!("{at}:vKnots"))[kv0..kv0 + nv + ov].to_vec();
                assert!(ku.windows(2).all(|x| x[0] <= x[1]));
                assert!(kv.windows(2).all(|x| x[0] <= x[1]));
                assert!(w.iter().all(|&x| x > 0.0));
                nurbs_at = (c0 + nu * nv, ku0 + nu + ou, kv0 + nv + ov);
                Shape3::Nurbs(cv, w, [nu, nv], [ku, kv])
            }
            t => panic!("surface type {t}"),
        });
    }
    let mut curves = BTreeMap::new();
    let mut lines = Vec::new();
    let mut c_at = 0;
    for t in &ctypes {
        let k = *curves.get(t.as_str()).unwrap_or(&0);
        *curves.entry(t.clone()).or_insert(0) += 1;
        let inst = match t.as_str() {
            "BrepCurve3dLineAPI" => "edge3dLine",
            "BrepCurve3dCircleAPI" => "edge3dCircle",
            "BrepCurve3dEllipseAPI" => "edge3dEllipse",
            "BrepCurve3dNurbAPI" => "edge3dNurb",
            t => panic!("curve type {t}"),
        };
        assert!(apis.contains(&format!("{t}:{inst}")), "{t} isn't applied");
        let at = |kind: &str, attr: &str| format!("brep:{inst}:curve3d:{kind}:{attr}");
        lines.push(match t.as_str() {
            "BrepCurve3dLineAPI" => {
                let d = a.d3(&at("line", "direction"))[k];
                assert!((dist(d, [0.0; 3]) - 1.0).abs() < 1e-12, "a unit direction");
                Curve3::Line(a.d3(&at("line", "origin"))[k], d)
            }
            "BrepCurve3dCircleAPI" => {
                let r = a.doubles(&at("circle", "radius"))[k];
                Curve3::Circle(
                    a.d3(&at("circle", "center"))[k],
                    a.d3(&at("circle", "axis"))[k],
                    a.d3(&at("circle", "refDirection"))[k],
                    r,
                    r,
                )
            }
            "BrepCurve3dEllipseAPI" => Curve3::Circle(
                a.d3(&at("ellipse", "center"))[k],
                a.d3(&at("ellipse", "axis"))[k],
                a.d3(&at("ellipse", "refDirection"))[k],
                a.doubles(&at("ellipse", "xRadius"))[k],
                a.doubles(&at("ellipse", "yRadius"))[k],
            ),
            _ => {
                let n = a.uints(&at("nurb", "vertexCount"))[k];
                let o = a.uints(&at("nurb", "order"))[k];
                let knots_at: usize = (0..k)
                    .map(|i| {
                        a.uints(&at("nurb", "vertexCount"))[i] + a.uints(&at("nurb", "order"))[i]
                    })
                    .sum();
                let cv = a.d3(&at("nurb", "controlVertices"))[c_at..c_at + n].to_vec();
                let w = a.doubles(&at("nurb", "weights"))[c_at..c_at + n].to_vec();
                let kn = a.doubles(&at("nurb", "knots"))[knots_at..knots_at + n + o].to_vec();
                c_at += n;
                Curve3::Nurbs(cv, w, kn)
            }
        });
    }
    // Edges end at their vertices (within the tolerance).
    for e in 0..edges {
        let [s, t] = evs[e];
        let [t0, t1] = [eranges[2 * e], eranges[2 * e + 1]];
        assert!(t0 < t1, "edge {e}'s range");
        assert!(
            dist(lines[e].point(t0), points[s as usize]) <= tol,
            "edge {e} starts off its vertex"
        );
        assert!(
            dist(lines[e].point(t1), points[t as usize]) <= tol,
            "edge {e} ends off its vertex"
        );
    }
    // Faces' loops: they chain at vertices and in the parameter plane (a
    // gap only along a pole), the outer one counterclockwise, every UV
    // curve on its surface along its edge, within the range.
    let has_uv = a.has("brep:curveUv:nurb:order");
    assert_eq!(has_uv, apis.contains(&"BrepCurveUvNurbAPI".to_owned()));
    let get_u = |n: &str| if has_uv { a.uints(n) } else { vec![] };
    let uv_counts = get_u("brep:curveUv:nurb:vertexCount");
    let uv_orders = get_u("brep:curveUv:nurb:order");
    let uv_cv = if has_uv {
        a.d2("brep:curveUv:nurb:controlVertices")
    } else {
        vec![]
    };
    let uv_w = if has_uv {
        a.doubles("brep:curveUv:nurb:weights")
    } else {
        vec![]
    };
    let uv_k = if has_uv {
        a.doubles("brep:curveUv:nurb:knots")
    } else {
        vec![]
    };
    assert_eq!(uv_counts.len(), if has_uv { eus } else { 0 }, "BA.375");
    // Each edgeuse's UV curve, or none for a (0, 0) record (BA.380).
    let mut uv_at = (0, 0);
    let mut uvs: Vec<Option<Uv>> = Vec::new();
    for u in 0..uv_counts.len() {
        let (n, o) = (uv_counts[u], uv_orders[u]);
        if (n, o) == (0, 0) {
            uvs.push(None);
            continue;
        }
        assert!(o >= 2 && o <= n, "UV curve {u}'s order (BA.590, BA.591)");
        let cv = uv_cv[uv_at.0..uv_at.0 + n].to_vec();
        let w = uv_w[uv_at.0..uv_at.0 + n].to_vec();
        let k = uv_k[uv_at.1..uv_at.1 + n + o].to_vec();
        assert!(
            k.windows(2).all(|x| x[0] <= x[1]),
            "UV curve {u}'s knots (BA.400)"
        );
        assert!(
            w.iter().all(|&x| x > 0.0),
            "UV curve {u}'s weights (BA.410)"
        );
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for q in &cv {
            for j in 0..2 {
                lo[j] = lo[j].min(q[j]);
                hi[j] = hi[j].max(q[j]);
            }
        }
        let diagonal =
            ((hi[0] - lo[0]) * (hi[0] - lo[0]) + (hi[1] - lo[1]) * (hi[1] - lo[1])).sqrt();
        assert!(diagonal > 1e-12, "UV curve {u} has no length (BA.764)");
        uv_at = (uv_at.0 + n, uv_at.1 + n + o);
        uvs.push(Some((cv, w, k)));
    }
    let uv = |u: usize, s: f64| {
        let (cv, w, k) = uvs[u].as_ref().expect("a UV curve");
        nurbs_point(cv, w, k, s)
    };
    let uv_domain = |u: usize| {
        let (cv, _, k) = uvs[u].as_ref().expect("a UV curve");
        let p = k.len() - cv.len() - 1;
        [k[p], k[cv.len()]]
    };
    let mut rectangular = 0;
    let mut uvless = 0;
    let (mut l, mut eu) = (0, 0);
    for f in 0..faces {
        let sh = &shapes[f];
        let [lo, hi] = [ranges[2 * f], ranges[2 * f + 1]];
        assert!(
            lo[0] < hi[0] && lo[1] < hi[1],
            "face {f}'s range (BA.155, BA.160)"
        );
        if trims[f] == "rectangular" {
            rectangular += 1;
        }
        for i in 0..loop_counts[f] {
            let n = eu_counts[l];
            if n == 0 {
                assert!(loop_vertices[l] < vertices, "a lone vertex loop (BA.175)");
                l += 1;
                continue;
            }
            let uses: Vec<usize> = (eu..eu + n).collect();
            // Chained at vertices.
            let ends = |u: usize| {
                let [s, t] = evs[eu_edges[u]].map(|x| x as usize);
                if eu_types[u] == "same" {
                    [s, t]
                } else {
                    [t, s]
                }
            };
            for (j, &u) in uses.iter().enumerate() {
                let next = uses[(j + 1) % n];
                assert_eq!(
                    ends(u)[1],
                    ends(next)[0],
                    "face {f} loop {i} breaks at use {j}"
                );
            }
            let authored = uses.iter().filter(|&&u| has_uv && uvs[u].is_some()).count();
            assert!(
                authored == 0 || authored == n,
                "face {f} loop {i}: some UV curves, not all"
            );
            if authored == 0 {
                uvless += 1;
            } else {
                let mut area = 0.0;
                for (j, &u) in uses.iter().enumerate() {
                    let [d0, d1] = uv_domain(u);
                    let e = eu_edges[u];
                    let [t0, t1] = [eranges[2 * e], eranges[2 * e + 1]];
                    // On its edge's range: SMLib maps one onto the other.
                    let slack = 1e-12 * (t0.abs() + t1.abs() + (t1 - t0));
                    assert!(
                        (d0 - t0).abs() <= slack && (d1 - t1).abs() <= slack,
                        "UV curve {u} over [{d0}, {d1}], its edge over [{t0}, {t1}]"
                    );
                    // From its edgeuse's first vertex to its last.
                    let [vs, ve] = ends(u);
                    let (a0, a1) = (uv(u, d0), uv(u, d1));
                    for (q, v, which) in [(a0, vs, "starts"), (a1, ve, "ends")] {
                        let off = dist(sh.point(q[0], q[1]), points[v]);
                        assert!(
                            off <= 2.0 * tol,
                            "face {f}: UV curve {u} {which} off its vertex by {off}"
                        );
                    }
                    // Head to tail in the plane (BA.763: within 1e-6).
                    let next = uv(uses[(j + 1) % n], uv_domain(uses[(j + 1) % n])[0]);
                    let gap = ((a1[0] - next[0]) * (a1[0] - next[0])
                        + (a1[1] - next[1]) * (a1[1] - next[1]))
                        .sqrt();
                    assert!(
                        gap <= 1e-6,
                        "face {f}: UV curves {gap} apart at {a1:?} {next:?}"
                    );
                    // Control points near the range (BA.750).
                    for q in &uvs[u].as_ref().unwrap().0 {
                        for k in 0..2 {
                            let m = 0.5 * (hi[k] - lo[k]).abs().max(1.0);
                            assert!(
                                q[k] >= lo[k] - m && q[k] <= hi[k] + m,
                                "UV curve {u}'s control point {q:?} (BA.750)"
                            );
                        }
                    }
                    // On its surface along its edge, inside the range.
                    let along: Vec<P3> = (0..=400)
                        .map(|i| lines[e].point(t0 + (t1 - t0) * i as f64 / 400.0))
                        .collect();
                    let step = along
                        .windows(2)
                        .map(|w| dist(w[0], w[1]))
                        .fold(0.0, f64::max);
                    let mut prev = a0;
                    for m in 1..=16 {
                        let q = uv(u, d0 + (d1 - d0) * m as f64 / 16.0);
                        for k in 0..2 {
                            let slack = 1e-9 * (1.0 + hi[k].abs() + lo[k].abs());
                            assert!(
                                q[k] >= lo[k] - slack && q[k] <= hi[k] + slack,
                                "face {f}: UV curve {u} leaves the range: {q:?} {lo:?} {hi:?}"
                            );
                        }
                        let on = sh.point(q[0], q[1]);
                        let near = along
                            .iter()
                            .map(|&c| dist(c, on))
                            .fold(f64::INFINITY, f64::min);
                        assert!(
                            near <= 2.0 * tol + step,
                            "face {f}: UV curve {u} off its edge by {near}"
                        );
                        area += prev[0] * q[1] - q[0] * prev[1];
                        prev = q;
                    }
                    area += a1[0] * next[1] - next[0] * a1[1];
                }
                if i == 0 {
                    assert!(area > 0.0, "face {f}'s outer loop runs clockwise: {area}");
                } else {
                    assert!(area < 0.0, "face {f}'s hole {i} runs counterclockwise");
                }
            }
            l += 1;
            eu += n;
        }
    }
    assert_eq!((l, eu), (loops, eus));
    // Analytic ranges (BA.560 to BA.565, BA.570, BA.571, BA.630) and
    // frames (BA.482 and the like).
    let turn = 2.0 * std::f64::consts::PI + 1e-6;
    let half = std::f64::consts::FRAC_PI_2 + 1e-6;
    for f in 0..faces {
        let [lo, hi] = [ranges[2 * f], ranges[2 * f + 1]];
        let (u, v) = (hi[0] - lo[0], hi[1] - lo[1]);
        let ok = match stypes[f].as_str() {
            "BrepSurfaceSphereAPI" => u <= turn && lo[1] >= -half && hi[1] <= half,
            "BrepSurfaceCylinderAPI" | "BrepSurfaceConeAPI" => u <= turn,
            "BrepSurfaceTorusAPI" => u <= turn && v <= turn,
            _ => true,
        };
        assert!(ok, "face {f}'s range {lo:?} {hi:?}");
        // Each periodic direction's band starts in its first turn: SMLib
        // frees an array twice rebuilding a torus over a band from 2π.
        let first = |x: f64| (-1e-9..turn - 1e-6).contains(&x);
        let ok = match stypes[f].as_str() {
            "BrepSurfaceSphereAPI" | "BrepSurfaceCylinderAPI" | "BrepSurfaceConeAPI" => {
                first(lo[0])
            }
            "BrepSurfaceTorusAPI" => first(lo[0]) && first(lo[1]),
            _ => true,
        };
        assert!(
            ok,
            "face {f}'s range {lo:?} {hi:?} starts past its first turn"
        );
    }
    for e in 0..edges {
        if ctypes[e] == "BrepCurve3dCircleAPI" || ctypes[e] == "BrepCurve3dEllipseAPI" {
            let [t0, t1] = [eranges[2 * e], eranges[2 * e + 1]];
            assert!(
                t1 - t0 <= turn && t1 > -1e-6 && t1 <= turn,
                "edge {e}'s range [{t0}, {t1}] (BA.570, BA.630)"
            );
        }
    }
    let unit = |v: P3| (dist(v, [0.0; 3]) - 1.0).abs() < 1e-9;
    let dot = |a: P3, b: P3| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    for kind in ["plane", "cylinder", "cone", "sphere", "torus"] {
        let axis = format!("brep:surface:{kind}:axis");
        if a.has(&axis) {
            let refs = a.d3(&format!("brep:surface:{kind}:refDirection"));
            for (z, x) in a.d3(&axis).into_iter().zip(refs) {
                assert!(
                    unit(z) && unit(x) && dot(z, x).abs() < 1e-9,
                    "a {kind}'s frame"
                );
            }
        }
    }
    if a.has("brep:surface:cone:semiAngle") {
        for x in a.doubles("brep:surface:cone:semiAngle") {
            assert!(
                x > 0.0 && x < std::f64::consts::FRAC_PI_2,
                "a cone's half angle (BA.515)"
            );
        }
    }
    // Extents (BA_040 to BA_050, BA_310, BA_365, BA_465).
    let ext = a.d3("brep:extent");
    let fext = match a.get("extent") {
        Value::Float3Array(v) => v.clone(),
        _ => panic!("extent"),
    };
    for j in 0..3 {
        assert!(ext[0][j] < ext[1][j]);
        assert!(fext[0][j] as f64 <= ext[0][j] && fext[1][j] as f64 >= ext[1][j]);
    }
    let inside = |q: &P3| (0..3).all(|j| q[j] >= ext[0][j] && q[j] <= ext[1][j]);
    assert!(points.iter().all(inside), "a vertex outside the extent");
    for name in [
        "brep:surface:nurb:controlVertices",
        "brep:edge3dNurb:curve3d:nurb:controlVertices",
    ] {
        if a.has(name) {
            assert!(a.d3(name).iter().all(inside), "{name} outside the extent");
        }
    }
    Summary {
        uvless,
        regions: rtypes,
        shells,
        faces,
        loops,
        edgeuses: eus,
        edges,
        vertices,
        surfaces,
        curves,
        rectangular,
        tolerance: tol,
    }
}
