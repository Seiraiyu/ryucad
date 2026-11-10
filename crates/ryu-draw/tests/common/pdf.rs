//! A reader of the PDFs `ryu_draw::pdf` writes, for tests (design 417,
//! plan 9.D): independent of the writer. It checks the cross-reference
//! table against every object's offset, and reads the pages (their size
//! and, through the font's ToUnicode map, the text each shows, in order),
//! the embedded fonts (each font file inflated), and the bookmarks'
//! titles depth first. Only what the writer writes is read: uncompressed
//! content streams, literal strings, a `bfchar` map.

#![allow(dead_code)]

use std::collections::BTreeMap;

/// What a PDF holds.
#[derive(Debug, Default)]
pub struct Read {
    /// Objects the cross-reference table lists in use.
    pub objects: usize,
    /// Each page's size in points and the text it shows, in order.
    pub pages: Vec<([f64; 2], Vec<String>)>,
    /// Each font's base name, and how many bytes its file inflates to.
    pub fonts: Vec<(String, usize)>,
    /// The bookmarks' titles, depth first.
    pub bookmarks: Vec<String>,
    pub title: Option<String>,
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

/// A literal string's bytes from `s` (just past its `(`): escapes and
/// balanced parentheses; and where it ends.
fn literal(s: &[u8]) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'\\' => {
                i += 1;
                match s.get(i) {
                    Some(b'n') => out.push(b'\n'),
                    Some(b'r') => out.push(b'\r'),
                    Some(b't') => out.push(b'\t'),
                    Some(b'b') => out.push(8),
                    Some(b'f') => out.push(12),
                    Some(c @ b'0'..=b'7') => {
                        let mut v = u32::from(c - b'0');
                        for _ in 0..2 {
                            match s.get(i + 1) {
                                Some(d @ b'0'..=b'7') => {
                                    v = v * 8 + u32::from(d - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(v as u8);
                    }
                    Some(c) => out.push(*c),
                    None => break,
                }
            }
            b'(' => {
                depth += 1;
                out.push(b'(');
            }
            b')' if depth == 0 => return (out, i + 1),
            b')' => {
                depth -= 1;
                out.push(b')');
            }
            c => out.push(c),
        }
        i += 1;
    }
    (out, i)
}

/// A text string (a title): UTF-16 with its byte order mark, else
/// PDFDocEncoding read as Latin-1.
fn text_string(b: &[u8]) -> String {
    if b.starts_with(&[0xfe, 0xff]) {
        let u: Vec<u16> = b[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u)
    } else {
        b.iter().map(|c| char::from(*c)).collect()
    }
}

/// The number after `key` in `dict` (`/Length 120`), or the first `n 0 R`.
fn number_after(dict: &[u8], key: &[u8]) -> Option<f64> {
    let at = find(dict, key, 0)? + key.len();
    // Up to the line's end: a stream's binary bytes may follow.
    let end = dict[at..].iter().position(|c| *c == b'\n').map_or(dict.len(), |e| at + e);
    let rest = std::str::from_utf8(&dict[at..end]).ok()?;
    rest.split_whitespace().next()?.trim_end_matches(['/', '>']).parse().ok()
}

fn reference_after(dict: &[u8], key: &[u8]) -> Option<u32> {
    number_after(dict, key).map(|n| n as u32)
}

/// Reads `bytes`, or says what isn't as the writer writes it.
pub fn read(bytes: &[u8]) -> Result<Read, String> {
    if !bytes.starts_with(b"%PDF-") {
        return Err("no %PDF- header".into());
    }
    // The cross-reference table, from startxref.
    let sx = bytes
        .windows(9)
        .rposition(|w| w == b"startxref")
        .ok_or("no startxref")?;
    let tail = std::str::from_utf8(&bytes[sx + 9..]).map_err(|_| "startxref")?;
    let at: usize = tail
        .split_whitespace()
        .next()
        .and_then(|t| t.parse().ok())
        .ok_or("startxref's offset")?;
    let table = std::str::from_utf8(&bytes[at..]).map_err(|_| "the xref table")?;
    let mut lines = table.lines();
    if lines.next() != Some("xref") {
        return Err(format!("no xref at {at}"));
    }
    let head: Vec<usize> = lines
        .next()
        .ok_or("xref's section")?
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    let (first, count) = (head[0], head[1]);
    let mut offsets: Vec<(u32, usize)> = Vec::new();
    for i in 0..count {
        let e = lines.next().ok_or("an xref entry")?;
        let f: Vec<&str> = e.split_whitespace().collect();
        if f.get(2) != Some(&"n") {
            continue;
        }
        let off: usize = f[0].parse().map_err(|_| "an entry's offset")?;
        let n = (first + i) as u32;
        if !bytes[off..].starts_with(format!("{n} 0 obj").as_bytes()) {
            return Err(format!("object {n} isn't at {off}"));
        }
        offsets.push((n, off));
    }
    // Each object runs to the next one's offset (or the table): a font
    // file's bytes may hold `endobj` by chance.
    let mut ends: Vec<usize> = offsets.iter().map(|(_, o)| *o).collect();
    ends.sort_unstable();
    let mut objects: BTreeMap<u32, &[u8]> = BTreeMap::new();
    for (n, off) in &offsets {
        let next = ends.iter().copied().find(|o| o > off).unwrap_or(at);
        let head = format!("{n} 0 obj").len();
        objects.insert(*n, &bytes[off + head..next]);
    }
    // A stream's data, by its length.
    let stream = |n: u32| -> Option<&[u8]> {
        let o = objects.get(&n)?;
        let s = find(o, b"stream", 0)?;
        let len = number_after(o, b"/Length")? as usize;
        let mut d = s + b"stream".len();
        while o.get(d).is_some_and(|c| *c == b'\r' || *c == b'\n') {
            d += 1;
        }
        o.get(d..d + len)
    };
    let mut read = Read {
        objects: objects.len(),
        ..Read::default()
    };
    // The ToUnicode map: two-byte glyph ids to characters.
    let mut glyphs: BTreeMap<u16, String> = BTreeMap::new();
    let mut fonts = Vec::new();
    for o in objects.values() {
        if let Some(t) = reference_after(o, b"/ToUnicode") {
            let cmap = std::str::from_utf8(stream(t).ok_or("the ToUnicode stream")?)
                .map_err(|_| "the ToUnicode map")?;
            let mut inside = false;
            for l in cmap.lines() {
                if l.ends_with("beginbfchar") {
                    inside = true;
                } else if l == "endbfchar" {
                    inside = false;
                } else if inside {
                    let hex: Vec<&str> =
                        l.split(['<', '>']).filter(|x| !x.trim().is_empty()).collect();
                    let g = u16::from_str_radix(hex[0], 16).map_err(|_| "a glyph id")?;
                    let u: Vec<u16> = (0..hex[1].len() / 4)
                        .map(|k| u16::from_str_radix(&hex[1][4 * k..4 * k + 4], 16).unwrap())
                        .collect();
                    glyphs.insert(g, String::from_utf16_lossy(&u));
                }
            }
        }
        if let Some(f) = reference_after(o, b"/FontFile2") {
            let name_at = find(o, b"/FontName /", 0).ok_or("a font's name")? + 11;
            let name: String = o[name_at..]
                .iter()
                .take_while(|c| !c.is_ascii_whitespace() && **c != b'/')
                .map(|c| char::from(*c))
                .collect();
            let packed = stream(f).ok_or("a font file")?;
            let font = miniz_oxide::inflate::decompress_to_vec_zlib(packed)
                .map_err(|_| "a font file doesn't inflate")?;
            fonts.push((name, font.len()));
        }
    }
    read.fonts = fonts;
    // The pages, in the page tree's order.
    let tree = objects
        .iter()
        .find(|(_, o)| find(o, b"/Type /Pages", 0).is_some())
        .ok_or("no page tree")?
        .1;
    let kids_at = find(tree, b"/Kids [", 0).ok_or("no kids")? + 7;
    let kids_end = find(tree, b"]", kids_at).ok_or("kids' end")?;
    let kids: Vec<u32> = std::str::from_utf8(&tree[kids_at..kids_end])
        .map_err(|_| "kids")?
        .split(" R")
        .filter_map(|r| r.split_whitespace().next()?.parse().ok())
        .collect();
    for k in kids {
        let page = objects.get(&k).ok_or("a page")?;
        let mb = find(page, b"/MediaBox [", 0).ok_or("a page's box")? + 11;
        let me = find(page, b"]", mb).ok_or("a page's box")?;
        let b: Vec<f64> = std::str::from_utf8(&page[mb..me])
            .map_err(|_| "a page's box")?
            .split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect();
        let content = stream(reference_after(page, b"/Contents").ok_or("a page's content")?)
            .ok_or("a page's content stream")?;
        // Each `(…) Tj`'s glyphs as text.
        let mut texts = Vec::new();
        let mut i = 0;
        while let Some(open) = find(content, b"(", i) {
            let (s, used) = literal(&content[open + 1..]);
            i = open + 1 + used;
            let rest = &content[i..];
            let op = rest.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(0);
            if rest[op..].starts_with(b"Tj") {
                let t: String = s
                    .chunks_exact(2)
                    .map(|c| {
                        let g = u16::from_be_bytes([c[0], c[1]]);
                        glyphs.get(&g).cloned().unwrap_or_else(|| "\u{fffd}".into())
                    })
                    .collect();
                texts.push(t);
            }
        }
        read.pages.push(([b[2] - b[0], b[3] - b[1]], texts));
    }
    // Bookmarks: outline items by their order in the file, which the
    // writer numbers depth first; and the document's title.
    let title = |o: &[u8]| -> Option<String> {
        let t = find(o, b"/Title ", 0)? + 7;
        match o.get(t)? {
            b'(' => Some(text_string(&literal(&o[t + 1..]).0)),
            b'<' => {
                let end = find(o, b">", t)?;
                let hex = std::str::from_utf8(&o[t + 1..end]).ok()?;
                let bytes: Vec<u8> = (0..hex.len() / 2)
                    .filter_map(|k| u8::from_str_radix(&hex[2 * k..2 * k + 2], 16).ok())
                    .collect();
                Some(text_string(&bytes))
            }
            _ => None,
        }
    };
    for o in objects.values() {
        if find(o, b"/Parent", 0).is_some()
            && let Some(t) = title(o)
        {
            read.bookmarks.push(t);
        }
        if find(o, b"/Creator", 0).is_some() {
            read.title = title(o);
        }
    }
    Ok(read)
}
