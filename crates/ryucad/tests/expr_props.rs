//! Expressions as written by anyone: random trees, spaced at random. The
//! canonical text must parse back to itself, ignore how the original was
//! spaced, and evaluate to the same bits.

use proptest::prelude::*;
use ryucad::error::Result;
use ryucad::expr::{Expr, Val, no_parameters};
use ryucad::units::Dim;

/// Parameters of each kind: `len*` lengths, `ang*` angles, the rest
/// numbers.
fn lookup(name: &str) -> Result<Val> {
    if name.starts_with("len") {
        Ok(Val::new(0.0125, Dim::LENGTH))
    } else if name.starts_with("ang") {
        Ok(Val::new(0.25, Dim::ANGLE))
    } else if name.starts_with('n') {
        Ok(Val::new(3.0, Dim::NUMBER))
    } else {
        no_parameters(name)
    }
}

fn pick(items: &'static [&'static str]) -> impl Strategy<Value = &'static str> + Clone {
    prop::sample::select(items)
}

fn space() -> impl Strategy<Value = &'static str> + Clone {
    pick(&["", " ", "  ", "\t"])
}

fn leaf() -> impl Strategy<Value = String> {
    prop_oneof![
        (1u32..1000, pick(&["", " mm", "in", " deg", " rad", "m"]))
            .prop_map(|(n, u)| format!("{n}{u}")),
        (0u32..100, 1u32..1000).prop_map(|(a, b)| format!("{a}.{b}")),
        pick(&["len", "len2", "ang", "n", "pi"]).prop_map(str::to_owned),
    ]
}

fn tree() -> impl Strategy<Value = String> {
    leaf().prop_recursive(5, 40, 3, |inner| {
        prop_oneof![
            (
                inner.clone(),
                space(),
                pick(&["+", "-", "*", "/"]),
                space(),
                inner.clone()
            )
                .prop_map(|(a, s1, op, s2, b)| format!("{a}{s1}{op}{s2}{b}")),
            (inner.clone(), space()).prop_map(|(a, s)| format!("({s}{a}{s})")),
            inner.clone().prop_map(|a| format!("-{a}")),
            (inner.clone(), 1u32..4).prop_map(|(a, n)| format!("({a})^{n}")),
            (pick(&["sin", "cos", "sqrt", "abs", "atan"]), inner.clone())
                .prop_map(|(f, a)| format!("{f}({a})")),
            (
                pick(&["min", "max", "atan2"]),
                inner.clone(),
                space(),
                inner
            )
                .prop_map(|(f, a, s, b)| format!("{f}({a},{s}{b})")),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn canonical_text_is_stable(s in tree()) {
        let e = Expr::parse(&s).unwrap();
        let again = Expr::parse(e.text()).unwrap();
        prop_assert_eq!(again.text(), e.text());
        // Spacing never matters.
        let squeezed: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        // Except between a number and its unit, which squeezing joins
        // anyway: "3 mm" and "3mm" are the same literal.
        let squeezed = Expr::parse(&squeezed).unwrap();
        prop_assert_eq!(squeezed.text(), e.text());
    }

    #[test]
    fn canonical_text_evaluates_alike(s in tree()) {
        let e = Expr::parse(&s).unwrap();
        let again = Expr::parse(e.text()).unwrap();
        match (e.eval(&lookup), again.eval(&lookup)) {
            (Ok(a), Ok(b)) => {
                prop_assert_eq!(a.x.to_bits(), b.x.to_bits());
                prop_assert_eq!(a.dim, b.dim);
                prop_assert!(a.x.is_finite());
            }
            (Err(a), Err(b)) => prop_assert_eq!(a, b),
            (a, b) => prop_assert!(false, "{s}: {a:?} vs {b:?}"),
        }
    }

    #[test]
    fn renaming_there_and_back_is_the_identity(s in tree()) {
        let e = Expr::parse(&s).unwrap();
        let there = e.rename("len", "zz_tmp");
        prop_assert!(!there.names().contains("len"));
        let back = there.rename("zz_tmp", "len");
        prop_assert_eq!(back.text(), e.text());
    }
}
