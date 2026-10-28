use ryukei::check::{Fault, check, check_intersections};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
#[test]
fn one() {
    let log = std::fs::read_to_string("/tmp/s133/cmp.log").unwrap();
    let mut n = 0;
    for l in log
        .lines()
        .filter(|l| l.starts_with("CASE") && l.contains("new Ok"))
    {
        let a = l.find('[').unwrap();
        let b = l.find(']').unwrap();
        let data: Vec<u8> = l[a + 1..b]
            .split(", ")
            .map(|x| x.parse().unwrap())
            .collect();
        let (body, small) = ryukei_testkit::along_bytes::carried(&data).unwrap();
        let c = check(&body); if !c.is_empty() { eprintln!("CHECKFAULT {data:?} {c:?} old-ok={}", l.contains("old Ok")); continue; }
        let params = MeshParams {
            max_triangles: 20_000,
            ..MeshParams::new((small / 8.0).clamp(1e-3, 0.05), 0.5)
        };
        match tessellate(&body, &params) {
            Ok(m) => {
                if !m.faults.is_empty() { eprintln!("MESHFAULT {data:?} {:?} old-ok={}", m.faults, l.contains("old Ok")); continue; }
                assert_eq!(check_mesh(&body, &m), Ok(()), "{data:?}");
            }
            Err(ryukei::Error::MeshTooLarge { .. }) => {}
            Err(e) => panic!("{data:?} {e:?}"),
        }
        if l.contains("old Err") {
            for f in check_intersections(&body) {
                assert!(matches!(f, Fault::CannotCheck { .. }), "{data:?} {f:?}");
            }
        }
        n += 1;
    }
    eprintln!("checked {n}");
}
