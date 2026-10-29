//! An ODB++ features writer whose output the reader reads back to the
//! same model.
//!
//! It writes millimetre files: coordinates in millimetres to six
//! decimals and symbol sizes in microns to three, so every nanometre is
//! written exactly. It writes what the reader reads, not every way a file
//! may say it: pad and text orientations are always `8` or `9` with an
//! angle, and a feature's attributes go through one lookup table.

use std::collections::BTreeMap;

use ryu_board::geom::{Contour, Dir, Edge, Mdeg, Nm, Place};
use ryu_board::model::{AttrValue, Feature, FeatureKind, Features, Polarity, Symbol};
use ryu_board::symbol::{Corners, Plating, StandardSymbol, ThermalForm};

use crate::odb::attrs::{Decl, Decls};

/// Nanometres as millimetres, exactly: up to six decimals, no trailing
/// zeros.
pub fn mm(v: Nm) -> String {
    decimal(v, 1_000_000)
}

/// Nanometres as microns, exactly.
pub fn um(v: Nm) -> String {
    decimal(v, 1_000)
}

fn decimal(v: Nm, per: i64) -> String {
    let sign = if v < 0 { "-" } else { "" };
    let a = v.unsigned_abs();
    let (whole, frac) = (a / per as u64, a % per as u64);
    if frac == 0 {
        return format!("{sign}{whole}");
    }
    let digits = per.ilog10() as usize;
    let f = format!("{frac:0digits$}");
    format!("{sign}{whole}.{}", f.trim_end_matches('0'))
}

/// Millidegrees as degrees, exactly.
pub fn deg(a: i64) -> String {
    decimal(a, 1_000)
}

fn corners(c: &Corners, chamfers: bool) -> String {
    let set = |which: ryu_board::symbol::CornerSet| {
        if which == ryu_board::symbol::CornerSet::ALL {
            String::new()
        } else {
            let d: String = (1..=4)
                .filter(|&k| which.has(k))
                .map(|k| char::from(b'0' + k))
                .collect();
            format!("x{d}")
        }
    };
    // A size with no corners to treat is no treatment.
    if c.round > 0 && c.rounded != ryu_board::symbol::CornerSet::NONE {
        format!("xr{}{}", um(c.round), set(c.rounded))
    } else if chamfers && c.chamfer > 0 && c.chamfered != ryu_board::symbol::CornerSet::NONE {
        format!("xc{}{}", um(c.chamfer), set(c.chamfered))
    } else {
        String::new()
    }
}

/// A standard symbol's ODB++ name in microns; `None` for the trapezoid,
/// which ODB++ doesn't have. Corners with a size but none to treat are
/// written sharp, as they draw (see [`canonical`]).
pub fn symbol_name(s: &StandardSymbol) -> Option<String> {
    use StandardSymbol as S;
    Some(match *s {
        S::Round { d } => format!("r{}", um(d)),
        S::Square { s } => format!("s{}", um(s)),
        S::Rect { w, h, corners: c } => format!("rect{}x{}{}", um(w), um(h), corners(&c, true)),
        S::Oval { w, h } => format!("oval{}x{}", um(w), um(h)),
        S::Diamond { w, h } => format!("di{}x{}", um(w), um(h)),
        S::Octagon { w, h, r } => format!("oct{}x{}x{}", um(w), um(h), um(r)),
        S::DonutRound { od, id } => format!("donut_r{}x{}", um(od), um(id)),
        S::DonutSquare { od, id, corners: c } => {
            format!("donut_s{}x{}{}", um(od), um(id), corners(&c, false))
        }
        S::DonutSquareRound { od, id } => format!("donut_sr{}x{}", um(od), um(id)),
        S::DonutRect {
            w,
            h,
            lw,
            corners: c,
        } => {
            format!(
                "donut_rc{}x{}x{}{}",
                um(w),
                um(h),
                um(lw),
                corners(&c, false)
            )
        }
        S::DonutOval { w, h, lw } => format!("donut_o{}x{}x{}", um(w), um(h), um(lw)),
        S::HexagonH { w, h, r } => format!("hex_l{}x{}x{}", um(w), um(h), um(r)),
        S::HexagonV { w, h, r } => format!("hex_s{}x{}x{}", um(w), um(h), um(r)),
        S::ButterflyRound { d } => format!("bfr{}", um(d)),
        S::ButterflySquare { s } => format!("bfs{}", um(s)),
        S::Triangle { base, h } => format!("tri{}x{}", um(base), um(h)),
        S::HalfOval { w, h } => format!("oval_h{}x{}", um(w), um(h)),
        S::Ellipse { w, h } => format!("el{}x{}", um(w), um(h)),
        S::Moire {
            ring_width,
            ring_gap,
            rings,
            line_width,
            line_length,
            line_angle,
        } => format!(
            "moire{}x{}x{rings}x{}x{}x{}",
            um(ring_width),
            um(ring_gap),
            um(line_width),
            um(line_length),
            deg(i64::from(line_angle.get()))
        ),
        S::Hole {
            d,
            plating,
            tol_plus,
            tol_minus,
        } => {
            let p = match plating {
                Plating::Plated => "p",
                Plating::NonPlated => "n",
                Plating::Via => "v",
            };
            format!("hole{}x{p}x{}x{}", um(d), um(tol_plus), um(tol_minus))
        }
        S::Null { ext } => format!("null{ext}"),
        S::Thermal(t) => {
            let tail = format!(
                "x{}x{}x{}",
                deg(i64::from(t.angle.get())),
                t.spokes,
                um(t.gap)
            );
            match t.form {
                ThermalForm::RoundRounded { od, id } => format!("thr{}x{}{tail}", um(od), um(id)),
                ThermalForm::RoundSquared { od, id } => format!("ths{}x{}{tail}", um(od), um(id)),
                ThermalForm::Square {
                    os,
                    is,
                    corners: c,
                    open,
                } => format!(
                    "{}{}x{}{tail}{}",
                    if open { "s_tho" } else { "s_ths" },
                    um(os),
                    um(is),
                    corners(&c, false)
                ),
                ThermalForm::SquareRound { os, id } => format!("sr_ths{}x{}{tail}", um(os), um(id)),
                ThermalForm::Line { os, is } => format!("s_thr{}x{}{tail}", um(os), um(is)),
                ThermalForm::Rect {
                    w,
                    h,
                    ring,
                    corners: c,
                    open,
                } => format!(
                    "{}{}x{}{tail}x{}{}",
                    if open { "rc_tho" } else { "rc_ths" },
                    um(w),
                    um(h),
                    um(ring),
                    corners(&c, false)
                ),
                ThermalForm::Oval { w, h, ring } => {
                    format!("o_ths{}x{}{tail}x{}", um(w), um(h), um(ring))
                }
                ThermalForm::Oblong {
                    w,
                    h,
                    ring,
                    rounded,
                } => format!(
                    "oblong_ths{}x{}{tail}x{}x{}",
                    um(w),
                    um(h),
                    um(ring),
                    if rounded { "r" } else { "s" }
                ),
            }
        }
        S::Trapezoid { .. } => return None,
    })
}

/// `s` as its name reads back: corners with a size and none to treat are
/// sharp.
pub fn canonical(s: &StandardSymbol) -> StandardSymbol {
    let fix = |c: Corners| {
        let none = ryu_board::symbol::CornerSet::NONE;
        let mut c = c;
        if c.rounded == none {
            c.round = 0;
        }
        if c.chamfered == none {
            c.chamfer = 0;
        }
        if c.round == 0 {
            c.rounded = none;
        }
        if c.chamfer == 0 {
            c.chamfered = none;
        }
        c
    };
    use StandardSymbol as S;
    match *s {
        S::Rect { w, h, corners } => S::Rect {
            w,
            h,
            corners: fix(corners),
        },
        S::DonutSquare { od, id, corners } => S::DonutSquare {
            od,
            id,
            corners: fix(corners),
        },
        S::DonutRect { w, h, lw, corners } => S::DonutRect {
            w,
            h,
            lw,
            corners: fix(corners),
        },
        S::Thermal(mut t) => {
            t.form = match t.form {
                ThermalForm::Square {
                    os,
                    is,
                    corners,
                    open,
                } => ThermalForm::Square {
                    os,
                    is,
                    corners: fix(corners),
                    open,
                },
                ThermalForm::Rect {
                    w,
                    h,
                    ring,
                    corners,
                    open,
                } => ThermalForm::Rect {
                    w,
                    h,
                    ring,
                    corners: fix(corners),
                    open,
                },
                f => f,
            };
            S::Thermal(t)
        }
        s => s,
    }
}

fn pol(p: Polarity) -> &'static str {
    match p {
        Polarity::Positive => "P",
        Polarity::Negative => "N",
    }
}

/// `8 a` or `9 a`: the model's placement as ODB++'s clockwise turn.
fn orient(p: &Place) -> String {
    let cw = if p.mirror {
        i64::from(p.rot.get())
    } else {
        i64::from(Mdeg::new(-i64::from(p.rot.get())).get())
    };
    format!("{} {}", if p.mirror { 9 } else { 8 }, deg(cw))
}

fn contour(out: &mut String, c: &Contour, island: bool) {
    // ODB++ runs islands clockwise and holes counterclockwise, the
    // model's way reversed.
    let c = c.reversed();
    let start = c.edges()[0].start();
    out.push_str(&format!(
        "OB {} {} {}\n",
        mm(start.x),
        mm(start.y),
        if island { "I" } else { "H" }
    ));
    for e in c.edges() {
        match e {
            Edge::Segment(s) => out.push_str(&format!("OS {} {}\n", mm(s.b.x), mm(s.b.y))),
            Edge::Arc(a) => out.push_str(&format!(
                "OC {} {} {} {} {}\n",
                mm(a.end.x),
                mm(a.end.y),
                mm(a.centre.x),
                mm(a.centre.y),
                if a.dir == Dir::Cw { "Y" } else { "N" }
            )),
        }
    }
    out.push_str("OE\n");
}

/// The lookup table a file's features need, and each feature's
/// assignment text.
struct Lookup {
    names: BTreeMap<String, usize>,
    texts: BTreeMap<String, usize>,
}

impl Lookup {
    fn new(features: &[Feature]) -> Lookup {
        let mut names = BTreeMap::new();
        let mut texts = BTreeMap::new();
        for f in features {
            for (k, v) in &f.attributes {
                let n = names.len();
                names.entry(k.clone()).or_insert(n);
                if let AttrValue::Text(t) = v {
                    let n = texts.len();
                    texts.entry(t.clone()).or_insert(n);
                }
            }
        }
        Lookup { names, texts }
    }

    fn assign(&self, f: &Feature, decls: &Decls) -> String {
        let mut parts = Vec::new();
        for (k, v) in &f.attributes {
            let i = self.names[k];
            parts.push(match (v, decls.get(k)) {
                (AttrValue::Flag, _) => format!("{i}"),
                (AttrValue::Number(n), _) => format!("{i}={n}"),
                (AttrValue::Text(t), Some(Decl::Option(opts))) => {
                    let o = opts.iter().position(|o| o == t).expect("an option it has");
                    format!("{i}={o}")
                }
                (AttrValue::Text(t), _) => format!("{i}={}", self.texts[t]),
            });
        }
        let mut s = String::new();
        if !parts.is_empty() || f.id.is_some() {
            s.push(';');
            s.push_str(&parts.join(","));
        }
        if let Some(id) = f.id {
            s.push_str(&format!(";ID={id}"));
        }
        s
    }
}

/// A features file of `features`, in millimetres. User symbols are named
/// as they are; unknown ones too. Panics on a trapezoid, which ODB++
/// can't name.
pub fn write_features(features: &Features) -> String {
    let decls = Decls::default();
    let mut out = String::from("UNITS=MM\n#\n#Feature symbol names\n#\n");
    for (i, s) in features.symbols().iter().enumerate() {
        let name = match s {
            Symbol::Standard(s) => symbol_name(s).expect("ODB++ has no trapezoid"),
            Symbol::User(n) | Symbol::Unknown(n) => n.clone(),
        };
        out.push_str(&format!("${i} {name}\n"));
    }
    let lookup = Lookup::new(features.features());
    let mut names: Vec<(&String, &usize)> = lookup.names.iter().collect();
    names.sort_by_key(|(_, i)| **i);
    for (n, i) in names {
        out.push_str(&format!("@{i} {n}\n"));
    }
    let mut texts: Vec<(&String, &usize)> = lookup.texts.iter().collect();
    texts.sort_by_key(|(_, i)| **i);
    for (t, i) in texts {
        out.push_str(&format!("&{i} {t}\n"));
    }
    out.push_str("#\n#Layer features\n#\n");
    for f in features.features() {
        let p = pol(f.polarity);
        let d = f.dcode.unwrap_or(0);
        let tail = lookup.assign(f, &decls);
        match &f.kind {
            FeatureKind::Line { a, b, symbol } => out.push_str(&format!(
                "L {} {} {} {} {symbol} {p} {d}{tail}\n",
                mm(a.x),
                mm(a.y),
                mm(b.x),
                mm(b.y)
            )),
            FeatureKind::Arc { arc, symbol } => out.push_str(&format!(
                "A {} {} {} {} {} {} {symbol} {p} {d} {}{tail}\n",
                mm(arc.start.x),
                mm(arc.start.y),
                mm(arc.end.x),
                mm(arc.end.y),
                mm(arc.centre.x),
                mm(arc.centre.y),
                if arc.dir == Dir::Cw { "Y" } else { "N" }
            )),
            FeatureKind::Pad { symbol, place } => out.push_str(&format!(
                "P {} {} {symbol} {p} {d} {}{tail}\n",
                mm(place.at.x),
                mm(place.at.y),
                orient(place)
            )),
            FeatureKind::Text(t) => {
                assert_eq!(t.line_width % 3_048, 0, "a width factor in hundredths");
                out.push_str(&format!(
                    "T {} {} {} {p} {} {} {} {} '{}' 0{tail}\n",
                    mm(t.place.at.x),
                    mm(t.place.at.y),
                    t.font,
                    orient(&t.place),
                    mm(t.width),
                    mm(t.height),
                    decimal(t.line_width / 3_048, 100),
                    t.string
                ));
            }
            FeatureKind::Barcode(b) => out.push_str(&format!(
                "B {} {} {} standard {p} {} E 0.2 5 Y N N N T '{}'{tail}\n",
                mm(b.place.at.x),
                mm(b.place.at.y),
                b.kind,
                orient(&b.place),
                b.string
            )),
            FeatureKind::Surface { regions } => {
                out.push_str(&format!("S {p} {d}{tail}\n"));
                for r in regions {
                    contour(&mut out, &r.outer, true);
                    for h in &r.holes {
                        contour(&mut out, h, false);
                    }
                }
                out.push_str("SE\n");
            }
        }
    }
    out
}
