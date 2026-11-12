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

#[test]
fn an_agents_highlights_are_everyones_to_see() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Here::open(&breakout(dir.path()));
    let dana = h.human("Dana");
    h.ask(
        None,
        "highlight.set",
        json!({"refs": ["breakout/r1", "breakout/r2"], "style": "pending", "label": "pull-ups"}),
    )
    .unwrap();
    let v = h
        .ask(None, "highlight.set", json!({"refs": ["breakout/sda"]}))
        .unwrap();
    assert_eq!(v["highlights"].as_array().unwrap().len(), 2, "{v}");
    // Every participant's JSON, the person's window's included, shows them.
    let list = h.ask(Some(&dana), "participant.list", json!({})).unwrap();
    let agent = list["participants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "agent")
        .unwrap();
    assert_eq!(
        agent["highlights"][0],
        json!({"style": "pending", "label": "pull-ups", "entities": ["breakout/r1", "breakout/r2"]})
    );
    assert_eq!(agent["highlights"][1]["style"], "note");
    let e = h
        .ask(None, "highlight.set", json!({"refs": ["breakout/nope"]}))
        .unwrap_err();
    assert_eq!(e.code, Code::RefUnresolved);
    let v = h
        .ask(None, "highlight.clear", json!({"style": "pending"}))
        .unwrap();
    assert_eq!(
        v["highlights"],
        json!([{"style": "note", "label": null, "entities": ["breakout/sda"]}])
    );
    assert_eq!(
        h.ask(None, "highlight.clear", json!({})).unwrap()["highlights"],
        json!([])
    );
}

#[test]
fn an_agent_says_what_it_is_doing() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Here::open(&breakout(dir.path()));
    let v = h
        .ask(
            None,
            "activity.set",
            json!({"status": "pulling up SDA", "refs": ["breakout/r1"]}),
        )
        .unwrap();
    assert_eq!(
        v["status"],
        json!({"text": "pulling up SDA", "entities": ["breakout/r1"]})
    );
    let e = h
        .ask(None, "activity.set", json!({"status": "x".repeat(201)}))
        .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam);
    let e = h
        .ask(
            None,
            "activity.set",
            json!({"status": "x", "refs": ["breakout/zz"]}),
        )
        .unwrap_err();
    assert_eq!(e.code, Code::RefUnresolved);
    assert_eq!(
        h.ask(None, "activity.set", json!({"status": ""})).unwrap()["status"],
        Value::Null
    );
}

#[test]
fn a_window_says_where_it_looks() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Here::open(&breakout(dir.path()));
    let dana = h.human("Dana");
    // A block alone: its first sheet, the design that places it.
    let v = h
        .ask(Some(&dana), "view.set", json!({"block": "sensor"}))
        .unwrap();
    let sheet = v["view"]["sheet"].as_str().unwrap().to_owned();
    assert_eq!(v["view"]["block"], "sensor");
    assert!(v["view"]["design"].is_string(), "{v}");
    assert_eq!(v["view"]["shown_by"], json!(dana));
    // The camera moves on the same sheet.
    let cam = json!({"centre_nm": [10_000_000, 5_000_000], "zoom_nm_per_pt": 20_000});
    h.ask(
        Some(&dana),
        "view.set",
        json!({"block": "sensor", "sheet": sheet, "camera": cam}),
    )
    .unwrap();
    // An agent reads the person's view.
    let got = h.ask(None, "view.get", json!({})).unwrap();
    assert_eq!(
        (&got["participant"], &got["view"]["camera"]),
        (&json!(dana), &cam)
    );
    let e = h
        .ask(
            Some(&dana),
            "view.set",
            json!({"block": "sensor", "sheet": "nope"}),
        )
        .unwrap_err();
    assert_eq!(e.code, Code::SheetNotFound);
    assert_eq!(e.details["sheets"][0]["sheet"], json!(sheet));
    let e = h
        .ask(
            Some(&dana),
            "view.set",
            json!({"block": "sensor", "camera": {"centre_nm": [0, 0], "zoom_nm_per_pt": 0}}),
        )
        .unwrap_err();
    assert_eq!(e.code, Code::InvalidParam);
    assert_eq!(
        h.ask(Some(&dana), "view.set", json!({"block": "nope"}))
            .unwrap_err()
            .code,
        Code::NotFound
    );
}

#[test]
fn an_agent_shows_the_person_what_it_means() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Here::open(&breakout(dir.path()));
    // Nobody attached: refused only if it names or draws nothing.
    let v = h
        .ask(None, "view.show", json!({"ref": "sensor/c1"}))
        .unwrap();
    assert_eq!(v["shown"], json!([]));
    let dana = h.human("Dana");
    h.ask(Some(&dana), "view.set", json!({"block": "breakout"}))
        .unwrap();
    let v = h
        .ask(None, "view.show", json!({"ref": "sensor/c1"}))
        .unwrap();
    assert_eq!(v["shown"], json!([dana]));
    let agent = h
        .live
        .participants()
        .find(|x| x.name == "agent")
        .unwrap()
        .id
        .clone();
    let seen = h.live.get(&dana).unwrap().clone();
    assert_eq!(seen.selection, vec!["sensor/c1".to_owned()]);
    assert_eq!(seen.view.block.as_deref(), Some("sensor"));
    assert_eq!(seen.view.shown_by.as_deref(), Some(agent.as_str()));
    // The camera frames c1's box as the scene gives it.
    let s = h
        .ask(
            Some(&dana),
            "scene",
            json!({"block": "sensor", "sheet": seen.view.sheet}),
        )
        .unwrap();
    let c1 = s["owners"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["ref"] == "sensor/c1")
        .unwrap();
    let b = |i: usize, k: usize| c1["box"][i][k].as_i64().unwrap();
    let boxed = ryusch::geom::Rect::new([b(0, 0), b(0, 1)], [b(1, 0), b(1, 1)]);
    assert_eq!(seen.view.camera, Some(ryueda::view::Camera::framing(boxed)));
    assert_eq!(v["box"], c1["box"]);
    // A pin: framed round where wires meet it.
    h.ask(None, "view.show", json!({"ref": "breakout/r1/1"}))
        .unwrap();
    assert_eq!(
        h.live.get(&dana).unwrap().selection,
        vec!["breakout/r1/1".to_owned()]
    );
    // A net, found by its wires or labels.
    let v = h
        .ask(None, "view.show", json!({"ref": "breakout/sda"}))
        .unwrap();
    assert_eq!(v["block"], "breakout");
    // One participant only: nobody else moves.
    let erin = h.human("Erin");
    let v = h
        .ask(
            None,
            "view.show",
            json!({"ref": "breakout/u2", "participant": erin}),
        )
        .unwrap();
    assert_eq!(v["shown"], json!([erin]));
    assert_eq!(
        h.live.get(&dana).unwrap().selection,
        vec!["breakout/sda".to_owned()]
    );
    let e = h
        .ask(None, "view.show", json!({"ref": "breakout/u9"}))
        .unwrap_err();
    assert_eq!(e.code, Code::RefUnresolved);
}

#[test]
fn what_is_deleted_leaves_every_selection() {
    let dir = tempfile::tempdir().unwrap();
    let board = breakout(dir.path());
    let mut h = Here::open(&board);
    let dana = h.human("Dana");
    h.ask(
        Some(&dana),
        "selection.set",
        json!({"refs": ["breakout/r1", "breakout/r2", "breakout/r2/1"]}),
    )
    .unwrap();
    h.ask(
        None,
        "highlight.set",
        json!({"refs": ["breakout/r2"], "style": "pending"}),
    )
    .unwrap();
    h.ask(
        None,
        "activity.set",
        json!({"status": "removing R2", "refs": ["breakout/r2"]}),
    )
    .unwrap();
    // A write through the session.
    h.ask(
        None,
        "component.delete",
        json!({"component": "breakout/r2"}),
    )
    .unwrap();
    let list = h.ask(None, "participant.list", json!({})).unwrap();
    let get = |name: &str| {
        list["participants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(get("Dana")["selection"], json!(["breakout/r1"]));
    assert_eq!(get("agent")["highlights"], json!([]));
    assert_eq!(get("agent")["status"]["entities"], json!([]));
    // A change from outside: r1's file deleted by hand.
    h.ask(
        Some(&dana),
        "selection.set",
        json!({"refs": ["breakout/r1", "breakout/sda"]}),
    )
    .unwrap();
    std::fs::remove_file(board.join("blocks/breakout/components/r1.json")).unwrap();
    let got = h
        .ask(None, "selection.get", json!({"participant": dana}))
        .unwrap();
    assert_eq!(got["selection"], json!(["breakout/sda"]));
}
