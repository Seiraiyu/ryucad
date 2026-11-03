//! Converts an SVG to a PNG with ryueda's own renderer (resvg, Inter as the
//! only font): `cargo run --release -p ryueda --example svg-png -- IN.svg
//! OUT.png [PX_PER_MM]`. Used by the schematic round's blind review to
//! draw the references KiCad exports alongside ryueda's own renders.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 || args.len() > 3 {
        eprintln!("usage: svg-png IN.svg OUT.png [PX_PER_MM]");
        std::process::exit(2);
    }
    let px: f32 = args
        .get(2)
        .map_or(Ok(8.0), |s| s.parse())
        .unwrap_or_else(|e| {
            eprintln!("PX_PER_MM: {e}");
            std::process::exit(2);
        });
    let svg = std::fs::read_to_string(&args[0]).unwrap_or_else(|e| {
        eprintln!("{}: {e}", args[0]);
        std::process::exit(1);
    });
    let bytes = ryueda::schemcmd::png(&svg, px).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    std::fs::write(&args[1], bytes).unwrap_or_else(|e| {
        eprintln!("{}: {e}", args[1]);
        std::process::exit(1);
    });
}
