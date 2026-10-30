//! `tests/corpus/manifest.txt`: what each board is, and the counts it
//! reads to.

use std::collections::BTreeMap;

use ryu_board::model::{Board, FeatureKind};

/// A board's counts, as `tests/corpus/manifest.txt` records them under
/// its declaration: its version and writer, steps, the board step's
/// layers, features by kind, components, nets, drill tools, stackup,
/// capabilities, and its faults by code.
pub fn summary(b: &Board) -> String {
    let s = &b.steps[b.board_step];
    let mut kinds = [0usize; 6];
    for l in &s.layers {
        for f in l.features() {
            kinds[match f.kind {
                FeatureKind::Line { .. } => 0,
                FeatureKind::Pad { .. } => 1,
                FeatureKind::Arc { .. } => 2,
                FeatureKind::Surface { .. } => 3,
                FeatureKind::Text(_) => 4,
                FeatureKind::Barcode(_) => 5,
            }] += 1;
        }
    }
    let c = b.capabilities;
    let caps: Vec<&str> = [
        (c.geometry, "geometry"),
        (c.nets, "nets"),
        (c.components, "components"),
        (c.stackup, "stackup"),
        (c.drill, "drill"),
        (c.flex, "flex"),
        (c.feature_ids, "feature_ids"),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, n)| *n)
    .collect();
    let mut faults: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &b.faults {
        *faults.entry(f.code.as_str()).or_default() += 1;
    }
    let faults: Vec<String> = faults.iter().map(|(k, n)| format!("{k}={n}")).collect();
    let steps: Vec<&str> = b.steps.iter().map(|s| s.name.as_str()).collect();
    let pins: usize = s.components.iter().map(|c| c.pins.len()).sum();
    let tools: usize = b.drills.iter().map(|d| d.tools.len()).sum();
    format!(
        "  version {} {}\n  steps {} (board {})\n  layers {}\n  features line={} pad={} arc={} surface={} text={} barcode={}\n  components {} pins={} packages={}\n  nets {} cad_points={}\n  drills {} tools={}\n  stackup {}\n  capabilities {}\n  faults {}\n",
        b.source.version,
        b.source.application.as_deref().unwrap_or("-"),
        steps.join(","),
        s.name,
        b.layers.len(),
        kinds[0],
        kinds[1],
        kinds[2],
        kinds[3],
        kinds[4],
        kinds[5],
        s.components.len(),
        pins,
        s.packages.len(),
        s.nets.len(),
        s.nets.iter().map(|n| n.cad.len()).sum::<usize>(),
        b.drills.len(),
        tools,
        b.stackup.as_ref().map_or(0, |s| s.layers.len()),
        if caps.is_empty() {
            "-".into()
        } else {
            caps.join(",")
        },
        if faults.is_empty() {
            "-".into()
        } else {
            faults.join(",")
        },
    )
}

/// What `manifest.txt` declares of a board, before its counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declared {
    pub name: String,
    /// Where it comes from: its author, origin and what was done to it.
    pub source: Vec<String>,
    /// An SPDX id whose text is `licenses/<id>.txt`, or `owner`.
    pub licence: Option<String>,
    /// An archive in `tests/corpus` and its SHA-256.
    pub archive: Option<(String, String)>,
    /// A folder in `tests/corpus`, the same job unpacked.
    pub folder: Option<String>,
    /// The board step, where the rule can't choose it.
    pub step: Option<String>,
    /// Fetched by `scripts/fetch-dfm-corpus.sh` (from `pinned.txt`), not
    /// committed.
    pub pinned: bool,
    /// The count lines recorded.
    pub counts: String,
}

const KEYS: [&str; 6] = ["source", "licence", "archive", "folder", "step", "pinned"];

/// Reads `manifest.txt`: a block per board, `board <name>` then its
/// declaration (indented `source`, `licence`, `archive <file> <sha256>`,
/// `folder`, `step` and `pinned` lines) and its counts.
pub fn manifest(text: &str) -> Vec<Declared> {
    let mut out: Vec<Declared> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("board ") {
            out.push(Declared {
                name: name.trim().to_string(),
                ..Declared::default()
            });
            continue;
        }
        let Some(d) = out.last_mut() else { continue };
        let t = line.trim();
        let (key, rest) = t.split_once(' ').unwrap_or((t, ""));
        match key {
            "source" => d.source.push(rest.to_string()),
            "licence" => d.licence = Some(rest.to_string()),
            "archive" => {
                let (f, h) = rest.split_once(' ').unwrap_or((rest, ""));
                d.archive = Some((f.to_string(), h.trim().to_string()));
            }
            "folder" => d.folder = Some(rest.to_string()),
            "step" => d.step = Some(rest.to_string()),
            "pinned" => d.pinned = true,
            _ if !t.is_empty() => {
                d.counts.push_str(line);
                d.counts.push('\n');
            }
            _ => {}
        }
    }
    out
}

/// Checks the counts of the boards in `got` (name and [`summary`])
/// against `manifest.txt` at `path`, returning those that differ. With
/// `UPDATE_MANIFEST` set, writes their counts as read instead, keeping
/// every declaration and every other board's block.
pub fn check_manifest(path: &std::path::Path, got: &[(String, String)]) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let declared = manifest(&text);
    let differing: Vec<String> = got
        .iter()
        .filter(|(name, counts)| {
            declared.iter().find(|d| d.name == *name).map(|d| &d.counts) != Some(counts)
        })
        .map(|(n, _)| n.clone())
        .collect();
    if std::env::var_os("UPDATE_MANIFEST").is_none() {
        return differing;
    }
    // Rewrite each block: its declaration lines as written, then counts.
    let mut out = String::new();
    let mut block: Option<String> = None;
    let flush = |out: &mut String, block: &Option<String>| {
        if let Some(name) = block {
            let counts = got
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, c)| c.clone())
                .or_else(|| {
                    declared
                        .iter()
                        .find(|d| d.name == *name)
                        .map(|d| d.counts.clone())
                })
                .unwrap_or_default();
            out.push_str(&counts);
            out.push('\n');
        }
    };
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("board ") {
            flush(&mut out, &block);
            block = Some(name.trim().to_string());
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let key = line.trim().split(' ').next().unwrap_or("");
        // Before the first board, everything; within one, its declaration.
        if block.is_none() || KEYS.contains(&key) {
            out.push_str(line);
            out.push('\n');
        }
    }
    flush(&mut out, &block);
    std::fs::write(path, out.trim_end().to_string() + "\n").unwrap();
    Vec::new()
}
