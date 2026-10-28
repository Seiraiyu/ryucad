//! Writing an exchange structure, the inverse of [`parse`](super::parse).
//!
//! The output is a pure function of the table: one statement a line,
//! instances in the table's order, reals as the shortest text that reads
//! back to the same bits (Rust's formatting, the same on every platform),
//! and strings encoded by `text::encode`. Reading it back gives the same
//! table, bit for bit.

use std::fmt::Write as _;

use super::{File, Param, Part, Record, text};
use crate::WriteError;

/// Writes `file`. Refuses what Part 21 can't hold: reals that aren't
/// finite, names, enumerations and binaries that aren't well formed,
/// complex instances without records, and instance numbers used twice.
pub fn write(file: &File) -> Result<Vec<u8>, WriteError> {
    let mut out = String::with_capacity(64 * file.instances.len() + 512);
    out.push_str("ISO-10303-21;\nHEADER;\n");
    for p in &file.header.entities {
        part(&mut out, p).map_err(|what| WriteError::Unwritable {
            instance: None,
            what,
        })?;
        out.push_str(";\n");
    }
    out.push_str("ENDSEC;\nDATA;\n");
    let mut ids: Vec<u64> = file.instances.iter().map(|i| i.id).collect();
    ids.sort_unstable();
    if let Some(w) = ids.windows(2).find(|w| w[0] == w[1]) {
        return Err(WriteError::Unwritable {
            instance: Some(w[0]),
            what: "instance number used twice",
        });
    }
    for inst in &file.instances {
        let fail = |what| WriteError::Unwritable {
            instance: Some(inst.id),
            what,
        };
        write!(out, "#{}=", inst.id).expect("writing to a string");
        match &inst.record {
            Record::Simple(p) => part(&mut out, p).map_err(fail)?,
            Record::Complex(parts) => {
                if parts.is_empty() {
                    return Err(fail("a complex instance needs a record"));
                }
                out.push('(');
                for p in parts {
                    part(&mut out, p).map_err(fail)?;
                }
                out.push(')');
            }
        }
        out.push_str(";\n");
    }
    out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    Ok(out.into_bytes())
}

fn part(out: &mut String, p: &Part) -> Result<(), &'static str> {
    keyword(out, &p.name)?;
    list(out, &p.params)
}

fn list(out: &mut String, params: &[Param]) -> Result<(), &'static str> {
    out.push('(');
    for (k, p) in params.iter().enumerate() {
        if k > 0 {
            out.push(',');
        }
        param(out, p)?;
    }
    out.push(')');
    Ok(())
}

fn param(out: &mut String, p: &Param) -> Result<(), &'static str> {
    match p {
        Param::Unset => out.push('$'),
        Param::Derived => out.push('*'),
        Param::Int(n) => write!(out, "{n}").expect("writing to a string"),
        Param::Real(x) => out.push_str(&real(*x)?),
        Param::Str(s) => {
            out.push('\'');
            out.push_str(&text::encode(s));
            out.push('\'');
        }
        Param::Enum(e) => {
            if !is_name(e) {
                return Err("an enumeration must be an upper-case name");
            }
            write!(out, ".{e}.").expect("writing to a string");
        }
        Param::Binary(b) => {
            let mut digits = b.chars();
            let ok = matches!(digits.next(), Some('0'..='3'))
                && digits.all(|c| matches!(c, '0'..='9' | 'A'..='F'));
            if !ok {
                return Err("a binary must be 0 to 3, then upper-case hex digits");
            }
            write!(out, "\"{b}\"").expect("writing to a string");
        }
        Param::Ref(id) => write!(out, "#{id}").expect("writing to a string"),
        Param::List(items) => list(out, items)?,
        Param::Typed(name, value) => {
            keyword(out, name)?;
            out.push('(');
            param(out, value)?;
            out.push(')');
        }
    }
    Ok(())
}

fn keyword(out: &mut String, name: &str) -> Result<(), &'static str> {
    if !is_name(name.strip_prefix('!').unwrap_or(name)) {
        return Err("an entity or type name must be an upper-case name");
    }
    out.push_str(name);
    Ok(())
}

/// `[A-Z_][A-Z0-9_]*`.
fn is_name(s: &str) -> bool {
    let mut b = s.bytes();
    b.next()
        .is_some_and(|c| c.is_ascii_uppercase() || c == b'_')
        && b.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
}

/// The shortest text that reads back as `x`, in Part 21's form: always a
/// point, and an exponent outside `[1e-5, 1e16)`.
pub(crate) fn real(x: f64) -> Result<String, &'static str> {
    if !x.is_finite() {
        return Err("a real must be finite");
    }
    if x == 0.0 {
        return Ok(if x.is_sign_negative() { "-0." } else { "0." }.into());
    }
    let a = x.abs();
    if (1e-5..1e16).contains(&a) {
        let mut s = format!("{x}");
        if !s.contains('.') {
            s.push('.');
        }
        Ok(s)
    } else {
        let s = format!("{x:e}");
        let (m, e) = s.split_once('e').expect("an exponent");
        let point = if m.contains('.') { "" } else { "." };
        Ok(format!("{m}{point}E{e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Header, Instance, parse};
    use super::*;
    use proptest::prelude::*;

    fn file(instances: Vec<Instance>) -> File {
        File {
            header: Header::default(),
            instances,
        }
    }

    fn simple(id: u64, name: &str, params: Vec<Param>) -> Instance {
        Instance {
            id,
            record: Record::Simple(Part {
                name: name.into(),
                params,
            }),
        }
    }

    #[test]
    fn reals() {
        let r = |x: f64| real(x).unwrap();
        assert_eq!(r(0.0), "0.");
        assert_eq!(r(-0.0), "-0.");
        assert_eq!(r(1.0), "1.");
        assert_eq!(r(-2.5), "-2.5");
        assert_eq!(r(0.1), "0.1");
        assert_eq!(r(1e-5), "0.00001");
        assert_eq!(r(1e-7), "1.E-7");
        assert_eq!(r(-1.5e-300), "-1.5E-300");
        assert_eq!(r(1e16), "1.E16");
        assert_eq!(r(123456789012345.6), "123456789012345.6");
        assert_eq!(r(5e-324), "5.E-324");
        assert_eq!(r(f64::MAX), "1.7976931348623157E308");
        assert!(real(f64::NAN).is_err());
        assert!(real(f64::INFINITY).is_err());
    }

    #[test]
    fn a_small_file() {
        let f = File {
            header: Header {
                entities: vec![Part {
                    name: "FILE_SCHEMA".into(),
                    params: vec![Param::List(vec![Param::Str(
                        "CONFIG_CONTROL_DESIGN".into(),
                    )])],
                }],
            },
            instances: vec![
                simple(
                    1,
                    "CARTESIAN_POINT",
                    vec![
                        Param::Str("".into()),
                        Param::List(vec![Param::Real(0.0), Param::Real(1e-7), Param::Int(3)]),
                    ],
                ),
                Instance {
                    id: 2,
                    record: Record::Complex(vec![
                        Part {
                            name: "LENGTH_UNIT".into(),
                            params: vec![],
                        },
                        Part {
                            name: "SI_UNIT".into(),
                            params: vec![Param::Enum("MILLI".into()), Param::Unset],
                        },
                    ]),
                },
                simple(
                    3,
                    "!X",
                    vec![
                        Param::Typed("LENGTH_MEASURE".into(), Box::new(Param::Real(2.0))),
                        Param::Ref(1),
                        Param::Derived,
                        Param::Binary("2FC".into()),
                        Param::Str("it's".into()),
                    ],
                ),
            ],
        };
        let text = String::from_utf8(write(&f).unwrap()).unwrap();
        assert_eq!(
            text,
            "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('CONFIG_CONTROL_DESIGN'));\nENDSEC;\nDATA;\n\
             #1=CARTESIAN_POINT('',(0.,1.E-7,3));\n\
             #2=(LENGTH_UNIT()SI_UNIT(.MILLI.,$));\n\
             #3=!X(LENGTH_MEASURE(2.),#1,*,\"2FC\",'it''s');\n\
             ENDSEC;\nEND-ISO-10303-21;\n"
        );
        assert_eq!(parse(text.as_bytes()).unwrap(), f);
    }

    #[test]
    fn refusals() {
        let what = |f: File| match write(&f) {
            Err(WriteError::Unwritable { what, .. }) => what,
            r => panic!("{r:?}"),
        };
        assert_eq!(
            what(file(vec![simple(1, "A", vec![Param::Real(f64::NAN)])])),
            "a real must be finite"
        );
        assert_eq!(
            what(file(vec![simple(1, "a", vec![])])),
            "an entity or type name must be an upper-case name"
        );
        assert_eq!(
            what(file(vec![simple(1, "A", vec![Param::Enum("T.".into())])])),
            "an enumeration must be an upper-case name"
        );
        assert_eq!(
            what(file(vec![simple(1, "A", vec![Param::Binary("4".into())])])),
            "a binary must be 0 to 3, then upper-case hex digits"
        );
        assert_eq!(
            what(file(vec![Instance {
                id: 1,
                record: Record::Complex(vec![]),
            }])),
            "a complex instance needs a record"
        );
        assert_eq!(
            write(&file(vec![simple(4, "A", vec![]), simple(4, "B", vec![])])),
            Err(WriteError::Unwritable {
                instance: Some(4),
                what: "instance number used twice",
            })
        );
    }

    fn name() -> impl Strategy<Value = Box<str>> {
        "[A-Z_][A-Z0-9_]{0,12}".prop_map(Into::into)
    }

    fn param() -> impl Strategy<Value = Param> {
        let leaf = prop_oneof![
            Just(Param::Unset),
            Just(Param::Derived),
            any::<i64>().prop_map(Param::Int),
            any::<f64>()
                .prop_filter("finite", |x| x.is_finite())
                .prop_map(Param::Real),
            any::<String>().prop_map(|s| Param::Str(s.into())),
            name().prop_map(Param::Enum),
            "[0-3][0-9A-F]{0,8}".prop_map(|b| Param::Binary(b.into())),
            any::<u64>().prop_map(Param::Ref),
        ];
        leaf.prop_recursive(4, 32, 6, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..6).prop_map(Param::List),
                (name(), inner).prop_map(|(n, p)| Param::Typed(n, Box::new(p))),
            ]
        })
    }

    fn part_() -> impl Strategy<Value = Part> {
        (
            prop_oneof![name(), name().prop_map(|n| format!("!{n}").into())],
            prop::collection::vec(param(), 0..5),
        )
            .prop_map(|(name, params)| Part { name, params })
    }

    fn table() -> impl Strategy<Value = File> {
        (
            prop::collection::vec(part_(), 0..3),
            prop::collection::btree_set(1..u64::MAX, 0..12),
            prop::collection::vec((any::<bool>(), prop::collection::vec(part_(), 1..4)), 12),
        )
            .prop_map(|(header, ids, records)| File {
                header: Header { entities: header },
                instances: ids
                    .into_iter()
                    .zip(records)
                    .map(|(id, (complex, mut parts))| Instance {
                        id,
                        record: if complex {
                            Record::Complex(parts)
                        } else {
                            Record::Simple(parts.remove(0))
                        },
                    })
                    .collect(),
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]
        #[test]
        fn written_tables_read_back_bit_for_bit(f in table()) {
            let bytes = write(&f).unwrap();
            let back = parse(&bytes).unwrap();
            // Debug shows -0.0 and every bit of a real, where == wouldn't.
            prop_assert_eq!(format!("{back:?}"), format!("{f:?}"));
            prop_assert_eq!(write(&back).unwrap(), bytes);
        }
    }
}
