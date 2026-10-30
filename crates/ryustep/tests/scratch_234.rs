use ryukei::name::OpKey;
use ryustep::{ReadOptions, open};

#[test]
fn scratch() {
    let bytes = std::fs::read("/tmp/cox-8f-plate.step").unwrap();
    let o = open(&bytes).unwrap();
    for key in ["120x120mm-mic6", "plate"] {
        let k = OpKey::from_bytes(format!("ryucad.feature\0{key}\0body").as_bytes());
        let opts = ReadOptions {
            key: k,
            heal: Default::default(),
        };
        let r = o.body(0, &opts);
        eprintln!(
            "SCRATCH {key}: {:?}",
            r.result
                .as_ref()
                .map(|(b, _, rep)| (b.counts(), rep.seams.len(), rep.joined.len()))
        );
    }
}
