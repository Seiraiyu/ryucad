//! The package's assembly files (design 732): the design's BOM as `bom`
//! writes it, pick and place, and both in the preset's assembly form.
//! The position is the footprint's origin (the placement's `at`) and the
//! rotation the placement's, counterclockwise; no per-package offsets.

use super::load::BoardFiles;
use super::occ::Occ;
use super::preset::AssemblyForm;
use crate::footprint::Flag;
use crate::netlist::{Netlist, Occurrence};
use crate::pdm::bom::{designator_key, lines_of, with_properties};
use crate::project::Project;
use crate::session::Session;
use ryu_gerber::mm;
use ryupdm::lines::Line;
use serde_json::{Value, json};

/// One placed, populated occurrence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub designator: String,
    pub value: String,
    pub footprint: String,
    pub at: [i64; 2],
    pub rotation_mdeg: i64,
    /// The part is on the bottom.
    pub bottom: bool,
}

/// The footprint's ID, as the BOM's `footprint` column writes it.
fn footprint_id(o: &Occ) -> String {
    o.footprint_ref
        .as_deref()
        .map(|f| f.rsplit('/').next().unwrap_or(f))
        .unwrap_or_default()
        .to_owned()
}

/// The rows of every placed, populated occurrence with a footprint,
/// sorted by designator, and those left out with why (`dnp`,
/// `exclude_from_pos`, `unplaced`, `footprint_unreadable`).
pub fn rows(occs: &[Occ], b: &BoardFiles) -> (Vec<Row>, Vec<Value>) {
    let mut rows = Vec::new();
    let mut left: Vec<(String, Value)> = Vec::new();
    let mut leave = |o: &Occ, why: &str| {
        left.push((
            o.label().to_owned(),
            json!({"occurrence": o.reference, "designator": o.label(), "why": why}),
        ));
    };
    for o in occs {
        let Some(f) = o.footprint else {
            if o.footprint_ref.is_some() {
                leave(o, "footprint_unreadable");
            }
            continue;
        };
        if o.dnp {
            leave(o, "dnp");
            continue;
        }
        if f.flags.contains(&Flag::ExcludeFromPos) {
            leave(o, "exclude_from_pos");
            continue;
        }
        match b
            .placements
            .get(&o.reference)
            .and_then(|p| p.item.as_ref().ok())
        {
            None => leave(o, "unplaced"),
            Some(p) => rows.push(Row {
                designator: o.label().to_owned(),
                value: o.value.clone().unwrap_or_default(),
                footprint: footprint_id(o),
                at: p.at,
                rotation_mdeg: i64::from(ryu_board::geom::Mdeg::new(p.rotation_mdeg).get()),
                bottom: !p.side.is_top(),
            }),
        }
    }
    rows.sort_by_key(|r| designator_key(&r.designator));
    left.sort_by_key(|l| designator_key(&l.0));
    (rows, left.into_iter().map(|(_, v)| v).collect())
}

/// One CSV field (RFC 4180).
fn field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

fn csv_line(out: &mut String, cells: &[String]) {
    let cells: Vec<String> = cells.iter().map(|c| field(c)).collect();
    out.push_str(&cells.join(","));
    out.push('\n');
}

/// Millidegrees as degrees with as few decimals as hold them: `90`,
/// `45.5`.
fn degrees(mdeg: i64) -> String {
    let (d, m) = (mdeg / 1000, (mdeg % 1000).abs());
    if m == 0 {
        d.to_string()
    } else {
        let frac = format!("{m:03}");
        let sign = if mdeg < 0 && d == 0 { "-" } else { "" };
        format!("{sign}{d}.{}", frac.trim_end_matches('0'))
    }
}

/// `<board>-pos.csv`.
pub fn pos_csv(rows: &[Row]) -> String {
    let mut out = String::from("Designator,Value,Footprint,X,Y,Rotation,Side\n");
    for r in rows {
        csv_line(
            &mut out,
            &[
                r.designator.clone(),
                r.value.clone(),
                r.footprint.clone(),
                mm(r.at[0]),
                mm(r.at[1]),
                degrees(r.rotation_mdeg),
                if r.bottom { "bottom" } else { "top" }.to_owned(),
            ],
        );
    }
    out
}

/// The pick and place file in the preset's assembly form.
pub fn pos_form(rows: &[Row], f: &AssemblyForm) -> String {
    let mut out = String::new();
    csv_line(&mut out, &f.pos.map(str::to_owned));
    for r in rows {
        csv_line(
            &mut out,
            &[
                r.designator.clone(),
                format!("{}{}", mm(r.at[0]), f.length_suffix),
                format!("{}{}", mm(r.at[1]), f.length_suffix),
                if r.bottom { f.bottom } else { f.top }.to_owned(),
                degrees(r.rotation_mdeg),
            ],
        );
    }
    out
}

/// The design's BOM lines, as `bom` gives them for the working files.
pub fn bom_lines(session: &Session, p: &Project, n: &Netlist) -> Vec<Line> {
    let all: Vec<&Occurrence> = n.occurrences.iter().collect();
    let mut lines = lines_of(p, &all, 1);
    with_properties(session, p, None, &mut lines);
    lines
}

/// The supplier's part number among a line's offers
/// (`Supplier=SKU;…`), the supplier compared ignoring case and spaces.
pub fn sku(offers: &str, supplier: &str) -> String {
    offers
        .split(';')
        .filter_map(|o| o.split_once('='))
        .find(|(s, _)| s.trim().eq_ignore_ascii_case(supplier.trim()))
        .map(|(_, k)| k.trim().to_owned())
        .unwrap_or_default()
}

/// The BOM in the preset's assembly form: a row per line with a
/// populated designator.
pub fn bom_form(lines: &[Line], f: &AssemblyForm) -> String {
    let col = |l: &Line, c: &str| l.columns.get(c).cloned().unwrap_or_default();
    let mut out = String::new();
    csv_line(&mut out, &f.bom.map(str::to_owned));
    for l in lines {
        let refdes = col(l, "refdes");
        if refdes.is_empty() {
            continue;
        }
        csv_line(
            &mut out,
            &[
                col(l, "value"),
                refdes,
                col(l, "footprint"),
                sku(&col(l, "offers"), f.supplier),
            ],
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::preset::JLC_2L;

    fn row(d: &str, x: i64, rot: i64) -> Row {
        Row {
            designator: d.into(),
            value: "10k".into(),
            footprint: "R_0402".into(),
            at: [x, -2_500_000],
            rotation_mdeg: rot,
            bottom: false,
        }
    }

    #[test]
    fn pick_and_place_writes_millimetres_and_degrees() {
        let rows = vec![
            row("R2", 1_500_000, 90_000),
            row("R10", 0, 45_500),
            Row {
                bottom: true,
                ..row("R11", 2_000_000, 90_000)
            },
        ];
        assert_eq!(
            pos_csv(&rows),
            "Designator,Value,Footprint,X,Y,Rotation,Side\n\
             R2,10k,R_0402,1.5,-2.5,90,top\n\
             R10,10k,R_0402,0.0,-2.5,45.5,top\n\
             R11,10k,R_0402,2.0,-2.5,90,bottom\n"
        );
        let f = JLC_2L.assembly.unwrap();
        let text = pos_form(&rows, &f);
        let head = f.pos.join(",");
        assert!(text.starts_with(&format!("{head}\n")), "{text}");
        assert!(
            text.contains(&format!(
                "R2,1.5{s},-2.5{s},{t},90\n",
                s = f.length_suffix,
                t = f.top
            )),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "R11,2.0{s},-2.5{s},{b},90\n",
                s = f.length_suffix,
                b = f.bottom
            )),
            "{text}"
        );
    }

    #[test]
    fn the_form_bom_takes_the_suppliers_part_number() {
        let f = JLC_2L.assembly.unwrap();
        let mut cols = std::collections::BTreeMap::new();
        cols.insert("value".to_owned(), "10k".to_owned());
        cols.insert("refdes".to_owned(), "R1,R2".to_owned());
        cols.insert("footprint".to_owned(), "R_0402".to_owned());
        cols.insert(
            "offers".to_owned(),
            format!("Mouser=m1; {} =C25744", f.supplier.to_lowercase()),
        );
        let line = ryupdm::lines::Line {
            key: "k".into(),
            part: "self/r".into(),
            project: None,
            revision: "working".into(),
            quantity: 2,
            properties: Default::default(),
            defaulted: vec![],
            columns: cols.clone(),
        };
        let mut dnp_only = line.clone();
        dnp_only.columns.insert("refdes".into(), String::new());
        let text = bom_form(&[line, dnp_only], &f);
        assert_eq!(
            text,
            format!("{}\n10k,\"R1,R2\",R_0402,C25744\n", f.bom.join(","))
        );
        assert_eq!(sku("", "x"), "");
    }

    #[test]
    fn placed_populated_occurrences_are_rows_and_the_rest_left_out() {
        let (_dir, p, id) = crate::board::shapes::tests::placed();
        let b = &p.boards[&id];
        let design = &b.board.item.as_ref().unwrap().design;
        let (_, occs) = crate::board::occ::occurrences(&p, design).unwrap();
        let (rows, left) = rows(&occs, b);
        let with_footprint = occs.iter().filter(|o| o.footprint.is_some()).count();
        assert_eq!(rows.len() + left.len(), with_footprint, "{left:?}");
        let mut sorted = rows.clone();
        sorted.sort_by_key(|r| designator_key(&r.designator));
        assert_eq!(rows, sorted);
    }
}
