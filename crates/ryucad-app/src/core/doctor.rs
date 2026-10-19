//! Diagnosing and repairing a project. A healthy project asks its daemon
//! (`doctor`, `doctor.repair`); a damaged one has no daemon (it can't open
//! the project), so the window runs `ryucad doctor` itself, exactly as an
//! agent would. Either way the report is the daemon's JSON.

use crate::core::link::Wake;
use ryucad::client::{Daemon, command};
use ryucad::error::{Code, Error, Result};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{Receiver, TryRecvError};

/// Runs `ryucad -C <dir> <args>` and returns its result, or its error.
pub fn cli(daemon: &Daemon, dir: &Path, args: &[String]) -> Result<Value> {
    let out = command(&daemon.exe)
        .arg("-C")
        .arg(dir)
        .args(["--no-daemon"])
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| {
            Error::new(
                Code::Daemon,
                format!("can't run {}: {e}", daemon.exe.display()),
            )
        })?;
    let mut doc: Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| Error::new(Code::Daemon, "ryucad didn't answer with JSON"))?;
    if doc["ok"] == true {
        return Ok(doc["result"].take());
    }
    Err(serde_json::from_value(doc["error"].take())
        .unwrap_or_else(|_| Error::new(Code::Internal, "ryucad failed without saying why")))
}

/// The arguments of `ryucad doctor` that apply `repairs` with `params`.
pub fn repair_args(repairs: &[String], params: &Map<String, Value>) -> Vec<String> {
    let mut args = vec!["doctor".to_owned()];
    for r in repairs {
        args.extend(["--repair".to_owned(), r.clone()]);
    }
    for (k, v) in params {
        let v = match v {
            Value::Array(a) => a
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(","),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        args.extend(["--param".to_owned(), format!("{k}={v}")]);
    }
    args
}

/// The report inside an answer: `doctor` gives one, `doctor.repair` gives
/// one under `report`.
pub fn report_of(v: &Value) -> Value {
    match v.get("report") {
        Some(r) => r.clone(),
        None => v.clone(),
    }
}

/// The feature order a `missing_part_index` repair proposes: the IDs in
/// the `part.json` it would write.
pub fn proposed_order(repair: &Value) -> Vec<String> {
    let content = repair["changes"][0]["content"].as_str().unwrap_or("{}");
    let v: Value = serde_json::from_str(content).unwrap_or_default();
    v["features"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f.as_str().map(str::to_owned))
        .collect()
}

/// A damaged project, diagnosed and repaired through the CLI, one run at a
/// time on a thread so the window keeps drawing.
pub struct Offline {
    pub root: PathBuf,
    daemon: Daemon,
    wake: Wake,
    running: Option<Receiver<Result<Value>>>,
    pub report: Option<Value>,
    pub error: Option<Error>,
}

impl Offline {
    /// Starts diagnosing the folder at `root`.
    pub fn new(root: &Path, daemon: &Daemon, wake: Wake) -> Offline {
        let mut o = Offline {
            root: root.to_path_buf(),
            daemon: daemon.clone(),
            wake,
            running: None,
            report: None,
            error: None,
        };
        o.run(vec!["doctor".to_owned()]);
        o
    }

    fn run(&mut self, args: Vec<String>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (daemon, root, wake) = (self.daemon.clone(), self.root.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let _ = tx.send(cli(&daemon, &root, &args));
            wake();
        });
        self.running = Some(rx);
    }

    /// Applies repairs by ID.
    pub fn repair(&mut self, repairs: &[String], params: &Map<String, Value>) {
        self.error = None;
        self.run(repair_args(repairs, params));
    }

    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Takes a finished run's answer. Returns whether one arrived.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.running else {
            return false;
        };
        let answer = match rx.try_recv() {
            Err(TryRecvError::Empty) => return false,
            Ok(a) => a,
            Err(TryRecvError::Disconnected) => Err(Error::new(Code::Internal, "doctor stopped")),
        };
        self.running = None;
        match answer {
            Ok(v) => self.report = Some(report_of(&v)),
            Err(e) => self.error = Some(e),
        }
        true
    }

    /// Whether the project can be opened now: its manifest is back.
    pub fn openable(&self) -> bool {
        self.root.join(ryucad::doc::project::MANIFEST).is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repairs_become_arguments() {
        let params = json!({"order": ["b", "a"], "unit": "in"});
        let args = repair_args(&["x:1".into(), "y:2".into()], params.as_object().unwrap());
        assert_eq!(
            args,
            [
                "doctor",
                "--repair",
                "x:1",
                "--repair",
                "y:2",
                "--param",
                "order=b,a",
                "--param",
                "unit=in"
            ]
        );
    }

    #[test]
    fn the_proposed_order_is_read_from_the_planned_file() {
        let repair = json!({"changes": [{"path": "parts/p/part.json", "action": "write",
            "content": "{\n  \"features\": [\n    \"b\",\n    \"a\"\n  ],\n  \"format\": 1\n}\n"}]});
        assert_eq!(proposed_order(&repair), ["b", "a"]);
        assert_eq!(
            report_of(&json!({"applied": [], "report": {"healthy": true}})),
            json!({"healthy": true})
        );
    }
}
