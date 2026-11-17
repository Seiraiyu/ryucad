//! `board export` (design 732): a board's fabrication package. Gerber X2
//! per layer and Excellon drills through `ryu-gerber`, the design's BOM
//! and pick and place (and both in the preset's assembly form), and
//! `manifest.json`: each file's name, function and BLAKE3, the board's,
//! design's and project's revisions, the DRC counts and ryueda's version.
//! Refused with `drc_failed` while the board has a DRC error, unless
//! `allow_errors`, which the manifest records. Each file's text is in the
//! answer; the CLI writes them into `--out`. Nothing exported is
//! committed.

use super::cmd;
use super::{assembly, fab, occ, preset};
use super::shapes::BoardShapes;
use crate::error::{Code, Error, Result};
use crate::pdm::bom::COLUMNS;
use crate::session::Session;
use rayon::prelude::*;
use ryu_gerber::Software;
use ryu_gerber::excellon::SlotForm;
use serde_json::{Value, json};

/// The manifest's file name in `--out`.
pub const MANIFEST: &str = "manifest.json";

/// The vendor in `.GenerationSoftware`.
pub const VENDOR: &str = "Seiraiyu";

/// How slots are written, settled by plan 5.C's Task 23 against gerbv and
/// JLC's stated format (`docs/claude/ryueda-board.md`): JLC's KiCad guide
/// asks for "alternate drill mode for oval holes", the drilled slot (G85).
// PROVISIONAL until Task 23 sees gerbv read it cleanly.
pub const SLOTS: SlotForm = SlotForm::G85;

/// The manifest's name for [`SLOTS`].
pub fn slots_name() -> &'static str {
    match SLOTS {
        SlotForm::G85 => "g85",
        SlotForm::Routed => "routed",
    }
}

/// One file of the package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub name: String,
    pub function: String,
    pub text: String,
}

fn hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex()[..32].to_owned()
}

/// A writer's refusal as ryueda's: past the formats' range is
/// `board_out_of_range`; anything else is ryueda's bug (it never makes
/// what the writer refuses).
pub fn gerber_error(e: ryu_gerber::Error, file: &str) -> Error {
    match e {
        ryu_gerber::Error::OutOfRange(_) => {
            Error::new(Code::BoardOutOfRange, format!("{file}: {e}")).with("file", file)
        }
        other => Error::new(Code::Internal, format!("{file} can't be written: {other}"))
            .with("file", file),
    }
}

/// The Gerber layers and the drill files (each drill file only when it
/// has holes), the layers written in parallel and kept in order.
pub fn fabrication(board: &str, s: &BoardShapes, version: &str) -> Result<Vec<File>> {
    let sw = Software {
        vendor: VENDOR.into(),
        application: "ryueda".into(),
        version: version.into(),
    };
    let layers = fab::layers(board, s)?;
    let mut out: Vec<File> = layers
        .par_iter()
        .map(|lf| {
            let name = format!("{board}-{}.{}", lf.function, lf.ext);
            let text =
                ryu_gerber::gerber::write(&lf.layer, &sw).map_err(|e| gerber_error(e, &name))?;
            Ok(File {
                name,
                function: lf.function.to_owned(),
                text,
            })
        })
        .collect::<Result<_>>()?;
    for (d, function) in fab::drills(s).iter().zip(["pth", "npth"]) {
        if d.holes.is_empty() {
            continue;
        }
        let name = format!("{board}-{function}.drl");
        let text =
            ryu_gerber::excellon::write(d, &sw, SLOTS).map_err(|e| gerber_error(e, &name))?;
        out.push(File {
            name,
            function: function.to_owned(),
            text,
        });
    }
    Ok(out)
}

/// `board.export`: the package, every file's text in the answer.
pub fn package(
    session: &mut Session,
    board: &str,
    allow_errors: bool,
    version: &str,
) -> Result<Value> {
    let id = cmd::find(session.project(), board)?.id.clone();
    let check = session.board_check(&id)?;
    let errors = check.errors();
    let drc = json!({
        "errors": errors,
        "warnings": check.warnings(),
        "counts": cmd::counts(&check.findings),
        "incomplete": check.incomplete,
    });
    if errors > 0 && !allow_errors {
        return Err(Error::new(
            Code::DrcFailed,
            format!(
                "board {id} has {errors} DRC error(s): fix them (`ryueda board drc {id}` lists them) or export with --allow-errors"
            ),
        )
        .with(
            "report",
            json!({"board": format!("board/{id}"), "findings": check.findings, "drc": drc}),
        ));
    }
    let session: &Session = session;
    let p = session.project();
    let b = &p.boards[&id];
    let file = cmd::board_of(b)?;
    let preset = preset::by_name(&file.preset).ok_or_else(|| {
        Error::new(
            Code::InvalidDocument,
            format!("preset {} isn't known", file.preset),
        )
    })?;
    let (n, occs) = occ::occurrences(p, &file.design)?;
    let mut files = fabrication(id.as_str(), &check.shapes, version)?;
    let config = ryupdm::config::Config::read_in(&*session.files(), crate::pdm::NAME)?;
    let lines = assembly::bom_lines(session, p, &n);
    let (rows, left_out) = assembly::rows(&occs, b);
    files.push(File {
        name: format!("{id}-bom.csv"),
        function: "bom".into(),
        text: ryupdm::lines::csv(&lines, &config, &COLUMNS),
    });
    files.push(File {
        name: format!("{id}-pos.csv"),
        function: "pick-and-place".into(),
        text: assembly::pos_csv(&rows),
    });
    if let Some(form) = &preset.assembly {
        files.push(File {
            name: format!("{id}-bom-{}.csv", form.name),
            function: format!("bom-{}", form.name),
            text: assembly::bom_form(&lines, form),
        });
        files.push(File {
            name: format!("{id}-pos-{}.csv", form.name),
            function: format!("pick-and-place-{}", form.name),
            text: assembly::pos_form(&rows, form),
        });
    }
    let listed = |f: &File| {
        json!({"name": f.name, "function": f.function, "blake3": hash(&f.text), "bytes": f.text.len()})
    };
    let manifest = json!({
        "schema": 1,
        "board": format!("board/{id}"),
        "board_revision": b.revision,
        "design": format!("design/{}", file.design),
        "design_netlist": hash(&crate::canonical::to_string(&n.to_json())),
        "project_revision": p.revision,
        "preset": preset.name,
        "slots": slots_name(),
        "drc": drc,
        "allow_errors": allow_errors,
        "left_out": left_out,
        "files": files.iter().map(listed).collect::<Vec<_>>(),
        "ryueda": version,
    });
    files.push(File {
        name: MANIFEST.to_owned(),
        function: "manifest".into(),
        text: crate::canonical::to_string(&manifest),
    });
    let answered: Vec<Value> = files
        .iter()
        .map(|f| {
            let mut v = listed(f);
            v["text"] = json!(f.text);
            v
        })
        .collect();
    Ok(json!({
        "board": format!("board/{id}"),
        "revision": b.revision,
        "files": answered,
        "left_out": left_out,
        "drc": drc,
        "allow_errors": allow_errors,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fabrication_files_are_named_for_the_board_and_read_back() {
        let (_d, p, id) = crate::board::shapes::tests::placed();
        let s = crate::board::shapes::build(&p, &p.boards[&id]).unwrap();
        let files = fabrication("b", &s, "9.9.9").unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(&names[..9], [
            "b-copper-top.gtl", "b-copper-bottom.gbl", "b-mask-top.gts", "b-mask-bottom.gbs",
            "b-paste-top.gtp", "b-paste-bottom.gbp", "b-silk-top.gto", "b-silk-bottom.gbo",
            "b-profile.gm1",
        ]);
        for f in &files[..9] {
            let img = ryu_gerber::read::gerber(&f.text).unwrap_or_else(|e| panic!("{}: {e}", f.name));
            assert_eq!(img.attrs[".GenerationSoftware"], "Seiraiyu,ryueda,9.9.9");
        }
        let top = ryu_gerber::read::gerber(&files[0].text).unwrap();
        let pads = top.objects.iter().filter(|o| o.attrs.contains_key(".P")).count();
        assert_eq!(pads, s.pads.iter().filter(|p| p.top).count());
        for f in files.iter().filter(|f| f.name.ends_with(".drl")) {
            ryu_gerber::read::excellon(&f.text).unwrap_or_else(|e| panic!("{}: {e}", f.name));
        }
        // The same board, the same bytes, on one thread and eight.
        let run = |n| {
            rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap().install(|| {
                fabrication("b", &s, "9.9.9").unwrap().into_iter().map(|f| f.text).collect::<Vec<_>>()
            })
        };
        assert_eq!(run(1), run(8));
    }

    #[test]
    fn past_the_formats_range_is_board_out_of_range() {
        let e = gerber_error(
            ryu_gerber::Error::OutOfRange(ryu_board::geom::Point::new(1 << 40, 0)),
            "b-copper-top.gtl",
        );
        assert_eq!(e.code, Code::BoardOutOfRange);
        assert_eq!(gerber_error(ryu_gerber::Error::Holes, "x").code, Code::Internal);
    }
}
