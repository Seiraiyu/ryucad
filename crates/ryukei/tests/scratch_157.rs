use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::check;
use ryukei::geom::{Frame, Point3, Rigid, Vector3};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;

#[test]
fn scratch() {
    // A block turned -30 degrees about z: its +y face's normal is (0.5, 0.866..).
    let turn = Rigid::rotation(Vector3::new(0.0, 0.0, 1.0).normalize().unwrap(), -core::f64::consts::PI / 6.0);
    let block = primitive::block(OpKey(1), 0.2, 0.2, 0.06, &turn).unwrap().0;
    let n = [0.5, 0.866025404, 0.0];
    let exact = [0.5, 0.8660254037844386, 0.0];
    for (name, z) in [("off", n), ("exact", exact)] {
        let z = Vector3::new(z[0], z[1], z[2]);
        let centre = turn.apply_point(Point3::new(0.1, 0.1, 0.03));
        let f = Frame::new(centre - z.normalize().unwrap().get() * 0.15, z, Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let rod = primitive::cylinder(OpKey(2), 0.02, 0.2, &Rigid::from_frame(&f)).unwrap().0;
        let r = boolean(OpKey(3), BooleanOp::Subtract, &block, &[Tool { key: ToolKey(1), body: &rod, place: Rigid::IDENTITY }]);
        match r {
            Ok((b, _)) => {
                let m = tessellate(&b, &MeshParams::new(1e-4, 0.3)).unwrap();
                let kinds: Vec<String> = b.edges().map(|e| format!("{:?}", b.curve(b.edge(e).unwrap().curve).unwrap()).chars().take(6).collect()).collect();
                eprintln!("SCRATCH {name}: check {:?} faults {:?} edges {:?}", check(&b), m.faults, kinds);
            }
            Err(e) => eprintln!("SCRATCH {name}: {e:?}"),
        }
    }
}
