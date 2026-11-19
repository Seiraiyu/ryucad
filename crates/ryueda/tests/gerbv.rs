//! A third-party reader for the fabrication package (design 732): gerbv
//! reads every exported Gerber and drill file without a warning and
//! rasterizes each, and each raster matches `ryu-board`'s raster of the
//! shapes that went into the file: at most `MAX_SHARE` of the ink pixels
//! differ, every one within `EDGE_REACH` pixels of an edge in either
//! picture, under the closer of two registrations (both cut to their
//! ink, or both over one window gerbv is given too; see `check`). Both slot forms are read too, and the one `export::SLOTS`
//! names must pass. gerbv is GPL C, installed in the devcontainer only:
//! verification, never a dependency.
//!
//! ```sh
//! RYUEDA_REQUIRE_GERBV=1 cargo test -p ryueda --release --test gerbv -- --ignored --nocapture
//! ```

#[path = "support/board_gen.rs"]
mod board_gen;
#[path = "support/breakout.rs"]
mod breakout;
#[path = "support/breakout_board.rs"]
mod breakout_board;
#[path = "support/draw.rs"]
mod draw;

use assert_cmd::Command as Cli;
use ryu_board::geom::{Pen, Rect, Region};
use ryu_board::model::Polarity;
use ryu_board::render::{Ink, Layer, Scene, View, raster};
use ryu_board::shape::{Shape, circle, line_outline};
use ryu_gerber::excellon::{self, Drill, Drills, Hole, SlotForm};
use std::path::Path;
use std::process::Command;

/// The share of ink pixels that may differ.
const MAX_SHARE: f64 = 0.03;
/// How far from an edge a differing pixel may be.
const EDGE_REACH: i64 = 2;
/// The rasters' longer side, about, in pixels: fine enough that a
/// 0.1 mm track is about 16 pixels wide on a 25 mm board, so the edge
/// rule's 2 pixels are 13 µm.
const LONG_SIDE: f64 = 4000.0;

fn gerbv() -> Command {
    Command::new("gerbv")
}

/// Whether gerbv is here; without it the test is skipped, unless
/// `RYUEDA_REQUIRE_GERBV=1`, when it fails.
fn have_gerbv() -> bool {
    let ok = gerbv()
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        assert!(
            std::env::var("RYUEDA_REQUIRE_GERBV").as_deref() != Ok("1"),
            "RYUEDA_REQUIRE_GERBV=1 but gerbv isn't installed (the devcontainer has it)"
        );
        eprintln!("gerbv isn't installed: skipped (the devcontainer has it)");
    }
    ok
}

/// A black and white picture: ink or not.
struct Raster {
    w: usize,
    h: usize,
    ink: Vec<bool>,
}

impl Raster {
    fn at(&self, x: i64, y: i64) -> Option<bool> {
        (x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h)
            .then(|| self.ink[y as usize * self.w + x as usize])
    }

    /// Whether some pixel within `EDGE_REACH` differs from (x, y).
    fn near_edge(&self, x: i64, y: i64) -> bool {
        let me = self.at(x, y);
        (-EDGE_REACH..=EDGE_REACH).any(|dy| {
            (-EDGE_REACH..=EDGE_REACH)
                .any(|dx| self.at(x + dx, y + dy).is_some_and(|v| Some(v) != me))
        })
    }

    /// The picture cut to its ink's box; `None` with no ink.
    fn cropped(&self) -> Option<Raster> {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.ink[y * self.w + x] {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                }
            }
        }
        (x0 <= x1).then(|| {
            let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
            let mut ink = Vec::with_capacity(w * h);
            for y in y0..=y1 {
                ink.extend_from_slice(&self.ink[y * self.w + x0..=y * self.w + x1]);
            }
            Raster { w, h, ink }
        })
    }
}

/// A PNG as ink: light pixels are ink.
fn decode(bytes: &[u8]) -> Raster {
    let mut d = png::Decoder::new(std::io::Cursor::new(bytes));
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = d.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    let ch = info.color_type.samples();
    let (w, h) = (info.width as usize, info.height as usize);
    let ink = (0..w * h)
        .map(|i| {
            let p = &buf[i * ch..i * ch + ch.min(3)];
            let lum: u32 = p.iter().map(|&v| u32::from(v)).sum::<u32>() / p.len() as u32;
            lum > 127
        })
        .collect();
    Raster { w, h, ink }
}

/// gerbv's raster of `file` at `dpi`, and whatever it printed: over the
/// box gerbv chooses, or over `window` (its lower left corner and its
/// size in pixels), so gerbv's pixels are ours.
fn gerbv_raster(
    file: &Path,
    dpi: u32,
    window: Option<(Rect, usize, usize)>,
    dir: &Path,
) -> (Raster, String) {
    let out = dir.join(format!(
        "{}{}.png",
        file.file_name().unwrap().to_string_lossy(),
        if window.is_some() { ".window" } else { "" }
    ));
    let at = window.map(|(r, w, h)| {
        // The origin is a whole number of 254 nm (1e-5 inch), so five
        // decimals write it exactly: gerbv refuses an --origin longer than
        // 20 characters. The size goes in inches, since gerbv caps
        // --window at 2000 pixels a side; half a pixel more, as gerbv
        // truncates it. The size only bounds the picture: the grid is the
        // origin's and the dpi's.
        let inch = |nm: i64| nm as f64 / 25_400_000.0;
        let px = |n: usize| (n as f64 + 0.5) / f64::from(dpi);
        [
            format!("--origin={:.5}x{:.5}", inch(r.min.x), inch(r.min.y)),
            format!("--window_inch={:.6}x{:.6}", px(w), px(h)),
        ]
    });
    let o = gerbv()
        .args([
            "-x",
            "png",
            "--border=0",
            "--background=#000000",
            "--foreground=#FFFFFFFF",
        ])
        .arg(format!("--dpi={dpi}"))
        .args(at.into_iter().flatten())
        .arg("-o")
        .arg(&out)
        .arg(file)
        .output()
        .unwrap();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        o.status.success(),
        "gerbv failed on {}: {said}",
        file.display()
    );
    (decode(&std::fs::read(&out).unwrap()), said)
}

/// Whether a line of gerbv's is its note that it doesn't know an X2
/// attribute command (`%TF`, `%TA`, `%TO`, `%TD`): gerbv 2.10 predates
/// X2 and skips them, which changes nothing it draws. Those are the only
/// lines let through; X2 is what fabricators read (design 732).
fn x2_attribute_note(line: &str) -> bool {
    ["%TF", "%TA", "%TO", "%TD"]
        .iter()
        .any(|c| line.contains(&format!("Unknown RS-274X extension found {c}")))
}

/// What gerbv may not say about a file we wrote.
fn warned(said: &str) -> bool {
    said.lines().filter(|l| !x2_attribute_note(l)).any(|l| {
        let s = l.to_ascii_lowercase();
        [
            "warning",
            "error",
            "critical",
            "unknown",
            "unsupported",
            "ignor",
        ]
        .iter()
        .any(|w| s.contains(w))
    })
}

/// `ryu-board`'s raster of `shapes` over their own box, `w` by `h`.
fn ours(shapes: &[&Shape], window: Rect, w: usize, h: usize) -> Raster {
    let scene = Scene {
        view: View {
            window,
            width: w as u32,
            height: h as u32,
            mirror: false,
        },
        background: [0, 0, 0],
        layers: vec![Layer {
            name: "x".into(),
            ink: Ink::Paint {
                colour: [255, 255, 255],
                alpha: 255,
            },
            shapes: shapes.to_vec(),
        }],
    };
    let img = raster(&scene);
    let ink = img
        .rgb
        .chunks(3)
        .map(|p| p.iter().map(|&v| u32::from(v)).sum::<u32>() / 3 > 127)
        .collect();
    Raster {
        w: img.width as usize,
        h: img.height as usize,
        ink,
    }
}

/// gerbv on `file`: no warning, and its raster against ours of `shapes`.
fn check(file: &Path, shapes: &[&Shape], dir: &Path) {
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    let bounds = ryu_board::render::bounds(shapes.iter().copied());
    let dpi = bounds.map_or(100, |b| {
        let long = (b.max.x - b.min.x).max(b.max.y - b.min.y) as f64 / 25_400_000.0;
        (LONG_SIDE / long).clamp(50.0, 4000.0) as u32
    });
    let (theirs, said) = gerbv_raster(file, dpi, None, dir);
    let notes = said.lines().filter(|l| x2_attribute_note(l)).count();
    let rest: Vec<&str> = said
        .lines()
        .filter(|l| !x2_attribute_note(l) && !l.trim().is_empty())
        .collect();
    println!(
        "{name}: gerbv at {dpi} dpi, {}x{}, {notes} X2 attribute notes{}",
        theirs.w,
        theirs.h,
        if rest.is_empty() {
            String::new()
        } else {
            format!(": {}", rest.join(" | "))
        }
    );
    assert!(!warned(&said), "{name}: gerbv said: {said}");
    if theirs.cropped().is_none() {
        assert!(
            shapes.iter().all(|s| s.parts.is_empty()),
            "{name}: gerbv drew nothing"
        );
        return;
    }
    // Ours at exactly gerbv's scale: the shapes' box grown to a whole
    // number of pixels at `dpi`, so the view's scale is `dpi` on both
    // axes. Two registrations of the pictures are tried, and the closer
    // judged: both cut to their ink, and both over one window that gerbv
    // is given too. Cutting to ink registers them only to a whole pixel
    // (either may ink a fraction of a pixel at an outermost edge that the
    // other doesn't), so stroke text, nearly all edge, differed by up to
    // half a pixel along every stroke (5.D: the breakout's designators,
    // 3.4 %); the shared window doesn't register gerbv's outlines at every
    // dpi (the profile at 3,892 dpi, 3.7 %). Within each, the closest of
    // the nine alignments within a pixel is the one judged.
    let b = bounds.expect("gerbv drew something");
    let per_px = 25_400_000.0 / f64::from(dpi);
    let window = |origin: ryu_board::geom::Point| {
        let whole = |span: i64| (span as f64 / per_px).ceil().max(1.0);
        let (pw, ph) = (whole(b.max.x - origin.x), whole(b.max.y - origin.y));
        let r = Rect::new(
            origin,
            ryu_board::geom::Point::new(
                origin.x + (pw * per_px).round() as i64,
                origin.y + (ph * per_px).round() as i64,
            ),
        );
        (r, pw as usize, ph as usize)
    };
    let (r, pw, ph) = window(b.min);
    let cut = (
        ours(shapes, r, pw, ph)
            .cropped()
            .expect("ours has ink where gerbv's has"),
        theirs.cropped().expect("checked above"),
    );
    let shared = window(ryu_board::geom::Point::new(
        b.min.x.div_euclid(254) * 254,
        b.min.y.div_euclid(254) * 254,
    ));
    let (theirs_there, said_there) = gerbv_raster(file, dpi, Some(shared), dir);
    assert!(!warned(&said_there), "{name}: gerbv said: {said_there}");
    let there = (ours(shapes, shared.0, shared.1, shared.2), theirs_there);
    let judged = |(mine, theirs): &(Raster, Raster)| {
        let (w, h) = (
            mine.w.max(theirs.w) as i64 + 1,
            mine.h.max(theirs.h) as i64 + 1,
        );
        let compare = |dx: i64, dy: i64| {
            let (mut diff, mut ink, mut far) = (0usize, 0usize, None);
            for y in 0..h {
                for x in 0..w {
                    let a = mine.at(x + dx, y + dy).unwrap_or(false);
                    let b = theirs.at(x, y).unwrap_or(false);
                    if a || b {
                        ink += 1;
                    }
                    if a != b {
                        diff += 1;
                        if far.is_none()
                            && !(mine.near_edge(x + dx, y + dy) || theirs.near_edge(x, y))
                        {
                            far = Some((x, y));
                        }
                    }
                }
            }
            (diff, ink, far, (dx, dy))
        };
        let best = (-1..=1)
            .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
            .map(|(dx, dy)| compare(dx, dy))
            .min_by_key(|c| c.0)
            .unwrap();
        (best, w, h)
    };
    let (one, two) = (judged(&cut), judged(&there));
    type Judged = ((usize, usize, Option<(i64, i64)>, (i64, i64)), i64, i64);
    let share = |j: &Judged| j.0.0 as f64 / j.0.1.max(1) as f64;
    let ((diff, ink, far, shift), w, h, (mine, theirs), how) = if share(&two) < share(&one) {
        (two.0, two.1, two.2, &there, "the shared window")
    } else {
        (one.0, one.1, one.2, &cut, "cut to ink")
    };
    assert!(
        far.is_none(),
        "{name}: pixel {far:?} differs away from any edge (ours shifted {shift:?})"
    );
    let share = diff as f64 / ink.max(1) as f64;
    println!(
        "{name}: {diff} of {ink} ink pixels differ ({:.3} %, {how})",
        share * 100.0
    );
    // Both pictures as PBM, to look at a layer that differs.
    if let Ok(to) = std::env::var("RYUEDA_GERBV_DUMP") {
        for (who, r) in [("ours", &mine), ("gerbv", &theirs)] {
            let mut pbm = format!("P1\n{w} {h}\n");
            for y in 0..h {
                for x in 0..w {
                    pbm.push(if r.at(x, y).unwrap_or(false) {
                        '1'
                    } else {
                        '0'
                    });
                }
                pbm.push('\n');
            }
            std::fs::write(Path::new(&to).join(format!("{name}.{who}.pbm")), pbm).unwrap();
        }
    }
    assert!(share <= MAX_SHARE, "{name}: {share}");
}

fn hole_shape(d: &Drill) -> Shape {
    let region = match *d {
        Drill::Round { at, d } => circle(at, ryu_board::geom::half(d)).ok().map(Region::solid),
        Drill::Slot { a, b, d } => line_outline(a, b, Pen::Round(d)),
    };
    Shape::regions(Polarity::Positive, region)
}

/// Every file of a board's package checked, the board in `root`.
fn check_package(root: &Path, board: &str, out: &Path) {
    let p = ryueda::project::Project::load(root).unwrap();
    let id = ryueda::id::Id::parse(board, "board").unwrap();
    let s = ryueda::board::shapes::build(&p, &p.boards[&id]).unwrap();
    for lf in ryueda::board::fab::layers(board, &s).unwrap() {
        let file = out.join(format!("{board}-{}.{}", lf.function, lf.ext));
        let shapes: Vec<&Shape> = lf.layer.items.iter().map(|i| &i.shape).collect();
        check(&file, &shapes, out);
    }
    for (function, d) in ryueda::board::fab::drills(&s) {
        if d.holes.is_empty() {
            continue;
        }
        let owned: Vec<Shape> = d.holes.iter().map(|h| hole_shape(&h.drill)).collect();
        let shapes: Vec<&Shape> = owned.iter().collect();
        check(&out.join(format!("{board}-{function}.drl")), &shapes, out);
    }
}

fn ryueda(dir: &Path, args: &[&str]) -> serde_json::Value {
    let o = Cli::cargo_bin("ryueda")
        .unwrap()
        .current_dir(dir)
        .env("RYUEDA_NO_DAEMON", "1")
        .env("RYUEDA_ID_SEED", "1")
        .args(args)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], true, "{args:?}: {v}");
    v["result"].clone()
}

/// The breakout placed by hand, one net routed, a via and an unplated
/// hole, exported with `--allow-errors`: `npth.drl` and the via's paths,
/// whatever the round drew.
#[test]
#[ignore = "needs gerbv: the devcontainer, full.yml"]
fn gerbv_reads_a_partly_routed_breakouts_package() {
    if !have_gerbv() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("board");
    std::fs::create_dir_all(&root).unwrap();
    ryueda(&root, &["init"]);
    for (path, text) in breakout::footprints() {
        let at = root.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    for args in breakout::script() {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        ryueda(&root, &args);
    }
    ryueda(
        &root,
        &[
            "board", "create", "Main", "--design", "board", "--rect", "26,20", "--id", "main",
        ],
    );
    for (r, xy, rot) in [
        ("U1", "8,10", "0"),
        ("C1", "8,14", "90"),
        ("U2", "18,10", "0"),
        ("C2", "15,6", "0"),
        ("C3", "21,6", "0"),
        ("R1", "12,15", "90"),
        ("R2", "14,15", "90"),
        ("J1", "3,10", "0"),
    ] {
        ryueda(
            &root,
            &["board", "place", "main", r, "--at", xy, "--rotation", rot],
        );
    }
    let pads = ryueda(&root, &["board", "show", "main", "--pads"])["pads"].clone();
    let net = pads[0]["net"].as_str().unwrap().to_owned();
    ryueda(
        &root,
        &[
            "board", "via", "add", "main", "--net", &net, "--at", "13,17", "--id", "v1",
        ],
    );
    ryueda(
        &root,
        &[
            "board", "track", "add", "main", "--net", &net, "--layer", "bottom", "--path",
            "via/v1", "13,18.5", "--id", "t1",
        ],
    );
    ryueda(
        &root,
        &[
            "board", "hole", "add", "main", "--at", "23,17", "--drill", "3.2", "--id", "h1",
        ],
    );
    ryueda(
        &root,
        &["board", "export", "main", "--out", "fab", "--allow-errors"],
    );
    check_package(&root, "main", &root.join("fab"));
}

#[test]
#[ignore = "needs gerbv: the devcontainer, full.yml"]
fn gerbv_reads_a_generated_boards_package() {
    if !have_gerbv() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("board");
    std::fs::create_dir_all(&root).unwrap();
    board_gen::write(&root, &board_gen::seeded(5));
    // A seeded board may carry one seeded fault: what gerbv reads is the
    // same either way.
    ryueda(
        &root,
        &["board", "export", "main", "--out", "fab", "--allow-errors"],
    );
    check_package(&root, "main", &root.join("fab"));
}

/// A generated board on four copper layers with parts on the bottom and
/// one via cut short at `in1` (ryueda 7.A): every copper file, the inner
/// ones too, and the blind span's drill file read by gerbv.
#[test]
#[ignore = "needs gerbv: the devcontainer, full.yml"]
fn gerbv_reads_a_four_layer_package() {
    if !have_gerbv() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("board");
    std::fs::create_dir_all(&root).unwrap();
    // The first seed whose board has inner connections, bottom parts and
    // a connection by the bottom.
    let spec = (0..64u64)
        .map(|seed| board_gen::Spec {
            layers: 4,
            bottom_every: 3,
            ..board_gen::seeded(seed)
        })
        .find(|s| {
            // Tracks wide enough that edges aren't most of the picture.
            s.width >= 200 * board_gen::UM
                && s.cols >= 4
                && (0..s.count()).any(|n| s.bottom(n))
                && (0..s.count() - 1).any(|n| s.inner(n))
                && (0..s.count() - 1).any(|n| s.by_vias(n))
        })
        .unwrap();
    let blind = (0..spec.count() - 1).find(|&n| spec.by_vias(n)).unwrap();
    let spec = board_gen::Spec {
        fault: Some(board_gen::Fault::ViaToIn1(blind)),
        ..spec
    };
    assert!((0..spec.count()).any(|n| spec.bottom(n)));
    assert!((0..spec.count() - 1).any(|n| spec.inner(n)));
    board_gen::write(&root, &spec);
    ryueda(
        &root,
        &["board", "export", "main", "--out", "fab", "--allow-errors"],
    );
    let fab = root.join("fab");
    for name in [
        "main-copper-in1.g2",
        "main-copper-in2.g3",
        "main-pth-l1-l2.drl",
    ] {
        assert!(fab.join(name).is_file(), "{name}");
    }
    check_package(&root, "main", &fab);
}

#[test]
#[ignore = "needs gerbv: the devcontainer, full.yml"]
fn gerbv_reads_slots_in_the_chosen_form() {
    if !have_gerbv() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = |x, y| ryu_board::geom::Point::new(x, y);
    let holes = vec![
        Hole {
            drill: Drill::Round {
                at: p(0, 0),
                d: 1_000_000,
            },
            function: None,
        },
        Hole {
            drill: Drill::Slot {
                a: p(3_000_000, 0),
                b: p(6_000_000, 0),
                d: 800_000,
            },
            function: None,
        },
        Hole {
            drill: Drill::Slot {
                a: p(0, 3_000_000),
                b: p(2_000_000, 5_000_000),
                d: 1_000_000,
            },
            function: None,
        },
    ];
    let d = Drills {
        function: "Plated,1,2,PTH".into(),
        holes: holes.clone(),
    };
    let sw = ryu_gerber::Software {
        vendor: "Seiraiyu".into(),
        application: "ryueda".into(),
        version: "test".into(),
    };
    let owned: Vec<Shape> = holes.iter().map(|h| hole_shape(&h.drill)).collect();
    let shapes: Vec<&Shape> = owned.iter().collect();
    for form in [SlotForm::G85, SlotForm::Routed] {
        let file = dir.path().join(format!("slots-{form:?}.drl"));
        std::fs::write(&file, excellon::write(&d, &sw, form).unwrap()).unwrap();
        let chosen = form == ryueda::board::export::SLOTS;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check(&file, &shapes, dir.path())
        }));
        println!(
            "slots {form:?}: {}",
            if r.is_ok() {
                "read and matched"
            } else {
                "NOT read cleanly"
            }
        );
        if chosen {
            assert!(
                r.is_ok(),
                "the chosen slot form {form:?} isn't read cleanly by gerbv"
            );
        }
    }
}

/// The breakout as agent round 5.D laid it out: clean, so exported
/// without `--allow-errors`, every file read by gerbv.
#[test]
#[ignore = "needs gerbv: the devcontainer, full.yml"]
fn gerbv_reads_the_breakouts_package() {
    if !have_gerbv() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("board");
    std::fs::create_dir_all(&root).unwrap();
    ryueda(&root, &["init"]);
    for (path, text) in breakout::footprints() {
        let at = root.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    for args in breakout::script()
        .into_iter()
        .chain(breakout_board::script())
    {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        ryueda(&root, &args);
    }
    let id = breakout_board::BOARD;
    ryueda(&root, &["board", "export", id, "--out", "fab"]);
    check_package(&root, id, &root.join("fab"));
}
