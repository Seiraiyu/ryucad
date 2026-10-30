use ryukei::geom::Rigid;
use ryukei::name::OpKey;
use ryustep::{LengthUnit, Product, WriteModel, WriteOptions, write};

#[test]
fn scratch() {
    let mut c = ryukei::primitive::cylinder(OpKey(4), 0.5, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    ryukei_testkit::nurbs::convert_body(&mut c).unwrap();
    let bytes = write(
        &WriteModel {
            products: vec![Product {
                id: "c".into(),
                name: "c".into(),
                bodies: vec![c],
                children: vec![],
                ..Default::default()
            }],
        },
        &WriteOptions {
            unit: LengthUnit::Millimeter,
            time_stamp: "2026-10-30T00:00:00".into(),
            ..WriteOptions::default()
        },
    )
    .unwrap();
    std::fs::write("/tmp/cox-8f-cyl.stp", bytes).unwrap();
}
