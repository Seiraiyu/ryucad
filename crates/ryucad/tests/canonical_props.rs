//! Canonical JSON round-trips: whatever ryucad writes, it reads back to
//! the same value and writes again byte for byte.

use proptest::prelude::*;
use ryucad::doc::canonical::{parse, to_string};
use serde_json::{Map, Number, Value};

fn scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::Number(n.into())),
        any::<u64>().prop_map(|n| Value::Number(n.into())),
        // Finite and not -0, which canonical form writes as 0.
        any::<f64>()
            .prop_filter("finite, not -0", |x| x.is_finite()
                && !(*x == 0.0 && x.is_sign_negative()))
            .prop_map(|x| Value::Number(Number::from_f64(x).unwrap())),
        ".{0,12}".prop_map(Value::String),
    ]
}

fn document() -> impl Strategy<Value = Value> {
    scalar().prop_recursive(4, 64, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::btree_map(".{0,8}", inner, 0..6)
                .prop_map(|m| Value::Object(m.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

proptest! {
    #[test]
    fn round_trip(v in document()) {
        let s = to_string(&v);
        let back = parse(&s).unwrap();
        prop_assert_eq!(&back, &v);
        prop_assert_eq!(to_string(&back), s);
    }
}
