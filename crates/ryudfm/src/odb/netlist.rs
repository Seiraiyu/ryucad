//! A step's CAD netlist (`netlists/cadnet/netlist`): its nets by serial
//! number and their points, kept apart from `eda/data`'s so stage 1 can
//! compare them.
//!
//! The header `H optimize n [staggered n]`, then `$n name` per net, then
//! a point per line: `net radius x y side [w h] epoint exp` and keyword
//! fields. A net of `-1` is a tooling hole, which no net holds.

use ryu_board::model::{CadPoint, Side};

use super::text::{Bad, lines, tokens};
use super::units::{Nums, Units};

/// The netlist's nets by serial number, each with its points.
pub type Netlist = Vec<(String, Vec<CadPoint>)>;

/// Reads a CAD netlist.
pub fn netlist(text: &str, units: Units) -> Result<(Netlist, Nums), Bad> {
    let mut nums = Nums::new(units);
    let mut nets: Netlist = Vec::new();
    for (n, line) in lines(text) {
        let bad = |c: u32, m: String| Bad::new(n, c, m);
        if let Some(u) = line.strip_prefix("UNITS=") {
            nums.units = Units::parse(u.trim())
                .ok_or_else(|| bad(7, format!("UNITS={u}: not MM or INCH")))?;
            continue;
        }
        if line.starts_with("H ") || line == "H" {
            continue;
        }
        let t = tokens(line).map_err(|b| bad(b.column, b.message))?;
        if let Some(i) = t[0].text.strip_prefix('$') {
            let i: usize = i
                .parse()
                .ok()
                .filter(|&i| i <= 10_000_000)
                .ok_or_else(|| bad(1, format!("{}: not a net number", t[0].text)))?;
            let name = t.get(1).map_or("", |t| t.text).to_string();
            if nets.len() <= i {
                nets.resize_with(i + 1, Default::default);
            }
            nets[i].0 = name;
            continue;
        }
        let net: i64 = t[0]
            .text
            .parse()
            .map_err(|_| bad(1, format!("{}: not a net number", t[0].text)))?;
        if t.len() < 7 {
            return Err(bad(1, "a point takes at least seven fields".into()));
        }
        let at = nums.point(&t[2], &t[3], n)?;
        let side = match t[4].text.chars().next() {
            Some('T') => Some(Side::Top),
            Some('D') => Some(Side::Bottom),
            Some('I') => Some(Side::Inner),
            // Both sides.
            Some('B') => None,
            _ => {
                return Err(bad(
                    t[4].column,
                    format!("side {}: not T, D, B or I", t[4].text),
                ));
            }
        };
        let via = t[5..].iter().any(|k| k.text == "v");
        if net < 0 {
            continue;
        }
        let Some(entry) = nets.get_mut(net as usize) else {
            return Err(bad(1, format!("net {net} isn't listed")));
        };
        entry.1.push(CadPoint { at, side, via });
    }
    Ok((nets, nums))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_board::geom::Point;

    #[test]
    fn nets_and_points() {
        let t = "UNITS=INCH\nH optimize n staggered n\n$0 $NONE$\n$1 4CC1\n#\n1 0.002 -0.7997255 8.2666645 D e e staggered 0 0 0 v\n\
1 0.012 -0.9619881 8.300222 B e c staggered 0 0 0\n1 0 2.5 3.2 T 0.04 0.04 e s staggered 0 0.01 0 e by=c ld=111\n\
1 0.0254 16.1339975 -0.2659975 I e e staggered 0 0 0 ld=61\n-1 0.05 0 0 B e e\n0 0.002 0 0 T e e\n";
        let (nets, nums) = netlist(t, Units::Mm).unwrap();
        assert_eq!(nets.len(), 2);
        assert_eq!(nets[0].0, "$NONE$");
        let (name, pts) = &nets[1];
        assert_eq!(name, "4CC1");
        assert_eq!(pts.len(), 4);
        assert_eq!(
            pts[0],
            CadPoint {
                at: Point::new(-20_313_028, 209_973_278),
                side: Some(Side::Bottom),
                via: true
            }
        );
        assert_eq!((pts[1].side, pts[1].via), (None, false));
        assert_eq!(pts[2].side, Some(Side::Top));
        assert_eq!(pts[3].side, Some(Side::Inner));
        assert_eq!(nets[0].1.len(), 1);
        assert!(nums.rounded() > 0);
    }

    #[test]
    fn faults() {
        let m = |t: &str| netlist(t, Units::Mm).unwrap_err().message;
        assert_eq!(m("$x a\n"), "$x: not a net number");
        assert_eq!(m("$0 a\n3 0.1 0 0 T e e\n"), "net 3 isn't listed");
        assert_eq!(m("$0 a\n0 0.1 0 0 Q e e\n"), "side Q: not T, D, B or I");
        assert_eq!(
            m("$0 a\n0 0.1 0 0\n"),
            "a point takes at least seven fields"
        );
    }
}
