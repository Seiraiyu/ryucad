//! The daemon on its own: started by hand, spoken to over its socket.

use assert_cmd::cargo::cargo_bin;
use interprocess::local_socket::{Stream, prelude::*};
use ryucad::doc::project::{find_root, write_atomic};
use ryucad::endpoint::{Endpoint, RUNTIME_DIR_VAR};
use ryucad::proto::{Request, parse_rpc_response, rpc_request};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A project with its own runtime folder, so tests never share daemons.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    run: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let run = dir.path().join("run");
    let st = Command::new(cargo_bin("ryucad"))
        .arg("init")
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(st.status.success());
    // The root exactly as the daemon finds it, so both name the same endpoint.
    let root = find_root(&project).unwrap();
    Fixture {
        _dir: dir,
        root,
        run,
    }
}

impl Fixture {
    fn endpoint(&self) -> Endpoint {
        with_env(&self.run, || Endpoint::for_project(&self.root).unwrap())
    }

    fn start(&self, idle: u64) -> Child {
        let child = Command::new(cargo_bin("ryucad"))
            .args(["daemon", "run", "--idle-timeout", &idle.to_string()])
            .current_dir(&self.root)
            .env(RUNTIME_DIR_VAR, &self.run)
            .env("RYUCAD_CONFIG_DIR", self.run.with_file_name("config"))
            .env("RYUCAD_CACHE_DIR", self.run.with_file_name("cache"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        self.wait_until(|| self.connect().is_some());
        child
    }

    fn connect(&self) -> Option<Stream> {
        Stream::connect(self.endpoint().name().unwrap().into_owned()).ok()
    }

    fn wait_until(&self, f: impl Fn() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(10), "timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Runs `f` with the runtime folder set to `run`. Tests run in threads of
/// one process, so the variable is only ever set under a lock.
fn with_env<T>(run: &Path, f: impl FnOnce() -> T) -> T {
    let _g = ENV.lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: no other thread reads or writes the environment meanwhile.
    unsafe { std::env::set_var(RUNTIME_DIR_VAR, run) };
    f()
}

fn ask(conn: &Stream, req: &Request) -> ryucad::error::Result<Value> {
    let mut w = conn;
    w.write_all(format!("{}\n", rpc_request(1, req)).as_bytes())
        .unwrap();
    let mut line = String::new();
    BufReader::new(conn).read_line(&mut line).unwrap();
    parse_rpc_response(&line)
}

fn exited(child: &mut Child, within: Duration) -> bool {
    let t = Instant::now();
    while t.elapsed() < within {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn serves_requests_and_shuts_down() {
    let f = fixture();
    let mut child = f.start(60);
    let conn = f.connect().unwrap();
    let status = ask(&conn, &Request::DaemonStatus {}).unwrap();
    assert_eq!(status["pid"], child.id());
    assert_eq!(status["version"]["protocol"], 2);
    ask(
        &conn,
        &Request::PartCreate {
            part: "p".into(),
            kind: None,
        },
    )
    .unwrap();
    let list = ask(&conn, &Request::PartList {}).unwrap();
    assert_eq!(list["parts"][0]["part"], "p");
    let e = ask(
        &conn,
        &Request::PartCreate {
            part: "p".into(),
            kind: None,
        },
    )
    .unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::DuplicateId);
    // Several connections at once are served.
    let other = f.connect().unwrap();
    assert!(ask(&other, &Request::PartList {}).is_ok());
    ask(&conn, &Request::DaemonShutdown {}).unwrap();
    assert!(exited(&mut child, Duration::from_secs(10)));
    assert!(f.connect().is_none());
}

#[test]
fn one_daemon_per_project() {
    let f = fixture();
    let mut first = f.start(60);
    let second = Command::new(cargo_bin("ryucad"))
        .args(["daemon", "run"])
        .current_dir(&f.root)
        .env(RUNTIME_DIR_VAR, &f.run)
        .env("RYUCAD_CONFIG_DIR", f.run.with_file_name("config"))
        .env("RYUCAD_CACHE_DIR", f.run.with_file_name("cache"))
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(v["error"]["code"], "daemon");
    assert_eq!(second.status.code(), Some(4));
    ask(&f.connect().unwrap(), &Request::DaemonShutdown {}).unwrap();
    assert!(exited(&mut first, Duration::from_secs(10)));
}

#[test]
fn exits_when_idle_or_when_the_project_goes() {
    let f = fixture();
    let mut child = f.start(1);
    assert!(exited(&mut child, Duration::from_secs(10)));
    let mut child = f.start(60);
    std::fs::remove_file(f.root.join("ryucad.json")).unwrap();
    assert!(exited(&mut child, Duration::from_secs(10)));
}

#[cfg(unix)]
#[test]
fn a_stale_socket_is_replaced() {
    let f = fixture();
    let mut child = f.start(60);
    // Kill it without letting it clean up.
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(Path::new(&f.endpoint().address).exists());
    let mut again = f.start(60);
    ask(&f.connect().unwrap(), &Request::DaemonShutdown {}).unwrap();
    assert!(exited(&mut again, Duration::from_secs(10)));
}

/// Runs the CLI in the fixture's project with its runtime folder.
fn cli(f: &Fixture, args: &[&str]) -> Value {
    let out = Command::new(cargo_bin("ryucad"))
        .args(args)
        .current_dir(&f.root)
        .env(RUNTIME_DIR_VAR, &f.run)
        .env("RYUCAD_CONFIG_DIR", f.run.with_file_name("config"))
        .env("RYUCAD_CACHE_DIR", f.run.with_file_name("cache"))
        .env("RYUCAD_IDLE_TIMEOUT", "60")
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

/// [`cli`], with `RYUCAD_APP` set to `app`.
fn cli_with_app(f: &Fixture, app: &Path, args: &[&str]) -> (Value, Option<i32>) {
    let out = Command::new(cargo_bin("ryucad"))
        .args(args)
        .current_dir(&f.root)
        .env(RUNTIME_DIR_VAR, &f.run)
        .env("RYUCAD_CONFIG_DIR", f.run.with_file_name("config"))
        .env("RYUCAD_CACHE_DIR", f.run.with_file_name("cache"))
        .env("RYUCAD_IDLE_TIMEOUT", "60")
        .env(ryucad::client::APP_VAR, app)
        .output()
        .unwrap();
    (
        serde_json::from_slice(&out.stdout).unwrap(),
        out.status.code(),
    )
}

#[test]
fn open_starts_the_app_when_no_window_is_attached() {
    let f = fixture();
    // Any program stands in for the app: this one, which exits at once.
    let (r, _) = cli_with_app(&f, &cargo_bin("ryucad"), &["open"]);
    assert_eq!(r["result"]["opened"], "new", "{r}");
    assert_eq!(r["result"]["dir"], serde_json::json!(f.root));
    assert!(r["result"]["pid"].as_u64().is_some());
    // No daemon was started just to ask.
    assert_eq!(cli(&f, &["daemon", "status"])["result"]["running"], false);
    let (r, code) = cli_with_app(&f, &f.root.join("no-such-app"), &["open"]);
    assert_eq!(
        (r["error"]["code"].as_str(), code),
        (Some("app_not_installed"), Some(4))
    );
}

#[test]
fn the_cli_starts_a_daemon_and_stops_it() {
    let f = fixture();
    assert_eq!(cli(&f, &["daemon", "status"])["result"]["running"], false);
    let r = cli(&f, &["part", "create", "p"]);
    assert_eq!(
        (r["ok"].as_bool(), r["warnings"].as_array().unwrap().len()),
        (Some(true), 0)
    );
    let status = cli(&f, &["daemon", "status"])["result"].clone();
    assert_eq!(status["running"], true);
    assert_eq!(status["idle_timeout_s"], 60);
    // The same daemon answers the next command.
    cli(&f, &["part", "list"]);
    assert_eq!(
        cli(&f, &["daemon", "status"])["result"]["pid"],
        status["pid"]
    );
    assert_eq!(cli(&f, &["daemon", "stop"])["result"]["stopped"], true);
    assert_eq!(cli(&f, &["daemon", "status"])["result"]["running"], false);
    assert_eq!(cli(&f, &["daemon", "stop"])["result"]["stopped"], false);
}

#[test]
fn stop_all_stops_every_project() {
    let (a, b) = (fixture(), fixture());
    // Both projects' daemons in one runtime folder.
    let b = Fixture {
        run: a.run.clone(),
        ..b
    };
    cli(&a, &["part", "list"]);
    cli(&b, &["part", "list"]);
    assert_eq!(
        cli(&a, &["daemon", "stop", "--all"])["result"]["stopped"],
        2
    );
    assert_eq!(cli(&b, &["daemon", "status"])["result"]["running"], false);
}

#[test]
fn a_daemon_of_another_version_is_replaced() {
    let f = fixture();
    let endpoint = f.endpoint();
    let listener = interprocess::local_socket::ListenerOptions::new()
        .name(endpoint.name().unwrap())
        .create_sync()
        .unwrap();
    // An impostor that claims an old version and leaves when asked.
    let impostor = std::thread::spawn(move || {
        for conn in listener.incoming() {
            let conn = conn.unwrap();
            let mut reader = BufReader::new(&conn);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 0 {
                let req: Value = serde_json::from_str(&line).unwrap();
                let result = match req["method"].as_str().unwrap() {
                    "daemon.status" => serde_json::json!({"version": {"ryucad": "0.0.0"}}),
                    _ => serde_json::json!({"stopping": true}),
                };
                let reply =
                    serde_json::json!({"jsonrpc": "2.0", "id": req["id"], "result": result});
                (&conn).write_all(format!("{reply}\n").as_bytes()).unwrap();
                if req["method"] == "daemon.shutdown" {
                    return;
                }
                line.clear();
            }
        }
    });
    let r = cli(&f, &["part", "list"]);
    impostor.join().unwrap();
    assert_eq!(r["ok"], true, "{r}");
    let status = cli(&f, &["daemon", "status"])["result"].clone();
    assert_eq!(status["version"]["ryucad"], env!("CARGO_PKG_VERSION"));
    cli(&f, &["daemon", "stop"]);
}

/// Another program (the app) starts the daemon from a named `ryucad`,
/// says hello as a human on two connections and hears what agents do.
#[test]
fn another_program_connects_as_a_human() {
    let f = fixture();
    let daemon = ryucad::client::Daemon::at(&cargo_bin("ryucad")).unwrap();
    assert!(ryucad::proto::VersionInfo::current().same_build(&serde_json::json!(daemon.version)));
    let (mut requests, mut notes) = with_env(&f.run, || {
        // SAFETY: under with_env's lock, like the runtime folder.
        unsafe {
            std::env::set_var("RYUCAD_CONFIG_DIR", f.run.with_file_name("config"));
            std::env::set_var("RYUCAD_CACHE_DIR", f.run.with_file_name("cache"));
        };
        (
            daemon.connect(&f.root).unwrap(),
            daemon.connect(&f.root).unwrap(),
        )
    });
    let hello = |conn: &mut ryucad::client::Connection, participant: Option<String>| {
        let req = Request::SessionHello {
            kind: ryucad::live::Kind::Human,
            name: Some("Dana".into()),
            participant,
        };
        conn.call(&req).unwrap().unwrap()["participant"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let me = hello(&mut requests, None);
    assert_eq!(hello(&mut notes, Some(me.clone())), me);
    let (tx, rx) = std::sync::mpsc::channel();
    let (_, sub) = notes
        .subscribe(&["project".into()], move |v| tx.send(v).unwrap())
        .unwrap();
    cli(&f, &["part", "create", "p"]);
    let v = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    assert_eq!(v["method"], "project.changed");
    assert_eq!(v["params"]["parts"], serde_json::json!(["p"]));
    let list = requests
        .call(&Request::ParticipantList {})
        .unwrap()
        .unwrap();
    assert_eq!(names(&list), ["human:Dana", "agent:agent"]);
    // A window that says hello makes its project the most recent one.
    let recent = cli(&f, &["config", "get", "app.recent_projects"]);
    assert_eq!(recent["result"]["value"][0], serde_json::json!(f.root));
    // Closing both connections ends the human, and the subscription's
    // reader doesn't take it for the daemon going.
    drop((sub, requests));
    f.wait_until(|| names(&cli(&f, &["participants"])["result"]) == ["agent:agent"]);
    assert!(rx.try_recv().is_err());
    // The daemon's going is heard as `None`.
    let mut again = with_env(&f.run, || daemon.connect(&f.root).unwrap());
    hello(&mut again, None);
    let (tx, rx) = std::sync::mpsc::channel();
    let (_, _sub) = again.subscribe(&[], move |v| tx.send(v).unwrap()).unwrap();
    cli(&f, &["daemon", "stop"]);
    let t = Instant::now();
    while rx.recv_timeout(Duration::from_secs(10)).unwrap().is_some() {
        assert!(t.elapsed() < Duration::from_secs(10));
    }
}

#[test]
fn without_a_daemon_the_cli_still_answers() {
    let f = fixture();
    // A runtime folder that can't be created: no daemon is possible.
    let blocked = Fixture {
        run: f.root.join("ryucad.json").join("run"),
        ..fixture()
    };
    let r = cli(&blocked, &["part", "list"]);
    assert_eq!(r["ok"], true);
    assert!(
        r["warnings"][0]
            .as_str()
            .unwrap()
            .contains("ran in process"),
        "{r}"
    );
    // --no-daemon never tries.
    let r = cli(&f, &["--no-daemon", "part", "list"]);
    assert_eq!(r["warnings"], serde_json::json!([]));
    assert_eq!(cli(&f, &["daemon", "status"])["result"]["running"], false);
}

/// The names of the attached participants, in ID order.
fn names(v: &Value) -> Vec<String> {
    v["participants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            format!(
                "{}:{}",
                p["kind"].as_str().unwrap(),
                p["name"].as_str().unwrap()
            )
        })
        .collect()
}

#[test]
fn participants_come_and_go() {
    let f = fixture();
    let r = cli(&f, &["participants"]);
    assert_eq!(names(&r["result"]), ["agent:agent"]);
    // An agent named with --as is a second participant; the first persists
    // between CLI calls.
    let r = cli(&f, &["--as", "reviewer", "participants"]);
    assert_eq!(names(&r["result"]), ["agent:agent", "agent:reviewer"]);
    // A human is attached while its connection is open.
    let conn = f.connect().unwrap();
    let hello = Request::SessionHello {
        kind: ryucad::live::Kind::Human,
        name: Some("Dana".into()),
        participant: None,
    };
    let me = ask(&conn, &hello).unwrap()["participant"].clone();
    let r = cli(&f, &["participants"]);
    assert_eq!(
        names(&r["result"]),
        ["agent:agent", "agent:reviewer", "human:Dana"]
    );
    assert_eq!(r["result"]["participants"][2]["id"], me);
    drop(conn);
    f.wait_until(|| names(&cli(&f, &["participants"])["result"]).len() == 2);
    let r = cli(&f, &["--as", " ", "participants"]);
    assert_eq!(r["error"]["code"], "invalid_param");
    // In process, nobody is attached.
    let r = cli(&f, &["--no-daemon", "participants"]);
    assert_eq!(r["result"], serde_json::json!({"participants": []}));
    cli(&f, &["daemon", "stop"]);
}

#[test]
fn an_agent_selects_through_the_cli() {
    let f = fixture();
    cli(&f, &["part", "create", "p"]);
    cli(
        &f,
        &[
            "feature", "add", "p", "box", "--id", "b", "--param", "x=1", "--param", "y=1",
            "--param", "z=1",
        ],
    );
    let body = cli(&f, &["inspect", "p", "b"])["result"]["bodies"][0].clone();
    let face = format!("p/b/{}", body["faces"][0]["name"].as_str().unwrap());
    let edge = format!("p/b/{}", body["edges"][0]["name"].as_str().unwrap());
    let r = cli(&f, &["select", &face]);
    assert_eq!(r["result"]["selection"], serde_json::json!([face]));
    let me = r["result"]["participant"].as_str().unwrap().to_owned();
    let r = cli(&f, &["select", "--add", &edge]);
    assert_eq!(r["result"]["selection"], serde_json::json!([face, edge]));
    // The selection outlives the CLI call; nobody human is attached.
    let r = cli(&f, &["selection", "--of", &me]);
    assert_eq!(r["result"]["selection"], serde_json::json!([face, edge]));
    let r = cli(&f, &["selection"]);
    assert_eq!(r["result"]["participant"], Value::Null);
    let r = cli(&f, &["select", "--clear"]);
    assert_eq!(r["result"]["selection"], serde_json::json!([]));
    let r = cli(&f, &["--no-daemon", "select", &face]);
    assert_eq!(r["error"]["code"], "no_live_session");
    cli(&f, &["daemon", "stop"]);
}

#[test]
fn camera_highlight_and_status_through_the_cli() {
    let f = fixture();
    cli(&f, &["part", "create", "p"]);
    cli(
        &f,
        &[
            "feature", "add", "p", "sphere", "--id", "s", "--param", "radius=5",
        ],
    );
    let body = cli(&f, &["inspect", "p", "s"])["result"]["bodies"][0].clone();
    let face = format!("p/s/{}", body["faces"][0]["name"].as_str().unwrap());
    let r = cli(
        &f,
        &[
            "camera", "--eye", "0,-100,0", "--target", "0,0,0", "--up", "0,0,1", "--ortho", "50",
        ],
    );
    assert_eq!(
        r["result"]["camera"]["eye"],
        serde_json::json!([0.0, -0.1, 0.0])
    );
    let me = r["result"]["participant"].as_str().unwrap().to_owned();
    let r = cli(&f, &["camera", "--of", &me]);
    assert_eq!(
        r["result"]["camera"]["projection"],
        serde_json::json!({"orthographic": {"height": 0.05}})
    );
    let r = cli(
        &f,
        &["highlight", &face, "--style", "changed", "--label", "grew"],
    );
    assert_eq!(r["result"]["highlights"][0]["style"], "changed");
    let r = cli(&f, &["status", "checking the sphere", "--entity", &face]);
    assert_eq!(r["result"]["status"]["text"], "checking the sphere");
    let p = cli(&f, &["participants"])["result"]["participants"][0].clone();
    assert_eq!(p["highlights"][0]["label"], "grew");
    let r = cli(&f, &["highlight", "--clear"]);
    assert_eq!(r["result"]["highlights"], serde_json::json!([]));
    let r = cli(&f, &["highlight", &face, "--style", "loud"]);
    assert_eq!(r["error"]["code"], "usage");
    cli(&f, &["daemon", "stop"]);
}

#[test]
fn an_agent_puts_the_watching_human_into_a_compare() {
    let f = fixture();
    cli(&f, &["part", "create", "p"]);
    let git = |args: &[&str]| {
        let st = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(&f.root)
            .output()
            .unwrap();
        assert!(st.status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "start"]);
    let w = Watch::new(&f, &["participants"]);
    let r = cli(&f, &["compare", "HEAD"]);
    assert_eq!(r["result"]["participant"], w.me.as_str(), "{r}");
    let p = w.next("participant.changed");
    assert_eq!(
        p["participant"]["compare"],
        serde_json::json!({"rev": "HEAD"})
    );
    let r = cli(&f, &["compare"]);
    assert_eq!(r["result"]["compare"]["rev"], "HEAD");
    assert_eq!(r["result"]["compare"]["commit"].as_str().unwrap().len(), 40);
    let r = cli(&f, &["compare", "nowhere"]);
    assert_eq!(r["error"]["code"], "rev_not_found");
    let r = cli(&f, &["compare", "HEAD", "--off"]);
    assert_eq!(r["error"]["code"], "usage");
    let r = cli(&f, &["compare", "--off"]);
    assert_eq!(r["result"]["compare"], Value::Null);
    let p = w.next("participant.changed");
    assert_eq!(p["participant"]["compare"], Value::Null);
    let r = cli(&f, &["--no-daemon", "compare", "HEAD"]);
    assert_eq!(r["error"]["code"], "no_live_session");
    cli(&f, &["daemon", "stop"]);
}

#[test]
fn an_agent_hides_and_shows_what_the_human_sees() {
    let f = fixture();
    cli(&f, &["part", "create", "bolt"]);
    cli(
        &f,
        &[
            "feature",
            "add",
            "bolt",
            "cylinder",
            "--id",
            "shank",
            "--param",
            "radius=2",
            "--param",
            "height=10",
        ],
    );
    cli(&f, &["part", "create", "pair", "--assembly"]);
    for (id, x) in [("a", "0"), ("b", "20")] {
        let origin = format!("{x},0,0");
        cli(
            &f,
            &[
                "feature", "add", "pair", "instance", "--id", id, "--of", "bolt", "--origin",
                &origin,
            ],
        );
    }
    let w = Watch::new(&f, &["participants"]);
    let r = cli(&f, &["hide", "pair/a"]);
    assert_eq!(r["result"]["participant"], w.me.as_str(), "{r}");
    assert_eq!(r["result"]["hidden"], serde_json::json!(["pair/a"]));
    let p = w.next("participant.changed");
    assert_eq!(p["participant"]["hidden"], serde_json::json!(["pair/a"]));
    let r = cli(&f, &["isolate", "pair/a"]);
    assert_eq!(r["result"]["hidden"], serde_json::json!(["pair/b"]));
    let r = cli(&f, &["show", "--all"]);
    assert_eq!(r["result"]["hidden"], serde_json::json!([]));
    let r = cli(&f, &["hide", "pair/c"]);
    assert_eq!(r["error"]["code"], "not_found");
    let r = cli(&f, &["show"]);
    assert_eq!(r["error"]["code"], "usage");
    let r = cli(&f, &["--no-daemon", "hide", "pair/a"]);
    assert_eq!(r["error"]["code"], "no_live_session");
    // A render can leave things out without anyone's window.
    let out = f.run.with_file_name("hidden.png");
    let out = out.to_str().unwrap();
    let r = cli(
        &f,
        &[
            "--no-daemon",
            "render",
            "pair",
            "--hidden",
            "pair/b",
            "--out",
            out,
        ],
    );
    assert_eq!(r["ok"], true, "{r}");
    cli(&f, &["daemon", "stop"]);
}

/// A human attached and subscribed: notifications arrive on `rx`.
struct Watch {
    me: String,
    rx: std::sync::mpsc::Receiver<Value>,
}

impl Watch {
    fn new(f: &Fixture, topics: &[&str]) -> Watch {
        let mut reader = BufReader::new(f.connect().unwrap());
        let mut call = |req: &Request| {
            let mut w = reader.get_ref();
            w.write_all(format!("{}\n", rpc_request(1, req)).as_bytes())
                .unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            parse_rpc_response(&line).unwrap()
        };
        let hello = Request::SessionHello {
            kind: ryucad::live::Kind::Human,
            name: Some("watcher".into()),
            participant: None,
        };
        let me = call(&hello)["participant"].as_str().unwrap().to_owned();
        let r = call(&Request::Subscribe {
            topics: topics.iter().map(|t| t.to_string()).collect(),
        });
        let ids = r["session"]["participants"].as_array().unwrap();
        assert!(ids.iter().any(|p| p["id"] == me.as_str()));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                if tx.send(serde_json::from_str(&line).unwrap()).is_err() {
                    return;
                }
                line.clear();
            }
        });
        Watch { me, rx }
    }

    /// The next notification with `method`, skipping others.
    fn next(&self, method: &str) -> Value {
        let t = Instant::now();
        loop {
            let left = Duration::from_secs(10).saturating_sub(t.elapsed());
            let v = self
                .rx
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no {method} notification"));
            assert_eq!(v["jsonrpc"], "2.0");
            assert!(v.get("id").is_none(), "a notification has no id: {v}");
            if v["method"] == method {
                return v["params"].clone();
            }
        }
    }
}

#[test]
fn open_brings_attached_windows_forward() {
    let f = fixture();
    cli(&f, &["participants"]);
    // A window that only watches the model still hears it is wanted.
    let w = Watch::new(&f, &["model"]);
    let missing = f.root.join("no-such-app");
    let (r, _) = cli_with_app(&f, &missing, &["open"]);
    assert_eq!(r["result"]["opened"], "existing", "{r}");
    assert_eq!(r["result"]["asked"], serde_json::json!([w.me]));
    assert_eq!(w.next("app.open"), serde_json::json!({}));
    // Asking for an agent's window is refused.
    let list = cli(&f, &["participants"]);
    let agent = list["result"]["participants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "agent")
        .unwrap()["id"]
        .clone();
    let conn = f.connect().unwrap();
    let e = ask(
        &conn,
        &Request::AppOpen {
            participant: Some(agent.as_str().unwrap().into()),
        },
    )
    .unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::InvalidParam);
    cli(&f, &["daemon", "stop"]);
}

#[test]
fn watchers_see_participants_change() {
    let f = fixture();
    cli(&f, &["part", "create", "p"]);
    cli(
        &f,
        &[
            "feature", "add", "p", "sphere", "--id", "s", "--param", "radius=5",
        ],
    );
    let body = cli(&f, &["inspect", "p", "s"])["result"]["bodies"][0].clone();
    let face = format!("p/s/{}", body["faces"][0]["name"].as_str().unwrap());
    let w = Watch::new(&f, &["participants"]);
    cli(&f, &["--as", "fitter", "select", &face]);
    // The agent arrives, then selects.
    loop {
        let p = w.next("participant.changed");
        if p["participant"]["selection"] == serde_json::json!([face]) {
            assert_eq!(p["participant"]["name"], "fitter");
            break;
        }
    }
    // Another human comes and goes.
    let other = f.connect().unwrap();
    let hello = Request::SessionHello {
        kind: ryucad::live::Kind::Human,
        name: Some("other".into()),
        participant: None,
    };
    let id = ask(&other, &hello).unwrap()["participant"].clone();
    let p = w.next("participant.changed");
    assert_eq!(p["participant"]["id"], id);
    drop(other);
    let p = w.next("participant.changed");
    assert_eq!(
        p,
        serde_json::json!({"participant": {"id": id}, "left": true})
    );
    assert!(!w.me.is_empty());
    cli(&f, &["daemon", "stop"]);
}

#[test]
fn subscribing_needs_a_hello() {
    let f = fixture();
    let mut child = f.start(60);
    let conn = f.connect().unwrap();
    let e = ask(&conn, &Request::Subscribe { topics: vec![] }).unwrap_err();
    assert_eq!(e.code, ryucad::error::Code::Usage);
    ask(&conn, &Request::DaemonShutdown {}).unwrap();
    assert!(exited(&mut child, Duration::from_secs(10)));
}

#[test]
fn watchers_see_the_model_change_however_it_changes() {
    let f = fixture();
    cli(&f, &["part", "create", "p"]);
    let w = Watch::new(&f, &["model"]);
    // Through the daemon.
    cli(
        &f,
        &[
            "feature", "add", "p", "sphere", "--id", "s", "--param", "radius=5",
        ],
    );
    let m = w.next("model.changed");
    assert_eq!(m["parts"][0]["features"][0]["id"], "s");
    let first = m["parts"][0]["features"][0]["hash"].clone();
    // Behind its back: another process with no daemon.
    let r = cli(
        &f,
        &[
            "--no-daemon",
            "feature",
            "edit",
            "p",
            "s",
            "--param",
            "radius=6",
        ],
    );
    assert_eq!(r["ok"], true);
    let m = w.next("model.changed");
    let second = m["parts"][0]["features"][0]["hash"].clone();
    assert_ne!(second, first);
    // An editor saving the same content changes nothing; a real edit does.
    // These save as ryucad does, renaming a finished file into place: one
    // that writes in place can be caught half-written, and the daemon then
    // rightly reports the feature broken until the write ends.
    let path = f.root.join("parts/p/features/s.json");
    let text = std::fs::read_to_string(&path).unwrap();
    write_atomic(&path, &text).unwrap();
    write_atomic(&path, text.replace("6 mm", "7 mm")).unwrap();
    let m = w.next("model.changed");
    assert_eq!(m["parts"][0]["failed"], 0);
    assert_ne!(m["parts"][0]["features"][0]["hash"], second);
    cli(&f, &["daemon", "stop"]);
}

#[test]
fn an_agent_waits_for_the_human_to_select() {
    let f = fixture();
    cli(&f, &["part", "create", "p"]);
    cli(
        &f,
        &[
            "feature", "add", "p", "sphere", "--id", "s", "--param", "radius=5",
        ],
    );
    let body = cli(&f, &["inspect", "p", "s"])["result"]["bodies"][0].clone();
    let face = format!("p/s/{}", body["faces"][0]["name"].as_str().unwrap());
    // The human arrives first, so what the agent waits for is only what
    // they select.
    let human = f.connect().unwrap();
    let hello = Request::SessionHello {
        kind: ryucad::live::Kind::Human,
        name: Some("Dana".into()),
        participant: None,
    };
    ask(&human, &hello).unwrap();
    let waiter = {
        let (root, run) = (f.root.clone(), f.run.clone());
        std::thread::spawn(move || {
            let out = Command::new(cargo_bin("ryucad"))
                .args([
                    "watch",
                    "--topic",
                    "participants",
                    "--count",
                    "2",
                    "--timeout",
                    "120",
                ])
                .current_dir(&root)
                .env(RUNTIME_DIR_VAR, &run)
                .env("RYUCAD_CONFIG_DIR", run.with_file_name("config"))
                .env("RYUCAD_CACHE_DIR", run.with_file_name("cache"))
                .output()
                .unwrap();
            serde_json::from_slice::<Value>(&out.stdout).unwrap()
        })
    };
    // Nothing says when the watcher has subscribed, so the human selects
    // the face and then nothing until it has heard two changes: any two in
    // a row include the face. (A sleep before selecting once was a guess a
    // busy machine broke.)
    let select = |entities: Vec<String>| {
        let req = Request::SelectionSet {
            entities,
            mode: ryucad::live::SelectMode::Replace,
        };
        ask(&human, &req).unwrap();
    };
    while !waiter.is_finished() {
        select(vec![face.clone()]);
        select(Vec::new());
        std::thread::sleep(Duration::from_millis(50));
    }
    let r = waiter.join().unwrap();
    assert_eq!(r["result"]["timed_out"], false, "{r}");
    let n = r["result"]["notifications"].as_array().unwrap();
    assert!(
        n.iter().all(|n| n["method"] == "participant.changed"),
        "{r}"
    );
    assert!(
        n.iter()
            .any(|n| n["params"]["participant"]["selection"] == serde_json::json!([face])),
        "{r}"
    );
    // Nothing happens: the watch times out with what it has.
    let r = cli(&f, &["watch", "--topic", "model", "--timeout", "1"]);
    assert_eq!(
        r["result"],
        serde_json::json!({"notifications": [], "timed_out": true})
    );
    let r = cli(&f, &["--no-daemon", "watch"]);
    assert_eq!(r["error"]["code"], "no_live_session");
    drop(human);
    cli(&f, &["daemon", "stop"]);
}

#[test]
fn an_agent_frames_a_face_for_the_human() {
    let f = fixture();
    cli(&f, &["part", "create", "p"]);
    cli(
        &f,
        &[
            "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=10",
            "--param", "z=10",
        ],
    );
    let body = cli(&f, &["inspect", "p", "b"])["result"]["bodies"][0].clone();
    let face = format!("p/b/{}", body["faces"][0]["name"].as_str().unwrap());
    let r = cli(&f, &["camera", "--view", "iso"]);
    // At the cube: near its middle, moved a little so that the near
    // corner, which looks bigger, is framed as the far ones are.
    let c = &r["result"]["camera"];
    for k in 0..3 {
        let t = c["target"][k].as_f64().unwrap();
        assert!((t - 0.005).abs() < 0.001, "{c}");
    }
    let r = cli(&f, &["camera", "--fit", &face]);
    assert_eq!(r["ok"], true, "{r}");
    let r = cli(&f, &["camera", "--view", "sideways"]);
    assert_eq!(r["error"]["code"], "invalid_param");
    cli(&f, &["daemon", "stop"]);
}

#[test]
fn render_what_a_participant_sees() {
    let f = fixture();
    cli(&f, &["part", "create", "p"]);
    cli(
        &f,
        &[
            "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=10",
            "--param", "z=10",
        ],
    );
    let body = cli(&f, &["inspect", "p", "b"])["result"]["bodies"][0].clone();
    let face = format!("p/b/{}", body["faces"][0]["name"].as_str().unwrap());
    let me = cli(&f, &["camera", "--view", "front"])["result"]["participant"].clone();
    cli(&f, &["highlight", &face, "--style", "pending"]);
    let out = f.root.join("seen.png");
    let r = cli(
        &f,
        &[
            "render",
            "--of",
            me.as_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--size",
            "120x90",
        ],
    );
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(
        r["result"]["camera"]["up"],
        serde_json::json!([0.0, 0.0, 1.0])
    );
    let bytes = std::fs::read(&out).unwrap();
    let mut rd = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; rd.output_buffer_size().unwrap()];
    rd.next_frame(&mut buf).unwrap();
    // The agent's pending highlight is amber.
    assert!(
        buf.chunks(4)
            .any(|px| px[0] > 200 && px[1] > 120 && px[2] < 110)
    );
    let r = cli(
        &f,
        &[
            "--no-daemon",
            "render",
            "--of",
            "p-00000001",
            "--out",
            "x.png",
        ],
    );
    assert_eq!(r["error"]["code"], "no_live_session");
    cli(&f, &["daemon", "stop"]);
}

/// The CLI and the app find one daemon whatever `$TMPDIR` each was given
/// (agent sessions, sudo and launchd jobs set their own).
#[cfg(unix)]
#[test]
fn the_runtime_folder_doesnt_follow_tmpdir() {
    let f = fixture();
    // Short, so a socket under either would fit.
    let tmps = [(); 2].map(|()| tempfile::tempdir_in("/tmp").unwrap());
    let cli = |tmp: usize, args: &[&str]| -> Value {
        let tmp = tmps[tmp].path();
        let out = Command::new(cargo_bin("ryucad"))
            .args(args)
            .current_dir(&f.root)
            .env_remove(RUNTIME_DIR_VAR)
            .env_remove("XDG_RUNTIME_DIR")
            .env("TMPDIR", tmp)
            .env("RYUCAD_CONFIG_DIR", f.run.with_file_name("config"))
            .env("RYUCAD_CACHE_DIR", f.run.with_file_name("cache"))
            .env("RYUCAD_IDLE_TIMEOUT", "30")
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let r = cli(0, &["part", "list"]);
    assert_eq!(r["warnings"], serde_json::json!([]), "{r}");
    let status = cli(1, &["daemon", "status"]);
    let stop = cli(0, &["daemon", "stop"]);
    assert_eq!(status["result"]["running"], true, "{status}");
    assert_eq!(stop["ok"], true, "{stop}");
}
