//! The templates ryucad compiles in (design 417): `asme-a` to `asme-e`,
//! ASME Y14.1 borders, zones and title block, and `iso-a4` to `iso-a0`,
//! ISO 5457 borders and zones with an ISO 7200 title block. Each is made
//! for the sheet it is used on, in either orientation, from its paper
//! alone: no file, so a drawing builds with nothing but `ryucad`.

use crate::doc::drawing::{Orientation, Size};
use crate::doc::drawing_template::{Anchor, Part, Template, Text, builtin, parse_fields};
use crate::error::{Code, Error, Result};

/// The text size of a title block's labels, and of its values.
const LABEL: f64 = 2.0;
const VALUE: f64 = 3.5;

/// The template `id` for a sheet of `size` turned `orientation`: a
/// built-in one made for it, else the project's file (`project`). A
/// project template drawn for another size or orientation is used as it
/// is (its geometry is the project's to keep right).
pub fn resolve(
    id: &str,
    size: Size,
    orientation: Orientation,
    project: Option<&Result<Template>>,
) -> Result<Template> {
    if builtin(id) {
        return Ok(compiled(id, size, orientation));
    }
    match project {
        Some(Ok(t)) => Ok(t.clone()),
        Some(Err(e)) => Err(e.clone()),
        None => Err(Error::new(
            Code::TemplateNotFound,
            format!("template {id} is neither built in nor in templates/"),
        )
        .with("template", id)),
    }
}

fn text(at: [f64; 2], size: f64, anchor: Anchor, s: &str) -> Text {
    Text {
        at,
        size,
        anchor,
        text: s.to_owned(),
        pieces: parse_fields(s).unwrap_or_else(|_| vec![Part::Literal(s.to_owned())]),
    }
}

fn rect(lo: [f64; 2], hi: [f64; 2]) -> Vec<[f64; 2]> {
    vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]], lo]
}

/// A title block's cells: its rows from the bottom, each a height and
/// cells of a width, a label and a value's text.
type Rows<'a> = &'a [(f64, &'a [(f64, &'a str, &'a str)])];

/// Draws `rows` as a block whose lower right corner is `corner`: its
/// frame thick, its cells thin, each label at its cell's upper left and
/// its value under it. The block's box.
fn block(t: &mut Template, corner: [f64; 2], width: f64, rows: Rows) -> [[f64; 2]; 2] {
    let height: f64 = rows.iter().map(|r| r.0).sum();
    let lo = [corner[0] - width, corner[1]];
    let hi = [corner[0], corner[1] + height];
    t.lines.push(rect(lo, hi));
    let mut y = corner[1];
    for (k, (h, cells)) in rows.iter().enumerate() {
        if k > 0 {
            t.thin.push(vec![[lo[0], y], [hi[0], y]]);
        }
        let mut x = lo[0];
        for (i, (w, label, value)) in cells.iter().enumerate() {
            if i > 0 {
                t.thin.push(vec![[x, y], [x, y + h]]);
            }
            t.texts.push(text(
                [x + 1.5, y + h - 1.5 - LABEL],
                LABEL,
                Anchor::Start,
                label,
            ));
            let size = if *h < 9.0 { LABEL * 1.25 } else { VALUE };
            t.texts
                .push(text([x + 1.5, y + 2.0], size, Anchor::Start, value));
            x += w;
        }
        y += h;
    }
    [lo, hi]
}

/// Zone marks round the border: columns numbered from the right and rows
/// lettered from the bottom (ASME Y14.1) or from the top (ISO 5457), each
/// zone about `zone` wide, marked in the margin between the trimmed edge
/// and the border.
fn zones(t: &mut Template, paper: [f64; 2], border: [[f64; 2]; 2], zone: f64, iso: bool) {
    let [lo, hi] = border;
    let w = hi[0] - lo[0];
    let h = hi[1] - lo[1];
    let cols = ((w / zone).round() as usize).max(2);
    let rows = ((h / zone).round() as usize).max(2);
    let letters = "ABCDEFGHJKLMNPRSTUVWXYZ".as_bytes();
    for i in 0..=cols {
        let x = lo[0] + w * i as f64 / cols as f64;
        if i > 0 && i < cols {
            t.thin.push(vec![[x, lo[1]], [x, lo[1] - 3.0]]);
            t.thin.push(vec![[x, hi[1]], [x, hi[1] + 3.0]]);
        }
        if i < cols {
            let mid = x + w / cols as f64 / 2.0;
            let n = if iso { i + 1 } else { cols - i };
            for y in [lo[1] / 2.0 - 1.2, (hi[1] + paper[1]) / 2.0 - 1.2] {
                t.texts
                    .push(text([mid, y], 2.5, Anchor::Middle, &n.to_string()));
            }
        }
    }
    for j in 0..=rows {
        let y = lo[1] + h * j as f64 / rows as f64;
        if j > 0 && j < rows {
            t.thin.push(vec![[lo[0], y], [lo[0] - 3.0, y]]);
            t.thin.push(vec![[hi[0], y], [hi[0] + 3.0, y]]);
        }
        if j < rows {
            let mid = y + h / rows as f64 / 2.0 - 1.2;
            let k = if iso { rows - 1 - j } else { j };
            let l = (letters[k % letters.len()] as char).to_string();
            for x in [lo[0] / 2.0, (hi[0] + paper[0]) / 2.0] {
                t.texts.push(text([x, mid], 2.5, Anchor::Middle, &l));
            }
        }
    }
}

/// The built-in template `id` (one of [`builtin`]'s) made for a sheet of
/// `size` turned `orientation`.
pub fn compiled(id: &str, size: Size, orientation: Orientation) -> Template {
    let paper = size.paper(orientation);
    let iso = id.starts_with("iso-");
    let mut t = Template {
        size,
        orientation,
        area: [[0.0, 0.0], paper],
        keep_out: Vec::new(),
        lines: Vec::new(),
        thin: Vec::new(),
        texts: Vec::new(),
    };
    // Margins: ASME Y14.1's half inch on C and larger, 10 mm on A and B;
    // ISO 5457's 20 mm filing margin on the left and 10 mm elsewhere.
    let (left, other) = if iso {
        (20.0, 10.0)
    } else if matches!(size, Size::AnsiA | Size::AnsiB) {
        (10.0, 10.0)
    } else {
        (12.7, 12.7)
    };
    let lo = [left, other];
    let hi = [paper[0] - other, paper[1] - other];
    t.lines.push(rect(lo, hi));
    // Zones: none on an A (ASME) or A4 (ISO) sheet, else about 50 mm.
    if !matches!(size, Size::AnsiA | Size::IsoA4) {
        zones(
            &mut t,
            paper,
            [lo, hi],
            if iso { 50.0 } else { 2.0 * 25.4 * 2.0 },
            iso,
        );
    }
    let corner = [hi[0], lo[1]];
    let keep = if iso {
        // ISO 7200: 180 mm wide, its fields in two rows of cells.
        block(
            &mut t,
            corner,
            180.0,
            &[
                (
                    9.0,
                    &[
                        (40.0, "Created by", "{drawn_by}"),
                        (40.0, "Approved by", "{approved_by}"),
                        (40.0, "Date of issue", "{date}"),
                        (30.0, "Scale", "{scale}"),
                        (30.0, "Sheet", "{sheet}/{sheets}"),
                    ],
                ),
                (
                    9.0,
                    &[
                        (40.0, "Material", "{material}"),
                        (40.0, "Standard", "{standard}"),
                        (60.0, "Document number", "{number}"),
                        (40.0, "Revision", "{revision}"),
                    ],
                ),
                (
                    14.0,
                    &[(120.0, "Title", "{name}"), (60.0, "Owner", "{owner}")],
                ),
            ],
        )
    } else {
        // ASME Y14.1: 178 mm (7 in) wide on every size here.
        block(
            &mut t,
            corner,
            177.8,
            &[
                (
                    8.0,
                    &[
                        (50.0, "SCALE", "{scale}"),
                        (50.0, "STANDARD", "{standard}"),
                        (77.8, "SHEET", "{sheet} OF {sheets}"),
                    ],
                ),
                (
                    10.0,
                    &[
                        (20.0, "SIZE", "{size}"),
                        (117.8, "DWG NO", "{number}"),
                        (40.0, "REV", "{revision}"),
                    ],
                ),
                (
                    10.0,
                    &[
                        (60.0, "DRAWN", "{drawn_by}"),
                        (40.0, "DATE", "{date}"),
                        (77.8, "MATERIAL", "{material}"),
                    ],
                ),
                (14.0, &[(177.8, "TITLE", "{name}")]),
            ],
        )
    };
    t.area = [lo, hi];
    t.keep_out.push(keep);
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::drawing_template::BUILTIN;

    #[test]
    fn every_built_in_template_fits_its_paper_and_reads_back() {
        for id in BUILTIN {
            for o in [Orientation::Landscape, Orientation::Portrait] {
                let size = match id.strip_prefix("iso-") {
                    Some(n) => Size::parse(&format!("iso_{n}")).unwrap(),
                    None => Size::parse(&format!("ansi_{}", &id[5..])).unwrap(),
                };
                let t = compiled(id, size, o);
                let paper = size.paper(o);
                let inside = |p: [f64; 2]| {
                    p[0] >= 0.0 && p[1] >= 0.0 && p[0] <= paper[0] && p[1] <= paper[1]
                };
                for l in t.lines.iter().chain(&t.thin) {
                    assert!(l.iter().all(|p| inside(*p)), "{id} {o:?}");
                }
                for x in &t.texts {
                    assert!(inside(x.at), "{id} {o:?} {:?}", x.text);
                }
                // The title block lies in the area, in its lower right.
                let [lo, hi] = t.area;
                let k = t.keep_out[0];
                assert!(k[0][0] >= lo[0] && k[1][0] <= hi[0] + 1e-9 && k[0][1] >= lo[1] - 1e-9);
                assert!((k[1][0] - hi[0]).abs() < 1e-9 && (k[0][1] - lo[1]).abs() < 1e-9);
                // It is a template a project could write.
                let again = Template::from_json(&t.to_json()).unwrap();
                assert_eq!(again.lines, t.lines, "{id}");
                assert_eq!(again.texts.len(), t.texts.len());
            }
        }
    }

    #[test]
    fn a_template_neither_built_in_nor_in_the_project_is_not_found() {
        let e = resolve("ours", Size::AnsiB, Orientation::Landscape, None).unwrap_err();
        assert_eq!(e.code, Code::TemplateNotFound);
        let t = resolve("asme-b", Size::AnsiB, Orientation::Landscape, None).unwrap();
        assert!(t.texts.iter().any(|x| x.text == "{number}"));
        let iso = resolve("iso-a3", Size::IsoA3, Orientation::Landscape, None).unwrap();
        assert!(iso.texts.iter().any(|x| x.text == "Document number"));
        assert!((iso.area[0][0] - 20.0).abs() < 1e-12);
    }
}
