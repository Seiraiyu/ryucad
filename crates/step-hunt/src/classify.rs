//! Signatures: what a case's failure, or a slow stage, is grouped by.
//!
//! `<stage>/<code>[/<reason>][/<kinds>]` for a refusal (a check's codes
//! are its failing findings' codes), `<stage>/<what>` for a round trip that
//! differs, `<stage>/timeout/<faces>/<surfaces>`, `<stage>/crash/<exit>`,
//! and `<stage>/slow/<faces>/<surfaces>` for a stage that passed slowly.
//! They group mechanically; which group is one root cause is the filer's
//! judgement.

use crate::split::Case;
use crate::stages::{Outcome, Ran, Stage};

/// A face count's bucket.
pub fn bucket(faces: usize) -> &'static str {
    match faces {
        0..50 => "faces<50",
        50..200 => "faces<200",
        200..1000 => "faces<1000",
        _ => "faces>=1000",
    }
}

fn joined(kinds: &[String]) -> String {
    if kinds.is_empty() {
        "-".to_owned()
    } else {
        kinds.join("+")
    }
}

/// The signature of a stage that didn't pass, or `None`.
pub fn of_stage(case: &Case, s: &Stage) -> Option<String> {
    let st = &s.stage;
    Some(match &s.outcome {
        Outcome::Ok => return None,
        Outcome::Failed { error, kinds } => {
            let code = error["code"].as_str().unwrap_or("unknown");
            let mut sig = format!("{st}/{code}");
            if code == "check_failed" {
                let mut codes: Vec<&str> = error["details"]["findings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|f| f["severity"] == "error")
                    .filter_map(|f| f["code"].as_str())
                    .collect();
                codes.sort_unstable();
                codes.dedup();
                sig.push('/');
                sig.push_str(&codes.join("+"));
            } else if let Some(r) = error["details"]["reason"].as_str() {
                sig.push('/');
                sig.push_str(r);
            }
            if !kinds.is_empty() {
                sig.push('/');
                sig.push_str(&joined(kinds));
            }
            sig
        }
        Outcome::Mismatch { what, .. } => format!("{st}/{what}"),
        Outcome::Timeout => format!(
            "{st}/timeout/{}/{}",
            bucket(case.faces),
            joined(&case.surfaces)
        ),
        Outcome::Crash { exit, .. } => match exit {
            Some(e) => format!("{st}/crash/{e}"),
            None => format!("{st}/crash/signal"),
        },
    })
}

/// A case's findings: its failure's signature first (if it failed), then
/// each stage that passed in more than `slow` seconds.
pub fn findings(case: &Case, ran: &Ran, slow: f64) -> Vec<String> {
    let mut out: Vec<String> = ran
        .stages
        .iter()
        .filter_map(|s| of_stage(case, s))
        .collect();
    for s in &ran.stages {
        if s.outcome == Outcome::Ok && s.seconds > slow {
            out.push(format!(
                "{}/slow/{}/{}",
                s.stage,
                bucket(case.faces),
                joined(&case.surfaces)
            ));
        }
    }
    out
}

/// Whether a signature depends on the machine's speed (`slow`,
/// `timeout`): a campaign reports them, but a test can't judge them.
pub fn is_timed(sig: &str) -> bool {
    sig.contains("/slow/") || sig.contains("/timeout/")
}

/// Whether a failed import came out otherwise under another part ID.
pub fn key_dependent(case: &Case, ran: &Ran) -> bool {
    match (&ran.key, ran.stages.first()) {
        (Some(k), Some(first)) => of_stage(case, k) != of_stage(case, first),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split::Kind;
    use serde_json::json;

    fn case(faces: usize) -> Case {
        Case {
            id: "0123456789abcdef".into(),
            kind: Kind::Body,
            path: "x".into(),
            faces,
            surfaces: vec!["B_SPLINE_SURFACE_WITH_KNOTS".into(), "PLANE".into()],
            unsplit: false,
            origins: Vec::new(),
        }
    }

    fn stage(name: &str, seconds: f64, outcome: Outcome) -> Stage {
        Stage {
            stage: name.into(),
            seconds,
            outcome,
        }
    }

    #[test]
    fn refusals_sign_with_reason_and_kinds() {
        let s = stage(
            "import",
            1.0,
            Outcome::Failed {
                error: json!({"code": "import_failed", "details": {"reason": "faces_overlap"}}),
                kinds: vec!["CYLINDRICAL_SURFACE".into(), "PLANE".into()],
            },
        );
        assert_eq!(
            of_stage(&case(6), &s).unwrap(),
            "import/import_failed/faces_overlap/CYLINDRICAL_SURFACE+PLANE"
        );
        let s = stage(
            "check",
            1.0,
            Outcome::Failed {
                error: json!({"code": "check_failed", "details": {"findings": [
                {"code": "mesh_faults", "severity": "error"},
                {"code": "faces_intersect", "severity": "error"},
                {"code": "mesh_faults", "severity": "error"},
                {"code": "instance_obsolete", "severity": "warning"}]}}),
                kinds: vec![],
            },
        );
        assert_eq!(
            of_stage(&case(6), &s).unwrap(),
            "check/check_failed/faces_intersect+mesh_faults"
        );
    }

    #[test]
    fn mismatches_timeouts_and_crashes_sign() {
        let c = case(117);
        assert_eq!(
            of_stage(
                &c,
                &stage(
                    "fixpoint",
                    1.0,
                    Outcome::Mismatch {
                        what: "bytes".into(),
                        details: json!({})
                    }
                )
            )
            .unwrap(),
            "fixpoint/bytes"
        );
        assert_eq!(
            of_stage(&c, &stage("import", 900.0, Outcome::Timeout)).unwrap(),
            "import/timeout/faces<200/B_SPLINE_SURFACE_WITH_KNOTS+PLANE"
        );
        assert_eq!(
            of_stage(
                &c,
                &stage(
                    "export",
                    1.0,
                    Outcome::Crash {
                        exit: Some(101),
                        stderr: String::new()
                    }
                )
            )
            .unwrap(),
            "export/crash/101"
        );
        assert_eq!(
            of_stage(
                &c,
                &stage(
                    "export",
                    1.0,
                    Outcome::Crash {
                        exit: None,
                        stderr: String::new()
                    }
                )
            )
            .unwrap(),
            "export/crash/signal"
        );
        assert_eq!(of_stage(&c, &stage("export", 1.0, Outcome::Ok)), None);
    }

    #[test]
    fn findings_add_slow_stages_after_the_failure() {
        let ran = Ran {
            stages: vec![
                stage("import", 45.0, Outcome::Ok),
                stage(
                    "check",
                    2.0,
                    Outcome::Failed {
                        error: json!({"code": "check_failed", "details": {"findings": []}}),
                        kinds: vec![],
                    },
                ),
            ],
            key: None,
        };
        assert_eq!(
            findings(&case(10), &ran, 30.0),
            [
                "check/check_failed/",
                "import/slow/faces<50/B_SPLINE_SURFACE_WITH_KNOTS+PLANE"
            ]
        );
    }

    #[test]
    fn timed_signatures_are_told_apart() {
        assert!(is_timed("import/slow/faces<50/PLANE"));
        assert!(is_timed("check/timeout/faces<1000/PLANE"));
        assert!(!is_timed("fixpoint/bytes"));
    }

    #[test]
    fn another_key_that_heals_is_key_dependent() {
        let failed = stage(
            "import",
            1.0,
            Outcome::Failed {
                error: json!({"code": "import_failed", "details": {"reason": "uncertain"}}),
                kinds: vec![],
            },
        );
        let mut ran = Ran {
            stages: vec![failed.clone()],
            key: Some(stage("import", 1.0, Outcome::Ok)),
        };
        assert!(key_dependent(&case(1), &ran));
        ran.key = Some(failed);
        assert!(!key_dependent(&case(1), &ran));
    }
}
