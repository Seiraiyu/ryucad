//! Doctor from the window: through the daemon for an open project, and
//! through the CLI for one too damaged to open.

mod common;

use common::{Project, ryucad};
use ryucad_app::core::doctor::Offline;
use ryucad_app::core::workspace::Workspace;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(30);

fn codes(report: &Value) -> Vec<String> {
    report["issues"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| i["code"].as_str().unwrap().to_owned())
        .collect()
}

/// The first repair offered for issue `code`.
fn repair(report: &Value, code: &str) -> String {
    let issue = report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["code"] == code)
        .unwrap();
    issue["repairs"][0]["id"].as_str().unwrap().to_owned()
}

#[test]
fn an_open_project_is_diagnosed_and_repaired_through_its_daemon() {
    let p = Project::new().with_part();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| ws.idle()));
    // A feature file nobody listed: an orphan.
    let boss = std::fs::read(p.root.join("parts/p/features/boss.json")).unwrap();
    std::fs::write(p.root.join("parts/p/features/spare.json"), boss).unwrap();
    ws.diagnose();
    assert!(ws.wait_until(WAIT, |ws| ws.doctor.is_some()));
    let report = ws.doctor.clone().unwrap();
    assert_eq!(codes(&report), ["orphan_feature"]);
    ws.repair(vec![repair(&report, "orphan_feature")], Default::default());
    assert!(ws.wait_until(WAIT, |ws| {
        ws.doctor.as_ref().is_some_and(|d| d["healthy"] == true)
    }));
    // The repair is in the log, as the window's.
    assert!(
        ws.wait_until(WAIT, |ws| ws.activity.entries.last().is_some_and(
            |e| e["method"] == "doctor.repair" && e["participant"]["id"] == ws.me()
        ))
    );
}

#[test]
fn a_damaged_project_is_repaired_through_the_cli() {
    let p = Project::new().with_part();
    p.ok(&["daemon", "stop"]);
    std::fs::remove_file(p.root.join("ryucad.json")).unwrap();
    let mut o = Offline::new(&p.root, ryucad(), Arc::new(|| {}));
    let wait = |o: &mut Offline, done: &dyn Fn(&Offline) -> bool| {
        let t = Instant::now();
        while !(o.poll() && done(o)) && !done(o) {
            assert!(t.elapsed() < WAIT, "timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait(&mut o, &|o| o.report.is_some());
    assert_eq!(codes(o.report.as_ref().unwrap()), ["missing_manifest"]);
    assert!(!o.openable());
    let fix = repair(o.report.as_ref().unwrap(), "missing_manifest");
    let params = json!({"unit": "in"});
    o.repair(&[fix], params.as_object().unwrap());
    wait(&mut o, &|o| !o.busy());
    assert_eq!(o.error, None);
    assert_eq!(o.report.as_ref().unwrap()["healthy"], true);
    assert!(o.openable());
    assert_eq!(p.ok(&["config", "get", "default_unit"])["value"], "in");
}
