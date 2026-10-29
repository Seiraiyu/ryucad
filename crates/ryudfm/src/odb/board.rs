//! A job read into a [`Board`]: every file in parallel, gathered in matrix
//! order, so the board is the same bits on any number of threads.
//!
//! What fails what: a missing or unreadable matrix, a version the reader
//! doesn't take, no step that is the board, or no folder for it, fails the
//! job. A layer, symbol, component or EDA file that fails is a fault on the
//! board, and what it held is left out; everything else loads.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rayon::prelude::*;
use ryu_board::model::{
    Attributes, Board, Capabilities, Component, Drill, FeatureAt, FeatureKind, Features, Font,
    Layer, LayerKind, Library, Location, Net, Notice, PinAt, Side, Source, Step, Tool, UserSymbol,
};

use super::attrs::{Decls, Ignored, attrlist};
use super::components::components;
use super::eda::{Eda, eda};
use super::error::{Code, Error, fault, location, notice, warning};
use super::features::{Context, Parsed, features};
use super::font::font;
use super::info::{Info, info};
use super::job::{Job, Limits};
use super::matrix::{self, Matrix, matrix};
use super::netlist::{Netlist, netlist};
use super::stackup;
use super::step::{Header, board_step, header};
use super::text::{Bad, decode};
use super::tools::tools;
use super::units::{Nums, Units, units_of};

/// How a job is read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadOptions {
    /// The step that is the board, when the rule can't choose one.
    pub step: Option<String>,
    pub limits: Limits,
}

/// Reads the job at `path`, a folder or an archive.
pub fn read_path(path: &Path, options: &ReadOptions) -> Result<Board, Error> {
    read(&Job::open(path, options.limits)?, options)
}

/// Faults and warnings, in the order they are found.
#[derive(Clone, Debug, Default)]
struct Notes {
    faults: Vec<Notice>,
    warnings: Vec<Notice>,
}

impl Notes {
    fn fault(&mut self, code: &str, about: Option<String>, message: String, at: Location) {
        self.faults.push(notice(code, about, message, Some(at)));
    }

    fn bad(&mut self, code: &str, about: Option<String>, path: &str, b: &Bad) {
        let at = location(path, Some(b.line), Some(b.column));
        self.fault(code, about, b.message.clone(), at);
    }

    fn nums(&mut self, path: &str, n: &Nums) {
        self.warnings.extend(n.warning(path));
    }

    /// One warning per reason, naming every attribute left out for it.
    fn ignored(&mut self, path: &str, ignored: Vec<Ignored>) {
        let mut by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for i in ignored {
            by.entry(i.why).or_default().insert(i.name);
        }
        for (why, names) in by {
            let names: Vec<String> = names.into_iter().collect();
            let m = format!("attributes left out ({why}): {}", names.join(", "));
            self.warnings.push(notice(
                warning::IGNORED,
                None,
                m,
                Some(location(path, None, None)),
            ));
        }
    }

    fn extend(&mut self, o: Notes) {
        self.faults.extend(o.faults);
        self.warnings.extend(o.warnings);
    }
}

/// A file's text, `None` if the job lacks it.
fn text(job: &Job, path: &str) -> Option<Result<String, String>> {
    job.read(path)
        .map(|r| r.map(|bytes| decode(&bytes).into_owned()))
}

/// What reading one layer of one step gave.
#[derive(Default)]
struct LayerRead {
    features: Option<Parsed>,
    attributes: Option<(Attributes, Units)>,
    tools: Option<Vec<Tool>>,
    components: Vec<Component>,
    notes: Notes,
}

struct Shared<'a> {
    job: &'a Job,
    units: Units,
    decls: &'a Decls,
    user: &'a BTreeSet<String>,
}

fn read_layer(
    sh: &Shared,
    step: &str,
    layer: &matrix::MatrixLayer,
    side: Option<Side>,
) -> LayerRead {
    let mut out = LayerRead::default();
    let dir = format!("steps/{step}/layers/{}", layer.name);
    let about = Some(format!("{step}/{}", layer.name));
    let path = format!("{dir}/features");
    let stored = sh.job.stored(&path);
    match text(sh.job, &path) {
        None if layer.kind == LayerKind::Component => {}
        None => out.notes.fault(
            fault::LAYER_UNREADABLE,
            about.clone(),
            format!("{} has no features file", layer.name),
            location(&path, None, None),
        ),
        Some(Err(e)) => out.notes.fault(
            fault::LAYER_UNREADABLE,
            about.clone(),
            e,
            location(&stored, None, None),
        ),
        Some(Ok(t)) => {
            let cx = Context {
                path: &stored,
                units: sh.units,
                decls: sh.decls,
                user: sh.user,
            };
            match features(&t, &cx) {
                Ok(p) => {
                    out.notes.nums(&stored, &p.nums);
                    out.notes.warnings.extend(p.warnings.iter().cloned());
                    if !p.unknown.is_empty() {
                        out.notes.fault(
                            fault::SYMBOL_UNKNOWN,
                            about.clone(),
                            format!(
                                "symbols neither standard nor in the job: {}",
                                p.unknown.join(", ")
                            ),
                            location(&stored, None, None),
                        );
                    }
                    out.features = Some(p);
                }
                Err(b) => out
                    .notes
                    .bad(fault::LAYER_UNREADABLE, about.clone(), &stored, &b),
            }
        }
    }
    let path = format!("{dir}/attrlist");
    if let Some(r) = text(sh.job, &path) {
        match r.map_err(|e| Bad::new(0, 0, e)).and_then(|t| {
            let u = units_of(&t).unwrap_or(sh.units);
            attrlist(&t, sh.decls).map(|(a, i)| (a, i, u))
        }) {
            Ok((a, ignored, u)) => {
                out.notes.ignored(&path, ignored);
                out.attributes = Some((a, u));
            }
            Err(b) => out.notes.bad(fault::FILE_INVALID, about.clone(), &path, &b),
        }
    }
    if matches!(layer.kind, LayerKind::Drill | LayerKind::Rout) {
        let path = format!("{dir}/tools");
        if let Some(r) = text(sh.job, &path) {
            match r
                .map_err(|e| Bad::new(0, 0, e))
                .and_then(|t| tools(&t, sh.units))
            {
                Ok((t, n)) => {
                    out.notes.nums(&path, &n);
                    out.tools = Some(t);
                }
                Err(b) => out.notes.bad(fault::FILE_INVALID, about.clone(), &path, &b),
            }
        }
    }
    if layer.kind == LayerKind::Component {
        let path = format!("{dir}/components");
        let stored = sh.job.stored(&path);
        if let Some(r) = text(sh.job, &path) {
            let side = side.unwrap_or(if layer.name.ends_with("bot") {
                Side::Bottom
            } else {
                Side::Top
            });
            match r
                .map_err(|e| Bad::new(0, 0, e))
                .and_then(|t| components(&t, &stored, side, sh.units, sh.decls))
            {
                Ok((c, n, w)) => {
                    out.notes.nums(&stored, &n);
                    out.notes.warnings.extend(w);
                    out.components = c;
                }
                Err(b) => out.notes.bad(fault::COMPONENT_INVALID, about, &stored, &b),
            }
        }
    }
    out
}

/// Reads a step's EDA data and CAD netlist.
fn read_nets(sh: &Shared, step: &str) -> (Option<Eda>, Option<Netlist>, Notes) {
    let mut notes = Notes::default();
    let path = format!("steps/{step}/eda/data");
    let stored = sh.job.stored(&path);
    let e = text(sh.job, &path).and_then(|r| {
        match r
            .map_err(|e| Bad::new(0, 0, e))
            .and_then(|t| eda(&t, &stored, sh.units, sh.decls))
        {
            Ok((e, n)) => {
                notes.nums(&stored, &n);
                notes.warnings.extend(e.warnings.iter().cloned());
                Some(e)
            }
            Err(b) => {
                notes.bad(fault::EDA_INVALID, Some(step.to_string()), &stored, &b);
                None
            }
        }
    });
    let path = format!("steps/{step}/netlists/cadnet/netlist");
    let c = text(sh.job, &path).and_then(|r| {
        match r
            .map_err(|e| Bad::new(0, 0, e))
            .and_then(|t| netlist(&t, sh.units))
        {
            Ok((c, n)) => {
                notes.nums(&path, &n);
                Some(c)
            }
            Err(b) => {
                notes.bad(fault::FILE_INVALID, Some(step.to_string()), &path, &b);
                None
            }
        }
    });
    (e, c, notes)
}

/// Reads a features file with no layer (a profile or a user symbol).
fn read_features(sh: &Shared, path: &str, about: &str, notes: &mut Notes) -> Option<Parsed> {
    let stored = sh.job.stored(path);
    let t = match text(sh.job, path)? {
        Ok(t) => t,
        Err(e) => {
            notes.fault(
                fault::FILE_INVALID,
                Some(about.to_string()),
                e,
                location(&stored, None, None),
            );
            return None;
        }
    };
    let cx = Context {
        path: &stored,
        units: sh.units,
        decls: sh.decls,
        user: sh.user,
    };
    match features(&t, &cx) {
        Ok(p) => {
            notes.nums(&stored, &p.nums);
            notes.warnings.extend(p.warnings.iter().cloned());
            if !p.unknown.is_empty() {
                notes.fault(
                    fault::SYMBOL_UNKNOWN,
                    Some(about.to_string()),
                    format!(
                        "symbols neither standard nor in the job: {}",
                        p.unknown.join(", ")
                    ),
                    location(&stored, None, None),
                );
            }
            Some(p)
        }
        Err(b) => {
            notes.bad(fault::FILE_INVALID, Some(about.to_string()), &stored, &b);
            None
        }
    }
}

fn read_attrlist(sh: &Shared, path: &str, about: Option<String>, notes: &mut Notes) -> Attributes {
    match text(sh.job, path) {
        None => Attributes::new(),
        Some(r) => match r
            .map_err(|e| Bad::new(0, 0, e))
            .and_then(|t| attrlist(&t, sh.decls))
        {
            Ok((a, ignored)) => {
                notes.ignored(path, ignored);
                a
            }
            Err(b) => {
                notes.bad(fault::FILE_INVALID, about, path, &b);
                Attributes::new()
            }
        },
    }
}

/// Reads an ODB++ job into a board.
pub fn read(job: &Job, options: &ReadOptions) -> Result<Board, Error> {
    let mut notes = Notes::default();
    let info = match text(job, "misc/info") {
        None => {
            notes.warnings.push(notice(
                warning::INFO_MISSING,
                None,
                "no misc/info: read as ODB++ 7.0 in inches",
                Some(location("misc/info", None, None)),
            ));
            Info::absent()
        }
        Some(Err(e)) => {
            return Err(Error::new(Code::Unreadable, e).at(location("misc/info", None, None)));
        }
        Some(Ok(t)) => {
            let i = info(&t)?;
            if !i.missing.is_empty() {
                notes.warnings.push(notice(
                    warning::INFO_MISSING,
                    None,
                    format!(
                        "misc/info lacks {}: read as {}",
                        i.missing.join(" and "),
                        i.version_text()
                    ),
                    Some(location("misc/info", None, None)),
                ));
            }
            i
        }
    };
    let units = info.units;

    let mut decls = Decls::default();
    for name in [
        "sysattr",
        "sysattr.dfm",
        "sysattr.fab",
        "sysattr.assy",
        "sysattr.test",
        "sysattr.gen",
        "userattr",
    ] {
        let path = format!("misc/{name}");
        if let Some(r) = text(job, &path)
            && let Err(b) = r
                .map_err(|e| Bad::new(0, 0, e))
                .and_then(|t| decls.read(&t))
        {
            notes.bad(fault::FILE_INVALID, None, &path, &b);
        }
    }

    let m: Matrix = match text(job, matrix::PATH) {
        None => {
            return Err(
                Error::new(Code::Unreadable, "no matrix/matrix").at(location(
                    matrix::PATH,
                    None,
                    None,
                )),
            );
        }
        Some(Err(e)) => {
            return Err(Error::new(Code::Unreadable, e).at(location(matrix::PATH, None, None)));
        }
        Some(Ok(t)) => matrix(&t)?,
    };
    let (mut layers, span_faults) = m.layers();
    notes.faults.extend(span_faults);

    // Step headers, for the step-and-repeat the board step's rule reads.
    let step_dirs = job.children("steps");
    let names: Vec<String> = m.steps.iter().map(|s| s.name.clone()).collect();
    let mut headers: Vec<Option<Header>> = Vec::new();
    for name in &names {
        let path = format!("steps/{name}/stephdr");
        headers.push(match text(job, &path) {
            None => None,
            Some(r) => match r
                .map_err(|e| Bad::new(0, 0, e))
                .and_then(|t| header(&t, units))
            {
                Ok((h, n)) => {
                    notes.nums(&path, &n);
                    if h.flipped > 0 {
                        notes.warnings.push(notice(
                            warning::IGNORED,
                            Some(name.clone()),
                            format!(
                                "{} step repeats flipped (FLIP=YES), placed unflipped",
                                h.flipped
                            ),
                            Some(location(&path, None, None)),
                        ));
                    }
                    Some(h)
                }
                Err(b) => {
                    notes.bad(fault::FILE_INVALID, Some(name.clone()), &path, &b);
                    None
                }
            },
        });
    }
    let repeats: Vec<Vec<String>> = headers
        .iter()
        .map(|h| {
            h.as_ref().map_or(Vec::new(), |h| {
                h.repeats.iter().map(|r| r.step.clone()).collect()
            })
        })
        .collect();
    let board = board_step(&names, &repeats, options.step.as_deref())?;
    if !step_dirs.contains(&names[board]) {
        return Err(Error::new(
            Code::Unreadable,
            format!("no steps/{} for the board step", names[board]),
        )
        .at(location(&format!("steps/{}", names[board]), None, None)));
    }

    // User symbols and fonts.
    let user: BTreeSet<String> = job
        .children("symbols")
        .into_iter()
        .filter(|s| job.exists(&format!("symbols/{s}/features")))
        .collect();
    let sh = Shared {
        job,
        units,
        decls: &decls,
        user: &user,
    };
    let symbols: Vec<(Option<UserSymbol>, Notes)> = user
        .par_iter()
        .map(|name| {
            let mut n = Notes::default();
            let p = read_features(&sh, &format!("symbols/{name}/features"), name, &mut n);
            let s = p.map(|p| UserSymbol {
                name: name.clone(),
                features: p.into_features(),
            });
            (s, n)
        })
        .collect();
    let mut user_symbols = Vec::new();
    for (s, n) in symbols {
        notes.extend(n);
        user_symbols.extend(s);
    }
    let mut fonts: Vec<Font> = Vec::new();
    for name in job.children("fonts") {
        let path = format!("fonts/{name}");
        let Some(r) = text(job, &path) else { continue };
        match r
            .map_err(|e| Bad::new(0, 0, e))
            .and_then(|t| font(&name, &t))
        {
            Ok((f, negative)) => {
                if negative > 0 {
                    notes.warnings.push(notice(
                        warning::IGNORED,
                        Some(name.clone()),
                        format!("{negative} negative font strokes"),
                        Some(location(&path, None, None)),
                    ));
                }
                fonts.push(f);
            }
            Err(b) => notes.bad(fault::FILE_INVALID, Some(name.clone()), &path, &b),
        }
    }
    let library = Library::new(user_symbols, fonts);
    for cycle in library.cycles() {
        notes.fault(
            fault::SYMBOL_CYCLE,
            Some(cycle[0].clone()),
            format!("user symbols that use themselves: {}", cycle.join(", ")),
            location(&format!("symbols/{}/features", cycle[0]), None, None),
        );
    }

    // Every layer of every step, and every step's nets, in parallel.
    let nl = m.layers.len();
    let reads: Vec<LayerRead> = (0..names.len() * nl)
        .into_par_iter()
        .map(|k| {
            let (s, l) = (k / nl, k % nl);
            if !step_dirs.contains(&names[s]) {
                return LayerRead::default();
            }
            read_layer(&sh, &names[s], &m.layers[l], layers[l].side)
        })
        .collect();
    let nets: Vec<(Option<Eda>, Option<Netlist>, Notes)> =
        names.par_iter().map(|s| read_nets(&sh, s)).collect();
    let mut reads = reads.into_iter();
    let mut nets = nets.into_iter();

    let mut steps = Vec::new();
    let mut drills = Vec::new();
    let mut layer_attrs: Vec<(Attributes, Units)> = vec![(Attributes::new(), units); nl];
    for (s, name) in names.iter().enumerate() {
        let here: Vec<LayerRead> = reads.by_ref().take(nl).collect();
        let (eda, cad, net_notes) = nets.next().unwrap();
        if !step_dirs.contains(name) {
            notes.fault(
                fault::FILE_INVALID,
                Some(name.clone()),
                format!("the matrix names step {name}, which has no folder"),
                location(&format!("steps/{name}"), None, None),
            );
        }
        let mut step_notes = Notes::default();
        let profile = read_features(&sh, &format!("steps/{name}/profile"), name, &mut step_notes)
            .and_then(|p| {
                let mut regions = p.features.into_iter().filter_map(|f| match f.kind {
                    FeatureKind::Surface { regions } => Some(regions),
                    _ => None,
                });
                match (regions.next(), regions.next()) {
                    (Some(r), None) if r.len() == 1 => r.into_iter().next(),
                    _ => {
                        step_notes.fault(
                            fault::FILE_INVALID,
                            Some(name.clone()),
                            "a profile that isn't one surface of one island".into(),
                            location(&format!("steps/{name}/profile"), None, None),
                        );
                        None
                    }
                }
            });
        let attributes = read_attrlist(
            &sh,
            &format!("steps/{name}/attrlist"),
            Some(name.clone()),
            &mut step_notes,
        );
        notes.extend(step_notes);

        let mut parsed: Vec<Option<Parsed>> = Vec::new();
        let mut comps = Vec::new();
        for (l, r) in here.into_iter().enumerate() {
            notes.extend(r.notes);
            if s == board {
                if let Some(a) = r.attributes {
                    layer_attrs[l] = a;
                }
                if matches!(m.layers[l].kind, LayerKind::Drill) {
                    drills.push(Drill {
                        layer: l,
                        tools: r.tools.unwrap_or_default(),
                        back_drill: m.layers[l]
                            .add_type
                            .as_deref()
                            .is_some_and(|t| t.eq_ignore_ascii_case("BACKDRILL")),
                    });
                }
            }
            parsed.push(r.features);
            comps.extend(r.components);
        }
        notes.extend(net_notes);
        let (packages, mut step_nets) = attach(name, &layers, &mut parsed, eda, &mut notes);
        // Components that name no package are left out.
        let mut components = Vec::new();
        let mut bad_nets = 0;
        for mut c in comps {
            if c.package as usize >= packages.len() {
                notes.fault(
                    fault::COMPONENT_INVALID,
                    Some(format!("{name}/comp/{}", c.refdes)),
                    format!(
                        "{} names package {}, which eda/data lacks",
                        c.refdes, c.package
                    ),
                    location(&format!("steps/{name}/eda/data"), None, None),
                );
                continue;
            }
            for p in &mut c.pins {
                if p.net.is_some_and(|n| n as usize >= step_nets.len()) {
                    p.net = None;
                    bad_nets += 1;
                }
            }
            components.push(c);
        }
        if bad_nets > 0 {
            notes.fault(
                fault::COMPONENT_INVALID,
                Some(name.clone()),
                format!("{bad_nets} pins name nets eda/data lacks"),
                location(&format!("steps/{name}/eda/data"), None, None),
            );
        }
        for (ci, c) in components.iter().enumerate() {
            for (pi, p) in c.pins.iter().enumerate() {
                if let Some(n) = p.net {
                    step_nets[n as usize].pins.push(PinAt {
                        component: ci,
                        pin: pi,
                    });
                }
            }
        }
        if let Some(cad) = cad {
            for (net_name, points) in cad {
                match step_nets.iter_mut().find(|n| n.name == net_name) {
                    Some(n) => n.cad.extend(points),
                    None => step_nets.push(Net {
                        name: net_name,
                        cad: points,
                        ..Net::default()
                    }),
                }
            }
        }
        // Text drawn in fonts the job lacks, or with characters they lack.
        for (l, p) in parsed.iter().enumerate() {
            let Some(p) = p else { continue };
            let mut missing: BTreeMap<String, BTreeSet<char>> = BTreeMap::new();
            for f in &p.features {
                if let FeatureKind::Text(t) = &f.kind {
                    let gaps = missing.entry(t.font.clone()).or_default();
                    match library.font(&t.font) {
                        Some(font) => gaps.extend(
                            t.string
                                .chars()
                                .filter(|c| !c.is_whitespace() && !font.chars.contains_key(c)),
                        ),
                        None => {
                            gaps.insert('\u{0}');
                        }
                    }
                }
            }
            for (font_name, gaps) in missing.into_iter().filter(|(_, g)| !g.is_empty()) {
                let path = format!("steps/{name}/layers/{}/features", layers[l].name);
                let message = if gaps.contains(&'\u{0}') {
                    format!("font {font_name} isn't in the job: its text has no shape")
                } else {
                    format!("font {font_name} lacks {}", gaps.iter().collect::<String>())
                };
                notes.warnings.push(notice(
                    warning::FONT_MISSING_CHAR,
                    Some(format!("{name}/{}", layers[l].name)),
                    message,
                    Some(location(&path, None, None)),
                ));
            }
        }
        let header = headers[s].clone();
        steps.push(Step {
            name: name.clone(),
            datum: header.as_ref().map_or(Default::default(), |h| h.datum),
            profile,
            repeats: header.map_or(Vec::new(), |h| h.repeats),
            layers: parsed
                .into_iter()
                .map(|p| p.map_or_else(Features::default, Parsed::into_features))
                .collect(),
            components,
            packages,
            nets: step_nets,
            attributes,
        });
    }
    for (l, (a, _)) in layers.iter_mut().zip(&layer_attrs) {
        l.attributes = a.clone();
    }

    let stackup = match text(job, stackup::PATH) {
        Some(Ok(t)) => match stackup::from_xml(&t) {
            Ok(s) => Some(s),
            Err(e) => {
                notes.fault(
                    fault::FILE_INVALID,
                    None,
                    e,
                    location(stackup::PATH, None, None),
                );
                stackup::from_matrix(&m.layers, &layer_attrs)
            }
        },
        Some(Err(e)) => {
            notes.fault(
                fault::FILE_INVALID,
                None,
                e,
                location(stackup::PATH, None, None),
            );
            stackup::from_matrix(&m.layers, &layer_attrs)
        }
        None => stackup::from_matrix(&m.layers, &layer_attrs),
    };
    let attributes = read_attrlist(&sh, "misc/attrlist", None, &mut notes);

    let capabilities = capabilities(
        &layers,
        &steps[board],
        &drills,
        stackup.as_ref(),
        &notes,
        &names[board],
    );
    Ok(Board {
        name: info
            .name
            .clone()
            .or_else(|| job.top.clone())
            .unwrap_or_else(|| job.stem.clone()),
        source: Source {
            format: "odb++".into(),
            version: info.version_text(),
            application: info.application.clone(),
            units: Some(units.as_str().into()),
            created: info.created.clone(),
            saved: info.saved.clone(),
        },
        layers,
        steps,
        board_step: board,
        library,
        drills,
        stackup,
        attributes,
        capabilities,
        faults: notes.faults,
        warnings: notes.warnings,
    })
}

/// Gives features their nets from `eda/data`'s feature IDs, and returns
/// the step's packages and nets.
fn attach(
    step: &str,
    layers: &[Layer],
    parsed: &mut [Option<Parsed>],
    eda: Option<Eda>,
    notes: &mut Notes,
) -> (Vec<ryu_board::model::Package>, Vec<Net>) {
    let Some(eda) = eda else {
        return (Vec::new(), Vec::new());
    };
    let index: Vec<Option<usize>> = eda
        .layers
        .iter()
        .map(|n| layers.iter().position(|l| l.name == *n))
        .collect();
    let mut nets = Vec::new();
    let mut dangling = 0;
    for (i, n) in eda.nets.into_iter().enumerate() {
        let mut net = Net {
            name: n.name,
            ..Net::default()
        };
        for s in &n.subnets {
            for f in &s.fids {
                let layer = index.get(f.layer as usize).copied().flatten();
                // A layer that failed to read has its own fault.
                if layer.is_some_and(|l| parsed[l].is_none()) {
                    continue;
                }
                let at = layer.and_then(|l| {
                    let p = parsed[l].as_mut()?;
                    let feature = p.features.get_mut(f.feature as usize)?;
                    if feature.net.is_none() {
                        feature.net = Some(i as u32);
                    }
                    Some(FeatureAt {
                        layer: l,
                        index: f.feature as usize,
                    })
                });
                match at {
                    Some(at) => net.features.push(at),
                    None => dangling += 1,
                }
            }
        }
        net.features.sort();
        net.features.dedup();
        nets.push(net);
    }
    if dangling > 0 {
        notes.fault(
            fault::EDA_INVALID,
            Some(step.to_string()),
            format!("{dangling} feature references to layers or features the step lacks"),
            location(&format!("steps/{step}/eda/data"), None, None),
        );
    }
    (eda.packages, nets)
}

fn capabilities(
    layers: &[Layer],
    board: &Step,
    drills: &[Drill],
    stackup: Option<&ryu_board::model::Stackup>,
    notes: &Notes,
    step: &str,
) -> Capabilities {
    let faulted = |l: &Layer| {
        let about = format!("{step}/{}", l.name);
        notes.faults.iter().any(|f| {
            f.code == fault::LAYER_UNREADABLE && f.about.as_deref() == Some(about.as_str())
        })
    };
    let copper_read = layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.kind.is_copper() && !faulted(l))
        .count();
    let features = || board.layers.iter().flat_map(|f| f.features());
    Capabilities {
        geometry: copper_read > 0,
        nets: features().any(|f| f.net.is_some()),
        components: !board.components.is_empty(),
        stackup: stackup.is_some_and(stackup::complete),
        drill: drills
            .iter()
            .any(|d| !d.tools.is_empty() && !board.layers[d.layer].features().is_empty()),
        flex: false,
        feature_ids: features().any(|f| f.id.is_some()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odb::job::Container;
    use ryu_board::geom::Point;
    use ryu_board::model::{AttrValue, StackKind, ToolKind};

    /// A small job in millimetres: two copper layers with a dielectric
    /// between, a drill, a top component layer, nets and a font.
    fn files() -> Vec<(String, Vec<u8>)> {
        let f = |p: &str, t: &str| (p.to_string(), t.as_bytes().to_vec());
        vec![
            f(
                "misc/info",
                "UNITS=MM\nPRODUCT_MODEL_NAME=tiny\nODB_VERSION_MAJOR=8\nODB_VERSION_MINOR=1\nSAVE_APP=hand\n",
            ),
            f(
                "matrix/matrix",
                "STEP {\nCOL=1\nNAME=PCB\n}\n\
LAYER {\nROW=1\nCONTEXT=BOARD\nTYPE=COMPONENT\nNAME=COMP_+_TOP\nPOLARITY=POSITIVE\n}\n\
LAYER {\nROW=2\nCONTEXT=BOARD\nTYPE=SIGNAL\nNAME=TOP\nPOLARITY=POSITIVE\n}\n\
LAYER {\nROW=3\nCONTEXT=BOARD\nTYPE=DIELECTRIC\nNAME=CORE\nDIELECTRIC_NAME=FR-4\n}\n\
LAYER {\nROW=4\nCONTEXT=BOARD\nTYPE=SIGNAL\nNAME=BOT\nPOLARITY=POSITIVE\n}\n\
LAYER {\nROW=5\nCONTEXT=BOARD\nTYPE=DRILL\nNAME=DRILL\nSTART_NAME=TOP\nEND_NAME=BOT\n}\n",
            ),
            f(
                "fonts/standard",
                "XSIZE 1\nYSIZE 1\nOFFSET 0\nCHAR A\nLINE 0 0 1 1 P R 0.1\nECHAR\n",
            ),
            f("steps/pcb/stephdr", "UNITS=MM\nX_DATUM=1\nY_DATUM=2\n"),
            f(
                "steps/pcb/profile",
                "UNITS=MM\nS P 0\nOB 0 0 I\nOS 10 0\nOS 10 10\nOS 0 10\nOS 0 0\nOE\nSE\n",
            ),
            f("steps/pcb/attrlist", "UNITS=MM\n.board_thickness = 1.6\n"),
            f(
                "steps/pcb/layers/top/features",
                "UNITS=MM\n$0 r200\n$1 rect1000x500\n@0 .smd\nP 1 1 1 P 0 0;0;ID=7\nL 1 1 5 5 0 P 0\nT 2 2 standard P 0 1 1 0.5 'AA' 0\n",
            ),
            f(
                "steps/pcb/layers/top/attrlist",
                "UNITS=MM\n.copper_weight = 35\n",
            ),
            f("steps/pcb/layers/core/features", "UNITS=MM\n"),
            f(
                "steps/pcb/layers/core/attrlist",
                "UNITS=MM\n.layer_dielectric = 1.5\n",
            ),
            f(
                "steps/pcb/layers/bot/features",
                "UNITS=MM\n$0 r600\nP 5 5 0 P 0 0\n",
            ),
            f(
                "steps/pcb/layers/bot/attrlist",
                "UNITS=MM\n.copper_weight = 35\n",
            ),
            f(
                "steps/pcb/layers/drill/features",
                "UNITS=MM\n$0 r300\nP 5 5 0 P 1 0\n",
            ),
            f(
                "steps/pcb/layers/drill/tools",
                "UNITS=MM\nTOOLS {\nNUM=1\nTYPE=VIA\nFINISH_SIZE=300\n}\n",
            ),
            f("steps/pcb/layers/comp_+_top/features", ""),
            f(
                "steps/pcb/layers/comp_+_top/components",
                "UNITS=MM\n@0 .comp_height\nCMP 0 1 1 90 N R1 RES;0=1.2\nPRP Value '10k'\nTOP 0 1 1 90 N 0 0 1\n",
            ),
            f(
                "steps/pcb/eda/data",
                "HDR test\nUNITS=MM\nLYR top bot drill\nNET VCC\nSNT TOP T 0 0\nFID C 0 0\nFID C 0 1\nSNT VIA\nFID C 1 0\nFID H 2 0\nNET GND\nPKG 0805 1 0 0 2 1\nRC 0 0 2 1\nPIN 1 S 0.5 0.5 0 E S\nRC 0 0 1 1\n",
            ),
            f(
                "steps/pcb/netlists/cadnet/netlist",
                "UNITS=MM\nH optimize n\n$0 VCC\n$1 NC\n0 0.3 5 5 B e e v\n1 0.002 9 9 T e e\n",
            ),
        ]
    }

    fn read_files(files: Vec<(String, Vec<u8>)>) -> Result<Board, Error> {
        read(
            &Job::from_files(Container::Folder, "tiny", files, Limits::default())?,
            &ReadOptions::default(),
        )
    }

    #[test]
    fn a_small_job_reads_whole() {
        let b = read_files(files()).unwrap();
        assert!(b.faults.is_empty(), "{:?}", b.faults);
        assert!(b.warnings.is_empty(), "{:?}", b.warnings);
        assert_eq!(
            (
                b.name.as_str(),
                b.source.version.as_str(),
                b.source.application.as_deref()
            ),
            ("tiny", "8.1", Some("hand"))
        );
        let names: Vec<&str> = b.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["comp_+_top", "top", "core", "bot", "drill"]);
        let s = &b.steps[b.board_step];
        assert_eq!(s.datum, Point::new(1_000_000, 2_000_000));
        assert!(s.profile.is_some());
        assert_eq!(
            s.attributes.get(".board_thickness"),
            Some(&AttrValue::Number("1.6".into()))
        );
        // Nets: the pad, the line and the bottom pad and hole are VCC.
        assert_eq!(
            s.nets.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            ["VCC", "GND", "NC"]
        );
        assert_eq!(s.nets[0].features.len(), 4);
        assert_eq!(s.layers[1].features()[0].net, Some(0));
        assert_eq!(s.layers[1].features()[0].id, Some(7));
        assert_eq!(
            s.nets[0].pins,
            [PinAt {
                component: 0,
                pin: 0
            }]
        );
        assert_eq!((s.nets[0].cad.len(), s.nets[2].cad.len()), (1, 1));
        // The component, its package, property and attribute.
        let c = &s.components[0];
        assert_eq!(
            (c.refdes.as_str(), c.side, c.properties["Value"].as_str()),
            ("R1", Side::Top, "10k")
        );
        assert_eq!(
            c.attributes.get(".comp_height"),
            Some(&AttrValue::Number("1.2".into()))
        );
        assert_eq!(s.packages[0].pins[0].name, "1");
        // The drill and its tool, the stackup from the attributes.
        assert_eq!(b.drills.len(), 1);
        assert_eq!(
            (b.drills[0].tools[0].kind, b.drills[0].tools[0].finished),
            (ToolKind::Via, Some(300_000))
        );
        let st = b.stackup.as_ref().unwrap();
        let got: Vec<_> = st
            .layers
            .iter()
            .map(|l| (l.name.as_str(), l.kind, l.thickness))
            .collect();
        assert_eq!(
            got,
            [
                ("top", StackKind::Copper, Some(35_000)),
                ("core", StackKind::Dielectric, Some(1_500_000)),
                ("bot", StackKind::Copper, Some(35_000))
            ]
        );
        assert_eq!(
            b.capabilities,
            Capabilities {
                geometry: true,
                nets: true,
                components: true,
                stackup: true,
                drill: true,
                flex: false,
                feature_ids: true
            }
        );
        assert!(b.library.font("standard").is_some());
    }

    #[test]
    fn faults_fail_only_what_they_reach() {
        let edit = |path: &str, text: &str| {
            let mut f = files();
            for (p, d) in &mut f {
                if p == path {
                    *d = text.as_bytes().to_vec();
                }
            }
            read_files(f).unwrap()
        };
        let b = edit("steps/pcb/layers/top/features", "UNITS=MM\nL 0 0 1\n");
        let codes: Vec<_> = b.faults.iter().map(|f| f.code.as_str()).collect();
        // The layer fails; eda/data's references into it are its fault's.
        assert_eq!(codes, ["odb_layer_unreadable"]);
        assert!(!b.capabilities.feature_ids);
        let b = edit(
            "steps/pcb/layers/comp_+_top/components",
            "UNITS=MM\nCMP 7 1 1 0 N R1 RES\n",
        );
        assert_eq!(b.faults[0].code, "odb_component_invalid");
        assert!(b.steps[0].components.is_empty());
        let b = edit(
            "steps/pcb/eda/data",
            "UNITS=MM\nLYR top\nNET A\nSNT TRC\nFID C 0 99\n",
        );
        assert_eq!(
            b.faults.iter().map(|f| f.code.as_str()).collect::<Vec<_>>(),
            ["odb_eda_invalid", "odb_component_invalid"]
        );
        let b = edit(
            "steps/pcb/layers/top/features",
            "UNITS=MM\n$0 r10\nT 0 0 standard P 0 1 1 1 'AZ' 0\nT 0 0 other P 0 1 1 1 'A' 0\n",
        );
        let w: Vec<_> = b.warnings.iter().map(|w| w.message.as_str()).collect();
        assert_eq!(
            w,
            [
                "font other isn't in the job: its text has no shape",
                "font standard lacks Z"
            ]
        );
        let b = edit("steps/pcb/layers/drill/tools", "TOOLS {\nNUM=1\n}\n");
        assert_eq!(b.faults[0].code, "odb_file_invalid");
        assert!(!b.capabilities.drill);
        let e = read_files(
            files()
                .into_iter()
                .filter(|(p, _)| p != "matrix/matrix")
                .collect(),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotABoard);
    }

    #[test]
    fn one_thread_and_eight_read_alike() {
        let read = |threads: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| read_files(files()).unwrap())
        };
        assert!(read(1) == read(8));
    }
}
