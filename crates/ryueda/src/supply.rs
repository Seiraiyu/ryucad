//! Supply findings (design 347), in `check` as `source: supply`: from the
//! catalogues this machine holds, never fetched, so never part of ERC's
//! findings. Once per placed library component per design, naming it and
//! the occurrences it covers (`dnp` ones left out), each carrying when
//! its data was fetched and whose it is.

use crate::error::{Code, Result};
use crate::fault::Severity;
use crate::parts::Catalogues;
use crate::project::Project;
use crate::sourcing::{Assembly, Config};
use ryueda_parts::record::{Record, squash};
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
    let mut any = false;
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
}
