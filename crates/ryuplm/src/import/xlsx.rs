//! The XLSX reader (design 734): one sheet, chosen by name, read in pure
//! Rust. The zip through ryupdm's bounded reader, the XML through
//! roxmltree; cells as the text a person sees stored (Decision 3). Elements are
//! matched by local name, so transitional and strict OOXML both read;
//! roxmltree refuses DTDs, so no entity can expand.

use super::rows::{Raw, Refusal};
use std::collections::BTreeMap;

const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn parts(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, Refusal> {
    ryupdm::package::zip::read(bytes, super::MAX_SOURCE, &[])
        .map(|v| v.into_iter().collect())
        .map_err(|e| Refusal::file(format!("not an XLSX file: {}", zip_why(&e))))
}

fn zip_why(e: &ryupdm::package::zip::Unread) -> String {
    use ryupdm::package::zip::Unread;
    match e {
        Unread::Malformed(w) => w.clone(),
        Unread::TooLarge(n) => format!("{n} is larger than 256 MiB unpacked"),
        Unread::Duplicate(n) => format!("{n} is in it twice"),
        Unread::Crc(n) => format!("{n}'s checksum doesn't match"),
    }
}

fn xml<'a>(parts: &'a BTreeMap<String, Vec<u8>>, name: &str) -> Result<Option<&'a str>, Refusal> {
    parts
        .get(name)
        .map(|b| std::str::from_utf8(b).map_err(|_| Refusal::file(format!("{name} isn't UTF-8"))))
        .transpose()
}

fn doc<'a>(text: &'a str, name: &str) -> Result<roxmltree::Document<'a>, Refusal> {
    roxmltree::Document::parse(text).map_err(|e| Refusal::file(format!("{name} isn't XML: {e}")))
}

/// Every sheet's name and its part, in the workbook's order.
fn sheet_parts(parts: &BTreeMap<String, Vec<u8>>) -> Result<Vec<(String, String)>, Refusal> {
    let wb = xml(parts, "xl/workbook.xml")?
        .ok_or_else(|| Refusal::file("not an XLSX file: no xl/workbook.xml"))?;
    let wb = doc(wb, "xl/workbook.xml")?;
    let rels = match xml(parts, "xl/_rels/workbook.xml.rels")? {
        Some(t) => doc(t, "xl/_rels/workbook.xml.rels")?
            .descendants()
            .filter(|n| n.tag_name().name() == "Relationship")
            .filter_map(|n| {
                Some((
                    n.attribute("Id")?.to_owned(),
                    n.attribute("Target")?.to_owned(),
                ))
            })
            .collect::<BTreeMap<_, _>>(),
        None => BTreeMap::new(),
    };
    let mut out = Vec::new();
    for s in wb
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "sheet")
    {
        let name = s.attribute("name").unwrap_or_default().to_owned();
        let rid = s.attribute((REL, "id")).unwrap_or_default();
        let target = rels.get(rid).cloned().unwrap_or_default();
        let path = match target.strip_prefix('/') {
            Some(abs) => abs.to_owned(),
            None => format!("xl/{target}"),
        };
        out.push((name, path));
    }
    Ok(out)
}

/// The workbook's sheet names, in order.
pub fn sheets(bytes: &[u8]) -> Result<Vec<String>, Refusal> {
    Ok(sheet_parts(&parts(bytes)?)?
        .into_iter()
        .map(|(n, _)| n)
        .collect())
}

/// The text of an `<si>` or `<is>`: its `<t>`, or its runs' `<t>`s joined.
fn string_text(n: roxmltree::Node) -> String {
    n.descendants()
        .filter(|d| d.is_element() && d.tag_name().name() == "t")
        .filter(|t| !t.ancestors().any(|a| a.tag_name().name() == "rPh"))
        .filter_map(|t| t.text())
        .collect()
}

/// The 0-based column of a reference like `AB12`.
fn column_of(r: &str) -> Option<usize> {
    let letters: String = r.chars().take_while(char::is_ascii_alphabetic).collect();
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let n = letters.bytes().fold(0usize, |n, b| {
        n * 26 + usize::from(b.to_ascii_uppercase() - b'A' + 1)
    });
    Some(n - 1)
}

/// Whether the workbook counts its date serials from 1904
/// (`<workbookPr date1904="1"/>`, as older Mac workbooks do).
fn date1904(parts: &BTreeMap<String, Vec<u8>>) -> Result<bool, Refusal> {
    let Some(wb) = xml(parts, "xl/workbook.xml")? else {
        return Ok(false);
    };
    Ok(doc(wb, "xl/workbook.xml")?
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "workbookPr")
        .any(|n| matches!(n.attribute("date1904"), Some("1" | "true"))))
}

/// The rows of sheet `sheet` (the first when `None`), each with its row
/// number.
pub fn read(bytes: &[u8], sheet: Option<&str>) -> Result<Vec<Raw>, Refusal> {
    read_book(bytes, sheet).map(|(rows, _)| rows)
}

/// The rows of sheet `sheet` (the first when `None`), each with its row
/// number, and whether the workbook counts its dates from 1904.
pub fn read_book(bytes: &[u8], sheet: Option<&str>) -> Result<(Vec<Raw>, bool), Refusal> {
    let parts = parts(bytes)?;
    let d1904 = date1904(&parts)?;
    let list = sheet_parts(&parts)?;
    let (name, path) = match sheet {
        Some(s) => list.iter().find(|(n, _)| n == s).ok_or_else(|| {
            Refusal::file(format!(
                "no sheet {s:?}; the sheets are {}",
                list.iter()
                    .map(|(n, _)| format!("{n:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?,
        None => list
            .first()
            .ok_or_else(|| Refusal::file("the workbook has no sheets"))?,
    };
    let shared: Vec<String> = match xml(&parts, "xl/sharedStrings.xml")? {
        Some(t) => doc(t, "xl/sharedStrings.xml")?
            .root_element()
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "si")
            .map(string_text)
            .collect(),
        None => Vec::new(),
    };
    let text = xml(&parts, path)?
        .ok_or_else(|| Refusal::file(format!("sheet {name:?}'s part {path} is missing")))?;
    let d = doc(text, path)?;
    let mut out: Vec<Raw> = Vec::new();
    let mut next_row = 1u64;
    for row in d
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "row")
    {
        let at = row
            .attribute("r")
            .and_then(|r| r.parse().ok())
            .unwrap_or(next_row);
        next_row = at + 1;
        let mut cells: Vec<String> = Vec::new();
        let mut next_col = 0usize;
        for c in row
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "c")
        {
            let col = c.attribute("r").and_then(column_of).unwrap_or(next_col);
            next_col = col + 1;
            if col >= super::MAX_COLUMNS {
                return Err(Refusal::at(at, None, "a cell past column 16,384"));
            }
            let v = c
                .children()
                .find(|n| n.tag_name().name() == "v")
                .and_then(|v| v.text())
                .unwrap_or("");
            let value = match c.attribute("t").unwrap_or("n") {
                "s" => {
                    let i: usize = v.trim().parse().map_err(|_| {
                        Refusal::at(at, None, "a shared string's index isn't a number")
                    })?;
                    shared.get(i).cloned().ok_or_else(|| {
                        Refusal::at(
                            at,
                            None,
                            format!("shared string {i} isn't in sharedStrings.xml"),
                        )
                    })?
                }
                "inlineStr" => c
                    .children()
                    .find(|n| n.tag_name().name() == "is")
                    .map(string_text)
                    .unwrap_or_default(),
                "b" => {
                    if v.trim() == "1" {
                        "TRUE".into()
                    } else {
                        "FALSE".into()
                    }
                }
                _ => v.to_owned(),
            };
            if cells.len() <= col {
                cells.resize(col + 1, String::new());
            }
            cells[col] = value;
        }
        out.push((at, cells));
        if out.len() > super::MAX_ROWS {
            return Err(Refusal::file("more than 2,000,000 rows"));
        }
    }
    Ok((out, d1904))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Cell, xlsx};

    #[test]
    fn a_sheet_reads_by_name_with_every_kind_of_cell() {
        let book = xlsx(&[
            ("Notes", vec![vec![Cell::Str("ignore me")]]),
            (
                "Items",
                vec![
                    vec![Cell::Str("Part"), Cell::Str("Qty"), Cell::Str("Ok")],
                    vec![Cell::Str("A-1"), Cell::Num("4"), Cell::Bool(true)],
                    vec![
                        Cell::Inline("Größe M3"),
                        Cell::Num("1E-3"),
                        Cell::Bool(false),
                    ],
                    vec![],
                    vec![Cell::Skip, Cell::Skip, Cell::Str("only C")],
                ],
            ),
        ]);
        assert_eq!(sheets(&book).unwrap(), ["Notes", "Items"]);
        let rows = read(&book, Some("Items")).unwrap();
        let text: Vec<(u64, Vec<&str>)> = rows
            .iter()
            .map(|(a, c)| (*a, c.iter().map(String::as_str).collect()))
            .collect();
        assert_eq!(
            text,
            [
                (1, vec!["Part", "Qty", "Ok"]),
                (2, vec!["A-1", "4", "TRUE"]),
                (3, vec!["Größe M3", "1E-3", "FALSE"]),
                (5, vec!["", "", "only C"]),
            ]
        );
        assert_eq!(read(&book, None).unwrap()[0].1, ["ignore me"]);
        let e = read(&book, Some("Nope")).unwrap_err();
        assert!(e.message.contains("\"Items\""), "{}", e.message);
    }

    #[test]
    fn not_a_workbook_is_refused() {
        assert!(read(b"PK nonsense", None).is_err());
        let z = ryupdm::package::zip::write(&[("a.txt", b"x".as_slice())]).unwrap();
        assert!(read(&z, None).unwrap_err().message.contains("workbook.xml"));
        assert_eq!(column_of("AB12"), Some(27));
        assert_eq!(column_of("A1"), Some(0));
        assert_eq!(column_of("12"), None);
    }

    #[test]
    fn a_workbook_may_count_dates_from_1904() {
        let book = xlsx(&[("S", vec![vec![Cell::Str("A")]])]);
        assert!(!read_book(&book, None).unwrap().1);
        let mut parts: BTreeMap<String, Vec<u8>> = ryupdm::package::zip::read(&book, 1 << 24, &[])
            .unwrap()
            .into_iter()
            .collect();
        let wb = String::from_utf8(parts["xl/workbook.xml"].clone()).unwrap();
        let wb = wb.replacen("<sheets>", "<workbookPr date1904=\"1\"/><sheets>", 1);
        parts.insert("xl/workbook.xml".into(), wb.into_bytes());
        let list: Vec<(&str, &[u8])> = parts
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect();
        let z = ryupdm::package::zip::write(&list).unwrap();
        let (rows, d1904) = read_book(&z, None).unwrap();
        assert!(d1904);
        assert_eq!(rows[0].1, ["A"]);
        let reader = crate::import::mapping::Reader {
            kind: crate::import::mapping::ReaderKind::Xlsx,
            file: None,
            encoding: None,
            delimiter: None,
            sheet: None,
            header_row: 1,
            records: None,
            children: None,
            lines: false,
        };
        assert!(crate::import::read_table(&z, &reader).unwrap().date1904);
        assert!(!crate::import::read_table(&book, &reader).unwrap().date1904);
    }
}
