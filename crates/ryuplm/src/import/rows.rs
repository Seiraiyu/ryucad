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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub at: u64,
    pub cells: Vec<String>,
}

/// Why a source, a row or a cell doesn't read: where, and why. `row` 0 is
/// the file or the mapping as a whole.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub row: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    pub message: String,
}

impl Refusal {
    pub fn file(message: impl Into<String>) -> Refusal {
        Refusal {
            row: 0,
            column: None,
            message: message.into(),
        }
    }

    pub fn at(row: u64, column: Option<&str>, message: impl Into<String>) -> Refusal {
        Refusal {
            row,
            column: column.map(str::to_owned),
            message: message.into(),
        }
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
            rows.push(Row { at, cells });
        }
        Ok(Table { columns, rows })
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
}
