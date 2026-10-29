//! Renders for people to look at: `cargo test -p ryusch --test look --
//! --ignored` writes seeded blocks' SVG into `target/ryusch-look`, each
//! whole and cropped to what it draws.

use ryusch::geom::{GRID, Place, Rect, mm};

/// The SVG cropped to the drawn parts of its first sheet.
fn cropped(block: &ryusch::input::Block, layout: &ryusch::layout::Layout, svg: &str) -> String {
    let r = block
        .parts
        .iter()
        .filter_map(|p| {
            let x = layout.placed.get(&p.key)?;
            Some(ryusch::shape::Shape::of(p, Place::new(x.at, x.rotation, x.mirror)).extent)
        })
        .reduce(Rect::union)
        .unwrap()
        .grow(8 * GRID);
    // The canvas pads by six steps; its y runs down from the top.
    let pad = 6 * GRID;
    let top = block.sheet[1] + 2 * pad;
    let vb = format!(
        "viewBox=\"{} {} {} {}\"",
        mm(r.min[0] + pad),
        mm(top - (r.max[1] + pad)),
        mm(r.width()),
        mm(r.height())
    );
    let start = svg.find("viewBox=\"").unwrap();
    let end = start + svg[start + 9..].find('"').unwrap() + 10;
    let s = format!("{}{}{}", &svg[..start], vb, &svg[end..]);
    s.replacen(
        &format!("width=\"{}mm\"", mm(block.sheet[0] + 2 * pad)),
        &format!("width=\"{}mm\"", mm(r.width())),
        1,
    )
    .replacen(
        &format!("height=\"{}mm\"", mm(block.sheet[1] + 2 * pad)),
        &format!("height=\"{}mm\"", mm(r.height())),
        1,
    )
}

#[test]
#[ignore]
fn seeded_blocks_render() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/ryusch-look");
    std::fs::create_dir_all(dir).unwrap();
    for (seed, ics) in [(1, 2), (2, 4), (3, 6)] {
        let block = ryusch::testing::seeded(seed, ics);
        let out = ryusch::engine::lay_out(&block);
        let svg = ryusch::svg::draw(&block, &out.layout, &[]);
        std::fs::write(format!("{dir}/seeded-{seed}.svg"), &svg).unwrap();
        std::fs::write(
            format!("{dir}/seeded-{seed}-crop.svg"),
            cropped(&block, &out.layout, &svg),
        )
        .unwrap();
        eprintln!(
            "seed {seed}: fallbacks {:?} notes {:?}",
            out.fallbacks, out.notes
        );
    }
}
