//! Every built-in value carries a source, and every module holding values
//! names its sources in its doc.

use ryu_standards::thread::{Pitch, Standard, lookup, sizes};

#[test]
fn every_lookup_value_has_a_source() {
    for s in Standard::ALL {
        for size in sizes(s) {
            // The constant-pitch series has no pitch of its own: give one.
            let pitch = (s == Standard::Un).then_some(Pitch::Tpi(16));
            let t = lookup(s, size, pitch, None).unwrap();
            let mut sources = vec![t.pitch.source];
            sources.extend(t.major.map(|v| v.source));
            if let Some((a, b)) = t.minor {
                sources.extend([a.source, b.source]);
            }
            sources.extend(t.major_min.map(|v| v.source));
            for src in sources {
                assert!(!src.text().is_empty(), "{s:?} {size}");
            }
        }
    }
}

#[test]
fn every_value_module_names_its_sources() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for name in [
        "fit",
        "iso_metric",
        "iso_metric_limits",
        "unified",
        "pipe",
        "drill",
        "tap",
        "thread",
    ] {
        let text = std::fs::read_to_string(src.join(format!("{name}.rs"))).unwrap();
        let doc: String = text.lines().take_while(|l| l.starts_with("//!")).collect();
        assert!(doc.contains("Sources:"), "{name}.rs names no sources");
    }
    // A module added later must be listed above.
    let listed = 8 + ["lib", "source", "exact"].len();
    assert_eq!(std::fs::read_dir(&src).unwrap().count(), listed);
}
