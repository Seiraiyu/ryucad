//! A component layer's `components` (`comp_+_top`, `comp_+_bot`): each
//! component, its properties and attributes, and its pins (toeprints)
//! with the nets `eda/data` numbers.
//!
//! Records: `UNITS=`, the lookup table, `CMP pkg x y rot mirror refdes
//! part;attrs;ID=`, `PRP name 'value'`, `TOP pin x y rot mirror net subnet
//! name`. Rotations turn clockwise and mirroring follows, as for pads. The
//! BOM records (`CPN`, `VND`, `MPN` and the rest) aren't read.

use std::collections::BTreeMap;

use ryu_board::geom::Mdeg;
use ryu_board::model::{Component, Notice, Pin, Side};

use super::attrs::{Decls, Table, tail};
use super::error::{Once, location, warning};
use super::text::{Bad, Token, lines, split_record, tokens};
use super::units::{Nums, Units};

/// A component as its file gives it: `package` and each pin's `net`
/// index the step's `eda/data` packages and nets, unchecked.
pub type Read = (Vec<Component>, Nums, Vec<Notice>);

fn mirror(t: Option<&Token>, line: u32) -> Result<bool, Bad> {
    match t.map(|t| t.text) {
        Some("N") => Ok(false),
        Some("M") => Ok(true),
        Some(other) => Err(Bad::new(
            line,
            t.unwrap().column,
            format!("mirror {other}: not N or M"),
        )),
        None => Err(Bad::new(line, 1, "mirror missing")),
    }
}

fn rotation(t: Option<&Token>, mirrored: bool, nums: &mut Nums, line: u32) -> Result<Mdeg, Bad> {
    let t = t.ok_or_else(|| Bad::new(line, 1, "rotation missing"))?;
    let cw = i64::from(nums.angle(t, line)?.get());
    Ok(if mirrored {
        Mdeg::new(cw)
    } else {
        Mdeg::from_clockwise(cw)
    })
}

/// Reads a components file of `side`.
pub fn components(
    text: &str,
    path: &str,
    side: Side,
    units: Units,
    decls: &Decls,
) -> Result<Read, Bad> {
    let mut nums = Nums::new(units);
    let mut table = Table::default();
    let mut once = Once::default();
    let mut out: Vec<Component> = Vec::new();
    for (n, line) in lines(text) {
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
        if line.starts_with("ID=") {
            continue;
        }
        let (fields, rest) = split_record(line);
        let t = tokens(fields).map_err(|b| bad(b.column, b.message))?;
        let need = |k: usize, what: &str| {
            t.get(k)
                .ok_or_else(|| bad(1, format!("{} without {what}", t[0].text)))
        };
        let Some(kind) = t.first().map(|t| t.text) else {
            return Err(bad(1, "a record with no kind".into()));
        };
        match kind {
            "CMP" => {
                let pkg = need(1, "a package")?;
                let package = pkg
                    .text
                    .parse()
                    .map_err(|_| bad(pkg.column, format!("package {}: not an index", pkg.text)))?;
                let at = nums.point(need(2, "a position")?, need(3, "a position")?, n)?;
                let m = mirror(t.get(5), n)?;
                let rot = rotation(t.get(4), m, &mut nums, n)?;
                let refdes = need(6, "a reference designator")?.text.to_string();
                let part = t.get(7).map_or("", |p| p.text).to_string();
                let (list, _) = tail(rest);
                let (attributes, ignored) = table.values(list);
                for g in ignored {
                    let msg = format!("attribute {}: {}", g.name, g.why);
                    once.add(
                        warning::IGNORED,
                        &msg.clone(),
                        None,
                        || msg,
                        location(path, Some(n), None),
                    );
                }
                out.push(Component {
                    refdes,
                    package,
                    part,
                    at,
                    rot,
                    mirror: m,
                    side,
                    properties: BTreeMap::new(),
                    attributes,
                    pins: Vec::new(),
                });
            }
            "PRP" => {
                let Some(c) = out.last_mut() else {
                    return Err(bad(1, "PRP before any CMP".into()));
                };
                let name = need(1, "a name")?.text.to_string();
                let value = need(2, "a value")?;
                if !value.quoted {
                    return Err(bad(value.column, "a property value not in quotes".into()));
                }
                c.properties
                    .entry(name)
                    .or_insert_with(|| value.text.to_string());
            }
            "TOP" => {
                let Some(c) = out.last_mut() else {
                    return Err(bad(1, "TOP before any CMP".into()));
                };
                let at = nums.point(need(2, "a position")?, need(3, "a position")?, n)?;
                let m = mirror(t.get(5), n)?;
                let rot = rotation(t.get(4), m, &mut nums, n)?;
                let net_t = need(6, "a net")?;
                let net: i64 = net_t
                    .text
                    .parse()
                    .map_err(|_| bad(net_t.column, format!("net {}: not a number", net_t.text)))?;
                let name = t.get(8).map_or("", |p| p.text).to_string();
                c.pins.push(Pin {
                    name,
                    at,
                    rot,
                    net: u32::try_from(net).ok(),
                });
            }
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
    Ok((out, nums, once.into_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_board::geom::Point;
    use ryu_board::model::AttrValue;

    const SPEC: &str = "#\nUNITS=INCH\n#\n@0 .no_pop\n@1 .comp_ignore\n@2 .desc1\n@3 .comp_mount_type\n@4 .comp_height\n&0 Internally created connector\n\
# CMP 0\nCMP 0 1.55 8.9 270.0 N J10 2300046 ;0,2=0,3=1\nPRP PART_NUMBER '1000045'\nPRP VALUE 'FAN1654'\nPRP PART_NUMBER '999'\n\
TOP 0 6.568 5.2296 0.0 N 8 163 1\nTOP 1 6.568 5.204 90.0 M -1 0 2\n#\n# BOM DATA\nCPN M62X971EL7\nVND MOTOROLA INC.\nMPN 0 Y MC74HC259ADT\n\
CMP 3 5.017 1.791 180.0 M C400 ??? ;3=1\n";

    #[test]
    fn components_with_properties_attributes_and_pins() {
        let (c, _, warnings) =
            components(SPEC, "comp", Side::Bottom, Units::Mm, &Decls::default()).unwrap();
        assert_eq!(c.len(), 2);
        let j = &c[0];
        assert_eq!(
            (j.refdes.as_str(), j.part.as_str(), j.package),
            ("J10", "2300046", 0)
        );
        assert_eq!(
            (j.at, j.rot, j.mirror, j.side),
            (
                Point::new(39_370_000, 226_060_000),
                Mdeg::new(90_000),
                false,
                Side::Bottom
            )
        );
        assert_eq!(j.properties["PART_NUMBER"], "1000045");
        assert_eq!(j.properties["VALUE"], "FAN1654");
        assert_eq!(j.attributes.get(".no_pop"), Some(&AttrValue::Flag));
        assert_eq!(
            j.attributes.get(".desc1"),
            Some(&AttrValue::Text("Internally created connector".into()))
        );
        assert_eq!(
            j.attributes.get(".comp_mount_type"),
            Some(&AttrValue::Text("SMT".into()))
        );
        assert_eq!(j.pins.len(), 2);
        assert_eq!((j.pins[0].name.as_str(), j.pins[0].net), ("1", Some(8)));
        assert_eq!((j.pins[1].rot, j.pins[1].net), (Mdeg::new(90_000), None));
        assert_eq!(
            (c[1].rot, c[1].mirror, c[1].part.as_str()),
            (Mdeg::new(180_000), true, "???")
        );
        let w: Vec<_> = warnings.iter().map(|w| w.message.as_str()).collect();
        assert_eq!(w, ["record kind CPN", "record kind VND", "record kind MPN"]);
    }

    #[test]
    fn faults() {
        let m = |t: &str| {
            components(t, "c", Side::Top, Units::Mm, &Decls::default())
                .unwrap_err()
                .message
        };
        assert_eq!(m("PRP A 'b'\n"), "PRP before any CMP");
        assert_eq!(m(";0=1\n"), "a record with no kind");
        assert_eq!(m("TOP 0 0 0 0 N 1 0 1\n"), "TOP before any CMP");
        assert_eq!(m("CMP x 0 0 0 N R1 p\n"), "package x: not an index");
        assert_eq!(m("CMP 0 0 0 0 Q R1 p\n"), "mirror Q: not N or M");
        assert_eq!(m("CMP 0 0 0 0 N\n"), "CMP without a reference designator");
        assert_eq!(
            m("CMP 0 0 0 0 N R1 p\nPRP A b\n"),
            "a property value not in quotes"
        );
    }
}
