//! The BOM tab: the bill of materials of the part the window shows, as
//! `bom.get` gives it (ryupdm reads it from the files, so a model whose
//! features fail still has one), indented or flat, its columns the CSV's.
//! Each line knows the occurrences that make its count, so choosing it
//! selects and frames them.
//!
//! A line's item number is its parent assembly's, edited in place and
//! saved with `bom.renumber` against the assembly's revision, as an agent
//! renumbers; a stale save conflicts, and the tab reads the BOM again and
//! says what changed rather than overwriting it.
//!
//! What the tab knows lives here, apart from egui, so the panel that
//! draws it (`ui::bom`) can move into a tile of its own unchanged.

use ryucad::error::Error;
use serde_json::Value;

/// One line of the tab: a row of the indented BOM below its root, or of
/// the flat BOM.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Levels below the root: 1 for the root's children; 0 in the flat
    /// BOM.
    pub depth: usize,
    pub part: String,
    /// `working` or a label.
    pub rev: String,
    /// The assembly whose `part.json` gives this line its item number,
    /// and the revision it is read at (`working` or a label); none in
    /// the flat BOM.
    pub parent: Option<(String, String)>,
    /// Its item number in its parent (tree lines only).
    pub item: Option<u64>,
    /// One text per column, in the answer's `columns` order.
    pub cells: Vec<String>,
    /// Why it can't be read, or what it places that can't.
    pub problem: Option<String>,
    /// It lies inside an assembly bought whole.
    pub bought: bool,
    /// The occurrences that make its count, as references
    /// (`<root>/<instance>/…`).
    pub occurrences: Vec<String>,
}

/// An item number being typed: the assembly that gives it, the part it
/// numbers, its number when the edit started, and the text typed.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemEdit {
    pub parent: String,
    pub part: String,
    pub was: Option<u64>,
    pub text: String,
}

/// Export CSV…, while open: the BOM as the tab shows it (indented or
/// flat), written where `path` says from `bom.get`'s CSV, the same bytes
/// `ryucad bom --out` writes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BomExport {
    pub flat: bool,
    /// Where to write it, as typed or picked.
    pub path: String,
    /// The file written and how many bytes, once it has been.
    pub done: Option<(String, usize)>,
    pub error: Option<Error>,
    pub waiting: bool,
}

/// The largest item number (`files::MAX_ITEM`).
pub const MAX_ITEM: u64 = 99_999;

/// An item number as typed: a whole number from 1 to [`MAX_ITEM`].
pub fn item_number(text: &str) -> Result<u64, String> {
    match text.trim().parse::<u64>() {
        Ok(n) if (1..=MAX_ITEM).contains(&n) => Ok(n),
        _ => Err(format!(
            "an item number is a whole number from 1 to {MAX_ITEM}, not {:?}",
            text.trim()
        )),
    }
}

/// What the tab knows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BomTab {
    /// Flat, not indented.
    pub flat: bool,
    /// The part whose BOM it is, once read.
    pub root: Option<String>,
    /// The CSV's columns, as the answer names them.
    pub columns: Vec<String>,
    /// The indented BOM's lines, then (when `flat`) the flat BOM's.
    pub tree: Vec<Line>,
    pub flat_lines: Vec<Line>,
    /// Findings about the BOM, for people (`warning bom_item_missing: …`).
    pub findings: Vec<String>,
    /// The roots there are to choose from, when the window shows several
    /// parts and the project has no one root assembly.
    pub roots: Vec<String>,
    /// Why the last reading failed.
    pub error: Option<Error>,
    /// The line chosen, by its occurrences: they are this window's
    /// selection.
    pub chosen: Option<Vec<String>>,
    /// The item number being typed, and whether its `bom.renumber` is on
    /// its way.
    pub editing: Option<ItemEdit>,
    pub saving: bool,
    /// A renumbering that conflicted, until the BOM is read again; then
    /// what changed, for people.
    pub conflict: Option<ItemEdit>,
    pub changed: Option<String>,
    /// Why the last renumbering was refused, other than a conflict.
    pub edit_error: Option<String>,
}

/// A JSON value as a cell shows it: text as written, nothing for null.
fn text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

/// `"working"` or `{"label": L}` as people read it.
pub fn rev_text(v: &Value) -> String {
    v.get("label")
        .and_then(Value::as_str)
        .or(v.as_str())
        .unwrap_or("working")
        .to_owned()
}

/// A quantity as the CSV gives it: the count of a part counted `each`,
/// else its amount in the unit, else the count with no amount.
fn quantity(q: &Value) -> String {
    match q.get("text").and_then(Value::as_str) {
        Some(t) if q["uom"] != "each" => t.to_owned(),
        _ => text(&q["count"]),
    }
}

/// The cell of `column` for a row.
fn cell(row: &Value, column: &str, level: &str, item: &str) -> String {
    match column {
        "level" => level.to_owned(),
        "item" => item.to_owned(),
        "part" => text(&row["part"]),
        "revision" => rev_text(&row["rev"]),
        "quantity" => quantity(&row["quantity"]),
        "uom" => text(&row["uom"]),
        "make_buy" => text(&row["make_buy"]),
        other => text(&row["properties"][other]),
    }
}

/// A problem as people read it: its message.
fn problem(row: &Value) -> Option<String> {
    let own = row["problem"]["message"].as_str().map(str::to_owned);
    let placed = row["problems"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["message"].as_str())
        .collect::<Vec<_>>();
    match (own, placed.is_empty()) {
        (Some(p), _) => Some(p),
        (None, false) => Some(placed.join("; ")),
        (None, true) => None,
    }
}

/// A finding as people read it.
pub fn finding(f: &Value) -> String {
    format!(
        "{} {}: {}",
        text(&f["severity"]),
        text(&f["code"]),
        text(&f["message"])
    )
}

impl BomTab {
    /// Takes a `bom.get` answer: the indented BOM's (`tree`) or the flat
    /// one's (`flat`), whichever it holds.
    pub fn take(&mut self, v: &Value) {
        self.error = None;
        self.roots.clear();
        self.root = v["root"].as_str().map(str::to_owned);
        self.columns = v["columns"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c.as_str().map(str::to_owned))
            .collect();
        self.findings = v["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .map(finding)
            .collect();
        if let Some(t) = v.get("tree") {
            self.tree = self.lines(t);
        }
        if let Some(f) = v.get("flat") {
            self.flat_lines = self.flat(f);
        }
        if let Some(c) = self.conflict.take() {
            let now = self
                .tree
                .iter()
                .find(|l| l.part == c.part && l.parent.as_ref().is_some_and(|p| p.0 == c.parent))
                .and_then(|l| l.item);
            let now = now.map_or("none".to_owned(), |n| n.to_string());
            self.changed = Some(format!(
                "Someone changed {}'s item numbers while you edited: {} is now {now} (you typed {})",
                c.parent,
                c.part,
                c.text.trim()
            ));
        }
    }

    /// The assembly whose item number a line's is, when it can be edited:
    /// an indented line whose parent is read at `working` (a release's
    /// numbers are what it released).
    pub fn editable(line: &Line) -> Option<&str> {
        match &line.parent {
            Some((p, rev)) if rev == "working" => Some(p),
            _ => None,
        }
    }

    /// Starts typing a line's item number.
    pub fn edit(&mut self, line: &Line) {
        let Some(parent) = BomTab::editable(line) else {
            return;
        };
        self.edit_error = None;
        self.changed = None;
        self.editing = Some(ItemEdit {
            parent: parent.to_owned(),
            part: line.part.clone(),
            was: line.item,
            text: line.item.map(|n| n.to_string()).unwrap_or_default(),
        });
    }

    /// Whether `line`'s item number is the one being typed.
    pub fn is_editing(&self, line: &Line) -> bool {
        self.editing.as_ref().is_some_and(|e| {
            e.part == line.part && BomTab::editable(line) == Some(e.parent.as_str())
        })
    }

    /// A reading failed: several roots to choose from, or why.
    pub fn failed(&mut self, e: Error) {
        self.roots = e.details["roots"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r.as_str().map(str::to_owned))
            .collect();
        self.tree.clear();
        self.flat_lines.clear();
        self.root = None;
        self.error = Some(e);
    }

    /// What the tab shows now.
    pub fn shown(&self) -> &[Line] {
        if self.flat {
            &self.flat_lines
        } else {
            &self.tree
        }
    }

    /// The indented BOM below `root`, depth first, each line with the
    /// level the CSV gives it.
    fn lines(&self, root: &Value) -> Vec<Line> {
        let mut out = Vec::new();
        let name = text(&root["part"]);
        let at = vec![name.clone()];
        self.walk(root, &at, "", 1, &mut out);
        out
    }

    fn walk(&self, parent: &Value, at: &[String], level: &str, depth: usize, out: &mut Vec<Line>) {
        let owner = (text(&parent["part"]), rev_text(&parent["rev"]));
        for (i, row) in parent["children"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let item = row["item"].as_u64();
            let step = item.unwrap_or(i as u64 + 1);
            let level = if level.is_empty() {
                step.to_string()
            } else {
                format!("{level}.{step}")
            };
            let item_text = item.map(|n| n.to_string()).unwrap_or_default();
            let occurrences: Vec<String> = at
                .iter()
                .flat_map(|p| {
                    row["instances"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(move |i| format!("{p}/{i}"))
                })
                .collect();
            out.push(Line {
                depth,
                part: text(&row["part"]),
                rev: rev_text(&row["rev"]),
                parent: Some(owner.clone()),
                item,
                cells: self
                    .columns
                    .iter()
                    .map(|c| cell(row, c, &level, &item_text))
                    .collect(),
                problem: problem(row),
                bought: row["bought"] == true,
                occurrences: occurrences.clone(),
            });
            self.walk(row, &occurrences, &level, depth + 1, out);
        }
    }

    /// The flat BOM's lines, each with the occurrences of the indented
    /// lines of its part and revision.
    fn flat(&self, rows: &Value) -> Vec<Line> {
        rows.as_array()
            .into_iter()
            .flatten()
            .map(|row| {
                let (part, rev) = (text(&row["part"]), rev_text(&row["rev"]));
                let mut items: Vec<String> = Vec::new();
                for i in row["items"].as_array().into_iter().flatten() {
                    let n = text(&i["item"]);
                    if !items.contains(&n) {
                        items.push(n);
                    }
                }
                let occurrences = self
                    .tree
                    .iter()
                    .filter(|l| l.part == part && l.rev == rev)
                    .flat_map(|l| l.occurrences.iter().cloned())
                    .collect();
                Line {
                    depth: 0,
                    cells: self
                        .columns
                        .iter()
                        .map(|c| cell(row, c, "", &items.join(";")))
                        .collect(),
                    part,
                    rev,
                    parent: None,
                    item: None,
                    problem: problem(row),
                    bought: false,
                    occurrences,
                }
            })
            .collect()
    }

    /// Every part the BOM reads: its root and each line's. A change to
    /// any of them may change it.
    pub fn reads(&self, part: &str) -> bool {
        self.root.as_deref() == Some(part) || self.tree.iter().any(|l| l.part == part)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ryucad::error::Code;
    use serde_json::json;

    pub const COLUMNS: [&str; 12] = [
        "level",
        "item",
        "number",
        "name",
        "part",
        "revision",
        "quantity",
        "uom",
        "make_buy",
        "material",
        "description",
        "finish",
    ];

    /// `asm` places two `p@A` (item 1) and a bought `kit` (item 2)
    /// holding two tubes counted in metres, one of them a pattern's copy.
    pub fn answer() -> Value {
        json!({
            "schema": 1, "root": "asm", "rev": "working", "columns": COLUMNS,
            "findings": [{"severity": "warning", "code": "bom_item_missing",
                          "message": "kit gives tube no item number"}],
            "tree": {"part": "asm", "rev": "working", "kind": "assembly", "children": [
                {"part": "p", "rev": {"label": "A"}, "item": 1, "instances": ["p1", "p2"],
                 "quantity": {"count": 2, "uom": "each"}, "uom": "each", "make_buy": "make",
                 "properties": {"number": "100", "name": "Plate", "finish": "anodized"}},
                {"part": "kit", "rev": "working", "item": 2, "instances": ["k"],
                 "kind": "assembly", "quantity": {"count": 1, "uom": "each"}, "uom": "each",
                 "make_buy": "buy", "properties": {"name": "Tube, kit"}, "children": [
                    {"part": "tube", "rev": "working", "instances": ["t1", "row:1/t1"],
                     "bought": true, "uom": "m", "make_buy": "make", "properties": {},
                     "quantity": {"count": 2, "uom": "m", "value": 0.5, "text": "0.5 m"}},
                    {"part": "gone", "rev": "working", "instances": ["g"], "kind": "unknown",
                     "bought": true, "uom": "each", "quantity": {"count": 1, "uom": "each"},
                     "problem": {"code": "not_found", "message": "no part gone"}},
                ]},
            ]},
        })
    }

    fn flat() -> Value {
        json!({
            "schema": 1, "root": "asm", "columns": COLUMNS, "findings": [],
            "flat": [
                {"part": "p", "rev": {"label": "A"}, "uom": "each", "make_buy": "make",
                 "quantity": {"count": 2, "uom": "each"},
                 "items": [{"assembly": "asm", "rev": "working", "item": 1}],
                 "properties": {"number": "100", "name": "Plate"}},
                {"part": "kit", "rev": "working", "uom": "each", "make_buy": "buy",
                 "quantity": {"count": 1, "uom": "each"},
                 "items": [{"assembly": "asm", "rev": "working", "item": 2},
                           {"assembly": "other", "rev": "working", "item": 2}],
                 "properties": {}},
            ],
        })
    }

    fn tab() -> BomTab {
        let mut t = BomTab::default();
        t.take(&answer());
        t
    }

    #[test]
    fn the_indented_bom_is_lines_with_the_csvs_cells_and_levels() {
        let t = tab();
        let parts: Vec<(&str, usize)> = t.tree.iter().map(|l| (l.part.as_str(), l.depth)).collect();
        assert_eq!(parts, [("p", 1), ("kit", 1), ("tube", 2), ("gone", 2)]);
        assert_eq!(
            t.tree[0].cells,
            [
                "1", "1", "100", "Plate", "p", "A", "2", "each", "make", "", "", "anodized"
            ]
        );
        // No item number: its place among its siblings.
        assert_eq!(&t.tree[2].cells[..2], ["2.1", ""]);
        assert_eq!(t.tree[2].cells[6], "0.5 m");
        assert!(t.tree[2].bought);
        assert_eq!(t.tree[2].parent, Some(("kit".into(), "working".into())));
        assert_eq!(t.tree[3].problem.as_deref(), Some("no part gone"));
        assert_eq!(
            t.findings,
            ["warning bom_item_missing: kit gives tube no item number"]
        );
        assert!(t.reads("asm") && t.reads("tube") && !t.reads("bolt"));
    }

    #[test]
    fn a_line_knows_the_occurrences_that_make_its_count() {
        let t = tab();
        assert_eq!(t.tree[0].occurrences, ["asm/p1", "asm/p2"]);
        assert_eq!(t.tree[2].occurrences, ["asm/k/t1", "asm/k/row:1/t1"]);
    }

    #[test]
    fn the_flat_bom_gathers_its_lines_occurrences_and_numbers() {
        let mut t = tab();
        t.flat = true;
        t.take(&flat());
        let shown = t.shown();
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0].occurrences, ["asm/p1", "asm/p2"]);
        assert_eq!(&shown[1].cells[..2], ["", "2"]);
        assert_eq!(shown[1].parent, None);
        // The tree stays, for the occurrences and for switching back.
        assert_eq!(t.tree.len(), 4);
    }

    #[test]
    fn item_numbers_are_edited_where_the_parent_is_working() {
        let mut t = tab();
        // p's parent is the root, read at working; tube's is the kit.
        assert_eq!(BomTab::editable(&t.tree[0]), Some("asm"));
        assert_eq!(BomTab::editable(&t.tree[2]), Some("kit"));
        let mut at_label = t.tree[2].clone();
        at_label.parent = Some(("kit".into(), "B".into()));
        assert_eq!(BomTab::editable(&at_label), None);
        let p = t.tree[0].clone();
        t.edit(&p);
        assert!(t.is_editing(&p) && !t.is_editing(&t.tree[1]));
        assert_eq!(t.editing.as_ref().unwrap().text, "1");
        assert_eq!(item_number(" 7 "), Ok(7));
        assert!(item_number("0").is_err());
        assert!(item_number("100000").is_err());
        assert!(item_number("2.5").is_err());
    }

    #[test]
    fn a_conflict_says_what_changed_once_read_again() {
        let mut t = tab();
        t.conflict = Some(ItemEdit {
            parent: "asm".into(),
            part: "p".into(),
            was: Some(1),
            text: "5".into(),
        });
        let mut v = answer();
        v["tree"]["children"][0]["item"] = json!(3);
        t.take(&v);
        assert_eq!(
            t.changed.as_deref(),
            Some("Someone changed asm's item numbers while you edited: p is now 3 (you typed 5)")
        );
        assert!(t.conflict.is_none());
    }

    #[test]
    fn several_roots_are_offered() {
        let mut t = tab();
        let e =
            Error::new(Code::InvalidParam, "no one root").with("roots", json!(["asm", "other"]));
        t.failed(e);
        assert_eq!(t.roots, ["asm", "other"]);
        assert!(t.tree.is_empty() && t.root.is_none());
    }
}
