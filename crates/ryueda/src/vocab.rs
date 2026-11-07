//! An agent's vocabulary for a block's schematic, both ways: designators
//! (`U3`, a unit `U1:2`), pins (`U3.VDD` by name, `U3.4` by number),
//! nets by name, rails (`rail:3V3`), ports (`port:SDA`), child instances
//! by ID; and instructions as text (`beside C1 U1 left`). The files hold
//! keys and IDs; these turn one into the other.

use crate::design::BlockFiles;
use crate::error::{Code, Error, Result};
use crate::project::Project;
use crate::schem::{Built, Of};
use ryusch::geom::Side;
use ryusch::layout::{Axis, IdiomChoice, IdiomKind, Instruction, NetStyle};

/// One block's names.
pub struct Names<'a> {
    pub p: &'a Project,
    pub b: &'a BlockFiles,
    pub built: &'a Built,
}

fn not_found(what: &str, text: &str, block: &BlockFiles) -> Error {
    Error::new(
        Code::NotFound,
        format!("block {} has no {what} {text}", block.id),
    )
    .with("reference", text)
    .with("block", &block.id)
}

/// Splits an instruction's text into words; double quotes hold spaces.
pub fn words(text: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if quoted {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{text:?} has an unclosed quote"),
        ));
    }
    if any {
        out.push(cur);
    }
    Ok(out)
}

impl Names<'_> {
    fn component(&self, text: &str) -> Option<String> {
        let by_refdes = self.b.components.iter().find(|(_, c)| {
            c.item
                .as_ref()
                .is_ok_and(|c| c.refdes.as_deref() == Some(text))
        });
        if let Some((id, _)) = by_refdes {
            return Some(id.to_string());
        }
        self.b
            .components
            .keys()
            .find(|id| id.as_str() == text)
            .map(|id| id.to_string())
    }

    /// The part key (or a component's, naming all its units) `text` names.
    pub fn part(&self, text: &str) -> Result<String> {
        if let Some(r) = text.strip_prefix("region:") {
            return self.region(r).map(|id| format!("region:{id}"));
        }
        if let Some(p) = text.strip_prefix("port:") {
            let block = self.b.block.item.as_ref().map_err(|e| e.clone())?;
            return block
                .port(p)
                .map(|id| format!("port:{id}"))
                .ok_or_else(|| not_found("port", p, self.b));
        }
        let iid = text.strip_prefix("instance:").unwrap_or(text);
        if let Some(id) = self.b.instances.keys().find(|k| k.as_str() == iid) {
            // A layer of a stack names the stack's one symbol.
            return Ok(format!("instance:{}", self.built.front(id)));
        }
        if let Some((c, unit)) = text.split_once(':')
            && let Some(id) = self.component(c)
        {
            let key = format!("{id}:{unit}");
            if self.built.input.part(&key).is_some() {
                return Ok(key);
            }
            return Err(not_found("unit", text, self.b));
        }
        if let Some(id) = self.component(text) {
            return Ok(id);
        }
        // As drawn: a unit's designator (U3B), a child block's name, unique.
        let drawn: Vec<&str> = self
            .built
            .input
            .parts
            .iter()
            .filter(|x| x.refdes == text)
            .map(|x| x.key.as_str())
            .collect();
        if let [one] = drawn.as_slice() {
            return Ok((*one).to_owned());
        }
        // A region's title on the block's layout.
        if let Ok(id) = self.region(text) {
            return Ok(format!("region:{id}"));
        }
        Err(not_found(
            "part (by designator, unit, instance, port: or as drawn)",
            text,
            self.b,
        ))
    }

    /// The region `text` names on the block's layout: an ID (`r3`), else
    /// a title one region has.
    pub fn region(&self, text: &str) -> Result<String> {
        let regions = match &self.b.layout {
            Some(Ok(l)) => &l.regions,
            _ => {
                return Err(Error::new(
                    Code::NotFound,
                    format!("block {} has no layout, so no region {text}", self.b.id),
                ));
            }
        };
        if regions.contains_key(text) {
            return Ok(text.to_owned());
        }
        let titled: Vec<&String> = regions
            .iter()
            .filter(|(_, r)| r.title == text)
            .map(|(id, _)| id)
            .collect();
        if let [one] = titled.as_slice() {
            return Ok((*one).clone());
        }
        let list: Vec<String> = regions
            .iter()
            .map(|(id, r)| format!("{id} ({})", r.title))
            .collect();
        Err(Error::new(
            Code::NotFound,
            format!(
                "block {} has no region {text}; its regions are {}",
                self.b.id,
                list.join(", ")
            ),
        )
        .with("regions", list))
    }

    /// The pin `text` (`U3.VDD`, `U3.4`) names: `<part key>/<number>`.
    pub fn pin(&self, text: &str) -> Result<String> {
        let (part, pin) = text
            .rsplit_once('.')
            .ok_or_else(|| not_found("pin", text, self.b))?;
        let key = self.part(part)?;
        let mut hits = Vec::new();
        for x in &self.built.input.parts {
            if x.key != key && !x.key.starts_with(&format!("{key}:")) {
                continue;
            }
            for p in &x.pins {
                if p.number == pin {
                    return Ok(format!("{}/{}", x.key, p.number));
                }
                // By its name, or one part of a name like RXD0/MODE0.
                if p.name == pin || (p.name.contains('/') && p.name.split('/').any(|n| n == pin)) {
                    hits.push(format!("{}/{}", x.key, p.number));
                }
            }
        }
        match hits.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(not_found("pin", text, self.b)),
            many => Err(Error::new(
                Code::InvalidParam,
                format!("{text} names {} pins; name one by number", many.len()),
            )
            .with("pins", many)),
        }
    }

    /// A pin or a part: what `near` is near.
    pub fn pin_or_part(&self, text: &str) -> Result<String> {
        match self.pin(text) {
            Ok(p) => Ok(p),
            Err(_) => self.part(text),
        }
    }

    /// What a word of `release` names: a part first, then `net:N`, an
    /// instruction's ID (`i7`), a net by name.
    fn released(&self, text: &str) -> Result<Released> {
        if let Some(n) = text.strip_prefix("net:") {
            return Ok(Released::Net(self.net(n)?));
        }
        let part = match self.part(text) {
            Ok(k) => return Ok(Released::Part(k)),
            Err(e) => e,
        };
        let id = text
            .strip_prefix('i')
            .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
        if id {
            return Ok(Released::Id(text.to_owned()));
        }
        self.net(text).map(Released::Net).map_err(|_| part)
    }

    /// The net key `text` names: a net of the block by name or ID, a rail
    /// (`rail:3V3`, or its name alone), a port's net (`port:SDA`).
    pub fn net(&self, text: &str) -> Result<String> {
        let input = &self.built.input;
        if let Some(r) = text.strip_prefix("rail:") {
            let id = self
                .p
                .conn
                .rail(r)
                .ok_or_else(|| not_found("rail", r, self.b))?;
            return Ok(format!("rail:{id}"));
        }
        if let Some(port) = text.strip_prefix("port:") {
            return self.part(&format!("port:{port}"));
        }
        if let Some(id) = self.b.net(text) {
            return Ok(id.to_string());
        }
        if let Some(n) = input.nets.iter().find(|n| n.name == text) {
            return Ok(n.key.clone());
        }
        Err(not_found("net", text, self.b))
    }

    /// How a part key reads: a designator, a unit, an instance or a port.
    pub fn part_text(&self, key: &str) -> String {
        if key.starts_with("instance:") {
            return key.to_owned();
        }
        if let Some(id) = key.strip_prefix("port:") {
            let name = self
                .b
                .block
                .item
                .as_ref()
                .ok()
                .and_then(|b| b.ports.iter().find(|(k, _)| k.as_str() == id))
                .map_or(id.to_owned(), |(_, p)| p.name.clone());
            return format!("port:{name}");
        }
        let (comp, unit) = match key.split_once(':') {
            Some((c, u)) => (c, Some(u)),
            None => (key, None),
        };
        let refdes = self
            .b
            .components
            .iter()
            .find(|(k, _)| k.as_str() == comp)
            .and_then(|(_, c)| c.item.as_ref().ok()?.refdes.clone())
            .unwrap_or_else(|| comp.to_owned());
        match unit {
            Some(u) => format!("{refdes}:{u}"),
            None => refdes,
        }
    }

    pub fn net_text(&self, key: &str) -> String {
        if let Some(port) = key.strip_prefix("port:") {
            return self.part_text(&format!("port:{port}"));
        }
        match self.built.input.nets.iter().find(|n| n.key == key) {
            Some(n) if key.starts_with("rail:") => format!("rail:{}", n.name),
            Some(n) => n.name.clone(),
            None => key.to_owned(),
        }
    }

    fn quote(s: &str) -> String {
        if s.contains(char::is_whitespace) || s.is_empty() {
            format!("\"{s}\"")
        } else {
            s.to_owned()
        }
    }

    /// A finding's fixes (`ryusch::lint::Finding::fixes`) as batches of
    /// text an agent sends back.
    pub fn fixes(&self, fixes: &[Vec<Instruction>]) -> Vec<Vec<String>> {
        fixes
            .iter()
            .map(|batch| batch.iter().map(|i| self.text(i)).collect())
            .collect()
    }

    /// The same, for a finding as JSON (a description's): its `fixes`
    /// written as text in place.
    pub fn fixes_in(&self, finding: &mut serde_json::Value) {
        let Some(v) = finding.get("fixes").cloned() else {
            return;
        };
        if let Ok(f) = serde_json::from_value::<Vec<Vec<Instruction>>>(v) {
            finding["fixes"] = serde_json::json!(self.fixes(&f));
        }
    }

    /// An instruction as text an agent sends back.
    pub fn text(&self, i: &Instruction) -> String {
        // A pin subject (a pin-level align's) as `U2.SCL`: by its name
        // where no other pin of its part has it, else by its number (a
        // child block's pins are numbered by random IDs).
        let pin_text = |p: &str, n: &str| -> String {
            let pins = self
                .built
                .input
                .parts
                .iter()
                .find(|x| x.key == p)
                .map(|x| &x.pins[..])
                .unwrap_or_default();
            let name = pins
                .iter()
                .find(|x| x.number == n)
                .map(|x| x.name.as_str())
                .filter(|m| !m.is_empty() && pins.iter().filter(|x| x.name == *m).count() == 1);
            format!("{}.{}", self.part_text(p), name.unwrap_or(n))
        };
        let parts = |l: &[String]| {
            l.iter()
                .map(|k| match k.rsplit_once('/') {
                    Some((p, n)) => Self::quote(&pin_text(p, n)),
                    None => Self::quote(&self.part_text(k)),
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        let side = |s: Side| match s {
            Side::Left => "left",
            Side::Right => "right",
            Side::Up => "above",
            Side::Down => "below",
        };
        match i {
            Instruction::Sheet { parts: l, sheet } => {
                format!("sheet {} {}", parts(l), Self::quote(sheet))
            }
            Instruction::Beside {
                parts: l,
                of,
                side: s,
            } => {
                format!(
                    "beside {} {} {}",
                    parts(l),
                    Self::quote(&self.part_text(of)),
                    side(*s)
                )
            }
            Instruction::Near { parts: l, to } => {
                let to = match to.rsplit_once('/') {
                    Some((k, n)) => format!("{}.{n}", self.part_text(k)),
                    None => self.part_text(to),
                };
                format!("near {} {}", parts(l), Self::quote(&to))
            }
            Instruction::Align { parts: l, axis } => format!(
                "align {} {}",
                parts(l),
                if *axis == Axis::Row { "row" } else { "column" }
            ),
            Instruction::Group { parts: l } => format!("group {}", parts(l)),
            Instruction::Edge { parts: l, side: s } => format!(
                "edge {} {}",
                parts(l),
                match s {
                    Side::Up => "top",
                    Side::Down => "bottom",
                    s => side(*s),
                }
            ),
            Instruction::Orient {
                parts: l,
                rotation,
                mirror,
            } => format!(
                "orient {} {rotation}{}",
                parts(l),
                if *mirror { " mirror" } else { "" }
            ),
            Instruction::Net { net, style } => format!(
                "net {} {}",
                Self::quote(&self.net_text(net)),
                match style {
                    NetStyle::Wire => "wire",
                    NetStyle::Label => "label",
                    NetStyle::Port => "port",
                }
            ),
            Instruction::Idiom { parts: l, kind } => format!(
                "idiom {} {}",
                parts(l),
                match kind {
                    IdiomChoice::None => "none",
                    IdiomChoice::Kind(k) => k.as_str(),
                }
            ),
            Instruction::Flow { parts: l } => format!("flow {} reversed", parts(l)),
            Instruction::Shift {
                parts: l,
                side: s,
                steps,
            } => format!(
                "shift {} {} {steps}",
                parts(l),
                match s {
                    Side::Up => "up",
                    Side::Down => "down",
                    Side::Left => "left",
                    Side::Right => "right",
                }
            ),
            Instruction::Region { title, parts: l } => {
                format!("region {} {}", Self::quote(title), parts(l))
            }
            Instruction::RegionNone { parts: l } => format!("region none {}", parts(l)),
            Instruction::Paper {
                sheet,
                size,
                orientation,
            } => format!(
                "paper {} {size}{}",
                Self::quote(sheet),
                match orientation {
                    Some(ryusch::layout::Orientation::Landscape) => " landscape",
                    Some(ryusch::layout::Orientation::Portrait) => " portrait",
                    None => "",
                }
            ),
        }
    }
}

/// What a batch's text says: an instruction, or `release` of subjects.
#[derive(Debug, PartialEq)]
pub enum Said {
    Do(Instruction),
    Release(Vec<Released>),
    /// `sheet rename S NAME`: a sheet (by ID or name) given a new name.
    RenameSheet {
        from: String,
        to: String,
    },
    /// `region rename R TITLE`: a region (by ID) given a title, as a
    /// lasting `region` instruction over its parts.
    RenameRegion {
        region: String,
        title: String,
    },
}

/// What `release` names: a part (every instruction naming it), one
/// instruction by its ID, or a net (its style instructions).
#[derive(Debug, PartialEq)]
pub enum Released {
    Part(String),
    Id(String),
    Net(String),
}

fn usage(text: &str, form: &str) -> Error {
    Error::new(
        Code::InvalidParam,
        format!("{text:?} isn't an instruction; write {form}"),
    )
    .with("instruction", text)
    .with("form", form)
}

/// The instruction forms, for refusals and help.
pub const FORMS: &[&str] = &[
    "sheet X... SHEET",
    "sheet rename SHEET NAME",
    "beside X... Y left|right|above|below",
    "near X... P (a pin U1.VDD or a part)",
    "align X... row|column (X a part, or a pin U1.SCL to line pins up)",
    "group X...",
    "edge X... left|right|top|bottom",
    "orient X... 0|90|180|270 [mirror]",
    "net N wire|label|port",
    "idiom X... KIND|none",
    "flow X... reversed",
    "shift X... up|down|left|right STEPS (a move only)",
    "release X...|ID|net:N",
    "region TITLE X... | region none X... | region rename REGION TITLE (X may be a region)",
    "paper SHEET SIZE [landscape|portrait]",
];

impl Names<'_> {
    /// Reads one instruction's text in this block's terms.
    pub fn read(&self, text: &str) -> Result<Said> {
        let w = words(text)?;
        let Some((verb, rest)) = w.split_first() else {
            return Err(usage(text, FORMS.join("; ").as_str()));
        };
        let form = FORMS
            .iter()
            .find(|f| f.split(' ').next() == Some(verb.as_str()))
            .copied()
            .ok_or_else(|| usage(text, &FORMS.join("; ")))?;
        let many = |l: &[String]| -> Result<Vec<String>> {
            if l.is_empty() {
                return Err(usage(text, form));
            }
            l.iter().map(|x| self.part(x)).collect()
        };
        let last = |n: usize| -> Result<(&[String], &[String])> {
            if rest.len() <= n {
                return Err(usage(text, form));
            }
            Ok(rest.split_at(rest.len() - n))
        };
        let side = |s: &str| -> Result<Side> {
            Ok(match s {
                "left" => Side::Left,
                "right" => Side::Right,
                "above" | "up" | "top" => Side::Up,
                "below" | "down" | "bottom" => Side::Down,
                _ => return Err(usage(text, form)),
            })
        };
        Ok(Said::Do(match verb.as_str() {
            "release" => {
                if rest.is_empty() {
                    return Err(usage(text, form));
                }
                let mut out = Vec::new();
                for x in rest {
                    out.push(self.released(x)?);
                }
                return Ok(Said::Release(out));
            }
            "region" if rest.first().is_some_and(|w| w == "rename") => {
                let [_, region, title] = rest else {
                    return Err(usage(text, "region rename REGION TITLE"));
                };
                return Ok(Said::RenameRegion {
                    region: self.region(region)?,
                    title: title.clone(),
                });
            }
            "region" if rest.first().is_some_and(|w| w == "none") => Instruction::RegionNone {
                parts: many(&rest[1..])?,
            },
            "region" => {
                let Some((title, x)) = rest.split_first() else {
                    return Err(usage(text, form));
                };
                Instruction::Region {
                    title: title.clone(),
                    parts: many(x)?,
                }
            }
            "paper" => {
                let (sheet, size, orientation) = match rest {
                    [s, z] => (s, z, None),
                    [s, z, o] => (
                        s,
                        z,
                        Some(match o.as_str() {
                            "landscape" => ryusch::layout::Orientation::Landscape,
                            "portrait" => ryusch::layout::Orientation::Portrait,
                            _ => return Err(usage(text, form)),
                        }),
                    ),
                    _ => return Err(usage(text, form)),
                };
                // A sheet's name reads as its ID.
                let sheet = match &self.b.layout {
                    Some(Ok(l)) => l
                        .sheets
                        .iter()
                        .find(|(id, x)| *id == sheet || x.name == *sheet)
                        .map(|(id, _)| id.clone()),
                    _ => None,
                }
                .unwrap_or_else(|| sheet.clone());
                Instruction::Paper {
                    sheet,
                    size: size.clone(),
                    orientation,
                }
            }
            "sheet" if rest.first().is_some_and(|w| w == "rename") => {
                let [_, from, to] = rest else {
                    return Err(usage(text, "sheet rename SHEET NAME"));
                };
                return Ok(Said::RenameSheet {
                    from: from.clone(),
                    to: to.clone(),
                });
            }
            "sheet" => {
                let (x, s) = last(1)?;
                // A sheet's ID names that sheet; anything else is a name.
                let sheet = match &self.b.layout {
                    Some(Ok(l)) => l.sheets.get(&s[0]).map(|x| x.name.clone()),
                    _ => None,
                }
                .unwrap_or_else(|| s[0].clone());
                Instruction::Sheet {
                    parts: many(x)?,
                    sheet,
                }
            }
            "beside" => {
                let (x, t) = last(2)?;
                Instruction::Beside {
                    parts: many(x)?,
                    of: self.part(&t[0])?,
                    side: side(&t[1])?,
                }
            }
            "near" => {
                let (x, t) = last(1)?;
                Instruction::Near {
                    parts: many(x)?,
                    to: self.pin_or_part(&t[0])?,
                }
            }
            "align" => {
                let (x, t) = last(1)?;
                // A subject with a pin (`J1.3`, `U2.SCL`) lines up by that
                // pin: every subject then must be one.
                let pins = x
                    .iter()
                    .any(|w| self.pin(w).is_ok() && self.part(w).is_err());
                let subjects = if pins {
                    x.iter().map(|w| self.pin(w)).collect::<Result<Vec<_>>>()?
                } else {
                    many(x)?
                };
                if subjects.is_empty() {
                    return Err(usage(text, form));
                }
                Instruction::Align {
                    parts: subjects,
                    axis: match t[0].as_str() {
                        "row" => Axis::Row,
                        "column" => Axis::Column,
                        _ => return Err(usage(text, form)),
                    },
                }
            }
            "group" => Instruction::Group { parts: many(rest)? },
            "edge" => {
                let (x, t) = last(1)?;
                Instruction::Edge {
                    parts: many(x)?,
                    side: side(&t[0])?,
                }
            }
            "orient" => {
                let mirror = rest.last().is_some_and(|x| x == "mirror");
                let body = if mirror {
                    &rest[..rest.len() - 1]
                } else {
                    rest
                };
                let Some((r, x)) = body.split_last() else {
                    return Err(usage(text, form));
                };
                let rotation: u16 = r
                    .parse()
                    .ok()
                    .filter(|r| [0, 90, 180, 270].contains(r))
                    .ok_or_else(|| usage(text, form))?;
                Instruction::Orient {
                    parts: many(x)?,
                    rotation,
                    mirror,
                }
            }
            "net" => {
                if rest.len() != 2 {
                    return Err(usage(text, form));
                }
                Instruction::Net {
                    net: self.net(&rest[0])?,
                    style: match rest[1].as_str() {
                        "wire" => NetStyle::Wire,
                        "label" => NetStyle::Label,
                        "port" => NetStyle::Port,
                        _ => return Err(usage(text, form)),
                    },
                }
            }
            "idiom" => {
                let (x, t) = last(1)?;
                let kind = match t[0].as_str() {
                    "none" => IdiomChoice::None,
                    k => IdiomChoice::Kind(
                        IdiomKind::ALL
                            .into_iter()
                            .find(|i| i.as_str() == k)
                            .ok_or_else(|| {
                                usage(
                                    text,
                                    &format!(
                                        "{form}, KIND one of {}",
                                        IdiomKind::ALL.map(|i| i.as_str()).join(", ")
                                    ),
                                )
                            })?,
                    ),
                };
                Instruction::Idiom {
                    parts: many(x)?,
                    kind,
                }
            }
            "shift" => {
                let (x, t) = last(2)?;
                let steps: u16 = t[1]
                    .parse()
                    .ok()
                    .filter(|n| (1..=40).contains(n))
                    .ok_or_else(|| usage(text, &format!("{form}, STEPS 1 to 40")))?;
                Instruction::Shift {
                    parts: many(x)?,
                    side: side(&t[0])?,
                    steps,
                }
            }
            "flow" => {
                let (x, t) = last(1)?;
                if t[0] != "reversed" {
                    return Err(usage(text, form));
                }
                Instruction::Flow { parts: many(x)? }
            }
            _ => return Err(usage(text, form)),
        }))
    }
}

/// Whether an instruction names `key` (a part key or a component's).
pub fn names(i: &Instruction, key: &str) -> bool {
    i.subjects().iter().any(|s| {
        let s = s.split_once('/').map_or(*s, |(k, _)| k);
        s == key || s.starts_with(&format!("{key}:")) || key.starts_with(&format!("{s}:"))
    })
}

/// What `of` stands for, for messages.
pub fn of_text(of: &Of) -> String {
    match of {
        Of::Unit { component, unit } => format!("{component}:{unit}"),
        Of::Instance(i) => format!("instance:{i}"),
        Of::Port(p) => format!("port:{p}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_keep_quoted_spaces() {
        assert_eq!(
            words("sheet U1 \"Power in\"").unwrap(),
            ["sheet", "U1", "Power in"]
        );
        assert!(words("sheet \"x").is_err());
        assert_eq!(words("  a  b ").unwrap(), ["a", "b"]);
    }
}
