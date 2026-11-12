//! Materials and attributes through ryustep (design 448, plan 24.C): two
//! committed files — `vendor.stp`, a block whose vendor gave it a
//! material, a density and an attribute of every type, and
//! `vendor_v2.stp`, its next version — kept equal to what this test
//! makes (`UPDATE_GOLDEN=1` rewrites them), and what they read back as.

use ryukei::geom::Rigid;
use ryukei::name::OpKey;
use ryustep::census::{Bucket, Category, Read};
use ryustep::part21::{self, Record};
use ryustep::properties::Value;
use ryustep::{LengthUnit, Product, WriteModel, WriteOptions, write};

fn opts(name: &str) -> WriteOptions {
    WriteOptions {
        unit: LengthUnit::Millimeter,
        file_name: name.into(),
        time_stamp: "2026-11-11T00:00:00".into(),
        ..Default::default()
    }
}

/// The block, its vendor's properties appended (the next version's when
/// `v2`).
fn vendor(v2: bool) -> Vec<u8> {
    let block = ryukei::primitive::block(OpKey(1), 0.04, 0.02, 0.005, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let model = WriteModel {
        products: vec![Product {
            id: "VB-100".into(),
            name: "vendor bracket".into(),
            bodies: vec![block],
            ..Default::default()
        }],
    };
    let name = if v2 { "vendor_v2.stp" } else { "vendor.stp" };
    let mut file = part21::parse(&write(&model, &opts(name)).unwrap()).unwrap();
    let first = |name: &str| {
        file.instances
            .iter()
            .find(|i| matches!(&i.record, Record::Simple(p) if &*p.name == name))
            .unwrap()
            .id
    };
    let (pd, pds) = (first("PRODUCT_DEFINITION"), first("PRODUCT_DEFINITION_SHAPE"));
    let n = file.instances.len() as u64;
    let k = |j: u64| n + j;
    let (supplier, density, thickness) = if v2 {
        ("Globex", "2.71", "3.")
    } else {
        ("ACME Metals", "2.7", "2.5")
    };
    let batch = if v2 {
        String::new()
    } else {
        format!(",#{}", k(16))
    };
    let data = format!(
        "#{a1}=MATERIAL_DESIGNATION('6061-T6',#{pds});\
         #{a2}=PROPERTY_DEFINITION('material property','density',#{pds});\
         #{a3}=PROPERTY_DEFINITION_REPRESENTATION(#{a2},#{a4});\
         #{a4}=REPRESENTATION('density',(#{a5}),#{a6});\
         #{a5}=MEASURE_REPRESENTATION_ITEM('density',POSITIVE_RATIO_MEASURE({density}),#{a7});\
         #{a6}=REPRESENTATION_CONTEXT('','');\
         #{a7}=DERIVED_UNIT((#{a8},#{a9}));\
         #{a8}=DERIVED_UNIT_ELEMENT(#{a10},1.);\
         #{a9}=DERIVED_UNIT_ELEMENT(#{a11},-3.);\
         #{a10}=(MASS_UNIT()NAMED_UNIT(*)SI_UNIT($,.GRAM.));\
         #{a11}=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.CENTI.,.METRE.));\
         #{a12}=PROPERTY_DEFINITION('user defined attributes','',#{pd});\
         #{a13}=PROPERTY_DEFINITION_REPRESENTATION(#{a12},#{a14});\
         #{a14}=REPRESENTATION('',(#{a15}{batch},#{a17},#{a18},#{a19},#{a20},#{a21},#{a22}),#{a6});\
         #{a15}=DESCRIPTIVE_REPRESENTATION_ITEM('Supplier','{supplier}');\
         #{a16}=VALUE_REPRESENTATION_ITEM('Batch Size',COUNT_MEASURE(250.));\
         #{a17}=VALUE_REPRESENTATION_ITEM('Yield',RATIO_MEASURE(0.95));\
         #{a18}=VALUE_REPRESENTATION_ITEM('RoHS',BOOLEAN(.T.));\
         #{a19}=MEASURE_REPRESENTATION_ITEM('Wall-Thickness',LENGTH_MEASURE({thickness}),#{a23});\
         #{a20}=(MEASURE_REPRESENTATION_ITEM()MEASURE_WITH_UNIT(MASS_MEASURE(0.12),#{a24})REPRESENTATION_ITEM('Net Mass'));\
         #{a21}=INTEGER_REPRESENTATION_ITEM('Revision Count',3);\
         #{a22}=VALUE_REPRESENTATION_ITEM('Broken',$);\
         #{a23}=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));\
         #{a24}=(MASS_UNIT()NAMED_UNIT(*)SI_UNIT(.KILO.,.GRAM.));\
         #{a25}=PROPERTY_DEFINITION('user defined attributes','',#{a26});\
         #{a26}=SHAPE_ASPECT('hole','',#{pds},.F.);\
         #{a27}=PROPERTY_DEFINITION_REPRESENTATION(#{a25},#{a28});\
         #{a28}=REPRESENTATION('',(#{a29}),#{a6});\
         #{a29}=DESCRIPTIVE_REPRESENTATION_ITEM('Thread','M4');",
        a1 = k(1), a2 = k(2), a3 = k(3), a4 = k(4), a5 = k(5), a6 = k(6), a7 = k(7),
        a8 = k(8), a9 = k(9), a10 = k(10), a11 = k(11), a12 = k(12), a13 = k(13),
        a14 = k(14), a15 = k(15), a16 = k(16), a17 = k(17), a18 = k(18), a19 = k(19),
        a20 = k(20), a21 = k(21), a22 = k(22), a23 = k(23), a24 = k(24), a25 = k(25),
        a26 = k(26), a27 = k(27), a28 = k(28), a29 = k(29),
    );
    let more = part21::parse(
        format!(
            "ISO-10303-21;HEADER;FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF'));ENDSEC;DATA;{data}ENDSEC;END-ISO-10303-21;"
        )
        .as_bytes(),
    )
    .unwrap();
    // v2 leaves Batch Size's item out of the representation but keeps
    // its instance: unreferenced, as a vendor's leftovers are.
    file.instances.extend(more.instances);
    part21::write(&file).unwrap()
}

fn kept(name: &str, bytes: &[u8]) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/properties")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
    }
    let on_disk =
        std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e} (UPDATE_GOLDEN=1 writes it)"));
    assert!(on_disk == bytes, "{name} isn't what this test makes now: UPDATE_GOLDEN=1");
}

#[test]
fn the_committed_fixtures_are_what_this_test_makes() {
    kept("vendor.stp", &vendor(false));
    kept("vendor_v2.stp", &vendor(true));
}

#[test]
fn vendor_reads_its_material_and_attributes() {
    let o = ryustep::open(&vendor(false)).unwrap();
    let p = &o.properties().products[&0];
    let m = p.material.as_ref().unwrap();
    assert_eq!((m.name.as_str(), m.density), ("6061-T6", Some(2700.0)));
    let names: Vec<&str> = p.attributes.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        ["Supplier", "Batch Size", "Yield", "RoHS", "Wall-Thickness", "Net Mass", "Revision Count"]
    );
    assert_eq!(p.attributes[1].value, Value::Integer(250));
    assert_eq!(o.properties().failed.len(), 1, "Broken");
    let v2 = ryustep::open(&vendor(true)).unwrap();
    let p2 = &v2.properties().products[&0];
    assert_eq!(p2.material.as_ref().unwrap().density, Some(2710.0));
    assert!(p2.attributes.iter().all(|a| a.name != "Batch Size"));
}

#[test]
fn the_census_counts_them_by_category() {
    let o = ryustep::open(&vendor(false)).unwrap();
    let t = o.census.transfer(&Read { product_tree: true, ..Default::default() }, &[]);
    let line = |c: Category| t.lines.iter().find(|l| l.category == c).unwrap();
    // Nothing interprets them yet: kept in the bytes.
    let materials = line(Category::Materials);
    assert!(materials.buckets[Bucket::Retained as usize].count >= 5, "{materials:?}");
    let props = line(Category::Properties);
    assert!(props.buckets[Bucket::Retained as usize].count >= 10, "{props:?}");
    assert_eq!(props.buckets[Bucket::Interpreted as usize].count, 0);
}
