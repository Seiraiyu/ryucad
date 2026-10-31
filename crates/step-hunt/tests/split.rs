//! The splitter's promise: every body of the committed corpus and the OCCT
//! files, split out and read alone, heals to what it heals to in its file.

use std::path::Path;

use ryukei::name::OpKey;
use ryustep::ReadOptions;
use step_hunt::sources::Source;

fn opts() -> ReadOptions {
    ReadOptions {
        key: OpKey(7),
        heal: Default::default(),
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn bodies_split_out_heal_as_in_their_file() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = tempfile::tempdir().unwrap();
    for (name, folder) in [
        ("committed", "crates/ryustep/tests/corpus"),
        ("occt", "crates/ryustep/tests/data/occt"),
    ] {
        let source = Source {
            name: name.into(),
            root: folder.into(),
            glob: "*.stp".into(),
            licence: "LicenseRef-test".into(),
            whole: false,
        };
        let split = step_hunt::split::split(&[source], &root, &dir.path().join(name)).unwrap();
        let mut checked = 0;
        // Each file healed once, whatever number of bodies it holds.
        let mut read: std::collections::HashMap<String, ryustep::Model> = Default::default();
        for case in split
            .cases
            .values()
            .filter(|c| c.kind == step_hunt::split::Kind::Body)
        {
            let alone = ryustep::read(&std::fs::read(&case.path).unwrap(), &opts()).unwrap();
            assert_eq!(alone.bodies.len(), 1, "{}", case.id);
            for o in &case.origins {
                let whole = read.entry(o.file.clone()).or_insert_with(|| {
                    ryustep::read(
                        &std::fs::read(root.join(folder).join(&o.file)).unwrap(),
                        &opts(),
                    )
                    .unwrap()
                });
                let there = whole
                    .bodies
                    .iter()
                    .find(|b| b.representation == o.representation)
                    .unwrap();
                let counts = |r: &Result<_, _>| match r {
                    Ok((b, _, _)) => Ok(ryukei::topo::Body::counts(b)),
                    // Instance numbers differ between the files; the refusal's kind
                    // doesn't.
                    Err(e) => Err(format!("{e:?}").split('{').next().unwrap().to_owned()),
                };
                assert_eq!(
                    counts(&alone.bodies[0].result),
                    counts(&there.result),
                    "{} from {} #{}",
                    case.id,
                    o.file,
                    o.representation
                );
                checked += 1;
            }
        }
        assert!(checked > 5, "{name}: {checked} bodies checked");
    }
}
