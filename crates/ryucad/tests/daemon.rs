//! The daemon on its own: started by hand, spoken to over its socket.

use assert_cmd::cargo::cargo_bin;
use interprocess::local_socket::{Stream, prelude::*};
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
    let root = dir
        .path()
        .join("project")
        .canonicalize()
        .unwrap_or_else(|_| {
            std::fs::create_dir_all(dir.path().join("project")).unwrap();
            dir.path().join("project").canonicalize().unwrap()
        });
    let run = dir.path().join("run");
    let st = Command::new(cargo_bin("ryucad"))
        .arg("init")
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(st.status.success());
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
    assert_eq!(status["version"]["protocol"], 1);
    ask(&conn, &Request::PartCreate { part: "p".into() }).unwrap();
    let list = ask(&conn, &Request::PartList {}).unwrap();
    assert_eq!(list["parts"][0]["part"], "p");
    let e = ask(&conn, &Request::PartCreate { part: "p".into() }).unwrap_err();
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
