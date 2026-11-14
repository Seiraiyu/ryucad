//! Expressions from bytes. Any text must either parse to an expression
//! whose canonical text parses back to itself and evaluates to the same
//! bits, or give a structured error. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryucad::error::Result;
use ryucad::expr::{Expr, Val, no_parameters};
use ryucad::units::{Dim, Quantity, Unit};

/// Names by their first letter: `l` a length, `a` an angle, `n` a number.
fn lookup(name: &str) -> Result<Val> {
    match name.as_bytes().first() {
        Some(b'l') => Ok(Val::new(0.0125, Dim::LENGTH)),
        Some(b'a') => Ok(Val::new(-0.5, Dim::ANGLE)),
        Some(b'n') => Ok(Val::new(1e300, Dim::NUMBER)),
        _ => no_parameters(name),
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = Quantity::eval(text, Unit::Mm, &lookup);
    let Ok(e) = Expr::parse(text) else {
        return;
    };
    let again = Expr::parse(e.text()).expect("canonical text parses");
    assert_eq!(again.text(), e.text());
    match (e.eval(&lookup), again.eval(&lookup)) {
        (Ok(a), Ok(b)) => {
            assert_eq!(a.x.to_bits(), b.x.to_bits());
            assert_eq!(a.dim, b.dim);
            assert!(a.x.is_finite());
        }
        (Err(a), Err(b)) => assert_eq!(a, b),
        (a, b) => panic!("{text:?}: {a:?} vs {b:?}"),
    }
    for name in e.names() {
        let renamed = e.rename(&name, "zz");
        assert!(Expr::parse(renamed.text()).is_ok());
    }
});
