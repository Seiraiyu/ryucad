//! A drill layer's `tools`: each tool's number, kind and sizes.

use ryu_board::model::{Tool, ToolKind};

use super::text::{Bad, structured};
use super::units::{Nums, Units, token};

/// Reads a tools file; sizes are mils or microns by its units.
pub fn tools(text: &str, units: Units) -> Result<(Vec<Tool>, Nums), Bad> {
    let s = structured(text)?;
    let units = match s.field("UNITS") {
        None => units,
        Some(f) => Units::parse(&f.value)
            .ok_or_else(|| Bad::new(f.line, 1, format!("UNITS={}: not MM or INCH", f.value)))?,
    };
    let mut nums = Nums::new(units);
    let mut out = Vec::new();
    for a in s.arrays("TOOLS") {
        let field = |k: &str| {
            a.field(k)
                .filter(|f| !f.value.is_empty())
                .ok_or_else(|| Bad::new(a.line, 1, format!("TOOLS without {k}")))
        };
        let num = field("NUM")?;
        let number = num
            .value
            .parse()
            .map_err(|_| Bad::new(num.line, 1, format!("NUM={}: not a number", num.value)))?;
        let t = field("TYPE")?;
        let kind = match t.value.to_ascii_uppercase().as_str() {
            "PLATED" => ToolKind::Plated,
            "NON_PLATED" => ToolKind::NonPlated,
            "VIA" => ToolKind::Via,
            other => {
                return Err(Bad::new(
                    t.line,
                    1,
                    format!("TYPE={other}: not a tool kind"),
                ));
            }
        };
        let mut size = |k: &str| -> Result<Option<i64>, Bad> {
            match a.field(k).filter(|f| !f.value.is_empty()) {
                None => Ok(None),
                Some(f) if f.value.trim() == "-1" => Ok(None),
                Some(f) => nums.size(&token(&f.value), f.line).map(Some),
            }
        };
        let finished = size("FINISH_SIZE")?;
        let drilled = size("DRILL_SIZE")?;
        out.push(Tool {
            number,
            kind,
            finished,
            drilled,
        });
    }
    Ok((out, nums))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_in_mils_and_microns() {
        let t = "UNITS=INCH\nTHICKNESS=62.5\nTOOLS {\n NUM=1\n TYPE=VIA\n TYPE2=STANDARD\n FINISH_SIZE=11.5\n DRILL_SIZE=13.5\n}\nTOOLS\n{\n NUM=2\n TYPE=PLATED\n FINISH_SIZE=-1\n}\nTOOLS {\nNUM=3\nTYPE=non_plated\nFINISH_SIZE=43.3071\nDRILL_SIZE=\n}\n";
        let (got, nums) = tools(t, Units::Mm).unwrap();
        assert_eq!(
            got,
            [
                Tool {
                    number: 1,
                    kind: ToolKind::Via,
                    finished: Some(292_100),
                    drilled: Some(342_900)
                },
                Tool {
                    number: 2,
                    kind: ToolKind::Plated,
                    finished: None,
                    drilled: None
                },
                Tool {
                    number: 3,
                    kind: ToolKind::NonPlated,
                    finished: Some(1_100_000),
                    drilled: None
                },
            ]
        );
        assert_eq!(nums.rounded(), 1);
        let (m, _) = tools(
            "TOOLS {\nNUM=1\nTYPE=VIA\nFINISH_SIZE=600.0\n}\n",
            Units::Mm,
        )
        .unwrap();
        assert_eq!(m[0].finished, Some(600_000));
    }

    #[test]
    fn faults() {
        let at = |t: &str| tools(t, Units::Mm).unwrap_err().message;
        assert_eq!(at("TOOLS {\nTYPE=VIA\n}\n"), "TOOLS without NUM");
        assert_eq!(at("TOOLS {\nNUM=x\nTYPE=VIA\n}\n"), "NUM=x: not a number");
        assert_eq!(
            at("TOOLS {\nNUM=1\nTYPE=LASER\n}\n"),
            "TYPE=LASER: not a tool kind"
        );
        assert_eq!(at("UNITS=FT\n"), "UNITS=FT: not MM or INCH");
    }
}
