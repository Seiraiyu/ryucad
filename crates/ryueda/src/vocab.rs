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
        if let Some(p) = text.strip_prefix("port:") {
            let block = self.b.block.item.as_ref().map_err(|e| e.clone())?;
            return block
                .port(p)
                .map(|id| format!("port:{id}"))
                .ok_or_else(|| not_found("port", p, self.b));
        }
        let iid = text.strip_prefix("instance:").unwrap_or(text);
        if let Some(id) = self.b.instances.keys().find(|k| k.as_str() == iid) {
            return Ok(format!("instance:{id}"));
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
        self.component(text).ok_or_else(|| {
            not_found(
                "part (by designator, unit, instance or port:)",
                text,
                self.b,
            )
        })
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
                if p.name == pin {
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

    fn net_text(&self, key: &str) -> String {
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

    /// An instruction as text an agent sends back.
    pub fn text(&self, i: &Instruction) -> String {
        let parts = |l: &[String]| {
            l.iter()
                .map(|k| Self::quote(&self.part_text(k)))
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
        }
    }
}

/// What a batch's text says: an instruction, or `release` of subjects.
#[derive(Debug, PartialEq)]
pub enum Said {
    Do(Instruction),
    Release(Vec<String>),
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
    "beside X... Y left|right|above|below",
    "near X... P (a pin U1.VDD or a part)",
    "align X... row|column",
    "group X...",
    "edge X... left|right|top|bottom",
    "orient X... 0|90|180|270 [mirror]",
    "net N wire|label|port",
    "idiom X... KIND|none",
    "flow X... reversed",
    "release X...",
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
            "release" => return Ok(Said::Release(many(rest)?)),
            "sheet" => {
                let (x, s) = last(1)?;
                Instruction::Sheet {
                    parts: many(x)?,
                    sheet: s[0].clone(),
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
                Instruction::Align {
                    parts: many(x)?,
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
