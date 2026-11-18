//! The XML reader (design 935): records found by an element path from the
//! root, each flattened to its attributes and its elements' text, named
//! by their paths relative to the record. Elements are matched by local
//! name, so a namespaced export reads without declaring its namespaces,
//! as XLSX's do; roxmltree refuses DTDs, so no entity expands.

use super::rows::{Leaves, Refusal};
use super::text::{self, Encoding};
use roxmltree::{Document, Node};
use std::collections::{BTreeMap, BTreeSet};

/// The deepest element nesting read: deeper is refused, which bounds the
/// recursion too.
pub const MAX_DEPTH: usize = 128;

/// Whether `p` is an element path: `/A/B` when absolute, `A/B` when not,
/// each step a local name (no `@`, `[`, `]`, `:` or space).
pub fn path_ok(p: &str, absolute: bool) -> bool {
    let rest = if absolute {
        match p.strip_prefix('/') {
            Some(r) => r,
            None => return false,
        }
    } else {
        p
    };
    !rest.is_empty()
        && rest
            .split('/')
            .all(|s| !s.is_empty() && !s.contains(['@', '[', ']', ':', ' ']))
}

/// The byte range of an XML declaration's `encoding="…"` at the start of
/// `s`, and the name it gives.
fn encoding_range(s: &str) -> Option<(usize, usize, &str)> {
    if !s.starts_with("<?xml") {
        return None;
    }
    let end = s.find("?>")?;
    let at = s[..end].find("encoding")?;
    let b = s.as_bytes();
    let mut i = at + "encoding".len();
    while i < end && b[i].is_ascii_whitespace() {
        i += 1;
    }
    if b.get(i) != Some(&b'=') {
        return None;
    }
    i += 1;
    while i < end && b[i].is_ascii_whitespace() {
        i += 1;
    }
    let q = *b.get(i)?;
    if q != b'"' && q != b'\'' {
        return None;
    }
    let v = i + 1;
    let close = v + s[v..end].find(char::from(q))?;
    Some((at, close + 1, &s[v..close]))
}

/// The document's text: in its byte-order mark's encoding, else its
/// declaration's, else UTF-8. The declaration's `encoding` is then blanked
/// with spaces, so the parser judges only the text it is given and no
/// position moves.
pub fn decode(bytes: &[u8]) -> Result<String, Refusal> {
    let enc = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Encoding::Utf8
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        Encoding::Utf16Le
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Encoding::Utf16Be
    } else {
        let head: String = bytes.iter().take(256).map(|&b| char::from(b)).collect();
        match encoding_range(&head) {
            Some((_, _, name)) if name.eq_ignore_ascii_case("us-ascii") => Encoding::Utf8,
            Some((_, _, name)) => Encoding::parse(name).ok_or_else(|| {
                Refusal::file(format!(
                    "encoding {name:?} isn't one this reads: utf-8, utf-16le, utf-16be, iso-8859-1 or windows-1252"
                ))
            })?,
            None => Encoding::Utf8,
        }
    };
    let mut s = text::decode(bytes, enc).map_err(|r| {
        if enc == Encoding::Utf8 {
            Refusal::file(format!(
                "line {} isn't UTF-8; a document in another encoding names it in its XML declaration",
                r.row
            ))
        } else {
            r
        }
    })?;
    if let Some((from, to, _)) = encoding_range(&s) {
        s.replace_range(from..to, &" ".repeat(to - from));
    }
    Ok(s)
}

/// The document `text` holds; roxmltree refuses a DTD by default.
fn parse(text: &str) -> Result<Document<'_>, Refusal> {
    Document::parse(text).map_err(|e| Refusal::file(format!("not XML: {e}")))
}

/// The elements `steps` names below `from`, by local name, in document
/// order.
fn select<'a, 'i>(from: &[Node<'a, 'i>], steps: &[&str]) -> Vec<Node<'a, 'i>> {
    let mut level = from.to_vec();
    for &s in steps {
        level = level
            .iter()
            .flat_map(|n| {
                n.children()
                    .filter(move |c| c.is_element() && c.tag_name().name() == s)
            })
            .collect();
    }
    level
}

/// The record elements at `records`, an absolute element path.
fn records_at<'a, 'i>(doc: &'a Document<'i>, records: &str) -> Result<Vec<Node<'a, 'i>>, Refusal> {
    let steps: Vec<&str> = records.trim_start_matches('/').split('/').collect();
    let root = doc.root_element();
    if root.tag_name().name() != steps[0] {
        return Err(Refusal::file(format!(
            "the root element is {:?}, not {:?} as records {records:?} says",
            root.tag_name().name(),
            steps[0]
        )));
    }
    let found = select(&[root], &steps[1..]);
    if found.is_empty() {
        return Err(Refusal::file(format!("no records at {records}")));
    }
    Ok(found)
}

/// The records `bytes` hold, each its place and its leaves, children
/// after their parent with `#level` when `children` is set.
pub fn read(bytes: &[u8], records: &str, children: Option<&str>) -> Result<Vec<Leaves>, Refusal> {
    let text = decode(bytes)?;
    let doc = parse(&text)?;
    let kids: Option<Vec<&str>> = children.map(|c| c.split('/').collect());
    let depth = records.trim_start_matches('/').split('/').count();
    let mut out = Vec::new();
    for n in records_at(&doc, records)? {
        record(&doc, n, kids.as_deref(), 0, depth, &mut out)?;
    }
    Ok(out)
}

/// Record `n`, pushed before its children so rows come in document order.
fn record<'a, 'i>(
    doc: &'a Document<'i>,
    n: Node<'a, 'i>,
    kids: Option<&[&str]>,
    level: usize,
    depth: usize,
    out: &mut Vec<Leaves>,
) -> Result<(), Refusal> {
    let p = out.len();
    let pos = doc.text_pos_at(n.range().start);
    let place = format!("line {}, column {}", pos.row, pos.col);
    out.push(Leaves {
        place: place.clone(),
        cells: Vec::new(),
    });
    let below = kids.map(|k| select(&[n], k)).unwrap_or_default();
    let mut cells = Vec::new();
    if kids.is_some() {
        cells.push(("#level".to_owned(), level.to_string()));
    }
    flatten(n, "", &below, depth, &mut cells)
        .map_err(|m| Refusal::at(p as u64 + 1, None, m).placed(Some(place.as_str())))?;
    out[p].cells = cells;
    let step = kids.map_or(0, <[&str]>::len);
    for c in below {
        record(doc, c, kids, level + 1, depth + step, out)?;
    }
    Ok(())
}

/// Element `n`'s leaves at `prefix`: its attributes, its own text, then
/// each child element (`skip`, the child records, left out), a repeated
/// name indexed from its second occurrence.
fn flatten(
    n: Node<'_, '_>,
    prefix: &str,
    skip: &[Node<'_, '_>],
    depth: usize,
    cells: &mut Vec<(String, String)>,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("nested deeper than 128 levels".to_owned());
    }
    let join = |name: &str| {
        if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        }
    };
    for a in n.attributes() {
        cells.push((join(&format!("@{}", a.name())), a.value().to_owned()));
    }
    let text: String = n
        .children()
        .filter(|c| c.is_text())
        .filter_map(|c| c.text())
        .filter(|t| !t.trim().is_empty())
        .collect();
    if !text.is_empty() {
        let at = if prefix.is_empty() { "." } else { prefix };
        cells.push((at.to_owned(), text));
    }
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for c in n.children().filter(|c| c.is_element()) {
        if skip.contains(&c) {
            continue;
        }
        let name = c.tag_name().name();
        let k = seen.entry(name).or_insert(0);
        *k += 1;
        let step = if *k == 1 {
            name.to_owned()
        } else {
            format!("{name}[{k}]")
        };
        flatten(c, &join(&step), skip, depth + 1, cells)?;
    }
    Ok(())
}

/// An element's names, as a record's columns would start them: its
/// attributes (`@a`) and its child elements' names.
fn names(n: Node<'_, '_>) -> BTreeSet<String> {
    n.attributes()
        .map(|a| format!("@{}", a.name()))
        .chain(
            n.children()
                .filter(|c| c.is_element())
                .map(|c| c.tag_name().name().to_owned()),
        )
        .collect()
}

/// `import guess`'s proposal for an XML file (design 935): as records, the
/// element path whose name repeats most under one parent (counting only
/// names that repeat; the first seen on a tie), unless `records` names
/// them; as children, the first element one or two steps below a record
/// named as the record is, or sharing at least two names with the records
/// and at least half of its own.
pub fn survey(bytes: &[u8], records: Option<&str>) -> Result<(String, Option<String>), Refusal> {
    let text = decode(bytes)?;
    let doc = parse(&text)?;
    let records = match records {
        Some(r) => r.to_owned(),
        None => {
            let path_of = |n: Node<'_, '_>| {
                let mut names: Vec<&str> = n
                    .ancestors()
                    .filter(|a| a.is_element())
                    .map(|a| a.tag_name().name())
                    .collect();
                names.reverse();
                format!("/{}", names.join("/"))
            };
            let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
            for e in doc.descendants().filter(|n| n.is_element()) {
                let mut by: BTreeMap<&str, usize> = BTreeMap::new();
                for c in e.children().filter(|c| c.is_element()) {
                    *by.entry(c.tag_name().name()).or_default() += 1;
                }
                for (name, k) in by {
                    if k < 2 {
                        continue;
                    }
                    let n = counts.len();
                    counts
                        .entry(format!("{}/{name}", path_of(e)))
                        .or_insert((0, n))
                        .0 += k;
                }
            }
            counts
                .into_iter()
                .max_by_key(|(_, (k, o))| (*k, std::cmp::Reverse(*o)))
                .map(|(p, _)| p)
                .ok_or_else(|| {
                    Refusal::file(
                        "no element repeats under one parent to read as records; name them with --records",
                    )
                })?
        }
    };
    let recs = records_at(&doc, &records)?;
    let rname = records.rsplit('/').next().unwrap_or("");
    let mut rk: BTreeSet<String> = BTreeSet::new();
    for r in recs.iter().take(100) {
        rk.extend(names(*r));
    }
    for r in &recs {
        for a in r.children().filter(|c| c.is_element()) {
            let an = a.tag_name().name();
            let mut cands = vec![(an.to_owned(), a)];
            cands.extend(
                a.children()
                    .filter(|c| c.is_element())
                    .map(|b| (format!("{an}/{}", b.tag_name().name()), b)),
            );
            for (path, c) in cands {
                let cn = names(c);
                let common = cn.intersection(&rk).count();
                if c.tag_name().name() == rname || (common >= 2 && common * 2 >= cn.len()) {
                    return Ok((records, Some(path)));
                }
            }
        }
    }
    Ok((records, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_read_in_its_declared_encoding() {
        let mut b = b"<?xml version=\"1.0\" encoding=\"windows-1252\"?><a>".to_vec();
        b.extend_from_slice(&[0x80, 0x96, 0xD7]);
        b.extend_from_slice(b"</a>");
        let s = decode(&b).unwrap();
        assert!(!s.contains("windows-1252"), "{s}");
        assert_eq!(parse(&s).unwrap().root_element().text(), Some("€–×"));
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain(
                "<?xml version=\"1.0\" encoding=\"UTF-16\"?><a>ø</a>"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes),
            )
            .collect();
        assert_eq!(
            parse(&decode(&utf16).unwrap())
                .unwrap()
                .root_element()
                .text(),
            Some("ø")
        );
        assert_eq!(
            parse(&decode("<a>ø</a>".as_bytes()).unwrap())
                .unwrap()
                .root_element()
                .text(),
            Some("ø")
        );
        let a = decode(b"<?xml version='1.0' encoding='US-ASCII'?><a>x</a>").unwrap();
        assert_eq!(parse(&a).unwrap().root_element().text(), Some("x"));
        let e = decode(b"<?xml version=\"1.0\" encoding=\"koi8-r\"?><a/>").unwrap_err();
        assert!(e.message.contains("koi8-r"), "{}", e.message);
        assert!(
            decode(b"<a>\xFF</a>")
                .unwrap_err()
                .message
                .contains("declaration")
        );
    }

    #[test]
    fn a_dtd_is_refused() {
        let s = decode(b"<!DOCTYPE a [<!ENTITY x \"y\">]><a>&x;</a>").unwrap();
        assert!(parse(&s).is_err());
    }

    fn cells(l: &Leaves) -> Vec<(&str, &str)> {
        l.cells
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect()
    }

    #[test]
    fn records_flatten_to_attributes_and_element_text() {
        let x = r#"<?xml version="1.0"?>
<e:Export xmlns:e="urn:demo" xmlns:m="urn:mfr"><e:Items>
  <e:Item id="1">
    <e:Number>100-2</e:Number>
    <e:Weight unit="kg">0.120</e:Weight>
    <m:Alt><m:MPN>X1</m:MPN></m:Alt>
    <m:Alt><m:MPN>X2</m:MPN></m:Alt>
    <e:Note><![CDATA[a < b]]></e:Note>
    <e:Empty/>
  </e:Item>
  <e:Item id="2">two</e:Item>
</e:Items></e:Export>"#;
        let r = read(x.as_bytes(), "/Export/Items/Item", None).unwrap();
        assert_eq!(
            cells(&r[0]),
            [
                ("@id", "1"),
                ("Number", "100-2"),
                ("Weight/@unit", "kg"),
                ("Weight", "0.120"),
                ("Alt/MPN", "X1"),
                ("Alt[2]/MPN", "X2"),
                ("Note", "a < b")
            ]
        );
        assert_eq!(cells(&r[1]), [("@id", "2"), (".", "two")]);
        assert_eq!(r[0].place, "line 3, column 3");
    }

    #[test]
    fn children_follow_their_parent_with_a_level() {
        let x = r#"<Parts><Part pn="A"><Uses><Part pn="B"><Uses><Part pn="C"/></Uses></Part><Part pn="D"/></Uses></Part><Part pn="E"/></Parts>"#;
        let r = read(x.as_bytes(), "/Parts/Part", Some("Uses/Part")).unwrap();
        let got: Vec<Vec<(&str, &str)>> = r.iter().map(cells).collect();
        assert_eq!(
            got,
            vec![
                vec![("#level", "0"), ("@pn", "A")],
                vec![("#level", "1"), ("@pn", "B")],
                vec![("#level", "2"), ("@pn", "C")],
                vec![("#level", "1"), ("@pn", "D")],
                vec![("#level", "0"), ("@pn", "E")],
            ]
        );
    }

    #[test]
    fn xml_refusals_say_where() {
        assert!(
            read(b"<A><B/></A>", "/X/B", None)
                .unwrap_err()
                .message
                .contains("root element")
        );
        assert!(
            read(b"<A><B/></A>", "/A/C", None)
                .unwrap_err()
                .message
                .contains("no records")
        );
        assert!(
            read(b"<A><B>", "/A/B", None)
                .unwrap_err()
                .message
                .contains("not XML")
        );
        let deep = format!("<A><B>{}x{}</B></A>", "<c>".repeat(130), "</c>".repeat(130));
        let e = read(deep.as_bytes(), "/A/B", None).unwrap_err();
        assert_eq!((e.row, e.place.as_deref()), (1, Some("line 1, column 4")));
        assert!(e.message.contains("128"), "{}", e.message);
    }

    #[test]
    fn a_survey_proposes_the_records_and_their_children() {
        let x = br#"<Export><Meta><Tag/></Meta><Items>
<Item number="A"><Name>a</Name><Qty>1</Qty><Uses><Use number="B"><Name>b</Name><Qty>2</Qty></Use></Uses></Item>
<Item number="C"><Name>c</Name><Qty>1</Qty></Item></Items></Export>"#;
        assert_eq!(
            survey(x, None).unwrap(),
            ("/Export/Items/Item".to_owned(), Some("Uses/Use".to_owned()))
        );
        assert_eq!(
            survey(x, Some("/Export/Meta/Tag")).unwrap(),
            ("/Export/Meta/Tag".to_owned(), None)
        );
        assert!(survey(b"<A><B/></A>", None).is_err());
    }
}
