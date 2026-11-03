//! Sheet bodies (design 295, plan 12.A) through the public API: kinds,
//! measures, refusals, the codec.

use ryukei::Error;
use ryukei::euler::{kfl, mvsr};
use ryukei::geom::{Point3, Rigid, Vector3};
use ryukei::intersect::point_in_body;
use ryukei::measure::{
    DistanceOptions, Entity, MassOptions, Properties, distance, mass, properties,
};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::primitive;
use ryukei::topo::{Body, BodyKind, EntityId};

const KEY: OpKey = OpKey(0x5348_4545);

/// The block `1 × 0.6 × 0.4` with its top taken off: a sheet of five
/// faces, area 1.88.
fn open_box() -> Body {
    let (mut b, _) = primitive::block(KEY, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let s = b.shells().next().unwrap();
    mvsr(&mut b, s).unwrap();
    let Some(EntityId::Face(top)) = b.by_name(Name::derive(KEY, Role::Face(FaceRole::PlusZ), &[]))
    else {
        panic!("the top");
    };
    kfl(&mut b, top).unwrap();
    assert_eq!(b.kind(), BodyKind::Sheet);
    b
}

#[test]
fn a_sheet_measures_its_area_and_has_no_mass() {
    let b = open_box();
    let Properties::Sheet { area } = properties(&b, Entity::Body).unwrap() else {
        panic!("a sheet's properties");
    };
    assert!(area.holds(1.88, 1e-12), "{area:?}");
    assert_eq!(
        mass(&b, 1.0, &MassOptions::default()).unwrap_err(),
        Error::NotASolid { op: "mass" }
    );
    assert!(point_in_body(&b, Point3::new(0.5, 0.3, 0.2)).is_err());
}

#[test]
fn a_solid_inside_an_open_box_is_apart_from_it() {
    let b = open_box();
    let place = Rigid::translation(Vector3::new(0.4, 0.2, 0.15));
    let (small, _) = primitive::block(OpKey(0x10), 0.2, 0.2, 0.2, &place).unwrap();
    let d = distance(
        (&b, Entity::Body, &Rigid::IDENTITY),
        (&small, Entity::Body, &Rigid::IDENTITY),
        &DistanceOptions::default(),
    )
    .unwrap();
    // The bottom is 0.15 below it; a solid box would hold it (distance 0).
    assert!(d.distance.holds(0.15, 1e-9), "{:?}", d.distance);
}

#[test]
fn solid_only_operations_refuse_sheets() {
    use ryukei::boolean::{BooleanOp, Tool, boolean};
    use ryukei::name::ToolKey;
    use ryukei::offset::{OffsetSpec, ShellSpec, offset_body, shell};
    let sheet = open_box();
    let not = |op| Err(Error::NotASolid { op });
    assert_eq!(
        ryukei::blend::blend(OpKey(1), &sheet, &[]).map(|_| ()),
        not("blend")
    );
    let spec = OffsetSpec {
        distance: 0.01,
        faces: Vec::new(),
    };
    assert_eq!(
        offset_body(OpKey(2), &sheet, &spec).map(|_| ()),
        not("offset")
    );
    let spec = ShellSpec {
        thickness: 0.01,
        outward: false,
        open: Vec::new(),
        faces: Vec::new(),
    };
    assert_eq!(shell(OpKey(3), &sheet, &spec).map(|_| ()), not("shell"));
    let (solid, _) = primitive::block(OpKey(0x11), 0.2, 0.2, 0.2, &Rigid::IDENTITY).unwrap();
    let tool = |body| Tool {
        key: ToolKey(1),
        body,
        place: Rigid::IDENTITY,
    };
    assert_eq!(
        boolean(OpKey(4), BooleanOp::Unite, &sheet, &[tool(&solid)]).map(|_| ()),
        not("boolean unite")
    );
    assert_eq!(
        boolean(OpKey(5), BooleanOp::Unite, &solid, &[tool(&sheet)]).map(|_| ()),
        not("boolean")
    );
}

#[test]
fn sheets_go_through_the_codec() {
    use ryukei::codec::{decode_body, encode_body};
    use ryukei::sheet::remove;
    let (c, _) = primitive::cylinder(KEY, 0.3, 0.5, &Rigid::IDENTITY).unwrap();
    let face = |r| Name::derive(KEY, Role::Face(r), &[]);
    let (band, _) = remove(
        OpKey(0x30),
        &c,
        &[face(FaceRole::Top), face(FaceRole::Bottom)],
    )
    .unwrap();
    for b in [open_box(), band] {
        let back = decode_body(&encode_body(&b)).unwrap();
        assert_eq!(back.kind(), BodyKind::Sheet);
        assert_eq!(back.dump(), b.dump());
    }
}
