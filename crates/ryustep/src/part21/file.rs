//! Whole exchange structures: the header, the sections, and the table of
//! instances, parsed in parallel.

use rayon::prelude::*;

use super::Part;
use super::lex::{Cursor, Fault};
use crate::StepError;

/// The header section's entities, as written. `FILE_DESCRIPTION`,
/// `FILE_NAME` and `FILE_SCHEMA` come first in every file, but nothing
/// here requires them: [`Header::schemas`] and its kin read what is there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Header {
    pub entities: Vec<Part>,
}

impl Header {
    /// The header entity named `name`, if there is one.
    pub fn get(&self, name: &str) -> Option<&Part> {
        self.entities.iter().find(|p| &*p.name == name)
    }

    /// The schema names `FILE_SCHEMA` lists, as written.
    pub fn schemas(&self) -> Vec<&str> {
        match self.get("FILE_SCHEMA").and_then(|p| p.params.first()) {
            Some(super::Param::List(items)) => items
                .iter()
                .filter_map(|p| match p {
                    super::Param::Str(s) => Some(&**s),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// `FILE_NAME`'s string at `index`: 0 the name, 1 the time stamp, 4
    /// the preprocessor, 5 the originating system, 6 the authorization.
    pub fn file_name_field(&self, index: usize) -> Option<&str> {
        match self.get("FILE_NAME")?.params.get(index)? {
            super::Param::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// What an instance holds: one entity's record, or a complex instance's
/// partial records, each with only the attributes its entity declares.
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    Simple(Part),
    Complex(Vec<Part>),
}

impl Record {
    /// The simple record, or the complex instance's parts.
    pub fn parts(&self) -> &[Part] {
        match self {
            Record::Simple(p) => std::slice::from_ref(p),
            Record::Complex(parts) => parts,
        }
    }
}

/// `#id = record;`
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub id: u64,
    pub record: Record,
}

/// A whole exchange structure. Instances from every data section are in
/// one table, sorted by instance number.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct File {
    pub header: Header,
    pub instances: Vec<Instance>,
}

impl File {
    /// The instance numbered `id`.
    pub fn get(&self, id: u64) -> Option<&Instance> {
        self.instances
            .binary_search_by_key(&id, |i| i.id)
            .ok()
            .map(|k| &self.instances[k])
    }
}

/// Reads an exchange structure. Statements are found in one pass over the
/// bytes, then parsed in parallel; the table and any syntax error are the
/// same on any number of threads (the first error in the file wins).
///
/// Anchor and reference sections are skipped, and anything after
/// `END-ISO-10303-21;` (a signature section) is ignored.
pub fn parse(src: &[u8]) -> Result<File, StepError> {
    let syntax = |f: Fault| syntax(src, f);
    let mut c = Cursor::new(src, 0, src.len());
    c.skip_space().map_err(syntax)?;
    let offset = c.offset();
    if !c.eat_word("ISO-10303-21").map_err(syntax)? || !c.eat(b';').map_err(syntax)? {
        return Err(StepError::NotPart21 { offset });
    }
    let header = header(&mut c).map_err(syntax)?;
    let mut ranges = Vec::new();
    let mut data = false;
    loop {
        if c.eat_word("DATA").map_err(syntax)? {
            data = true;
            if c.peek().map_err(syntax)? == Some(b'(') {
                c.params().map_err(syntax)?;
            }
            c.expect(b';', "expected ; after DATA").map_err(syntax)?;
            let at = statements(src, c.offset(), &mut ranges).map_err(syntax)?;
            c = Cursor::new(src, at, src.len());
        } else if c.eat_word("ANCHOR").map_err(syntax)?
            || c.eat_word("REFERENCE").map_err(syntax)?
        {
            c.expect(b';', "expected ; after a section's name")
                .map_err(syntax)?;
            let at = statements(src, c.offset(), &mut Vec::new()).map_err(syntax)?;
            c = Cursor::new(src, at, src.len());
        } else if c.eat_word("END-ISO-10303-21").map_err(syntax)? {
            c.expect(b';', "expected ; after END-ISO-10303-21")
                .map_err(syntax)?;
            break;
        } else {
            return Err(syntax(c.fault("expected DATA or END-ISO-10303-21")));
        }
    }
    if !data {
        return Err(StepError::NoData);
    }
    let parsed: Vec<Result<Instance, Fault>> = ranges
        .par_iter()
        .with_min_len(256)
        .map(|&(start, end)| instance(src, start, end))
        .collect();
    let mut instances = Vec::with_capacity(parsed.len());
    for p in parsed {
        instances.push(p.map_err(syntax)?);
    }
    instances.par_sort_unstable_by_key(|i| i.id);
    if let Some(w) = instances.windows(2).find(|w| w[0].id == w[1].id) {
        return Err(StepError::DuplicateInstance { id: w[0].id });
    }
    Ok(File { header, instances })
}

/// `HEADER; … ENDSEC;`
fn header(c: &mut Cursor) -> Result<Header, Fault> {
    if !c.eat_word("HEADER")? {
        return Err(c.fault("expected HEADER"));
    }
    c.expect(b';', "expected ; after HEADER")?;
    let mut entities = Vec::new();
    while !c.eat_word("ENDSEC")? {
        let name = c.keyword()?;
        let params = c.params()?;
        c.expect(b';', "expected ; after a header entity")?;
        entities.push(Part { name, params });
    }
    c.expect(b';', "expected ; after ENDSEC")?;
    Ok(Header { entities })
}

/// Finds the statements of a section starting at `start`, up to its
/// `ENDSEC;`, pushing each one's range (without its `;`). Returns the
/// offset after `ENDSEC;`.
fn statements(src: &[u8], start: usize, out: &mut Vec<(usize, usize)>) -> Result<usize, Fault> {
    let mut pos = start;
    loop {
        let mut c = Cursor::new(src, pos, src.len());
        if c.eat_word("ENDSEC")? {
            c.expect(b';', "expected ; after ENDSEC")?;
            return Ok(c.offset());
        }
        let begin = c.offset();
        if begin == src.len() {
            return Err(c.fault("expected ENDSEC"));
        }
        let end = statement_end(src, begin)?;
        out.push((begin, end));
        pos = end + 1;
    }
}

/// The offset of the `;` that ends the statement at `start`: the first
/// one outside strings and comments.
fn statement_end(src: &[u8], start: usize) -> Result<usize, Fault> {
    let mut i = start;
    while i < src.len() {
        match src[i] {
            b';' => return Ok(i),
            b'\'' => {
                let open = i;
                i += 1;
                loop {
                    match src.get(i) {
                        None => {
                            return Err(Fault {
                                offset: open,
                                what: "unterminated string",
                            });
                        }
                        Some(b'\'') => break,
                        Some(_) => i += 1,
                    }
                }
            }
            b'/' if src.get(i + 1) == Some(&b'*') => {
                let open = i;
                i += 2;
                loop {
                    match src.get(i) {
                        None => {
                            return Err(Fault {
                                offset: open,
                                what: "unterminated comment",
                            });
                        }
                        Some(b'*') if src.get(i + 1) == Some(&b'/') => {
                            i += 1;
                            break;
                        }
                        Some(_) => i += 1,
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err(Fault {
        offset: start,
        what: "statement without ;",
    })
}

/// `#id = NAME(…)` or `#id = (A(…) B(…) …)`, between `start` and the `;`
/// at `end`.
fn instance(src: &[u8], start: usize, end: usize) -> Result<Instance, Fault> {
    let mut c = Cursor::new(src, start, end);
    if c.peek()? != Some(b'#') {
        return Err(c.fault("expected an instance or ENDSEC"));
    }
    let id = c.reference()?;
    c.expect(b'=', "expected = after an instance number")?;
    let record = if c.eat(b'(')? {
        let mut parts = Vec::new();
        while !c.eat(b')')? {
            let name = c.keyword()?;
            let params = c.params()?;
            parts.push(Part { name, params });
        }
        if parts.is_empty() {
            return Err(c.fault("a complex instance needs a record"));
        }
        Record::Complex(parts)
    } else {
        let name = c.keyword()?;
        let params = c.params()?;
        Record::Simple(Part { name, params })
    };
    if !c.at_end()? {
        return Err(c.fault("expected ;"));
    }
    Ok(Instance { id, record })
}

/// A fault's offset as a line and a byte column, both from 1.
fn syntax(src: &[u8], f: Fault) -> StepError {
    let before = &src[..f.offset.min(src.len())];
    let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
    let column = before.len()
        - before
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |k| k + 1)
        + 1;
    StepError::Syntax {
        line,
        column,
        what: f.what,
    }
}

#[cfg(test)]
mod tests {
    use super::super::Param;
    use super::*;

    const HEAD: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('x'),'2;1');\n\
        FILE_NAME('a.stp','2026-10-22',(''),(''),'p','sys','');\n\
        FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\nENDSEC;\n";

    fn file(data: &str) -> Result<File, StepError> {
        parse(format!("{HEAD}{data}").as_bytes())
    }

    fn line_of(e: StepError) -> (usize, usize, &'static str) {
        match e {
            StepError::Syntax { line, column, what } => (line, column, what),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn a_small_file() {
        let f = file(
            "DATA;\n#2 = CARTESIAN_POINT('',(0.,1.,2.));\n\
             #1 = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );\n\
             ENDSEC;\nEND-ISO-10303-21;\n",
        )
        .unwrap();
        assert_eq!(
            f.header.schemas(),
            ["AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }"]
        );
        assert_eq!(f.header.file_name_field(5), Some("sys"));
        assert_eq!(f.instances.iter().map(|i| i.id).collect::<Vec<_>>(), [1, 2]);
        let Record::Complex(parts) = &f.get(1).unwrap().record else {
            panic!("complex");
        };
        let names: Vec<&str> = parts.iter().map(|p| &*p.name).collect();
        assert_eq!(names, ["LENGTH_UNIT", "NAMED_UNIT", "SI_UNIT"]);
        assert_eq!(parts[1].params, [Param::Derived]);
        let Record::Simple(p) = &f.get(2).unwrap().record else {
            panic!("simple");
        };
        assert_eq!(&*p.name, "CARTESIAN_POINT");
        assert_eq!(f.get(2).unwrap().record.parts().len(), 1);
        assert!(f.get(3).is_none());
    }

    #[test]
    fn semicolons_in_strings_and_comments() {
        let f = file(
            "DATA;\n#1 = PRODUCT('a;b','c''d;','',(#2));/* ; */\n\
             #2 = PRODUCT_CONTEXT('/* not a comment ;',#3,'');\nENDSEC;\n\
             END-ISO-10303-21;\n",
        )
        .unwrap();
        let Record::Simple(p) = &f.get(1).unwrap().record else {
            panic!()
        };
        assert_eq!(p.params[0], Param::Str("a;b".into()));
        assert_eq!(p.params[1], Param::Str("c'd;".into()));
        assert_eq!(f.instances.len(), 2);
    }

    #[test]
    fn sections() {
        // Data sections with parameters, several of them, and anchors
        // skipped; nothing after the end is read.
        let f = file(
            "ANCHOR;\n<a> = #1;\nENDSEC;\nDATA(('one'),('S'));\n#1 = A(1);\nENDSEC;\n\
             DATA;\n#2 = B(2);\nENDSEC;\nEND-ISO-10303-21;\nSIGNATURE;garbage",
        )
        .unwrap();
        assert_eq!(f.instances.len(), 2);
        assert_eq!(file("END-ISO-10303-21;"), Err(StepError::NoData));
        assert_eq!(
            parse(b"  \nISO-10303-22;"),
            Err(StepError::NotPart21 { offset: 3 })
        );
        assert_eq!(
            file("DATA;\n#7 = A(1);\n#7 = B(2);\nENDSEC;\nEND-ISO-10303-21;"),
            Err(StepError::DuplicateInstance { id: 7 })
        );
    }

    #[test]
    fn syntax_errors_name_the_line() {
        let base = HEAD.lines().count();
        let e = file("DATA;\n#1 = A(1);\n#2 = B(1 2);\nENDSEC;\nEND-ISO-10303-21;").unwrap_err();
        assert_eq!(line_of(e), (base + 3, 10, "expected , or )"));
        let e = file("DATA;\n#1 = A(1)\n#2 = B(2);\nENDSEC;\nEND-ISO-10303-21;").unwrap_err();
        assert_eq!(line_of(e), (base + 3, 1, "expected ;"));
        let e = file("DATA;\n#1 = A('x);\nENDSEC;\nEND-ISO-10303-21;").unwrap_err();
        assert_eq!(line_of(e), (base + 2, 8, "unterminated string"));
        let e = file("DATA;\n#1 = A(1);\n").unwrap_err();
        assert_eq!(line_of(e).2, "expected ENDSEC");
        let e = file("DATA;\nA(1);\nENDSEC;\nEND-ISO-10303-21;").unwrap_err();
        assert_eq!(line_of(e), (base + 2, 1, "expected an instance or ENDSEC"));
        let e = file("DATA;\n#1 = ();\nENDSEC;\nEND-ISO-10303-21;").unwrap_err();
        assert_eq!(line_of(e).2, "a complex instance needs a record");
        let e = file("DATA;\nENDSEC;\n").unwrap_err();
        assert_eq!(line_of(e).2, "expected DATA or END-ISO-10303-21");
    }

    #[test]
    fn the_first_error_wins_on_any_number_of_threads() {
        let mut data = String::from("DATA;\n");
        for k in 1..=5000 {
            if k % 1000 == 0 {
                data.push_str(&format!("#{k} = A(1 2);\n"));
            } else {
                data.push_str(&format!("#{k} = A({k});\n"));
            }
        }
        data.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
        for threads in [1, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let e = pool.install(|| file(&data)).unwrap_err();
            assert_eq!(line_of(e).0, HEAD.lines().count() + 1000 + 1);
        }
    }
}
