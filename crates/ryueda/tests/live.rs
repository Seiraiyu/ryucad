//! The live session in process (plan 3.B): a person's window and agents
//! attached to one `Live`, answered by `server::answer` as the daemon
//! answers them, over the sensor breakout laid out. Socket-level runs are
//! `tests/cli.rs`'s.

#[path = "support/breakout.rs"]
mod breakout;
#[path = "support/draw.rs"]
mod draw;

use ryu_session::live::{Kind, Live};
use ryueda::Eda;
use ryueda::error::{Code, Error};
use ryueda::proto::Request;
use ryueda::session::Session;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The breakout entered by its script and laid out, in `dir`.
fn breakout(dir: &Path) -> PathBuf {
    let board = dir.join("board");
    std::fs::create_dir_all(&board).unwrap();
    let run = |args: &[&str]| {
        let mut cmd = assert_cmd::Command::cargo_bin("ryueda").unwrap();
        cmd.current_dir(&board)
            .env("RYUEDA_NO_DAEMON", "1")
            .env("RYUEDA_ID_SEED", "live")
            .env("RYUEDA_CONFIG_DIR", dir.join("config"))
            .env("RYUEDA_CACHE_DIR", dir.join("cache"));
        for v in ryueda_parts::distributor::env_vars() {
            cmd.env_remove(v);
        }
        let out = cmd.args(args).output().unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["ok"], true, "{args:?}: {v}");
    };
    run(&["init"]);
    for (path, text) in breakout::footprints() {
        let at = board.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    for args in breakout::script() {
        run(&args.iter().map(String::as_str).collect::<Vec<_>>());
    }
    run(&["layout"]);
    board
}

/// A session and its live participants, as a daemon holds them.
struct Here {
    session: Session,
    live: Live<Eda>,
}

impl Here {
    fn open(board: &Path) -> Here {
        Here {
            session: Session::open(board).unwrap(),
            live: Live::new(),
        }
    }

    fn human(&mut self, name: &str) -> String {
        self.live.hello(Kind::Human, Some(name), None).unwrap()
    }

    /// `method` asked by `me` (the default agent when `None`).
    fn ask(&mut self, me: Option<&str>, method: &str, params: Value) -> Result<Value, Error> {
        let req = Request::from_rpc(method, Some(params))?;
        let job = ryu_session::job::Job::default();
        let a = ryueda::server::answer(&mut self.session, Some(&mut self.live), me, req, &job)?;
        Ok(a.into_json()?.result)
    }
}

#[test]
fn an_agent_reads_what_the_person_points_at() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Here::open(&breakout(dir.path()));
    // Nobody attached: nobody's selection.
    assert_eq!(
        h.ask(None, "selection.get", json!({})).unwrap(),
        json!({"participant": null, "selection": [], "objects": []})
    );
    let dana = h.human("Dana");
    let s = h
        .ask(
            Some(&dana),
            "selection.set",
            json!({"refs": ["breakout/r1", "breakout/sda"]}),
        )
        .unwrap();
    assert_eq!(s["selection"], json!(["breakout/r1", "breakout/sda"]));
    h.ask(
        Some(&dana),
        "selection.set",
        json!({"refs": ["breakout/r1/1"], "mode": "add"}),
    )
    .unwrap();
    h.ask(
        Some(&dana),
        "selection.set",
        json!({"refs": ["breakout/sda"], "mode": "remove"}),
    )
    .unwrap();
    // The agent names no one and reads the person's.
    let got = h.ask(None, "selection.get", json!({})).unwrap();
    assert_eq!(got["participant"], json!(dana));
    assert_eq!(got["selection"], json!(["breakout/r1", "breakout/r1/1"]));
    assert_eq!(
        got["objects"],
        json!([
            {"ref": "breakout/r1", "kind": "component", "block": "breakout"},
            {"ref": "breakout/r1/1", "kind": "pin", "block": "breakout"}
        ])
    );
    // Naming nothing is refused, with the closest names; removing what
    // is gone is not.
    let e = h
        .ask(
            Some(&dana),
            "selection.set",
            json!({"refs": ["breakout/r9"]}),
        )
        .unwrap_err();
    assert_eq!(e.code, Code::RefUnresolved);
    assert!(
        e.details["closest"]
            .as_array()
            .unwrap()
            .contains(&json!("breakout/r1"))
    );
    h.ask(
        Some(&dana),
        "selection.set",
        json!({"refs": ["breakout/r9"], "mode": "remove"}),
    )
    .unwrap();
    // Two people: the agent names one.
    let erin = h.human("Erin");
    assert_eq!(
        h.ask(None, "selection.get", json!({})).unwrap_err().code,
        Code::AmbiguousParticipant
    );
    assert_eq!(
        h.ask(None, "selection.get", json!({"participant": erin}))
            .unwrap()["selection"],
        json!([])
    );
}
