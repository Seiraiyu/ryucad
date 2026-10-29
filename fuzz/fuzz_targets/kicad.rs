//! KiCad files from bytes: raw text, or a template of our own (not
//! KiCad's files) with its values replaced by tokens the bytes choose,
//! read by the s-expression reader and the symbol and footprint readers.
//! Each is a typed refusal or objects that write as canonical JSON and
//! read back the same, and whose checks run. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryueda::kicad::{Notes, footprint, sexpr, symbol};

/// A symbol library: each `@` a value the bytes choose.
const SYMBOL: &str = r#"(kicad_symbol_lib (version @) (generator kicad_symbol_editor)
  (symbol "A" (pin_names (offset @)) (in_bom @) (on_board yes) (power)
    (property "Reference" "@" (at 0 0 0))
    (property "Footprint" "@" (at 0 0 0))
    (symbol "A_@_@"
      (rectangle (start @ @) (end @ @) (fill (type @)))
      (arc (start @ @) (mid @ @) (end @ @))
      (circle (center @ @) (radius @))
      (text "@" (at @ @ @) (effects (font (size @ @))))
      (pin @ @ (at @ @ @) (length @) (name "@") (number "@") (alternate "@" @ @))
      (pin input line (at 0 0 @) (length 1) (name "B") (number "@")))
    (symbol "A_@_1" (polyline (pts (xy @ @) (xy @ @)) (fill (type none)))))
  (symbol "B" (extends "@") (property "Value" "@" (at 0 0 0)))
)"#;

/// A footprint: each `@` a value the bytes choose.
const FOOTPRINT: &str = r#"(footprint "F" (version @) (layer "F.Cu") (attr @ @)
  (property "Reference" "R" (at @ @ @) (layer "@") (effects (font (size @ @) (thickness @))))
  (fp_line (start @ @) (end @ @) (stroke (width @)) (layer "@"))
  (fp_circle (center @ @) (end @ @) (fill @) (layer "F.Fab"))
  (fp_arc (start @ @) (mid @ @) (end @ @) (layer "F.SilkS"))
  (fp_poly (pts (xy @ @) (xy @ @) (xy @ @)) (layer "@"))
  (pad "@" @ @ (at @ @ @) (size @ @) (drill @ @ (offset @ @)) (layers "@" "@")
    (roundrect_rratio @) (chamfer_ratio @) (chamfer @) (rect_delta @ @) (zone_connect @))
  (pad "2" smd custom (at 0 0) (size 1 1) (layers "F.Cu") (options (anchor @))
    (primitives (gr_poly (pts (xy @ @) (xy @ @) (xy @ @)) (width @) (fill yes))))
  (zone (layers "@") (keepout (tracks @) (pads @)) (polygon (pts (xy @ @) (xy @ @) (xy @ @))))
  (model "@" (offset (xyz @ @ @)) (scale (xyz @ @ @)) (rotate (xyz @ @ @)))
)"#;

const TOKENS: [&str; 32] = [
    "0", "1", "-1", "0.5", "1.27", "-2.54", "1e3", "0.0000005", "900", "90", "45", "270",
    "1000000000", "20211014", "20241209", "29999999", "yes", "no", "input", "power_in",
    "inverted", "line", "smd", "thru_hole", "roundrect", "custom", "oval", "trapezoid",
    "F.Cu", "*.Cu", "In3.Cu", "~",
];

/// The template with each `@` replaced by a token the next byte picks,
/// or by a byte as text when the high bit is set.
fn fill(template: &str, bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    for c in template.chars() {
        if c != '@' {
            out.push(c);
            continue;
        }
        let b = bytes.get(i).copied().unwrap_or(0);
        i += 1;
        if b & 0x80 == 0 {
            out.push_str(TOKENS[usize::from(b) % TOKENS.len()]);
        } else {
            out.push(char::from(b & 0x7f).max(' '));
        }
    }
    out
}

/// What a library object must do once read: write canonically and read
/// back the same, and have its checks run.
fn round_trips<T>(t: &T, kind: &str, format: u64)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let text = ryueda::doc::to_string(t, kind, format);
    let back: T = ryueda::doc::read(text.as_bytes(), "x.json", kind, format).unwrap();
    assert_eq!(&back, t);
}

fn check(text: &str) {
    let _ = sexpr::parse(text);
    if let Ok(lib) = symbol::read(text) {
        for (_, item) in &lib.items {
            if let Ok(item) = item
                && let Some(s) = &item.symbol
            {
                let _ = s.check();
                round_trips(s, ryueda::symbol::KIND, ryueda::symbol::FORMAT);
            }
        }
    }
    let mut notes = Notes::default();
    if let Ok(r) = footprint::read(text, &mut notes) {
        let _ = r.footprint.check();
        round_trips(&r.footprint, ryueda::footprint::KIND, ryueda::footprint::FORMAT);
    }
    let _ = notes.faults("x");
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    match mode % 3 {
        0 => check(&String::from_utf8_lossy(rest)),
        1 => check(&fill(SYMBOL, rest)),
        _ => check(&fill(FOOTPRINT, rest)),
    }
});
