//! A step's `eda/data`: its nets with their subnets and the features each
//! subnet holds, and its packages with their outlines and pins.
//!
//! Records: `HDR`, `UNITS=`, `LYR` (the layers `FID` records number),
//! `PRP`, the lookup table (`@n`, `&n`, also written `#@n` and `#&n` for
//! old readers), `NET`, `SNT` (`TOP`, `VIA`, `TRC`, `PLN`), `FID`, `PKG`
//! and `PIN` with their outlines (`RC`, `CR`, `SQ`, or `CT` … `CE` around
//! polygons), and `FGR` groups, whose features are a text's strokes and
//! are passed over.

use ryu_board::geom::{Point, Region};
use ryu_board::model::{Attributes, Notice, Package, PackagePin, Side};
use ryu_board::shape::{circle, rect};
use ryu_board::symbol::Corners;

use super::attrs::{Decls, Table, tail};
use super::error::{Once, location, warning};
use super::outline::Outline;
use super::text::{Bad, Token, split_record, tokens};
use super::units::{Nums, Units};

/// What kind of piece of a net a subnet is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubnetKind {
    /// A component pin's toeprint: its side, component and toeprint, by
    /// index in that side's components file.
    Toeprint {
        side: Side,
        component: u32,
        toeprint: u32,
    },
    Via,
    Trace,
    Plane,
}

/// What a feature ID's feature is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FidKind {
    Copper,
    Laminate,
    Hole,
}

/// A feature of a subnet: by layer (an index into [`Eda::layers`]) and
/// index in that layer's features file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fid {
    pub kind: FidKind,
    pub layer: u32,
    pub feature: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subnet {
    pub kind: SubnetKind,
    pub fids: Vec<Fid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdaNet {
    pub name: String,
    pub attributes: Attributes,
    pub subnets: Vec<Subnet>,
}

/// An `eda/data` file, read.
#[derive(Clone, Debug, Default)]
pub struct Eda {
    /// The `LYR` names, lower case.
    pub layers: Vec<String>,
    pub nets: Vec<EdaNet>,
    pub packages: Vec<Package>,
    pub warnings: Vec<Notice>,
}

/// Where records land.
enum In {
    Top,
    Net,
    Package,
    Pin,
    Group,
}

/// An outline record (`RC`, `CR`, `SQ`) as a region, or `None` where it
/// has no area.
fn simple(head: &str, t: &[Token], nums: &mut Nums, line: u32) -> Result<Option<Region>, Bad> {
    let need = |n: usize| {
        t.get(n).ok_or_else(|| {
            Bad::new(
                line,
                1,
                format!("{head} takes {} numbers", if head == "RC" { 4 } else { 3 }),
            )
        })
    };
    let c = Point::new(nums.coord(need(1)?, line)?, nums.coord(need(2)?, line)?);
    let a = nums.coord(need(3)?, line)?;
    let contour = match head {
        "RC" => {
            let h = nums.coord(need(4)?, line)?;
            rect(c.x, c.y, c.x + a, c.y + h, &Corners::SHARP)
        }
        "CR" => circle(c, a),
        _ => rect(c.x - a, c.y - a, c.x + a, c.y + a, &Corners::SHARP),
    };
    Ok(contour.ok().map(Region::solid))
}

/// Reads an `eda/data` file.
pub fn eda(text: &str, path: &str, units: Units, decls: &Decls) -> Result<(Eda, Nums), Bad> {
    let mut nums = Nums::new(units);
    let mut out = Eda::default();
    let mut table = Table::default();
    let mut once = Once::default();
    let mut at = In::Top;
    // An outline being read (`CT` … `CE`), and whether it is a pin's.
    let mut contour: Option<Outline> = None;
    let mut last = 0;
    for (i, raw) in text.split('\n').enumerate() {
        let n = i as u32 + 1;
        last = n;
        let raw = raw.trim();
        let line = match raw.strip_prefix('#') {
            Some(rest) if rest.starts_with('@') || rest.starts_with('&') => rest,
            Some(_) => continue,
            None if raw.is_empty() => continue,
            None => raw,
        };
        let bad = |c: u32, m: String| Bad::new(n, c, m);
        if let Some(u) = line.strip_prefix("UNITS=") {
            nums.units = Units::parse(u.trim())
                .ok_or_else(|| bad(7, format!("UNITS={u}: not MM or INCH")))?;
            continue;
        }
        if let Some(rest) = line.strip_prefix('@') {
            table.name(rest, n, decls)?;
            continue;
        }
        if let Some(rest) = line.strip_prefix('&') {
            table.text(rest, n)?;
            continue;
        }
        let (fields, rest) = split_record(line);
        let t = tokens(fields).map_err(|b| bad(b.column, b.message))?;
        let Some(head) = t.first().map(|t| t.text) else {
            return Err(bad(1, "a record with no kind".into()));
        };
        if let Some(o) = &mut contour {
            if head == "CE" {
                let o = contour.take().unwrap();
                let regions = o.finish(n)?;
                add_outline(&mut out, &at, regions);
            } else {
                o.line(&t, &mut nums, n)?;
            }
            continue;
        }
        let index = |k: usize, what: &str| -> Result<u32, Bad> {
            let tok = t
                .get(k)
                .ok_or_else(|| bad(1, format!("{head} without {what}")))?;
            tok.text
                .parse()
                .map_err(|_| bad(tok.column, format!("{what} {}: not an index", tok.text)))
        };
        match head {
            "HDR" | "PRP" => {}
            "LYR" => out
                .layers
                .extend(t[1..].iter().map(|l| l.text.to_ascii_lowercase())),
            "NET" => {
                let name = t
                    .get(1)
                    .ok_or_else(|| bad(1, "NET without a name".into()))?;
                let (list, _) = tail(rest);
                let (attributes, ignored) = table.values(list);
                for g in ignored {
                    let m = format!("attribute {}: {}", g.name, g.why);
                    once.add(
                        warning::IGNORED,
                        &m.clone(),
                        None,
                        || m,
                        location(path, Some(n), None),
                    );
                }
                out.nets.push(EdaNet {
                    name: name.text.to_string(),
                    attributes,
                    subnets: Vec::new(),
                });
                at = In::Net;
            }
            "SNT" => {
                let Some(net) = out.nets.last_mut() else {
                    return Err(bad(1, "SNT before any NET".into()));
                };
                if !matches!(at, In::Net) {
                    return Err(bad(1, "SNT outside a net".into()));
                }
                let kind = match t.get(1).map(|k| k.text) {
                    Some("TOP") => {
                        let side = match t.get(2).map(|s| s.text) {
                            Some("T") => Side::Top,
                            Some("B") => Side::Bottom,
                            _ => return Err(bad(1, "SNT TOP without T or B".into())),
                        };
                        SubnetKind::Toeprint {
                            side,
                            component: index(3, "a component")?,
                            toeprint: index(4, "a toeprint")?,
                        }
                    }
                    Some("VIA") => SubnetKind::Via,
                    Some("TRC") => SubnetKind::Trace,
                    Some("PLN") => SubnetKind::Plane,
                    other => {
                        return Err(bad(
                            1,
                            format!("SNT {}: not a subnet kind", other.unwrap_or("")),
                        ));
                    }
                };
                net.subnets.push(Subnet {
                    kind,
                    fids: Vec::new(),
                });
            }
            "FID" => {
                let kind = match t.get(1).map(|k| k.text) {
                    Some("C") => FidKind::Copper,
                    Some("L") => FidKind::Laminate,
                    Some("H") => FidKind::Hole,
                    _ => return Err(bad(1, "FID without C, L or H".into())),
                };
                let fid = Fid {
                    kind,
                    layer: index(2, "a layer")?,
                    feature: index(3, "a feature")?,
                };
                match at {
                    In::Group => {}
                    In::Net => match out.nets.last_mut().and_then(|n| n.subnets.last_mut()) {
                        Some(s) => s.fids.push(fid),
                        None => return Err(bad(1, "FID before any SNT".into())),
                    },
                    _ => return Err(bad(1, "FID outside a net or group".into())),
                }
            }
            "PKG" => {
                let name = t
                    .get(1)
                    .ok_or_else(|| bad(1, "PKG without a name".into()))?;
                out.packages.push(Package {
                    name: name.text.to_string(),
                    outline: Vec::new(),
                    pins: Vec::new(),
                });
                at = In::Package;
            }
            "PIN" => {
                let Some(pkg) = out.packages.last_mut() else {
                    return Err(bad(1, "PIN before any PKG".into()));
                };
                let name = t
                    .get(1)
                    .ok_or_else(|| bad(1, "PIN without a name".into()))?;
                let (x, y) = match (t.get(3), t.get(4)) {
                    (Some(x), Some(y)) => (x, y),
                    _ => return Err(bad(1, "PIN without a centre".into())),
                };
                let at_ = nums.point(x, y, n)?;
                pkg.pins.push(PackagePin {
                    name: name.text.to_string(),
                    at: at_,
                    shape: Vec::new(),
                });
                at = In::Pin;
            }
            "RC" | "CR" | "SQ" => {
                if !matches!(at, In::Package | In::Pin) {
                    return Err(bad(1, format!("{head} outside a package")));
                }
                match simple(head, &t, &mut nums, n)? {
                    Some(r) => add_outline(&mut out, &at, vec![r]),
                    None => once.add(
                        warning::IGNORED,
                        "degenerate",
                        None,
                        || "outlines with no area".into(),
                        location(path, Some(n), None),
                    ),
                }
            }
            "CT" => {
                if !matches!(at, In::Package | In::Pin) {
                    return Err(bad(1, "CT outside a package".into()));
                }
                contour = Some(Outline::default());
            }
            "FGR" => at = In::Group,
            other => {
                let m = format!("record kind {other}");
                once.add(
                    warning::IGNORED,
                    &m.clone(),
                    None,
                    || m,
                    location(path, Some(n), None),
                );
            }
        }
    }
    if contour.is_some() {
        return Err(Bad::new(last, 1, "CT without CE"));
    }
    out.warnings = once.into_vec();
    Ok((out, nums))
}

fn add_outline(out: &mut Eda, at: &In, regions: Vec<Region>) {
    let Some(pkg) = out.packages.last_mut() else {
        return;
    };
    match at {
        In::Pin => {
            if let Some(pin) = pkg.pins.last_mut() {
                pin.shape.extend(regions);
            }
        }
        _ => pkg.outline.extend(regions),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_board::model::AttrValue;

    fn read(t: &str) -> Result<Eda, Bad> {
        eda(t, "steps/pcb/eda/data", Units::Inch, &Decls::default()).map(|(e, _)| e)
    }

    const DATA: &str = "#\nHDR Mentor Boardstation database\nUNITS=MM\nLYR sst sigt sigb drill \nPRP MILLING_ORIGIN 'MILLING 0 0.0 0' 0 0\n\
#Net attribute names\n#@0 .critical_net\n@2 .net_type\n&1 clocks\n\
# NET 0\nNET /D_CL_TX_CLK ;0,2=1;ID=1030\nSNT TOP T 16 0\nFID C 1 33\nFID C 2 34\nSNT VIA\nFID H 3 7\nSNT TRC\nSNT PLN S C 0\nFID C 1 5\n\
NET $NONE$\nSNT TOP B 0 2\n\
# PKG 0\nPKG *PDXC 0.1 -0.25 -0.145 0.25 0.145;0;ID=5\nRC -0.25 -0.1 0.5 0.2\nPRP X 'y'\nPIN 1 T -0.2 -0.1105 0 E S ID=1034\nRC -0.2125 -0.145 0.025 0.069\n\
PIN P T 0.0688976 0 0 E T\nCR 0.0688976 0 0.011811\nPIN Q S 0 0 0 E S\nSQ 0 0 0.1\nCT\nOB 1 1 I\nOS 1 2\nOS 2 2\nOS 1 1\nOE\nCE\n\
FGR TEXT\nPRP string '030'\nFID C 1 11018\nRC 0 0 0 0\n";

    #[test]
    fn nets_subnets_and_packages() {
        let e = read(DATA).unwrap_err();
        assert_eq!(e.message, "RC outside a package");
        let e = read(&DATA.replace("RC 0 0 0 0\n", "")).unwrap();
        assert_eq!(e.layers, ["sst", "sigt", "sigb", "drill"]);
        assert_eq!(e.nets.len(), 2);
        let n = &e.nets[0];
        assert_eq!(n.name, "/D_CL_TX_CLK");
        assert_eq!(n.attributes.get(".critical_net"), Some(&AttrValue::Flag));
        assert_eq!(
            n.attributes.get(".net_type"),
            Some(&AttrValue::Text("clocks".into()))
        );
        assert_eq!(n.subnets.len(), 4);
        assert_eq!(
            n.subnets[0].kind,
            SubnetKind::Toeprint {
                side: Side::Top,
                component: 16,
                toeprint: 0
            }
        );
        assert_eq!(
            n.subnets[0].fids,
            [
                Fid {
                    kind: FidKind::Copper,
                    layer: 1,
                    feature: 33
                },
                Fid {
                    kind: FidKind::Copper,
                    layer: 2,
                    feature: 34
                },
            ]
        );
        assert_eq!(n.subnets[1].fids[0].kind, FidKind::Hole);
        assert_eq!(
            (n.subnets[2].kind, n.subnets[2].fids.len()),
            (SubnetKind::Trace, 0)
        );
        assert_eq!(n.subnets[3].kind, SubnetKind::Plane);
        assert_eq!(
            e.nets[1].subnets[0].kind,
            SubnetKind::Toeprint {
                side: Side::Bottom,
                component: 0,
                toeprint: 2
            }
        );
        let p = &e.packages[0];
        assert_eq!(p.name, "*PDXC");
        assert_eq!(p.outline.len(), 1);
        assert_eq!(p.outline[0].bounds().min, Point::new(-250_000, -100_000));
        assert_eq!(p.pins.len(), 3);
        assert_eq!(
            (p.pins[0].name.as_str(), p.pins[0].at),
            ("1", Point::new(-200_000, -110_500))
        );
        assert_eq!(
            p.pins[0].shape[0].bounds().max,
            Point::new(-187_500, -76_000)
        );
        assert_eq!(p.pins[1].shape[0].bounds().min, Point::new(57_087, -11_811));
        assert_eq!(p.pins[2].shape.len(), 2);
        // The text group's feature went nowhere.
        assert_eq!(e.nets[1].subnets[0].fids.len(), 0);
        assert!(e.warnings.is_empty(), "{:?}", e.warnings);
    }

    #[test]
    fn faults_and_warnings() {
        let m = |t: &str| read(t).unwrap_err().message;
        assert_eq!(m("SNT VIA\n"), "SNT before any NET");
        // Found by the odb fuzz target: attributes and no fields.
        assert_eq!(m(";0=1\n"), "a record with no kind");
        assert_eq!(m("NET a\nFID C 1 2\n"), "FID before any SNT");
        assert_eq!(m("NET a\nSNT TOP X 1 2\n"), "SNT TOP without T or B");
        assert_eq!(m("NET a\nSNT WHAT\n"), "SNT WHAT: not a subnet kind");
        assert_eq!(m("PIN 1 T 0 0 0 E S\n"), "PIN before any PKG");
        assert_eq!(m("PKG a 0 0 0 1 1\nCT\nOB 0 0 I\n"), "CT without CE");
        assert_eq!(m("FID C 1 2\n"), "FID outside a net or group");
        let e = read("PKG a 0 0 0 1 1\nRC 0 0 0 1\nXYZ 1\nXYZ 2\n").unwrap();
        let w: Vec<_> = e.warnings.iter().map(|w| w.message.as_str()).collect();
        assert_eq!(w, ["outlines with no area", "record kind XYZ (2 times)"]);
    }
}
