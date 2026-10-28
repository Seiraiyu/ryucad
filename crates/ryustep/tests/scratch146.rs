use ryukei::heal::seamed;
use ryustep::{ReadOptions, read};

#[test]
#[ignore]
fn scratch() {
    for f in ["nist_ctc_04_asme1_rd.stp", "nist_ctc_05_asme1_rd.stp"] {
        let bytes = std::fs::read(format!("tests/corpus/{f}")).unwrap();
        let m = read(
            &bytes,
            &ReadOptions {
                key: ryukei::name::OpKey(7),
                heal: Default::default(),
            },
        )
        .unwrap();
        for (i, b) in m.bodies.iter().enumerate() {
            let Ok((body, _, _)) = &b.result else {
                continue;
            };
            match seamed(body, 1e-7) {
                Ok(_) => println!("{f} {i} ok"),
                Err(e) => println!("{f} {i} {e:?}"),
            }
        }
    }
}
