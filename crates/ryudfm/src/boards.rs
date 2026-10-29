//! `board list`, `show`, `edit` and `delete`: a board's file and input, as
//! the project holds them.

use crate::error::{Code, Error, Result};
use crate::project::check_revision;
use crate::project::{BOARD_FILE, Entry, Project, board_dir};
use crate::session::{Session, path_of};
use crate::source::{join, write_atomic};
use serde_json::{Value, json};

/// A board as its files say: its file's fields, its revision, and
/// `error` where it can't be read.
fn entry_json(e: &Entry) -> Value {
    let mut v = json!({"board": e.id, "revision": e.revision});
    if let Ok(f) = &e.file {
        v["name"] = json!(f.name);
        v["step"] = json!(f.step);
        v["input"] = json!(f.input);
        v["source"] = json!(f.source);
    }
    if let Err(err) = e.ready() {
        v["error"] = json!(err);
    }
    v
}

pub fn list(p: &Project) -> Value {
    json!({
        "revision": p.revision,
        "boards": p.boards.values().map(entry_json).collect::<Vec<_>>(),
    })
}

pub fn show(p: &Project, board: &str) -> Result<Value> {
    let e = p.board(board)?;
    let mut v = entry_json(e);
    v["bytes"] = json!(e.input.as_ref().map(|i| i.len()).ok());
    if let Some(h) = &e.hash {
        v["found"] = json!(h);
    }
    Ok(v)
}

/// Gives a board another name or board step. A step must be one the
/// board's job has, which reading it with that step proves.
pub fn edit(
    session: &mut Session,
    board: &str,
    name: Option<&str>,
    step: Option<&str>,
    expected: Option<&str>,
) -> Result<Value> {
    if name.is_none() && step.is_none() {
        return Err(Error::new(
            Code::InvalidParam,
            "nothing to change: give a name or a step",
        ));
    }
    let e = session.project().board(board)?;
    check_revision(&format!("board {board}"), &e.revision, expected)?;
    let mut file = e.file.clone()?;
    if let Some(n) = name {
        if n.trim().is_empty() || n.contains('\n') {
            return Err(
                Error::new(Code::InvalidParam, "a board's name is one line of text")
                    .with("param", "name"),
            );
        }
        file.name = n.to_owned();
    }
    if let Some(s) = step {
        let (_, input) = e.ready()?;
        let b = crate::session::read(input, board, s)?;
        file.step = b.steps[b.board_step].name.clone();
    }
    let rel = join(&board_dir(board), BOARD_FILE);
    write_atomic(&path_of(session.root(), &rel), file.to_text())?;
    session.reload()?;
    show(session.project(), board)
}

/// Deletes a board's folder: its file and input.
pub fn delete(session: &mut Session, board: &str, expected: Option<&str>) -> Result<Value> {
    let e = session.project().board(board)?;
    check_revision(&format!("board {board}"), &e.revision, expected)?;
    let dir = format!("{}/", board_dir(board));
    let files: Vec<String> = session
        .snapshot()
        .keys()
        .filter(|k| k.starts_with(&dir))
        .cloned()
        .collect();
    let root = session.root().to_path_buf();
    // The file first, so a crash part way leaves no board, only its input.
    let mut order: Vec<&String> = files.iter().collect();
    order.sort_by_key(|k| !k.ends_with(&format!("/{BOARD_FILE}")));
    for rel in order {
        crate::session::remove_file(&root, &path_of(&root, rel))?;
    }
    session.reload()?;
    Ok(json!({
        "board": board,
        "deleted": files,
        "revision": session.project().revision,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::tests::{corpus, import_of, project};
    use crate::import::{Options, import};

    #[test]
    fn boards_are_listed_edited_and_deleted() {
        let (dir, mut s) = project();
        import(&mut s, &import_of(corpus("minimal-clean.tgz")), None).unwrap();
        import(
            &mut s,
            &Options {
                board: Some("mc".into()),
                ..import_of(corpus("minimal-clean"))
            },
            None,
        )
        .unwrap();
        let l = list(s.project());
        assert_eq!(l["boards"].as_array().unwrap().len(), 2);
        let shown = show(s.project(), "mc").unwrap();
        assert_eq!(shown["input"]["container"], json!("folder"));
        assert_eq!(shown["found"], shown["input"]["blake3"]);
        assert_eq!(
            edit(&mut s, "mc", None, None, None).unwrap_err().code,
            Code::InvalidParam
        );
        assert_eq!(
            edit(&mut s, "mc", Some(" "), None, None).unwrap_err().code,
            Code::InvalidParam
        );
        assert_eq!(
            edit(&mut s, "mc", None, Some("panel"), None)
                .unwrap_err()
                .code,
            Code::OdbStepNotFound
        );
        let e = edit(&mut s, "mc", Some("Folder"), Some("pcb"), None).unwrap();
        assert_eq!(
            (e["name"].clone(), e["step"].clone()),
            (json!("Folder"), json!("pcb"))
        );
        assert_eq!(
            delete(&mut s, "mc", Some("stale")).unwrap_err().code,
            Code::Conflict
        );
        let d = delete(&mut s, "mc", None).unwrap();
        assert!(d["deleted"].as_array().unwrap().len() > 2);
        assert!(!dir.path().join("boards/mc").exists());
        assert_eq!(show(s.project(), "mc").unwrap_err().code, Code::NotFound);
        // A board whose input changed by hand says so, and fails alone.
        std::fs::write(dir.path().join("boards/minimal-clean/input.tgz"), "x").unwrap();
        s.reload().unwrap();
        let l = list(s.project());
        assert_eq!(
            l["boards"][0]["error"]["code"],
            json!("board_input_changed")
        );
    }
}
