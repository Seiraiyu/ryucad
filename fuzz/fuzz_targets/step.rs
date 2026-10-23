//! STEP files from bytes. Reading must never panic: a file is a table or
//! a `StepError`. A table must write and read back to itself bit for bit,
//! and decoding it, building its product tree and reading the units of
//! every context must give values or `ImportError`s, never a panic.
//! Reading its bodies must give, for each representation, an
//! `ImportError` or a body that passes the checker.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::name::OpKey;
use ryustep::part21;
use ryustep::products::Products;
use ryustep::schema::{Entities, Entity, Schema};
use ryustep::{ReadOptions, read};

fuzz_target!(|data: &[u8]| {
    let Ok(file) = part21::parse(data) else {
        return;
    };
    let _ = Schema::of(&file.header);
    let bytes = part21::write(&file).expect("a table read from a file writes");
    let back = part21::parse(&bytes).expect("written tables read");
    assert_eq!(format!("{back:?}"), format!("{file:?}"));
    assert_eq!(part21::write(&back).unwrap(), bytes);
    let e = Entities::decode(&file);
    let p = Products::build(&e);
    for d in &p.definitions {
        for s in &d.shapes {
            let _ = e.units(s.representation, s.context);
        }
    }
    for &id in e.ids() {
        if let Ok(Entity::Context { .. }) = e.get(id, id) {
            let _ = e.units(id, id);
        }
        if let Ok(Entity::Unit(_)) = e.get(id, id) {
            let _ = e.unit_factor(id, id);
        }
    }
    let opts = ReadOptions {
        key: OpKey(1),
        heal: Default::default(),
    };
    if let Ok(model) = read(data, &opts) {
        for b in &model.bodies {
            if let Ok((body, _, _)) = &b.result {
                assert_eq!(ryukei::check::check(body), [], "a body read must check");
            }
        }
    }
});
