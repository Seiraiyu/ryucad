//! Supply findings (design 347), in `check` as `source: supply`: from the
//! catalogues and distributors' records this machine holds, never
//! fetched, so never part of ERC's findings. Once per placed library component per design, naming it and
//! the occurrences it covers (`dnp` ones left out), each carrying when
//! its data was fetched and whose it is.

use crate::error::{Code, Result};
use crate::fault::Severity;
use crate::parts::Catalogues;
use crate::project::Project;
use crate::sourcing::{Assembly, Config};
use ryueda_parts::record::{Lifecycle, Record, squash};
use ryueda_parts::source::Call;
use serde_json::{Value, json};

/// A choice of a library component (the first, or alternate `alternate`):
/// its maker and part number, the records of its offers this machine
/// holds, and how many offers it has.
struct Choice {
    alternate: Option<usize>,
    manufacturer: String,
    mpn: String,
    records: Vec<Record>,
    offers: usize,
}

fn choices(c: &crate::libcomp::LibComp, cats: &Catalogues) -> Result<Vec<Choice>> {
    let mut out = Vec::new();
    let all = std::iter::once((None, &c.manufacturer, &c.mpn, &c.offers)).chain(
        c.alternates
            .iter()
            .enumerate()
            .map(|(i, a)| (Some(i), &a.manufacturer, &a.mpn, &a.offers)),
    );
    for (alternate, manufacturer, mpn, offers) in all {
        let mut records = Vec::new();
        for o in offers {
            for cat in cats.for_supplier(&o.supplier)? {
                records.extend(cat.get(o.sku.trim(), &Call::default())?);
            }
            // A distributor's record as kept here (`parts show`, `parts
            // refresh`): fresh within its time to live.
            for d in ryueda_parts::distributor::for_supplier(&o.supplier) {
                records.extend(cats.live.cached(d, &o.sku, cats.now));
            }
        }
        out.push(Choice {
            alternate,
            manufacturer: manufacturer.clone(),
            mpn: mpn.clone(),
            records,
            offers: offers.len(),
        });
    }
    Ok(out)
}

/// When a record was fetched and whose it is.
fn stamp(r: &Record) -> Value {
    json!({"fetched_at": r.fetched_at, "attribution": r.attribution, "source": r.source, "key": r.key})
}

fn alternate(ch: &Choice, r: &Record) -> Value {
    json!({"index": ch.alternate, "manufacturer": ch.manufacturer, "mpn": ch.mpn,
           "source": r.source, "key": r.key, "stock": r.stock})
}

/// Supply findings over every design, as `cfg` configures them, sorted by
/// code and then references.
pub fn findings_with(p: &Project, cats: &Catalogues, cfg: &Config) -> Result<Vec<Value>> {
    let mut out: Vec<Value> = Vec::new();
    let mut add = |code: Code, refs: Vec<String>, message: String, details: Value| {
        let severity = match cfg.level(code) {
            Some(crate::erc::Level::Off) => return,
            Some(crate::erc::Level::Error) => Severity::Error,
            _ => Severity::Warning,
        };
        out.push(
            json!({"code": code, "severity": severity, "source": "supply", "refs": refs,
                        "message": message, "details": details}),
        );
    };
    // Each catalogue the placed parts' offers draw on, stale or missing,
    // once; with none of them synced, nothing more to say.
    let used = crate::parts::catalogues_placed(p);
    // A distributor the offers draw on counts where this machine has its
    // keys: a part none of whose records is kept is then `part_no_data`,
    // which `parts refresh` answers. Without keys, as without catalogues,
    // there is nothing to judge by.
    let mut any = crate::parts::distributors_placed(p)
        .into_iter()
        .any(|d| cats.live.keys(d).ok().flatten().is_some());
    for name in &used {
        let (_, stale) = cats.state(name)?;
        if let Some(w) = stale {
            add(
                Code::PartsCacheStale,
                vec![],
                w["message"].as_str().unwrap_or("").to_owned(),
                w.clone(),
            );
        }
        any |= cats.get(name)?.is_some();
    }
    if !any {
        return Ok(sorted(out));
    }
    let per_board = cfg.build_quantity();
    for design in p.conn.designs.keys() {
        let Ok(n) = crate::netlist::flatten(p, design) else {
            continue;
        };
        for (of, c, placed) in crate::erc::placed_by_libcomp(p, &n) {
            if c.offers.is_empty() && c.alternates.iter().all(|a| a.offers.is_empty()) {
                continue;
            }
            let needed = placed.len() as u64 * per_board;
            let mut refs = vec![of.to_owned()];
            refs.extend(placed.iter().map(|o| o.reference.clone()));
            let named = placed
                .iter()
                .map(|o| o.refdes.clone().unwrap_or_else(|| o.reference.clone()))
                .collect::<Vec<_>>()
                .join(", ");
            let ch = choices(c, cats)?;
            if ch.iter().all(|x| x.records.is_empty()) {
                add(
                    Code::PartNoData,
                    refs,
                    format!("no source here has a record of {of}'s offers; {named} place it"),
                    json!({"libcomp": of, "design": design,
                           "offers": ch.iter().map(|x| x.offers).sum::<usize>()}),
                );
                continue;
            }
            for x in &ch {
                for r in &x.records {
                    if !x.mpn.trim().is_empty()
                        && !r.mpn.is_empty()
                        && squash(&r.mpn) != squash(&x.mpn)
                    {
                        let mut d = stamp(r);
                        d["libcomp"] = json!(of);
                        d["design"] = json!(design);
                        d["mpn"] = json!(x.mpn);
                        d["record_mpn"] = json!(r.mpn);
                        d["alternate"] = json!(x.alternate);
                        add(
                            Code::PartsMismatch,
                            refs.clone(),
                            format!("{of}: {} {} is {}, not {}", r.source, r.key, r.mpn, x.mpn),
                            d,
                        );
                    }
                }
            }
            let first = &ch[0];
            let stocked: Vec<&Record> =
                first.records.iter().filter(|r| r.stock.is_some()).collect();
            if let Some(best) = stocked.iter().max_by_key(|r| r.stock)
                && best.stock.unwrap_or(0) < needed
            {
                let mut d = stamp(best);
                d["libcomp"] = json!(of);
                d["design"] = json!(design);
                d["needed"] = json!(needed);
                d["stock"] = json!(best.stock);
                let clears = ch[1..].iter().find_map(|a| {
                    a.records
                        .iter()
                        .find(|r| r.stock.unwrap_or(0) >= needed)
                        .map(|r| alternate(a, r))
                });
                let tail = clears.as_ref().map_or(String::new(), |a| {
                    format!(
                        "; alternate {} {} has {}",
                        a["manufacturer"].as_str().unwrap_or(""),
                        a["mpn"].as_str().unwrap_or(""),
                        a["stock"]
                    )
                });
                d["alternate"] = clears.unwrap_or(Value::Null);
                add(
                    Code::PartOutOfStock,
                    refs.clone(),
                    format!(
                        "{of} ({}) has {} in stock at {}; {named} need {needed}{tail}",
                        first.mpn,
                        best.stock.unwrap_or(0),
                        best.source
                    ),
                    d,
                );
            }
            // A lifecycle any record states past active, the worst of them
            // (6.C: distributors give it; catalogues don't).
            if let Some(worst) = first
                .records
                .iter()
                .filter(|r| r.lifecycle.is_some_and(|l| l != Lifecycle::Active))
                .max_by_key(|r| rank(r.lifecycle))
            {
                let mut d = stamp(worst);
                d["libcomp"] = json!(of);
                d["design"] = json!(design);
                d["lifecycle"] = json!(worst.lifecycle);
                let active = ch[1..].iter().find_map(|a| {
                    a.records
                        .iter()
                        .find(|r| r.lifecycle == Some(Lifecycle::Active))
                        .map(|r| alternate(a, r))
                });
                let tail = active.as_ref().map_or(String::new(), |a| {
                    format!("; alternate {} is active", a["mpn"].as_str().unwrap_or(""))
                });
                d["alternate"] = active.unwrap_or(Value::Null);
                add(
                    Code::PartLifecycle,
                    refs.clone(),
                    format!(
                        "{of} ({}) is {} at {}{tail}",
                        first.mpn,
                        words(worst.lifecycle),
                        worst.source
                    ),
                    d,
                );
            }
            // The assembly house's Basic/Extended flag, from the records
            // that state it.
            let flagged: Vec<&Record> =
                first.records.iter().filter(|r| r.basic.is_some()).collect();
            if cfg.assembly == Some(Assembly::Jlcpcb)
                && !flagged.is_empty()
                && flagged.iter().all(|r| r.basic == Some(false))
            {
                let mut d = stamp(flagged[0]);
                d["libcomp"] = json!(of);
                d["design"] = json!(design);
                let basic = ch[1..].iter().find_map(|a| {
                    a.records
                        .iter()
                        .find(|r| r.basic == Some(true))
                        .map(|r| alternate(a, r))
                });
                let tail = basic.as_ref().map_or(String::new(), |a| {
                    format!("; alternate {} is Basic", a["mpn"].as_str().unwrap_or(""))
                });
                d["alternate"] = basic.unwrap_or(Value::Null);
                add(
                    Code::PartExtended,
                    refs.clone(),
                    format!(
                        "{of} ({}) is an Extended part at the assembly house{tail}",
                        first.mpn
                    ),
                    d,
                );
            }
        }
    }
    Ok(sorted(out))
}

/// How far past active a lifecycle is.
fn rank(l: Option<Lifecycle>) -> u8 {
    match l {
        Some(Lifecycle::Obsolete) => 3,
        Some(Lifecycle::LastTimeBuy) => 2,
        Some(Lifecycle::NotRecommended) => 1,
        _ => 0,
    }
}

fn words(l: Option<Lifecycle>) -> &'static str {
    match l {
        Some(Lifecycle::Obsolete) => "obsolete",
        Some(Lifecycle::LastTimeBuy) => "at last time buy",
        Some(Lifecycle::NotRecommended) => "not recommended for new designs",
        _ => "active",
    }
}

/// Findings by code, then references, so the order never depends on a
/// design's walk; one a second design repeats is kept once.
fn sorted(mut v: Vec<Value>) -> Vec<Value> {
    v.sort_by(|a, b| {
        (a["code"].as_str(), a["refs"].to_string())
            .cmp(&(b["code"].as_str(), b["refs"].to_string()))
    });
    v.dedup();
    v
}

/// Supply findings as the project's `parts.json` configures them.
pub fn findings(p: &Project, cats: &Catalogues) -> Result<Vec<Value>> {
    findings_with(p, cats, &crate::sourcing::config(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed_project() -> (tempfile::TempDir, Project) {
        let lc = |extra: Value| {
            let mut v = json!({"kind": "ryueda.libcomp", "format": 2, "symbol": {"id": "r"}, "prefix": "R",
                               "footprints": [{"footprint": {"id": "r_0402_1005metric"}}]});
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        let placed = |of: &str, refdes: &str| json!({"kind": "ryueda.component", "format": 1, "of": {"id": of}, "refdes": refdes});
        let files = vec![
            (
                "library/components/pa.json",
                lc(json!({"manufacturer": "A", "mpn": "PA",
                "offers": [{"supplier": "LCSC", "sku": "C1"}],
                "alternates": [{"manufacturer": "B", "mpn": "ALT1", "offers": [{"supplier": "LCSC", "sku": "C2"}]}]})),
            ),
            (
                "library/components/pb.json",
                lc(json!({"mpn": "M2", "offers": [{"supplier": "lcsc", "sku": "C3"}]})),
            ),
            (
                "library/components/pc.json",
                lc(json!({"mpn": "PC", "offers": [{"supplier": "DigiKey", "sku": "1-ND"}]})),
            ),
            ("library/components/pd.json", lc(json!({"mpn": "PD"}))),
            ("blocks/top/components/p1.json", placed("pa", "R11")),
            ("blocks/top/components/p2.json", placed("pa", "R12")),
            ("blocks/top/components/p3.json", placed("pb", "R13")),
            ("blocks/top/components/p4.json", placed("pc", "R14")),
            ("blocks/top/components/p5.json", placed("pd", "R15")),
        ];
        crate::netlist::tests::project(&files)
    }

    #[test]
    fn supply_findings_come_from_the_catalogue() {
        let (_dir, p) = placed_project();
        let cache = tempfile::tempdir().unwrap();
        ryueda_parts::jlc::testing::catalogue_of(
            &cache.path().join("jlc"),
            &[
                ("C1", "PA", 1, false),        // two placed, one in stock, Extended
                ("C2", "ALT1", 5_000, true),   // the alternate clears both
                ("C3", "M2-OTHER", 100, true), // a mismatch
                ("C25741", "RC0402FR-07100KL", 1_000_000, true), // the test library's r_100k
            ],
            1_000,
        );
        let cats = crate::parts::Catalogues::at(cache.path(), 2_000, crate::parts::MAX_AGE);
        let mut cfg = crate::sourcing::Config {
            assembly: Some(crate::sourcing::Assembly::Jlcpcb),
            ..Default::default()
        };
        let found = findings_with(&p, &cats, &cfg).unwrap();
        let codes: Vec<(&str, &str)> = found
            .iter()
            .map(|f| (f["code"].as_str().unwrap(), f["refs"][0].as_str().unwrap()))
            .collect();
        assert_eq!(
            codes,
            [
                ("part_extended", "lib/self/libcomp/pa"),
                ("part_no_data", "lib/self/libcomp/pc"),
                ("part_out_of_stock", "lib/self/libcomp/pa"),
                ("parts_mismatch", "lib/self/libcomp/pb"),
            ],
            "{found:#?}"
        );
        let oos = &found[2];
        assert_eq!(
            (
                oos["details"]["needed"].as_u64(),
                oos["details"]["stock"].as_u64()
            ),
            (Some(2), Some(1))
        );
        assert_eq!(
            oos["refs"].as_array().unwrap().len(),
            3,
            "the component and both occurrences"
        );
        assert_eq!(oos["details"]["alternate"]["mpn"], "ALT1");
        assert_eq!(oos["details"]["fetched_at"], 1_000);
        assert_eq!(
            oos["details"]["attribution"],
            ryueda_parts::jlc::ATTRIBUTION
        );
        assert_eq!(
            (oos["severity"].as_str(), oos["source"].as_str()),
            (Some("warning"), Some("supply"))
        );
        assert_eq!(
            found[0]["details"]["alternate"]["mpn"], "ALT1",
            "the Basic alternate"
        );
        assert_eq!(found[3]["details"]["record_mpn"], "M2-OTHER");
        // Ten boards: C2's 5,000 still clears 20; severities as set.
        cfg.build_quantity = Some(10);
        cfg.severity
            .insert("part_extended".into(), crate::erc::Level::Off);
        cfg.severity
            .insert("part_out_of_stock".into(), crate::erc::Level::Error);
        let found = findings_with(&p, &cats, &cfg).unwrap();
        assert!(found.iter().all(|f| f["code"] != "part_extended"));
        let oos = found
            .iter()
            .find(|f| f["code"] == "part_out_of_stock")
            .unwrap();
        assert_eq!(
            (oos["severity"].as_str(), oos["details"]["needed"].as_u64()),
            (Some("error"), Some(20))
        );
        // No assembly house: no Extended finding.
        let found = findings_with(&p, &cats, &crate::sourcing::Config::default()).unwrap();
        assert!(found.iter().all(|f| f["code"] != "part_extended"));
        // No catalogue: one stale finding, no per-part no-data.
        let none =
            crate::parts::Catalogues::at(&cache.path().join("none"), 2_000, crate::parts::MAX_AGE);
        let found = findings_with(&p, &none, &crate::sourcing::Config::default()).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            (
                found[0]["code"].as_str(),
                found[0]["details"]["reason"].as_str()
            ),
            (Some("parts_cache_stale"), Some("missing"))
        );
        // Old: stale once, the rest as before.
        let old = crate::parts::Catalogues::at(
            cache.path(),
            1_000 + crate::parts::MAX_AGE + 1,
            crate::parts::MAX_AGE,
        );
        let found = findings_with(&p, &old, &crate::sourcing::Config::default()).unwrap();
        assert_eq!(
            found
                .iter()
                .filter(|f| f["code"] == "parts_cache_stale")
                .count(),
            1
        );
        assert!(found.iter().any(|f| f["code"] == "part_out_of_stock"));
        // A project placing nothing with an offer says nothing.
        let (_e, bare) = crate::netlist::tests::project(&[]);
        let found = findings_with(&bare, &none, &crate::sourcing::Config::default()).unwrap();
        assert!(
            found.iter().all(|f| f["code"] == "parts_cache_stale"),
            "{found:?}"
        );
    }

    #[test]
    fn lifecycle_findings_come_from_kept_distributor_records() {
        let (_dir, p) = placed_project();
        let (cfg, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let live = crate::distributors::tests::fake(
            cfg.path(),
            cache.path(),
            std::sync::Arc::new(crate::distributors::NoNetwork),
            2_000,
        );
        let cats = crate::parts::Catalogues::at(cache.path(), 2_000, crate::parts::MAX_AGE)
            .with_live(live.clone());
        let dk = ryueda_parts::distributor::distributor("digikey").unwrap();
        // No keys here: DigiKey's offers say nothing, as before 6.C.
        let found = findings(&p, &cats).unwrap();
        assert!(
            found.iter().all(|f| f["code"] == "parts_cache_stale"),
            "{found:#?}"
        );
        crate::distributors::tests::logged_in(cfg.path());
        // Keys, nothing kept: pc has no data.
        let found = findings(&p, &cats).unwrap();
        assert!(
            found
                .iter()
                .any(|f| f["code"] == "part_no_data" && f["refs"][0] == "lib/self/libcomp/pc"),
            "{found:#?}"
        );
        // A record kept at 1,000, obsolete.
        let mut r = Record::new("digikey", "1-ND");
        r.mpn = "PC".into();
        r.stock = Some(10);
        r.lifecycle = Some(Lifecycle::Obsolete);
        r.fetched_at = Some(1_000);
        r.attribution = "Data from DigiKey".into();
        live.keep(dk, &[r]);
        let found = findings(&p, &cats).unwrap();
        let life: Vec<&Value> = found
            .iter()
            .filter(|f| f["code"] == "part_lifecycle")
            .collect();
        assert_eq!(life.len(), 1, "{found:#?}");
        assert_eq!(life[0]["refs"][0], "lib/self/libcomp/pc");
        assert_eq!(
            (
                life[0]["details"]["lifecycle"].as_str(),
                life[0]["details"]["fetched_at"].as_u64()
            ),
            (Some("obsolete"), Some(1_000))
        );
        assert_eq!(life[0]["details"]["attribution"], "Data from DigiKey");
        assert!(
            life[0]["message"]
                .as_str()
                .unwrap()
                .contains("PC) is obsolete at digikey")
        );
        assert!(
            found
                .iter()
                .all(|f| f["code"] != "part_no_data" || f["refs"][0] != "lib/self/libcomp/pc")
        );
        // Past its day the record is gone, and so is the finding.
        let later =
            crate::parts::Catalogues::at(cache.path(), 1_000 + 86_401, crate::parts::MAX_AGE)
                .with_live(live);
        assert!(
            findings(&p, &later)
                .unwrap()
                .iter()
                .all(|f| f["code"] != "part_lifecycle")
        );
    }
}
