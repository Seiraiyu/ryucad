//! The row model every reader yields (design 734): column names from the
//! header row, and below it each row's cells exactly as the source wrote
//! them, with the row's number in the source. A [`Record`] is a row seen
//! through a mapping.

use serde::Serialize;
use std::collections::BTreeMap;

/// A row as a reader gives it: its number in the source (the line a CSV
/// record starts on, an XLSX row's number) and its cells.
pub type Raw = (u64, Vec<String>);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
    /// Whether the source counts date serials from 1904 (an XLSX
    /// workbook's `date1904`); false for every other reader.
    pub date1904: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub at: u64,
    pub cells: Vec<String>,
    /// Where a tree reader found the row: its record's JSON Pointer, its
    /// JSON Lines line, or its element's line and column (design 935).
    /// `None` for CSV and XLSX, whose row number says where.
    pub place: Option<String>,
}

/// Why a source, a row or a cell doesn't read: where, and why. `row` 0 is
/// the file or the mapping as a whole; `place` is a tree reader's place
/// for the row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub row: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    pub message: String,
}

impl Refusal {
    pub fn file(message: impl Into<String>) -> Refusal {
        Refusal {
            row: 0,
            column: None,
            place: None,
            message: message.into(),
        }
    }

    pub fn at(row: u64, column: Option<&str>, message: impl Into<String>) -> Refusal {
        Refusal {
            row,
            column: column.map(str::to_owned),
            place: None,
            message: message.into(),
        }
    }

    /// This refusal with `place`.
    pub fn placed(mut self, place: Option<&str>) -> Refusal {
        self.place = place.map(str::to_owned);
        self
    }
}

impl From<Refusal> for crate::error::Error {
    fn from(r: Refusal) -> Self {
        let mut e =
            crate::error::Error::new(crate::error::Code::InvalidDocument, r.message.clone())
                .with("row", r.row);
        if let Some(c) = &r.column {
            e = e.with("column", c);
        }
        if let Some(p) = &r.place {
            e = e.with("place", p);
        }
        e
    }
}

/// One record: a row through a mapping. `cells` holds every non-empty
/// cell by column name, as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub at: u64,
    pub key: String,
    pub cells: BTreeMap<String, String>,
    pub parent: Option<String>,
    pub quantity: Option<String>,
    pub find: Option<String>,
    pub refdes: Option<String>,
}

/// One record from a tree reader: where it is, and its leaf values by
/// path relative to the record, in the order the record holds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaves {
    pub place: String,
    pub cells: Vec<(String, String)>,
}

impl Table {
    /// The table of `raw` with its header at source row `header` (1-based):
    /// rows above it skipped, empty rows skipped, short rows padded, blank
    /// names `#<n>`, repeated names ` #2`, ` #3`.
    pub fn from_raw(raw: Vec<Raw>, header: u64) -> Result<Table, Refusal> {
        if raw.len() > super::MAX_ROWS {
            return Err(Refusal::file("more than 2,000,000 rows"));
        }
        let mut it = raw.into_iter().skip_while(|(at, _)| *at < header);
        let Some((at, names)) = it.next() else {
            return Err(Refusal::file(format!("no header row at row {header}")));
        };
        if at != header {
            return Err(Refusal::file(format!(
                "row {header}, the header row, is empty"
            )));
        }
        if names.len() > super::MAX_COLUMNS {
            return Err(Refusal::at(at, None, "more than 16,384 columns"));
        }
        let mut columns: Vec<String> = Vec::with_capacity(names.len());
        for (i, n) in names.iter().enumerate() {
            let base = if n.trim().is_empty() {
                format!("#{}", i + 1)
            } else {
                n.trim().to_owned()
            };
            let mut name = base.clone();
            let mut k = 2;
            while columns.contains(&name) {
                name = format!("{base} #{k}");
                k += 1;
            }
            columns.push(name);
        }
        let width = columns.len();
        let mut rows = Vec::new();
        for (at, mut cells) in it {
            if cells.iter().all(|c| c.trim().is_empty()) {
                continue;
            }
            if cells.len() > width {
                let extra = cells[width..].iter().any(|c| !c.trim().is_empty());
                if extra {
                    return Err(Refusal::at(
                        at,
                        None,
                        format!(
                            "{} cells, but the header names {width} columns",
                            cells.len()
                        ),
                    ));
                }
                cells.truncate(width);
            }
            cells.resize(width, String::new());
            rows.push(Row {
                at,
                cells,
                place: None,
            });
        }
        Ok(Table {
            columns,
            rows,
            date1904: false,
        })
    }

    /// The table of a tree reader's records (design 935): every path any
    /// record has, in the order first seen, is a column; each record is a
    /// row numbered from 1 in document order, its missing paths empty.
    pub fn from_records(records: Vec<Leaves>) -> Result<Table, Refusal> {
        if records.len() > super::MAX_ROWS {
            return Err(Refusal::file("more than 2,000,000 records"));
        }
        let mut columns: Vec<String> = Vec::new();
        let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut rows = Vec::with_capacity(records.len());
        // The last row each column was given in: a path twice in one
        // record (two XML attributes sharing a local name) is refused, not
        // kept as whichever came last.
        let mut given: Vec<u64> = Vec::new();
        for (i, rec) in records.into_iter().enumerate() {
            let at = i as u64 + 1;
            let mut cells = vec![String::new(); columns.len()];
            for (path, value) in rec.cells {
                let c = match index.get(&path) {
                    Some(&c) => c,
                    None => {
                        if columns.len() == super::MAX_COLUMNS {
                            return Err(Refusal::at(at, None, "more than 16,384 columns")
                                .placed(Some(rec.place.as_str())));
                        }
                        index.insert(path.clone(), columns.len());
                        columns.push(path);
                        cells.push(String::new());
                        given.push(0);
                        columns.len() - 1
                    }
                };
                if given[c] == at {
                    return Err(Refusal::at(
                        at,
                        Some(columns[c].as_str()),
                        format!(
                            "{} is in this record twice (names are local: two attributes sharing a local name?)",
                            columns[c]
                        ),
                    )
                    .placed(Some(rec.place.as_str())));
                }
                given[c] = at;
                cells[c] = value;
            }
            rows.push(Row {
                at,
                cells,
                place: Some(rec.place),
            });
        }
        let width = columns.len();
        for r in &mut rows {
            r.cells.resize(width, String::new());
        }
        Ok(Table {
            columns,
            rows,
            date1904: false,
        })
    }

    /// Each refusal at a row given that row's place, when its reader gave
    /// one (`import::prepare` calls this on `records::map`'s refusals).
    pub fn place(&self, refusals: &mut [Refusal]) {
        for r in refusals
            .iter_mut()
            .filter(|r| r.row > 0 && r.place.is_none())
        {
            if let Ok(i) = self.rows.binary_search_by_key(&r.row, |x| x.at) {
                r.place.clone_from(&self.rows[i].place);
            }
        }
    }

    /// The index of column `name`.
    pub fn column(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c == name)
    }

    /// Row `r`'s non-empty cells by column name, as read.
    pub fn cells(&self, r: &Row) -> BTreeMap<String, String> {
        self.columns
            .iter()
            .zip(&r.cells)
            .filter(|(_, v)| !v.is_empty())
            .map(|(c, v)| (c.clone(), v.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(rows: &[(u64, &[&str])]) -> Vec<Raw> {
        rows.iter()
            .map(|(at, c)| (*at, c.iter().map(|s| (*s).to_owned()).collect()))
            .collect()
    }

    #[test]
    fn the_header_names_the_columns() {
        let t = Table::from_raw(
            raw(&[
                (1, &["Exported by us"]),
                (2, &["Part", "", "Qty", "Part"]),
                (3, &["A-1", "x", "2"]),
                (4, &["", "", "", ""]),
                (5, &["A-2", "", "1", "y", ""]),
            ]),
            2,
        )
        .unwrap();
        assert_eq!(t.columns, ["Part", "#2", "Qty", "Part #2"]);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0].at, 3);
        assert_eq!(t.rows[0].cells, ["A-1", "x", "2", ""]);
        assert_eq!(t.rows[1].cells[3], "y");
        let c = t.cells(&t.rows[1]);
        assert_eq!(c.get("Qty").map(String::as_str), Some("1"));
        assert!(!c.contains_key("#2"));
    }

    #[test]
    fn a_row_wider_than_the_header_is_refused() {
        let e = Table::from_raw(raw(&[(1, &["A"]), (2, &["x", "y"])]), 1).unwrap_err();
        assert_eq!(e.row, 2);
        let e = Table::from_raw(raw(&[(1, &["A"])]), 3).unwrap_err();
        assert_eq!(e.row, 0);
    }

    #[test]
    fn records_make_a_table_of_their_paths() {
        let t = Table::from_records(vec![
            Leaves {
                place: "/items/0".into(),
                cells: vec![("/pn".into(), "A".into()), ("/qty".into(), "2".into())],
            },
            Leaves {
                place: "/items/1".into(),
                cells: vec![
                    ("/mfr/name".into(), "Acme".into()),
                    ("/pn".into(), "B".into()),
                ],
            },
        ])
        .unwrap();
        assert_eq!(t.columns, ["/pn", "/qty", "/mfr/name"]);
        assert_eq!(t.rows[0].cells, ["A", "2", ""]);
        assert_eq!(t.rows[1].cells, ["B", "", "Acme"]);
        assert_eq!(
            (t.rows[1].at, t.rows[1].place.as_deref()),
            (2, Some("/items/1"))
        );
        let mut r = vec![Refusal::at(2, Some("/pn"), "bad"), Refusal::file("whole")];
        t.place(&mut r);
        assert_eq!(r[0].place.as_deref(), Some("/items/1"));
        assert_eq!(r[1].place, None);
        let e: crate::error::Error = r.remove(0).into();
        assert_eq!(e.details["place"], "/items/1");
        // CSV rows have no place, and their refusals serialize as before.
        let csv = Table::from_raw(raw(&[(1, &["A"]), (2, &["x"])]), 1).unwrap();
        assert_eq!(csv.rows[0].place, None);
        assert_eq!(
            serde_json::to_string(&Refusal::at(2, None, "m")).unwrap(),
            r#"{"row":2,"message":"m"}"#
        );
    }
}
