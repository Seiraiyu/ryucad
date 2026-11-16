//! Thread anchors (design 667): a reference path with its design,
//! `<design>:<block>/<object>` or a sheet `<design>:<block>/sheet:<id>`,
//! as the hosted review service pins a comment thread, and whether it
//! stands in a project: the sheet and box that draw it, or what took it
//! away. Read-only.

use crate::error::{Code, Error, Result};
use crate::project::Project;
use serde_json::{Value, json};

/// `anchor` as its design and its reference path, or `invalid_param`
/// (`param: "ref"`).
pub fn split(anchor: &str) -> Result<(&str, &str)> {
    match anchor.split_once(':') {
        Some((d, rest)) if !d.is_empty() && !d.contains('/') && rest.contains('/') => Ok((d, rest)),
        _ => Err(Error::new(
            Code::InvalidParam,
            format!("{anchor} isn't a thread anchor: <design>:<block>/<object>"),
        )
        .with("param", "ref")
        .with("ref", anchor)),
    }
}

/// Where `anchor` stands in `p`: `{ref, stands: true, kind, block,
/// sheet, box}` (the first sheet in canvas order drawing it, its box in
/// `scene`'s frame, the box `scene` gives its owner; both null when it is
/// drawn on no sheet), `{ref, stands: false, removed_in: "design" |
/// "block"}` when its design is gone or no longer places its block, else
/// `{ref, stands: false, gone: true}`.
pub fn stand(p: &Project, anchor: &str) -> Result<Value> {
    let (design, rest) = split(anchor)?;
    let missing = |why: Value| {
        let mut v = json!({"ref": anchor, "stands": false});
        if let (Some(o), Value::Object(w)) = (v.as_object_mut(), why) {
            o.extend(w);
        }
        v
    };
    if !p.conn.designs.keys().any(|k| k.as_str() == design) {
        return Ok(missing(json!({"removed_in": "design"})));
    }
    let block = rest.split_once('/').map_or(rest, |(b, _)| b);
    let placed = crate::schemcmd::blocks(p, Some(design), None)?;
    let Some(b) = placed.iter().find(|b| b.id.as_str() == block) else {
        return Ok(missing(json!({"removed_in": "block"})));
    };
    if let Some(sheet) = rest.strip_prefix(&format!("{block}/sheet:")) {
        let laid = crate::schem::lay(p, b, None, &[], ryusch::input::Relayout::None)?;
        return Ok(match laid.outcome.layout.sheets.get(sheet) {
            Some(_) => json!({"ref": anchor, "stands": true, "kind": "sheet", "block": block,
                              "sheet": sheet, "box": null}),
            None => missing(json!({"gone": true})),
        });
    }
    let r = match crate::refs::resolve(p, rest) {
        Ok(r) => r,
        Err(e) if e.code == Code::RefUnresolved => return Ok(missing(json!({"gone": true}))),
        Err(e) => return Err(e),
    };
    let stands = |sheet: Value, boxed: Value| {
        json!({"ref": anchor, "stands": true, "kind": r.kind, "block": r.block,
               "sheet": sheet, "box": boxed})
    };
    match crate::scene::locate(p, &r, rest, &crate::view::View::default()) {
        // `locate` boxes what the sheet paints, in the sheet's own frame,
        // as `scene` boxes its owners: no shift.
        Ok(f) => {
            let (a, z) = (f.boxed.min, f.boxed.max);
            Ok(stands(
                json!(f.shown.sheet),
                json!([[a[0], a[1]], [z[0], z[1]]]),
            ))
        }
        Err(e) if e.details.get("reason").and_then(Value::as_str) == Some("not_drawn") => {
            Ok(stands(Value::Null, Value::Null))
        }
        Err(e) => Err(e),
    }
}

/// `resolve`: each of `anchors` as [`stand`] answers it, in order;
/// the first malformed one refuses the whole call.
pub fn stand_all(p: &Project, anchors: &[String]) -> Result<Value> {
    for a in anchors {
        split(a)?;
    }
    let refs: Vec<Value> = anchors.iter().map(|a| stand(p, a)).collect::<Result<_>>()?;
    Ok(json!({"refs": refs}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlist::tests::project;

    #[test]
    fn anchors_split_into_design_and_path() {
        assert_eq!(split("d:top/r1").unwrap(), ("d", "top/r1"));
        assert_eq!(split("d:top/rail:gnd").unwrap(), ("d", "top/rail:gnd"));
        for bad in ["top/r1", ":top/r1", "d:top", "top/rail:gnd"] {
            let e = split(bad).unwrap_err();
            assert_eq!(
                (e.code, &e.details["param"]),
                (Code::InvalidParam, &json!("ref")),
                "{bad}"
            );
        }
    }

    #[test]
    fn what_stands_and_what_is_gone() {
        let (_dir, p) = project(&[]);
        let r = stand(&p, "d:top/r1").unwrap();
        assert_eq!(
            (&r["stands"], &r["kind"], &r["block"]),
            (&json!(true), &json!("component"), &json!("top")),
            "{r}"
        );
        assert!(r["sheet"].is_string() && r["box"].is_array(), "{r}");
        assert_eq!(stand(&p, "d:top/nope").unwrap()["gone"], true);
        assert_eq!(stand(&p, "d:nope/r1").unwrap()["removed_in"], "block");
        assert_eq!(stand(&p, "x:top/r1").unwrap()["removed_in"], "design");
    }

    #[test]
    fn a_components_box_is_its_box_in_the_scene() {
        let (_dir, p) = project(&[]);
        let r = stand(&p, "d:top/r1").unwrap();
        let ask = crate::scene::Ask {
            design: None,
            block: Some("top".into()),
            sheet: r["sheet"].as_str().map(str::to_owned),
            layer: None,
            theme: Default::default(),
        };
        let scene: Value = serde_json::from_str(&crate::scene::scene(&p, &ask).unwrap()).unwrap();
        let owner = scene["owners"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["ref"] == "top/r1")
            .unwrap();
        assert_eq!(r["box"], owner["box"], "the picker and resolve agree");
    }
}
