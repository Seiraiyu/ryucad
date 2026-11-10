//! ryu-draw's DXF read back by the test reader `ryuformats`' tests use:
//! every item kind on its layer, every curve its own entity, the same
//! bytes twice.

#[path = "../../ryuformats/tests/common/dxf.rs"]
#[allow(dead_code)]
mod reader;

use ryu_draw::dxf::{Unit, write};
use ryu_draw::{Anchor, Colour, Item, LineKind, Path, Segment, Standard};
use std::f64::consts::PI;

fn sheet() -> Vec<Item> {
    let s = Standard::Asme;
    let path = |p: Path, k: LineKind| Item::Path {
        path: p,
        stroke: Some(s.stroke(k)),
        fill: None,
    };
    let mut warn = s.stroke(LineKind::Visible);
    warn.colour = Colour(0xc0, 0x40, 0x00);
    vec![
        path(
            Path::polyline(&[[10.0, 10.0], [100.0, 10.0], [100.0, 60.0]], true),
            LineKind::Visible,
        ),
        path(Path::circle([50.0, 40.0], 8.0), LineKind::Hidden),
        path(
            Path {
                start: [70.0, 40.0],
                segments: vec![Segment::Arc {
                    centre: [60.0, 40.0],
                    radius: 10.0,
                    from: 0.0,
                    sweep: -PI / 2.0,
                }],
                closed: false,
            },
            LineKind::Centre,
        ),
        path(
            Path {
                start: [130.0, 40.0],
                segments: vec![Segment::Ellipse {
                    centre: [120.0, 40.0],
                    a: [10.0, 0.0],
                    b: [0.0, 4.0],
                    from: 0.0,
                    sweep: PI,
                }],
                closed: false,
            },
            LineKind::Phantom,
        ),
        path(
            Path {
                start: [10.0, 100.0],
                segments: vec![Segment::Bezier {
                    points: vec![[10.0, 110.0], [0.0, 110.0]],
                    weights: Some(vec![1.0, std::f64::consts::FRAC_1_SQRT_2, 1.0]),
                }],
                closed: false,
            },
            LineKind::Hatch,
        ),
        Item::Path {
            path: Path::polyline(&[[0.0, 0.0], [5.0, 0.0]], false),
            stroke: Some(warn),
            fill: None,
        },
        Item::Path {
            path: Path::polyline(&[[0.0, 0.0], [3.5, 1.0], [3.5, -1.0]], true),
            stroke: None,
            fill: Some(Colour::BLACK),
        },
        Item::Text {
            text: "SECTION A-A".into(),
            at: [50.0, 5.0],
            size: 3.5,
            angle: 0.0,
            anchor: Anchor::Middle,
            colour: Colour::BLACK,
        },
    ]
}

#[test]
fn every_item_reads_back_on_its_layer_as_its_own_kind() {
    let bytes = write([431.8, 279.4], &sheet(), Unit::Mm).unwrap();
    assert_eq!(bytes, write([431.8, 279.4], &sheet(), Unit::Mm).unwrap());
    let d = reader::read(&bytes);
    assert_eq!(d.var("$INSUNITS").trim(), "4");
    let layers: Vec<&str> = d.layers.iter().map(|l| l.name.as_str()).collect();
    for l in [
        "VISIBLE", "HIDDEN", "CENTRE", "PHANTOM", "HATCH", "CUTTING", "TEXT",
    ] {
        assert!(layers.contains(&l), "{l} in {layers:?}");
    }
    // The closed polyline: its two sides and the side back.
    assert_eq!(
        d.on("VISIBLE").iter().filter(|e| e.kind == "LINE").count(),
        4
    );
    let c = &d.on("HIDDEN")[0];
    assert_eq!((c.kind.as_str(), c.f(40)), ("CIRCLE", 8.0));
    // Clockwise a quarter from 0: counterclockwise from 270 to 360 (0).
    let a = &d.on("CENTRE")[0];
    assert_eq!(a.kind, "ARC");
    assert_eq!((a.f(50), a.f(51)), (270.0, 0.0));
    let e = &d.on("PHANTOM")[0];
    assert_eq!(e.kind, "ELLIPSE");
    assert_eq!((e.f(11), e.f(40), e.f(41)), (10.0, 0.4, 0.0));
    assert!((e.f(42) - PI).abs() < 1e-12);
    let s = &d.on("HATCH")[0];
    assert_eq!(s.kind, "SPLINE");
    assert_eq!((s.f(71), s.all(41).len(), s.all(10).len()), (2.0, 3, 3));
    let t = &d.on("TEXT")[0];
    assert_eq!(
        (t.kind.as_str(), t.text(1), t.text(7)),
        ("TEXT", "SECTION A-A", "Inter")
    );
    // The arrowhead is a SOLID; the warning line carries ACI 30.
    assert_eq!(d.entities.iter().filter(|e| e.kind == "SOLID").count(), 1);
    assert!(
        d.entities
            .iter()
            .any(|e| e.groups.iter().any(|(c, v)| *c == 62 && v.trim() == "30"))
    );
}

#[test]
fn inches_scale_every_length() {
    let d = reader::read(&write([431.8, 279.4], &sheet(), Unit::In).unwrap());
    assert_eq!(d.var("$INSUNITS").trim(), "1");
    let c = &d.on("HIDDEN")[0];
    assert!((c.f(40) - 8.0 / 25.4).abs() < 1e-15);
}
