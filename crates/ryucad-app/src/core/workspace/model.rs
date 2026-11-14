//! The model: what the window asks the daemon for it (an `impl Workspace`
//! block) and, from 27.F's Task 7 on, what it does with the answers.
//! Its fields live on `core::model::Model`.

use super::*;

impl Workspace {
    /// Takes a preview solve's poses: each instance it moved, drawn there.
    pub(super) fn preview_poses(&mut self, part: &str, result: &Value) {
        let Some(instances) = result["instances"].as_object() else {
            return;
        };
        let triple = |v: &Value| [0, 1, 2].map(|k| v[k].as_f64().unwrap_or_default());
        for (id, i) in instances {
            let p = &i["pose"];
            let pose = crate::core::model::Pose::of(
                triple(&p["origin_m"]),
                triple(&p["z"]),
                triple(&p["x"]),
            );
            self.model
                .preview
                .insert((part.to_owned(), id.clone()), pose);
        }
        self.model.generation += 1;
    }

    /// Opens the form for a feature (or closes it, with `None`); a
    /// dimension's form closes.
    /// Chooses a feature in the tree (or none): what it made is read, to
    /// be painted.
    pub fn choose(&mut self, feature: Option<BodyKey>) {
        self.model.made = None;
        self.model.deleted = None;
        self.model.swept = None;
        self.model.swept_at = None;
        self.model.made_for.clone_from(&feature);
        if let Some(key) = feature {
            self.ask_deleted(&key);
            self.ask_swept(&key);
            self.ask_sheet_paint(&key);
            self.ask_made(key);
        }
    }

    /// Reads `key`'s sheet metal table (the part and the solid, `""` for
    /// its one sheet metal solid) unless it was read at the part's
    /// revision or is on its way: the Part tile's sheet metal section asks
    /// every frame it shows.
    pub fn show_sheet(&mut self, key: &BodyKey) {
        let Some(revision) = self.revision_of(&key.0) else {
            return;
        };
        if self.model.sheets.want_table(key, &revision) {
            let req = Request::SheetGet {
                part: key.0.clone(),
                solid: (!key.1.is_empty()).then(|| key.1.clone()),
                rev: self.model_rev(),
            };
            self.send(
                req,
                Pending::Model(model::Asked::SheetTable(key.clone(), revision)),
            );
        }
    }

    /// The sheet metal solid the Part tile's section shows of `part`: the
    /// chosen feature's solid when it is in `part`, else the solid a base
    /// flange founds, else `""` (the part's one sheet metal solid, as the
    /// daemon finds it); `None` for a part with no sheet metal feature.
    pub fn sheet_solid(&self, part: &str, chosen: Option<&BodyKey>) -> Option<BodyKey> {
        let p = self.model.parts.get(part)?;
        let sheet = |f: &crate::core::model::Feature| f.sheet.is_some();
        if !p.features.iter().any(sheet) {
            return None;
        }
        if let Some(k) = chosen.filter(|k| k.0 == part)
            && p.feature(&k.1).is_some_and(sheet)
            && let Some(s) = self.model.solid_of(k)
        {
            return Some(s);
        }
        let founder = p
            .features
            .iter()
            .find(|f| f.sheet.as_ref().is_some_and(|s| s.kind == "base_flange"))
            .map_or_else(String::new, |f| f.id.clone());
        Some((part.to_owned(), founder))
    }

    /// Reads `key`'s flat bend lines, as [`Workspace::show_sheet`] its
    /// table.
    fn ask_flat_lines(&mut self, key: &BodyKey) {
        let Some(revision) = self.revision_of(&key.0) else {
            return;
        };
        if self.model.sheets.want_flat(key, &revision) {
            let req = Request::FlatGet {
                part: key.0.clone(),
                solid: (!key.1.is_empty()).then(|| key.1.clone()),
                rev: self.model_rev(),
                fixed: None,
                bends: Vec::new(),
            };
            self.send(
                req,
                Pending::Model(model::Asked::FlatLines(key.clone(), revision)),
            );
        }
    }

    /// What a chosen unfold or refold paints needs: its target's table,
    /// and an unfold's flat too.
    pub(super) fn ask_sheet_paint(&mut self, key: &BodyKey) {
        let Some(info) = self
            .model
            .parts
            .get(&key.0)
            .and_then(|p| p.feature(&key.1))
            .filter(|f| f.error.is_none())
            .and_then(|f| f.sheet.clone())
        else {
            return;
        };
        let Some(target) = info.target else {
            return;
        };
        let solid = (key.0.clone(), target);
        match info.kind.as_str() {
            "unfold" => {
                self.show_sheet(&solid);
                self.ask_flat_lines(&solid);
            }
            "refold" => self.show_sheet(&solid),
            _ => {}
        }
    }

    /// The faces and edges a chosen unfold or refold paints, from what
    /// the window has read ([`crate::core::sheet::painted`]).
    pub fn sheet_paint(&self, key: &BodyKey) -> (Vec<String>, Vec<String>) {
        let none = (Vec::new(), Vec::new());
        let Some(p) = self.model.parts.get(&key.0) else {
            return none;
        };
        let Some(info) = p.feature(&key.1).and_then(|f| f.sheet.as_ref()) else {
            return none;
        };
        let Some(target) = &info.target else {
            return none;
        };
        let solid = (key.0.clone(), target.clone());
        let table = match self.model.sheets.table(&solid, &p.revision) {
            Some(crate::core::sheet::Shown::Table(t)) => Some(t),
            _ => None,
        };
        let flat = self
            .model
            .sheets
            .flat(&solid, &p.revision)
            .and_then(|f| f.as_ref().ok());
        let unfold_only = info
            .unfold
            .as_ref()
            .and_then(|u| p.feature(u))
            .and_then(|f| f.sheet.as_ref())
            .map(|u| u.only.clone())
            .unwrap_or_default();
        crate::core::sheet::painted(&key.1, info, &unfold_only, table, flat)
    }

    /// Turns the flat view on, for `solid` (a part and its solid, `""` for
    /// the part's one sheet metal solid), or off.
    pub fn show_flat_view(&mut self, solid: Option<BodyKey>) {
        self.model.show_flat(None);
        self.model.flat_view = None;
        let Some(key) = solid else {
            return;
        };
        let Some(revision) = self.revision_of(&key.0) else {
            return;
        };
        self.model.flat_view = Some(crate::core::flat::FlatView {
            part: key.0.clone(),
            solid: key.1.clone(),
            revision: revision.clone(),
            lines: None,
            error: None,
            turned: false,
        });
        self.read_flat_view();
    }

    /// Reads the flat view's bend lines and body again, at its part's
    /// revision.
    pub(super) fn read_flat_view(&mut self) {
        let Some(v) = &self.model.flat_view else {
            return;
        };
        let key = (v.part.clone(), v.solid.clone());
        let revision = v.revision.clone();
        self.ask_flat_lines(&key);
        let req = Request::MeshGet {
            part: key.0.clone(),
            feature: None,
            features: Vec::new(),
            quality: None,
            transfer: Some(Transfer::Compact),
            rev: self.model_rev(),
            flat: Some(key.1.clone()),
            before: None,
        };
        self.send(req, Pending::Model(model::Asked::FlatMesh(key, revision)));
    }

    /// Turns the camera to the flat view, once its body and lines have
    /// come: along the pattern's `z`, from its viewer side (where its bend
    /// lines lie), as `render --flat` looks at it; the view frames it.
    pub(super) fn turn_to_flat(&mut self) {
        let Some(v) = self.model.flat_view.as_mut().filter(|v| !v.turned) else {
            return;
        };
        let (Some(l), Some((_, body))) = (&v.lines, &self.model.flat) else {
            return;
        };
        let points: Vec<[f64; 3]> = match &body.mesh {
            Ok(m) => m
                .positions
                .iter()
                .map(|p| [0, 1, 2].map(|k| m.origin[k] + f64::from(p[k])))
                .collect(),
            Err(_) => l.lines.iter().flat_map(|b| [b.from, b.to]).collect(),
        };
        v.turned = true;
        self.view.looked = Some(l.camera(&points, 1.0));
    }

    /// The solid View > Flat pattern lays flat: the chosen feature's
    /// solid, else the one part shown (not an assembly), its one sheet
    /// metal solid; `None` where neither is.
    pub fn flat_candidate(&self, chosen: Option<&BodyKey>) -> Option<BodyKey> {
        let shown = self.model.shown();
        if let Some(k) = chosen
            && shown.contains(&k.0.as_str())
            && let Some(s) = self.model.solid_of(k)
            && self.model.parts.get(&k.0).is_some_and(|p| !p.assembly)
        {
            return Some(s);
        }
        match shown.as_slice() {
            [one] if self.model.parts.get(*one).is_some_and(|p| !p.assembly) => {
                Some(((*one).to_owned(), String::new()))
            }
            _ => None,
        }
    }

    /// Reads what a sweep or loft that built is built from, to draw it;
    /// anything else draws nothing of the kind.
    pub(super) fn ask_swept(&mut self, key: &BodyKey) {
        let Some(p) = self.model.parts.get(&key.0) else {
            return;
        };
        self.model.swept_at = Some(p.revision.clone());
        let Some(f) = p.feature(&key.1) else {
            return;
        };
        if !crate::core::sweeps::drawn(f.ty.as_deref()) || f.error.is_some() {
            return;
        }
        let revision = p.revision.clone();
        let req = Request::SweepGet {
            part: key.0.clone(),
            feature: key.1.clone(),
            quality: None,
            rev: self.model_rev(),
        };
        self.send(
            req,
            Pending::Model(model::Asked::Sweep(key.clone(), revision)),
        );
    }

    /// Reads the faces `key` made, unless it is a sketch: it makes none. A
    /// mate's are the faces its connectors are on, known already.
    /// What a chosen delete ghosts: each solid it takes out, meshed as it
    /// read them.
    fn ask_deleted(&mut self, key: &BodyKey) {
        let Some((d, asked)) = self
            .model
            .parts
            .get(&key.0)
            .and_then(|p| crate::core::inserts::Deleted::of(p, &key.1))
        else {
            return;
        };
        self.model.deleted = Some(d);
        for (part, solid) in asked {
            let req = Request::MeshGet {
                part,
                feature: Some(solid),
                features: Vec::new(),
                quality: None,
                transfer: Some(Transfer::Compact),
                rev: self.model_rev(),
                flat: None,
                before: Some(key.1.clone()),
            };
            self.send(req, Pending::Model(model::Asked::Deleted(key.clone())));
        }
    }

    pub(super) fn ask_made(&mut self, key: BodyKey) {
        let Some(p) = self.model.parts.get(&key.0) else {
            return;
        };
        if let Some(m) = p.feature(&key.1).and_then(|f| f.mate.as_ref()) {
            let mut faces: std::collections::BTreeMap<BodyKey, std::collections::BTreeSet<String>> =
                std::collections::BTreeMap::new();
            for c in [&m.a, &m.b] {
                if let Some((body, name)) = crate::core::model::MateInfo::face_of(c) {
                    faces.entry((key.0.clone(), body)).or_default().insert(name);
                }
            }
            self.model.made = Some(Made {
                feature: key.clone(),
                revision: p.revision.clone(),
                faces,
                ..Made::default()
            });
            self.model.generation += 1;
            return;
        }
        // An insert paints its solids whole, a delete ghosts what it took
        // out: neither reads the model's faces by origin.
        if p.feature(&key.1)
            .is_none_or(|f| f.sketch.is_some() || f.insert.is_some() || f.delete.is_some())
        {
            return;
        }
        let revision = p.revision.clone();
        // A tweak makes nothing an origin names, and a direct edit's
        // healed hosts keep their makers': what each modified and made
        // comes from inspect of the feature itself.
        let tweak = p.feature(&key.1).is_some_and(|f| f.tweak.is_some());
        let req = Request::Inspect {
            part: key.0.clone(),
            feature: tweak.then(|| key.1.clone()),
            entity: None,
            rev: self.model_rev(),
        };
        self.send(req, Pending::Model(model::Asked::Made(key, revision)));
    }

    /// Fetches the drawings of `part`'s sketches, paths and helices that
    /// are missing or older than the part, one per request.
    pub(super) fn fetch_sketches(&mut self, part: &str) {
        for (key, revision) in self.model.ask_sketches(part) {
            let req = Request::SketchGet {
                part: key.0.clone(),
                sketch: key.1.clone(),
                quality: None,
                rev: self.model_rev(),
            };
            self.send(req, Pending::Model(model::Asked::Sketch(key, revision)));
        }
        for (key, revision) in self.model.ask_paths(part) {
            let req = Request::PathGet {
                part: key.0.clone(),
                path: key.1.clone(),
                quality: None,
                rev: self.model_rev(),
            };
            self.send(req, Pending::Model(model::Asked::Path(key, revision)));
        }
        if let Some(revision) = self.model.ask_holes(part) {
            let req = Request::HolesList {
                part: part.to_owned(),
                solid: None,
                csv: false,
                group: false,
                reference: None,
                rev: self.model_rev(),
            };
            self.send(
                req,
                Pending::Model(model::Asked::Holes(part.to_owned(), revision)),
            );
        }
    }

    /// Fetches display meshes, one body per request (an assembly's by the
    /// batch), at [`Model::fetch_level`]: a part with one changed feature
    /// sends one mesh, and a body already asked for at its current hash
    /// isn't asked for again.
    pub(super) fn fetch(&mut self, bodies: Vec<BodyKey>) {
        // Coarse first in a large view: finer levels follow where they show.
        let level = self.model.fetch_level();
        let quality = (level != self.model.base).then(|| level.as_str().to_owned());
        let mut batches: BTreeMap<String, Vec<(BodyKey, String)>> = BTreeMap::new();
        for (key, hash) in self.model.ask(bodies) {
            self.model.fetched += 1;
            // An assembly's bodies come by occurrence, many to a request,
            // meshed side by side: an assembly may place 10,000.
            let assembly = self.model.parts.get(&key.0).is_some_and(|p| p.assembly);
            match key.1.rsplit_once('/') {
                Some(_) if assembly => {
                    batches.entry(key.0.clone()).or_default().push((key, hash));
                }
                _ => {
                    let req = Request::MeshGet {
                        part: key.0.clone(),
                        feature: Some(key.1.clone()),
                        features: Vec::new(),
                        quality: quality.clone(),
                        transfer: Some(Transfer::Compact),
                        rev: self.model_rev(),
                        flat: None,
                        before: None,
                    };
                    self.send(req, Pending::Model(model::Asked::Mesh(key, hash)));
                }
            }
        }
        for (part, asked) in batches {
            for batch in asked.chunks(MESH_BATCH) {
                let features = batch
                    .iter()
                    .filter_map(|(k, _)| Some(k.1.rsplit_once('/')?.0.to_owned()))
                    .collect();
                let req = Request::MeshGet {
                    part: part.clone(),
                    feature: None,
                    features,
                    quality: quality.clone(),
                    transfer: Some(Transfer::Compact),
                    rev: self.model_rev(),
                    flat: None,
                    before: None,
                };
                self.send(req, Pending::Model(model::Asked::Meshes(batch.to_vec())));
            }
        }
    }

    /// Fetches bodies' meshes at the levels of detail the view wants,
    /// in the order given (nearest first), each body and level once.
    pub fn fetch_levels(&mut self, wanted: Vec<(BodyKey, ryucad::mesh::Quality)>) {
        for (key, hash, level) in self.model.ask_levels(wanted) {
            self.model.fetched += 1;
            let assembly = self.model.parts.get(&key.0).is_some_and(|p| p.assembly);
            let feature = match key.1.rsplit_once('/') {
                Some((path, _)) if assembly => path.to_owned(),
                _ => key.1.clone(),
            };
            let req = Request::MeshGet {
                part: key.0.clone(),
                feature: Some(feature),
                features: Vec::new(),
                quality: Some(level.as_str().to_owned()),
                transfer: Some(Transfer::Compact),
                rev: self.model_rev(),
                flat: None,
                before: None,
            };
            self.send(req, Pending::Model(model::Asked::Level(hash, level)));
        }
    }

    /// The part `id`, if the window knows it.
    pub fn part(&self, id: &str) -> Option<&Part> {
        self.model.parts.get(id)
    }

    /// Asks for `id`'s report unless it is loaded or asked for: when its
    /// row in the tree opens, the window shows it, or something in it is
    /// chosen.
    pub fn load_part(&mut self, id: &str) {
        if !self.model.parts.contains_key(id) {
            self.ask_part(id);
        }
    }

    /// Notes, while the tree draws, that `id` should be loaded.
    pub fn want(&self, id: &str) {
        if !self.model.parts.contains_key(id) {
            self.model
                .wanted
                .lock()
                .expect("never poisoned")
                .insert(id.to_owned());
        }
    }

    /// Loads what the tree wanted as it drew.
    pub fn load_wanted(&mut self) {
        let wanted = std::mem::take(&mut *self.model.wanted.lock().expect("never poisoned"));
        for id in wanted {
            self.load_part(&id);
        }
    }

    /// Asks for `id`'s report again, unless it is being asked for.
    pub(super) fn ask_part(&mut self, id: &str) {
        if !self.model.loading.insert(id.to_owned()) {
            return;
        }
        let rev = self.model_rev();
        self.model.asked_at.insert(id.to_owned(), rev.clone());
        let req = Request::Rebuild {
            part: Some(id.to_owned()),
            rev: rev.clone(),
        };
        self.send(
            req,
            Pending::Model(model::Asked::Rebuild(id.to_owned(), rev)),
        );
    }
}

/// What one of this sub-state's requests was for (design 541,
/// "Typed handles"): `Workspace::answer` routes its answer here.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Asked {
    Info,
    Parts,
    /// A part's report, asked for at this revision (the draft, or the
    /// working files).
    Rebuild(String, Option<String>),
    /// A body's mesh, asked for at this hash.
    Mesh(BodyKey, String),
    /// Several bodies of one assembly's occurrences, each at its hash.
    Meshes(Vec<(BodyKey, String)>),
    /// A body's mesh at another level of detail.
    Level(String, ryucad::mesh::Quality),
    /// A sketch's drawing, asked for at this part revision.
    Sketch(BodyKey, String),
    /// A path's or helix's drawing, asked for at this part revision.
    Path(BodyKey, String),
    /// A part's holes, at its revision.
    Holes(String, String),
    /// The chosen sweep's or loft's drawing, asked for at this part
    /// revision.
    Sweep(BodyKey, String),
    /// A chosen delete's ghost: the delete it was asked for.
    Deleted(BodyKey),
    /// The model's solids, read for the faces a feature made, at this
    /// part revision.
    Made(BodyKey, String),
    /// A sheet metal solid's table (`sheet.get`): its part and the solid
    /// asked for, at a part revision.
    SheetTable(BodyKey, String),
    /// A sheet metal solid's flat bend lines (`flat.get`), likewise.
    FlatLines(BodyKey, String),
    /// The flat view's body (`mesh.get` with `flat`), likewise.
    FlatMesh(BodyKey, String),
}

impl Workspace {
    pub(super) fn answer_model(&mut self, why: Asked, answer: Result<Answer>) {
        match (why, answer) {
            (Asked::Info, Ok(a)) => {
                self.model.default_unit = a.result["default_unit"].as_str().unwrap_or("mm").into();
            }
            (Asked::Parts, Ok(a)) => {
                let listed: BTreeMap<String, Listed> = a.result["parts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| Some((p["part"].as_str()?.to_owned(), Listed::from_json(p))))
                    .collect();
                // Parts deleted while the window wasn't listening go.
                let gone: Vec<String> = self
                    .model
                    .parts
                    .keys()
                    .filter(|id| !listed.contains_key(*id))
                    .cloned()
                    .collect();
                for id in gone {
                    self.model.remove_part(&id);
                }
                self.model.model_parts = a.result["model"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p.as_str().map(str::to_owned))
                    .collect();
                // A project of a few parts loads whole; a larger one, what
                // the window shows, and the rest as its tree asks.
                let whole = listed.len() <= LOAD_WHOLE;
                let shown: Vec<String> = self.model.shown().iter().map(|s| s.to_string()).collect();
                for (id, l) in &listed {
                    match &l.error {
                        Some(e) => {
                            self.model.apply_part(
                                &json!({"part": id, "revision": l.revision, "error": e}),
                            );
                        }
                        None if whole
                            || shown.contains(id)
                            || self.model.parts.contains_key(id) =>
                        {
                            self.ask_part(id);
                        }
                        None => {}
                    }
                }
                self.model.listed = listed;
                self.model.generation += 1;
            }
            (Asked::Rebuild(id, rev), r) => {
                // Asked at a revision the window no longer draws: its
                // answer is dropped, and the part asked for at the one it
                // draws unless that is on its way already (a part not yet
                // loaded when the revision moved isn't asked again then).
                let now = self.model_rev();
                if rev != now {
                    if self.model.asked_at.get(&id) != Some(&now) {
                        self.model.loading.remove(&id);
                        self.ask_part(&id);
                    }
                    return;
                }
                self.model.loading.remove(&id);
                self.model.asked_at.remove(&id);
                // A failed feature fails the rebuild; the report is in the details.
                let report = match r {
                    Ok(a) => a.result["parts"][0].clone(),
                    Err(e) if e.code == Code::RebuildFailed => e.details["parts"][0].clone(),
                    Err(e) => json!({"part": id, "error": e}),
                };
                self.part_changed(&report);
            }
            // A delete chosen before this one answers too late to matter:
            // its ghosts and its failures alike are dropped.
            (Asked::Deleted(key), _)
                if !self.model.deleted.as_ref().is_some_and(|d| d.is_for(&key)) => {}
            (Asked::Deleted(key), a) => {
                match a.map(|a| a.binary.map(|b| ryucad::mesh::decode(&b))) {
                    Ok(Some(Ok(bodies))) => {
                        for b in bodies {
                            if self.model.deleted.as_mut().is_some_and(|d| d.take(&key, b)) {
                                self.model.generation += 1;
                            }
                        }
                    }
                    Ok(Some(Err(e))) | Err(e) => self.fail(e),
                    Ok(None) => {
                        self.fail(Error::new(Code::Daemon, "a mesh came without its bytes"));
                    }
                }
            }
            (Asked::Sketch(key, revision), a) => {
                self.model.sketch_answered(&key, &revision);
                match a {
                    Ok(a) => {
                        self.model
                            .take_sketch(&key, crate::core::sketch::Drawing::from_get(&a.result));
                    }
                    Err(e) => self.fail(e),
                }
            }
            (Asked::Path(key, revision), a) => {
                self.model.path_answered(&key, &revision);
                match a {
                    Ok(a) => {
                        self.model
                            .take_path(&key, crate::core::datums::PathDrawing::from_get(&a.result));
                    }
                    Err(e) => self.fail(e),
                }
            }
            // A part that doesn't rebuild has no holes to show.
            (Asked::Holes(part, revision), a) => {
                self.model.holes_answered(&part, &revision);
                let mut t = match a {
                    Ok(a) => crate::core::threads::HoleTable::from_list(&a.result),
                    Err(_) => crate::core::threads::HoleTable::default(),
                };
                t.revision = revision;
                self.model.take_holes(&part, t);
            }
            (Asked::Sweep(key, _), a) => {
                if self.model.made_for.as_ref() == Some(&key) {
                    match a {
                        Ok(a) => {
                            self.model.swept =
                                Some(crate::core::sweeps::SweepDrawing::from_get(key, &a.result));
                        }
                        Err(e) => self.fail(e),
                    }
                    self.model.generation += 1;
                }
            }
            (Asked::SheetTable(key, revision), a) => {
                use crate::core::sheet::{SheetTable, Shown};
                let t = match a {
                    Ok(a) => Shown::Table(SheetTable::from_answer(&revision, &a.result)),
                    Err(e) => Shown::Failed(e),
                };
                self.model.sheets.take_table(key, revision, t);
                self.model.generation += 1;
            }
            (Asked::FlatLines(key, revision), a) => {
                let f = a.map(|a| crate::core::flat::FlatLines::from_answer(&a.result));
                if let Some(v) =
                    self.model.flat_view.as_mut().filter(|v| {
                        (v.part.clone(), v.solid.clone()) == key && v.revision == revision
                    })
                {
                    match &f {
                        Ok(l) => v.lines = Some(l.clone()),
                        Err(e) => {
                            // The flat no longer reads: draw the folded
                            // body again, not the last revision's flat.
                            v.lines = None;
                            v.error = Some(e.clone());
                            self.model.show_flat(None);
                        }
                    }
                }
                self.model.sheets.take_flat(key, revision, f);
                self.turn_to_flat();
                self.model.generation += 1;
            }
            (Asked::FlatMesh(key, revision), a) => {
                let wanted = self.model.flat_view.as_ref().is_some_and(|v| {
                    (v.part.clone(), v.solid.clone()) == key && v.revision == revision
                });
                if !wanted {
                    return;
                }
                match a.map(|a| a.binary.map(|b| ryucad::mesh::decode(&b))) {
                    Ok(Some(Ok(mut bodies))) if bodies.len() == 1 => {
                        let body = bodies.remove(0);
                        // The solid it is, named now the daemon has chosen it.
                        let solid = (key.0.clone(), body.feature.clone());
                        if let Some(v) = &mut self.model.flat_view {
                            v.solid.clone_from(&body.feature);
                        }
                        self.model.show_flat(Some((solid, body)));
                        self.turn_to_flat();
                    }
                    Ok(Some(Err(e))) | Err(e) => {
                        // Refused (an edit left the solid unfolding no
                        // more): the folded body is drawn again, and the
                        // view keeps the refusal, not the stale flat.
                        if let Some(v) = &mut self.model.flat_view {
                            v.lines = None;
                            v.error = Some(e.clone());
                        }
                        self.model.show_flat(None);
                        self.fail(e);
                    }
                    Ok(_) => {
                        self.model.show_flat(None);
                        self.fail(Error::new(Code::Daemon, "a flat came without its body"));
                    }
                }
            }
            (Asked::Made(key, revision), a) => {
                if self.model.made_for.as_ref() == Some(&key) {
                    match a {
                        Ok(a) => {
                            self.model.made = Some(Made::from_inspect(&key, &revision, &a.result))
                        }
                        Err(e) => self.fail(e),
                    }
                    self.model.generation += 1;
                }
            }
            (Asked::Level(hash, level), a) => {
                let bodies = match a.map(|a| a.binary.map(|b| ryucad::mesh::decode(&b))) {
                    Ok(Some(Ok(bodies))) => bodies,
                    Ok(Some(Err(e))) | Err(e) => {
                        self.fail(e);
                        Vec::new()
                    }
                    Ok(None) => Vec::new(),
                };
                let mut taken = false;
                for b in bodies.into_iter().filter(|b| b.hash == hash) {
                    taken |= self.model.take_level(b, level);
                }
                if !taken {
                    self.model.levels_asked.remove(&(hash, level));
                }
            }
            (Asked::Mesh(key, hash), a) => {
                self.answered_meshes(vec![(key, hash)], a);
            }
            (Asked::Meshes(asked), a) => self.answered_meshes(asked, a),
            (_, Err(e)) => self.fail(e),
        }
    }
}
