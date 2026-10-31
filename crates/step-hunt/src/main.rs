//! `step-hunt`: the STEP bug finder's command line.
//!
//! ```text
//! step-hunt run --ryucad <bin> --out <dir> [--source NAME]… [--jobs N] [--timeout S] [--slow S] [--resume] [--only REPORT] [--keep]
//! step-hunt rerun <report.json> --ryucad <bin> --out <dir>      the report's failed cases only
//! step-hunt report <dir>                                         report.json and report.md again from cases.jsonl
//! step-hunt split --out <dir>                                    the cases, without running them
//! step-hunt extract <case> --out <file>                          one case's file, from the sources
//! ```
//!
//! Run from the workspace's root (or give `--root`): sources come from
//! `crates/step-hunt/sources.txt` and the ledger from `known.txt` beside
//! it.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use rayon::prelude::*;
use serde_json::{Value, json};

use step_hunt::call::{Ended, Ryucad};
use step_hunt::ledger::Ledger;
use step_hunt::report::{self, Record};
use step_hunt::{classify, sources, split, stages};

#[derive(Parser)]
#[command(
    name = "step-hunt",
    about = "Finds STEP import, healing and round-trip bugs"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Args, Clone)]
struct Common {
    /// The workspace's root, which relative sources and the defaults below
    /// are read from.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// The sources file (default: crates/step-hunt/sources.txt).
    #[arg(long)]
    sources: Option<PathBuf>,
    /// Only these sources, by name (repeatable; default: all).
    #[arg(long = "source")]
    only_sources: Vec<String>,
    /// Fail when a source's root is missing, instead of skipping it.
    #[arg(long)]
    require: bool,
    /// Where cases and runs go (default: a folder in the system's
    /// temporary folder).
    #[arg(long)]
    work: Option<PathBuf>,
}

#[derive(Args)]
struct RunArgs {
    #[command(flatten)]
    common: Common,
    /// The ryucad binary under test.
    #[arg(long)]
    ryucad: PathBuf,
    /// Where cases.jsonl, report.json and report.md go.
    #[arg(long)]
    out: PathBuf,
    /// Cases run side by side (default: the cores).
    #[arg(long)]
    jobs: Option<usize>,
    /// Seconds a command may take before it is killed.
    #[arg(long, default_value_t = 900)]
    timeout: u64,
    /// Seconds over which a stage that passes is a `slow` finding.
    #[arg(long, default_value_t = 30.0)]
    slow: f64,
    /// Keep cases.jsonl's finished cases and run the rest.
    #[arg(long)]
    resume: bool,
    /// Only the cases a report found failing or slow.
    #[arg(long)]
    only: Option<PathBuf>,
    /// Keep each case's projects under the work folder.
    #[arg(long)]
    keep: bool,
    /// The ledger (default: crates/step-hunt/known.txt).
    #[arg(long)]
    ledger: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Split the sources and run every case.
    Run(RunArgs),
    /// Run a report's failed and slow cases again.
    Rerun {
        report: PathBuf,
        #[command(flatten)]
        run: RunArgs,
    },
    /// Write report.json and report.md from a run's cases.jsonl.
    Report {
        out: PathBuf,
        /// The ledger (default: crates/step-hunt/known.txt).
        #[arg(long)]
        ledger: Option<PathBuf>,
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Split the sources into cases and list them, running nothing.
    Split {
        #[command(flatten)]
        common: Common,
        /// Where cases.json goes.
        #[arg(long)]
        out: PathBuf,
    },
    /// Write one case's file, made again from the sources.
    Extract {
        case: String,
        #[command(flatten)]
        common: Common,
        #[arg(long)]
        out: PathBuf,
    },
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("step-hunt: {msg}");
    std::process::exit(2);
}

fn work_dir(c: &Common) -> PathBuf {
    c.work
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join(format!("step-hunt-{}", std::process::id())))
}

/// Splits the chosen sources; the work folder holds the cases.
fn split_sources(c: &Common, work: &Path) -> split::Split {
    let path = c
        .sources
        .clone()
        .unwrap_or_else(|| c.root.join("crates/step-hunt/sources.txt"));
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    let mut list = sources::parse(&text).unwrap_or_else(|e| fail(e));
    if !c.only_sources.is_empty() {
        for n in &c.only_sources {
            if !list.iter().any(|s| &s.name == n) {
                fail(format!("no source {n}"));
            }
        }
        list.retain(|s| c.only_sources.contains(&s.name));
    }
    let s = split::split(&list, &c.root, work).unwrap_or_else(|e| fail(e));
    for m in &s.missing {
        if c.require {
            fail(format!("source {m}: its root is missing"));
        }
        eprintln!("step-hunt: source {m}: its root is missing, skipped");
    }
    s
}

fn ledger(path: Option<PathBuf>, root: &Path) -> Ledger {
    let path = path.unwrap_or_else(|| root.join("crates/step-hunt/known.txt"));
    match std::fs::read_to_string(&path) {
        Ok(t) => Ledger::parse(&t).unwrap_or_else(|e| fail(e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ledger::default(),
        Err(e) => fail(format!("{}: {e}", path.display())),
    }
}

/// Writes report.json and report.md under `out` from its cases.jsonl.
fn write_report(out: &Path, ryucad: &Value, ledger: &Ledger, partial: bool) -> Value {
    let text = std::fs::read_to_string(out.join("cases.jsonl")).unwrap_or_default();
    let records = report::read_records(&text);
    let v = report::report(ryucad, &records, ledger, partial);
    let json = serde_json::to_string_pretty(&v).expect("serializes") + "\n";
    std::fs::write(out.join("report.json"), json).unwrap_or_else(|e| fail(e));
    std::fs::write(out.join("report.md"), report::markdown(&v, &records))
        .unwrap_or_else(|e| fail(e));
    v
}

/// The case ids a report found failing or slow.
fn failing_in(path: &Path) -> BTreeSet<String> {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    let v: Value =
        serde_json::from_str(&text).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    v["records"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["findings"].as_array().is_some_and(|f| !f.is_empty()))
        .filter_map(|r| r["case"]["id"].as_str().map(str::to_owned))
        .collect()
}

fn run(a: RunArgs) {
    let work = work_dir(&a.common);
    let ryucad = Ryucad {
        exe: a.ryucad.clone(),
        timeout: Duration::from_secs(a.timeout),
    };
    std::fs::create_dir_all(&a.out).unwrap_or_else(|e| fail(format!("{}: {e}", a.out.display())));
    std::fs::create_dir_all(&work).unwrap_or_else(|e| fail(format!("{}: {e}", work.display())));
    let version = match ryucad.call(&work, &work, "version", &["version"]).ended {
        Ended::Answered(v) if v["ok"] == true => v["result"].clone(),
        other => fail(format!(
            "{} doesn't answer `version`: {other:?}",
            a.ryucad.display()
        )),
    };
    let ledger = ledger(a.ledger.clone(), &a.common.root);
    let split = split_sources(&a.common, &work);
    let jsonl = a.out.join("cases.jsonl");
    let done: BTreeSet<String> = if a.resume {
        report::read_records(&std::fs::read_to_string(&jsonl).unwrap_or_default())
            .into_keys()
            .collect()
    } else {
        let _ = std::fs::remove_file(&jsonl);
        BTreeSet::new()
    };
    let only = a.only.as_deref().map(failing_in);
    let cases: Vec<&split::Case> = split
        .cases
        .values()
        .filter(|c| !done.contains(&c.id))
        .filter(|c| only.as_ref().is_none_or(|o| o.contains(&c.id)))
        .collect();
    eprintln!(
        "step-hunt: {} cases to run ({} done before)",
        cases.len(),
        done.len()
    );
    let file = Mutex::new(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&jsonl)
            .unwrap_or_else(|e| fail(format!("{}: {e}", jsonl.display()))),
    );
    let jobs = a
        .jobs
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build()
        .expect("a pool");
    let total = cases.len();
    let count = std::sync::atomic::AtomicUsize::new(0);
    pool.install(|| {
        cases.par_iter().with_max_len(1).for_each(|case| {
            let ran = stages::run(&ryucad, case, &work, a.keep);
            let record = Record {
                case: (*case).clone(),
                findings: classify::findings(case, &ran, a.slow),
                key_dependent: classify::key_dependent(case, &ran),
                ran,
            };
            let line = serde_json::to_string(&record).expect("serializes") + "\n";
            file.lock()
                .expect("one writer")
                .write_all(line.as_bytes())
                .unwrap_or_else(|e| fail(e));
            let n = count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            eprintln!(
                "step-hunt: {n}/{total} {} {}",
                case.id,
                record.findings.first().map_or("ok", String::as_str)
            );
        });
    });
    let v = write_report(&a.out, &version, &ledger, false);
    let new: usize = v["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| g["new"].as_u64().unwrap_or(0) as usize)
        .sum();
    println!(
        "{}",
        json!({"cases": v["cases"], "failed": v["failed"], "groups": v["groups"].as_array().map_or(0, Vec::len), "new": new, "fixed": v["fixed"].as_array().map_or(0, Vec::len), "report": a.out.join("report.md")})
    );
    if a.common.work.is_none() && !a.keep {
        let _ = std::fs::remove_dir_all(&work);
    }
}

fn main() {
    match Cli::parse().command {
        Cmd::Run(a) => run(a),
        Cmd::Rerun { report, run: mut a } => {
            a.only = Some(report);
            run(a);
        }
        Cmd::Report {
            out,
            ledger: l,
            root,
        } => {
            // A run stopped before its end wrote no report.json: the
            // report made from its records says it is partial.
            let before = std::fs::read_to_string(out.join("report.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok());
            let ryucad = before.as_ref().map_or(Value::Null, |v| v["ryucad"].clone());
            write_report(&out, &ryucad, &ledger(l, &root), before.is_none());
        }
        Cmd::Split { common, out } => {
            std::fs::create_dir_all(&out).unwrap_or_else(|e| fail(e));
            let work = common.work.clone().unwrap_or_else(|| out.clone());
            let s = split_sources(&common, &work);
            let cases: Vec<&split::Case> = s.cases.values().collect();
            let json = serde_json::to_string_pretty(&json!({"cases": cases, "missing": s.missing}))
                .expect("serializes");
            std::fs::write(out.join("cases.json"), json + "\n").unwrap_or_else(|e| fail(e));
            println!("{} cases in {}", cases.len(), out.join("cases").display());
        }
        Cmd::Extract { case, common, out } => {
            let work = work_dir(&common);
            let s = split_sources(&common, &work);
            let Some(c) = s.cases.get(&case) else {
                fail(format!("no case {case} in the sources"))
            };
            std::fs::copy(&c.path, &out).unwrap_or_else(|e| fail(e));
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"case": c, "out": out})).expect("serializes")
            );
            if common.work.is_none() {
                let _ = std::fs::remove_dir_all(&work);
            }
        }
    }
}
