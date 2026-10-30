use ryuformats::Unit;
use ryuformats::usd::stage::{Model, Options, Product, Solid, layer};
use ryuformats::usd::usda;
use ryukei_testkit::nurbs_race;
use std::sync::Arc;

#[test]
fn scratch() {
    let dir = std::path::Path::new("/work/target/scratch-usd");
    std::fs::create_dir_all(dir).unwrap();
    let opts = Options {
        unit: Unit::Meter,
        brep: true,
        mesh: false,
        pipe_tolerance: 1e-7,
        layer_data: vec![],
    };
    for (name, b) in [
        ("block", nurbs_race::block()),
        ("pin", nurbs_race::clear_pin()),
        ("flush", nurbs_race::flush_pin()),
    ] {
        let model = Model {
            products: vec![Product {
                id: name.into(),
                name: name.into(),
                part: name.into(),
                label: None,
                solids: vec![Solid {
                    name: "s".into(),
                    body: Arc::new(b),
                    mesh: None,
                }],
                children: vec![],
            }],
        };
        let l = layer(&model, &opts).unwrap();
        std::fs::write(
            dir.join(format!("{name}.usda")),
            usda::to_string(&l).unwrap(),
        )
        .unwrap();
    }
}
