//! jlc is a connector: outside its own module, only the connector table
//! names it (the owner, on plan 387's question 248). Tests and comments
//! may; a supplier's name in an offer is the agent's data, not code.

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name() != Some(std::ffi::OsStr::new("jlc")) {
                rust_files(&p, out);
            }
        } else if p.extension() == Some(std::ffi::OsStr::new("rs")) {
            out.push(p);
        }
    }
}

#[test]
fn no_file_outside_the_connector_names_jlc() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for c in ["ryueda/src", "ryueda-parts/src"] {
        rust_files(&root.join(c), &mut files);
    }
    assert!(files.len() > 20, "{files:?}");
    let mut hits = Vec::new();
    // Besides the table: the connector's module declared, and KiCad
    // import's spellings of suppliers' columns (6.A: an offer names its
    // supplier, as files write it; that is data, not a connector).
    let allowed = ["catalogue.rs", "ryueda-parts/src/lib.rs", "kicad/import.rs"];
    for f in files.iter().filter(|f| {
        !allowed
            .iter()
            .any(|a| f.to_string_lossy().replace('\\', "/").ends_with(a))
    }) {
        let text = std::fs::read_to_string(f).unwrap();
        // Code only: a file's tests module is left out.
        let code = text.split("#[cfg(test)]").next().unwrap_or("");
        for (i, line) in code.lines().enumerate() {
            let l = line.to_ascii_lowercase();
            if l.trim_start().starts_with("//") {
                continue;
            }
            // `jlcpcb` is the assembly house (`parts.json`'s `assembly`),
            // not the catalogue.
            let l = l.replace("jlcpcb", "");
            if l.contains("jlc") || l.contains("lcsc") {
                hits.push(format!("{}:{}: {}", f.display(), i + 1, line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "named outside the connector:\n{}",
        hits.join("\n")
    );
}
