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

    /// The region a title instruction names: an ID (`r3`) as given, since
    /// the batch may make it, else a title one region has.
    pub fn region_id(&self, text: &str) -> Result<String> {
        let id = text
            .strip_prefix('r')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if id {
            return Ok(text.to_owned());
        }
        self.region(text)
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

    /// The bus `text` names (plan 2.C): `bus:<name or id>`, or a bus's
    /// name: its key `bus:<id>`.
    pub fn bus(&self, text: &str) -> Result<String> {
        let want = text.strip_prefix("bus:").unwrap_or(text);
        let buses = &self.built.input.buses;
        if let Some(b) = buses.iter().find(|b| b.key == format!("bus:{want}")) {
            return Ok(b.key.clone());
        }
        let named: Vec<&ryusch::input::Bus> = buses.iter().filter(|b| b.name == want).collect();
        if let [one] = named.as_slice() {
            return Ok(one.key.clone());
        }
        Err(not_found("bus", text, self.b))
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
        // A bus by its name, `bus:` before it where a net has that name or
        // the name doesn't read back as the bus (plan 2.C).
        if key.starts_with("bus:") {
            let input = &self.built.input;
            return match input.buses.iter().find(|b| b.key == key) {
                Some(b) => {
                    let clash = input.nets.iter().any(|n| n.name == b.name)
                        || self.bus(&b.name).ok().as_deref() != Some(key);
                    if clash {
                        format!("bus:{}", b.name)
                    } else {
                        b.name.clone()
                    }
                }
                None => key.to_owned(),
            };
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
                    NetStyle::Bus => "bus",
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
            Instruction::Title { region, title } => {
                format!("region {region} title {}", Self::quote(title))
            }
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
            Instruction::Text {
                parts: l,
                field,
                side: s,
            } => {
                format!("text {} {} {}", parts(l), field.as_str(), side(*s))
            }
            Instruction::Note { on, text } => {
                // A region by its title where that names it back, a sheet
                // by its ID.
                let on = match on.strip_prefix("region:") {
                    Some(id) => match &self.b.layout {
                        Some(Ok(l)) => l
                            .regions
                            .get(id)
                            .filter(|r| self.region(&r.title).ok().as_deref() == Some(id))
                            .map_or_else(|| on.clone(), |r| r.title.clone()),
                        _ => on.clone(),
                    },
                    None => on.clone(),
                };
                format!("note {} {}", Self::quote(&on), Self::quote(text))
            }
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
    "near X... P (a pin U1.VDD or a part; several parts and a pin: one network off the pin; port:NAME and a pin: the flag on its row)",
    "align X... row|column (X a part, or a pin U1.SCL to line pins up)",
    "group X...",
    "edge X... left|right|top|bottom",
    "orient X... 0|90|180|270 [mirror]",
    "text X... refdes|value|both left|right|above|below",
    "net N wire|label|port|bus (a bus by its name or bus:NAME)",
    "note REGION|SHEET TEXT (one line, up to 120 characters; {property} filled)",
    "idiom X... KIND|none",
    "flow X... reversed",
    "shift X... up|down|left|right STEPS (a move only)",
    "release X...|ID|net:N",
    "region TITLE X... | region none X... | region REGION title TITLE (region rename REGION TITLE reads so) (X may be a region)",
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
            // A region's title by its ID (plan 3.A): lasting, applied after
            // the batch's regions are made.
            "region" if rest.get(1).is_some_and(|w| w == "title") => {
                let [region, _, title] = rest else {
                    return Err(usage(text, "region REGION title TITLE"));
                };
                Instruction::Title {
                    region: self.region_id(region)?,
                    title: title.clone(),
                }
            }
            "region" if rest.first().is_some_and(|w| w == "rename") => {
                let [_, region, title] = rest else {
                    return Err(usage(text, "region rename REGION TITLE"));
                };
                Instruction::Title {
                    region: self.region_id(region)?,
                    title: title.clone(),
                }
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
            "note" => {
                let Some((on, words)) = rest.split_first() else {
                    return Err(usage(text, "note REGION|SHEET TEXT"));
                };
                let note = words.join(" ");
                if note.trim().is_empty()
                    || note.chars().count() > ryusch::layout::NOTE_MAX
                    || note.contains(['\n', '\r'])
                {
                    return Err(usage(
                        text,
                        "note REGION|SHEET TEXT (one line of 1 to 120 characters)",
                    ));
                }
                let on = if let Some(id) = on.strip_prefix("region:") {
                    format!("region:{}", self.region(id)?)
                } else if let Ok(id) = self.region(on) {
                    format!("region:{id}")
                } else {
                    // A sheet by its ID or its name; any other name is
                    // kept as said, unresolved until a sheet takes it.
                    match &self.b.layout {
                        Some(Ok(l)) => l
                            .sheets
                            .iter()
                            .find(|(id, x)| *id == on || x.name == *on)
                            .map(|(id, _)| id.clone()),
                        _ => None,
                    }
                    .unwrap_or_else(|| on.clone())
                };
                Instruction::Note { on, text: note }
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
            "text" => {
                let (x, t) = last(2)?;
                let field = match t[0].as_str() {
                    "refdes" => ryusch::layout::TextField::Refdes,
                    "value" => ryusch::layout::TextField::Value,
                    "both" => ryusch::layout::TextField::Both,
                    _ => return Err(usage(text, form)),
                };
                Instruction::Text {
                    parts: many(x)?,
                    field,
                    side: side(&t[1])?,
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
                let style = match rest[1].as_str() {
                    "wire" => NetStyle::Wire,
                    "label" => NetStyle::Label,
                    "port" => NetStyle::Port,
                    "bus" => NetStyle::Bus,
                    _ => return Err(usage(text, form)),
                };
                // A net first; a bus by `bus:NAME`, or by its name where no
                // net has it (plan 2.C).
                let net = if rest[0].starts_with("bus:") {
                    self.bus(&rest[0])?
                } else {
                    match self.net(&rest[0]) {
                        Ok(n) => n,
                        Err(e) => self.bus(&rest[0]).map_err(|_| e)?,
                    }
                };
                if style == NetStyle::Bus && !net.starts_with("bus:") {
                    return Err(usage(
                        text,
                        "net BUS bus: only a bus is drawn as one (bus:NAME)",
                    ));
                }
                Instruction::Net { net, style }
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

    #[test]
    fn notes_read_on_regions_and_sheets_and_write_back() {
        let (dir, _) = crate::netlist::tests::project(&[]);
        let mut session = crate::session::Session::open(&dir.path().join("board")).unwrap();
        crate::schemcmd::layout(&mut session, None, None, None, false, None, None).unwrap();
        session.reload().unwrap();
        let p = session.project();
        let b = p
            .conn
            .block(&crate::id::Id::parse("top", "b").unwrap())
            .unwrap();
        let built = crate::schem::build(p, b).unwrap();
        let names = Names {
            p,
            b,
            built: &built,
        };
        let l = match &b.layout {
            Some(Ok(l)) => l.clone(),
            _ => panic!("laid out"),
        };
        let (rid, region) = l.regions.iter().next().expect("a region");
        let sid = l.sheets.keys().next().unwrap().clone();
        let quoted = format!("note {} \"Fused at 2 A\"", Names::quote(&region.title));
        let Said::Do(i) = names.read(&quoted).unwrap() else {
            panic!("an instruction")
        };
        assert_eq!(
            i,
            Instruction::Note {
                on: format!("region:{rid}"),
                text: "Fused at 2 A".into()
            }
        );
        assert_eq!(names.text(&i), quoted);
        // On a sheet, its words unquoted.
        let Said::Do(i) = names.read(&format!("note {sid} Rev {{rev}}")).unwrap() else {
            panic!("an instruction")
        };
        assert_eq!(
            i,
            Instruction::Note {
                on: sid.clone(),
                text: "Rev {rev}".into()
            }
        );
        assert_eq!(names.text(&i), format!("note {sid} \"Rev {{rev}}\""));
        // Too long, or empty: refused.
        let long = "x".repeat(121);
        assert_eq!(
            names.read(&format!("note {sid} {long}")).unwrap_err().code,
            Code::InvalidParam
        );
        assert_eq!(
            names.read(&format!("note {sid}")).unwrap_err().code,
            Code::InvalidParam
        );
    }

    #[test]
    fn a_bus_is_named_by_its_name_or_bus_name() {
        use serde_json::json;
        let (_dir, p) = crate::netlist::tests::project(&[
            (
                "blocks/top/block.json",
                json!({"format": 1, "kind": crate::conn::BLOCK_KIND, "name": "Top",
                    "buses": {"b1": {"name": "D", "nets": ["sda", "scl"]},
                              "b2": {"name": "SDA", "nets": ["sda"]}}}),
            ),
            (
                "blocks/top/nets/scl.json",
                json!({"format": 1, "kind": crate::conn::NET_KIND, "name": "SCL"}),
            ),
        ]);
        let b = p
            .conn
            .block(&crate::id::Id::parse("top", "b").unwrap())
            .unwrap();
        let built = crate::schem::build(&p, b).unwrap();
        let names = Names {
            p: &p,
            b,
            built: &built,
        };
        let read = |t: &str| match names.read(t) {
            Ok(Said::Do(i)) => i,
            other => panic!("{t}: {other:?}"),
        };
        let bus = |key: &str, style| Instruction::Net {
            net: key.into(),
            style,
        };
        assert_eq!(read("net D bus"), bus("bus:b1", NetStyle::Bus));
        assert_eq!(read("net bus:D label"), bus("bus:b1", NetStyle::Label));
        // A net and a bus of one name: the net, and the bus as bus:NAME.
        assert_eq!(read("net SDA label"), bus("sda", NetStyle::Label));
        assert_eq!(read("net bus:SDA bus"), bus("bus:b2", NetStyle::Bus));
        let e = names.read("net SDA bus").unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert!(
            e.message.contains("only a bus is drawn as one"),
            "{}",
            e.message
        );
        // Written back as read.
        assert_eq!(names.text(&bus("bus:b1", NetStyle::Bus)), "net D bus");
        assert_eq!(names.text(&bus("bus:b2", NetStyle::Bus)), "net bus:SDA bus");
    }
}
