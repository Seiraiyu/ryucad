//! The viewport: the 3D view with navigation, hover and picking. A click
//! sets this window's selection through the daemon; everyone's selections
//! and highlights are painted over the model in their colours.

use crate::core::compare::FaceMark;
use crate::core::cube::{Cube, Hit as CubeHit};
use crate::core::model::BodyKey;
use crate::core::nav::{Buttons, Gesture, Modifiers, Nav, Projector};
use crate::core::pick::{Hit, Picker, Place};
use crate::core::workspace::Workspace;
use crate::ui::scene::{self, DrawList, PaintedFace, Scene, linear};
use crate::ui::style::{self, Rgb};
use eframe::egui::{self, PointerButton, Sense};
use eframe::{egui_wgpu, wgpu};
use ryucad::live::SelectMode;
use ryucad::mesh::WireBody;
use ryucad::sketch_view::ConstraintState;
use ryucad::view::{self, Camera, Projection, StandardView};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A mate frame's name (`a` or `b`) and where its origin is.
pub type FrameName = (String, [f64; 3]);

/// What the window's viewport keeps between frames.
pub struct View {
    pub nav: Nav,
    /// Draw B-rep edges (`display.edges`).
    pub edges: bool,
    /// Draw sketches (`display.sketches`).
    pub sketches: bool,
    /// Draw the navigation cube (`display.nav_cube`), the axis triad
    /// (`display.triad`) and the origin's axes and planes
    /// (`display.origin`).
    pub cube: bool,
    pub triad: bool,
    pub origin: bool,
    /// The spacebar view selector, while open.
    pub selector: Option<crate::core::views::Selector>,
    /// What of the cube the pointer is over.
    pub cube_hover: Option<CubeHit>,
    /// The frame's time in seconds, for turns.
    now: f64,
    /// The window's look (`ui.style`), for the background.
    pub look: crate::ui::style::Look,
    /// The feature chosen in the tree: its body is painted.
    pub chosen: Option<BodyKey>,
    /// Entities a merge conflict concerns, shown from the conflict panel.
    pub marked: Vec<String>,
    pub hover: Option<Hit>,
    /// Where the viewport was last drawn, in points.
    pub rect: egui::Rect,
    /// The viewport was under the pointer when last drawn.
    hovered: bool,
    /// The viewport's commands whose keys were pressed over it, taken
    /// from the frame's input ([`View::take_keys`]).
    tabs: Vec<&'static str>,
    /// The viewport's commands' keys (`view.hide_under`, `view.show_under`,
    /// `view.show_all`), from the window's keymap.
    pub keys: Vec<(egui::KeyboardShortcut, &'static str)>,
    /// The window's commands as this frame shows them, for the context
    /// menu.
    pub commands: crate::ui::commands::Shown,
    /// A command a context menu chose, with its target: the window runs
    /// it once the frame is drawn.
    pub ran: Option<(&'static str, Option<String>)>,
    picker: Option<Picker>,
    /// The model generation and whether sketches were shown, when the
    /// picker was made.
    picked_at: (u64, bool),
    /// The point vertex positions are relative to on the GPU.
    reference: Option<[f64; 3]>,
    scene: Option<Scene>,
    texture: Option<egui::TextureId>,
    /// The camera has framed the whole model once: loading finished, or
    /// the person moved the camera first.
    framed: bool,
    /// What the camera last framed while the model loaded.
    framing: Option<([f64; 3], [f64; 3])>,
    /// Each body's few points that frame it as the camera looks.
    framers: BTreeMap<BodyKey, Framers>,
    pivot: Option<[f64; 3]>,
    /// What a Shift+two-finger orbit on a trackpad turns about, from when
    /// the fingers go down until they lift.
    finger_pivot: Option<[f64; 3]>,
    /// What the context menu was opened on.
    menu: Option<Hit>,
    /// An instance being dragged: its assembly, the instance, and the
    /// point grabbed, which follows the pointer across the plane through
    /// it facing the camera.
    grab: Option<(String, String, [f64; 3])>,
    /// A handle being dragged: a section plane's, or a new explode step's.
    handle: Option<Handle>,
}

/// What a handle drag moves.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Handle {
    Section,
    Step,
}

/// A body's points that frame it from one direction
/// (`view::framing_points`): a few hundred of its mesh's thousands.
struct Framers {
    hash: String,
    pose: crate::core::model::Pose,
    /// The direction, unit back (towards the eye) and up.
    back: [f64; 3],
    up: [f64; 3],
    points: Vec<[f64; 3]>,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn unit(a: [f64; 3]) -> [f64; 3] {
    let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    a.map(|x| x / n)
}

/// Whether unit `a` and `b` point the same way, but for rounding.
fn same_way(a: [f64; 3], b: [f64; 3]) -> bool {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] > 1.0 - 1e-12
}

impl Default for View {
    fn default() -> View {
        View::new()
    }
}

/// An entity's kind and where it comes from, for people: "face of boss
/// (cylinder)", of an assembly's occurrence "face of p/base (plate, box)",
/// or for a face picked in a compare's ghost, "face of boss
/// at HEAD, gone".
pub fn describe(ws: &Workspace, hit: &Hit) -> String {
    if let Some(rev) = &hit.rev {
        return format!("{} of {} at {rev}, gone", hit.kind.as_str(), hit.feature);
    }
    let from = ws.model.source(&hit.part, &hit.feature);
    if from.is_empty() {
        return format!("{} of {}", hit.kind.as_str(), hit.feature);
    }
    format!("{} of {} ({from})", hit.kind.as_str(), hit.feature)
}

impl View {
    /// Takes presses of the viewport's commands' keys (Tab and its kin)
    /// over the viewport out of a frame's input before egui sees them, so
    /// they hide and show (in [`View::show`]) rather than move keyboard
    /// focus through the menu bar (issue #136). A text field being typed
    /// in keeps its keys.
    pub fn take_keys(&mut self, ctx: &egui::Context, events: &mut Vec<egui::Event>) {
        if !self.hovered || ctx.text_edit_focused() {
            return;
        }
        let rect = self.rect;
        let mut at = ctx.input(|i| i.pointer.latest_pos());
        let (tabs, keys) = (&mut self.tabs, &self.keys);
        events.retain(|e| match e {
            egui::Event::PointerMoved(p) => {
                at = Some(*p);
                true
            }
            egui::Event::PointerGone => {
                at = None;
                true
            }
            egui::Event::Key {
                key,
                pressed,
                modifiers,
                ..
            } if at.is_some_and(|p| rect.contains(p)) => {
                // The most modified chord a press matches is its command.
                // Ctrl counts as Mod too, as macOS sends Ctrl+Shift+Tab
                // without Command (#142).
                let as_mod = egui::Modifiers {
                    command: modifiers.command || modifiers.ctrl,
                    ..*modifiers
                };
                let mut matched: Vec<&(egui::KeyboardShortcut, &'static str)> = keys
                    .iter()
                    .filter(|(s, _)| {
                        s.logical_key == *key
                            && (modifiers.matches_exact(s.modifiers)
                                || as_mod.matches_exact(s.modifiers))
                    })
                    .collect();
                matched.sort_by_key(|(s, _)| std::cmp::Reverse(s.modifiers.any() as u8));
                match matched.first() {
                    Some((_, id)) => {
                        if *pressed {
                            tabs.push(id);
                        }
                        false
                    }
                    None => true,
                }
            }
            _ => true,
        });
    }

    pub fn new() -> View {
        let camera = view::frame(
            [-0.05; 3],
            [0.05; 3],
            StandardView::Iso,
            view::DEFAULT_PROJECTION,
        );
        View {
            nav: Nav::new(camera),
            edges: true,
            sketches: true,
            cube: true,
            triad: true,
            origin: false,
            selector: None,
            cube_hover: None,
            now: 0.0,
            look: crate::ui::style::Look::default(),
            chosen: None,
            marked: Vec::new(),
            hover: None,
            rect: egui::Rect::NOTHING,
            hovered: false,
            tabs: Vec::new(),
            keys: Vec::new(),
            commands: Default::default(),
            ran: None,
            picker: None,
            picked_at: (0, true),
            reference: None,
            scene: None,
            texture: None,
            framed: false,
            framing: None,
            framers: BTreeMap::new(),
            pivot: None,
            finger_pivot: None,
            menu: None,
            grab: None,
            handle: None,
        }
    }

    /// How long handles are drawn: a tenth of the model's diagonal.
    pub fn handle_length(&mut self, ws: &Workspace) -> f64 {
        self.bounds(ws).map_or(0.01, |(lo, hi)| {
            let d = (0..3)
                .map(|k| (hi[k] - lo[k]) * (hi[k] - lo[k]))
                .sum::<f64>()
                .sqrt();
            (d / 10.0).max(1e-4)
        })
    }

    /// The model prepared for picking, with a compare's ghost, rebuilt
    /// when either changes or sketches are shown or hidden.
    pub fn picker(&mut self, ws: &Workspace) -> &Picker {
        let wanted = (ws.model.generation, self.sketches);
        if self.picker.is_none() || self.picked_at != wanted {
            self.picker = Some(
                Picker::of(&ws.model, self.sketches)
                    .with_compare(ws.compare.as_ref())
                    .with_sections(&ws.sections.planes),
            );
            self.picked_at = wanted;
        }
        self.picker.as_ref().expect("just made")
    }

    /// The box around the model, if it has any meshes.
    pub fn bounds(&mut self, ws: &Workspace) -> Option<([f64; 3], [f64; 3])> {
        self.picker(ws).bounds()
    }

    /// The projection on a viewport of `aspect`.
    pub fn projector(&mut self, ws: &Workspace, aspect: f64) -> Projector {
        let (lo, hi) = self.bounds(ws).unwrap_or(([-0.05; 3], [0.05; 3]));
        let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
        let d = [0, 1, 2].map(|k| hi[k] - lo[k]);
        let radius = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / 2.0;
        Projector::new(&self.nav.camera, aspect, centre, radius)
    }

    /// Width over height of the viewport (1 before it is laid out).
    fn aspect(&self) -> f64 {
        let (w, h) = (self.rect.width(), self.rect.height());
        if w > 0.0 && h > 0.0 {
            f64::from(w / h)
        } else {
            1.0
        }
    }

    /// Turns to look from a standard view at the whole model, and tells
    /// the session as it goes.
    pub fn standard(&mut self, ws: &mut Workspace, v: StandardView) {
        let points = self.picker(ws).points(None);
        if !points.is_empty() {
            let to = view::frame_points(&points, v, self.nav.camera.projection, self.aspect());
            self.turn(ws, to);
        }
    }

    /// Starts turning the camera to `to` over [`crate::core::nav::TURN_SECONDS`]:
    /// following stops, and the session hears of each step (see `show`).
    pub fn turn(&mut self, ws: &mut Workspace, to: Camera) {
        self.framed = true;
        ws.following = None;
        self.nav.turn_to(to, self.now);
    }

    /// The navigation cube where this viewport draws it.
    pub fn cube_widget(&self) -> Cube {
        Cube::at([self.rect.max.x, self.rect.min.y])
    }

    /// Whether the cube's widget takes the point `p`, so the model under
    /// it is never picked.
    fn on_cube(&self, p: egui::Pos2) -> bool {
        if !self.cube {
            return false;
        }
        let (lo, hi) = self.cube_widget().bounds();
        (lo[0]..=hi[0]).contains(&p.x) && (lo[1]..=hi[1]).contains(&p.y)
    }

    /// A click on the cube: a face, edge or corner turns to its view
    /// fitting the model; an arrow turns or rolls a quarter turn.
    pub fn cube_click(&mut self, ws: &mut Workspace, hit: CubeHit) {
        match hit {
            CubeHit::View(v) => self.standard(ws, v),
            CubeHit::Roll(_) | CubeHit::Turn { .. } => {
                if let Some(to) = crate::core::cube::turned(&self.nav.camera, hit) {
                    self.turn(ws, to);
                }
            }
            CubeHit::Menu => {}
        }
    }

    /// Turns to the named view `id` of the part shown, as it was saved.
    pub fn restore_view(&mut self, ws: &mut Workspace, id: &str) {
        if let Some(c) = ws.views.camera(id).cloned() {
            self.turn(ws, c);
        }
    }

    /// The face this window has selected, if it has one face selected.
    pub fn selected_face(&mut self, ws: &Workspace) -> Option<String> {
        let selection = ws.myself()?.selection.clone();
        let [one] = selection.as_slice() else {
            return None;
        };
        let face = ryucad::entity::Target::Body(ryucad::entity::EntityKind::Face);
        (self.picker(ws).kind(one) == Some(face)).then(|| one.clone())
    }

    /// Asks for the view normal to the selected face; the view turns to
    /// it when the answer comes (see `show`).
    pub fn normal_to(&mut self, ws: &mut Workspace) {
        if let Some(face) = self.selected_face(ws) {
            ws.look_normal_to(&face);
        }
    }

    /// Switches between perspective and orthographic, keeping how big the
    /// model looks at the target.
    pub fn set_projection(&mut self, ws: &mut Workspace, orthographic: bool) {
        let c = &self.nav.camera;
        let (_, _, forward) = c.basis();
        let height = 2.0 * c.half_height(c.distance());
        let camera = match (c.projection, orthographic) {
            (Projection::Perspective { .. }, true) => Camera {
                projection: Projection::Orthographic { height },
                ..c.clone()
            },
            (Projection::Orthographic { .. }, false) => {
                let p = view::DEFAULT_PROJECTION;
                // Back as far as that field of view needs to see as much.
                let unit = Camera {
                    projection: p,
                    ..c.clone()
                }
                .half_height(1.0);
                let d = height / 2.0 / unit;
                Camera {
                    eye: [0, 1, 2].map(|k| c.target[k] - forward[k] * d),
                    projection: p,
                    ..c.clone()
                }
            }
            _ => return,
        };
        self.nav.camera = camera;
        self.moved(ws);
    }

    /// Opens the spacebar view selector at `at` (in points), or closes it.
    pub fn toggle_selector(&mut self, at: egui::Pos2) {
        self.selector = match self.selector {
            Some(_) => None,
            None => Some(crate::core::views::Selector::at([at.x, at.y])),
        };
    }

    /// The points that frame the whole model as the camera looks: each
    /// body's few, kept while its mesh and the camera's direction stay.
    /// While a model loads it is framed again as each batch of meshes
    /// comes: over all their points, each frame would cost more than the
    /// last.
    fn framing_points(&mut self, ws: &Workspace) -> Vec<[f64; 3]> {
        let c = &self.nav.camera;
        let (back, up) = (unit(sub(c.eye, c.target)), unit(c.up));
        let drawn: Vec<(&BodyKey, &Arc<WireBody>, crate::core::model::Pose)> =
            ws.model.drawn().collect();
        self.framers
            .retain(|k, _| drawn.iter().any(|(d, ..)| *d == k));
        let mut out = Vec::new();
        for (key, body, pose) in drawn {
            let kept = self.framers.get(key).is_some_and(|f| {
                f.hash == body.hash
                    && f.pose == pose
                    && same_way(f.back, back)
                    && same_way(f.up, up)
            });
            if !kept {
                let points = self.picker(ws).points(Some(key));
                let framers = Framers {
                    hash: body.hash.clone(),
                    pose,
                    back,
                    up,
                    points: view::framing_points(&points, back, up),
                };
                self.framers.insert(key.clone(), framers);
            }
            out.extend_from_slice(&self.framers[key].points);
        }
        // Sketches are few points: all of them.
        if self.sketches {
            out.extend(ws.model.drawn_sketches().flat_map(|(_, d)| d.points()));
        }
        out
    }

    /// Drops the feature chosen in the tree, and the form it opened: the
    /// tree and the property panel follow what was picked or cleared, so
    /// they never show a feature nothing points at any more.
    pub fn unchoose(&mut self, ws: &mut Workspace) {
        if self.chosen.take().is_some() {
            self.marked.clear();
            ws.open_form(None);
            ws.choose(None);
        }
    }

    /// Whether `entity` (`<part>/<solid>/<name>`) lies on the solid that
    /// shows the feature chosen in the tree.
    fn on_chosen(&self, ws: &Workspace, entity: &str) -> bool {
        let Some(k) = &self.chosen else {
            return false;
        };
        let solid = ws.model.solid_of(k).unwrap_or_else(|| k.clone());
        let mut parts = entity.splitn(3, '/');
        parts.next() == Some(solid.0.as_str()) && parts.next() == Some(solid.1.as_str())
    }

    /// Frames the whole model, or one body.
    pub fn fit(&mut self, ws: &mut Workspace, body: Option<&BodyKey>) {
        let points = match body {
            None => self.framing_points(ws),
            Some(_) => self.picker(ws).points(body),
        };
        if !points.is_empty() {
            self.nav.fit(&points, self.aspect());
            self.moved(ws);
        }
    }

    /// Frames `bodies` together.
    pub fn fit_all(&mut self, ws: &mut Workspace, bodies: &[BodyKey]) {
        let picker = self.picker(ws);
        let points: Vec<[f64; 3]> = bodies.iter().flat_map(|b| picker.points(Some(b))).collect();
        if !points.is_empty() {
            self.nav.fit(&points, self.aspect());
            self.moved(ws);
        }
    }

    /// Frames the given entities, occurrences (`<asm>/<path>`) whole.
    pub fn fit_entities(&mut self, ws: &mut Workspace, entities: &[String]) {
        let picker = self.picker(ws);
        let mut points = picker.points_of(entities);
        for e in entities {
            for key in ws.model.occurrence_bodies(e) {
                points.extend(picker.points(Some(&key)));
            }
        }
        if !points.is_empty() {
            self.nav.fit(&points, self.aspect());
            self.moved(ws);
        }
    }

    /// Frames `points` (a clearance's nearest points), as Zoom to fit does.
    pub fn fit_points(&mut self, ws: &mut Workspace, points: &[[f64; 3]]) {
        if !points.is_empty() {
            self.nav.fit(points, self.aspect());
            self.moved(ws);
        }
    }

    /// The camera moved by this window's hand: following stops, and the
    /// session hears of it.
    fn moved(&mut self, ws: &mut Workspace) {
        // The person's camera from now on, loaded or not.
        self.framed = true;
        ws.following = None;
        ws.share_camera(&self.nav.camera);
    }

    /// The point vertices are drawn relative to: kept while the model stays
    /// near it, so a change to one body doesn't upload them all.
    fn reference(&mut self, ws: &Workspace) -> [f64; 3] {
        let Some((lo, hi)) = self.bounds(ws) else {
            return self.reference.unwrap_or([0.0; 3]);
        };
        let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
        let size = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max).max(1e-3);
        let far = |r: [f64; 3]| (0..3).any(|k| (r[k] - centre[k]).abs() > 10.0 * size);
        if self.reference.is_none_or(far) {
            self.reference = Some(centre);
        }
        self.reference.expect("just set")
    }

    /// Everything painted over the model this frame, last on top: the
    /// chosen feature, a compare's new and changed faces, other people's
    /// selections, highlights, this window's selection, then the hover.
    fn painted(&mut self, ws: &Workspace) -> (Vec<PaintedFace>, Vec<(Place, Rgb)>) {
        let mut wanted: Vec<(String, Rgb)> = Vec::new();
        for p in ws.people.others(ws.me()) {
            let c = style::participant(&p.id);
            wanted.extend(p.selection.iter().map(|e| (e.clone(), c)));
        }
        for p in ws.people.all.values() {
            for h in &p.highlights {
                let c = style::highlight(h.style);
                wanted.extend(h.entities.iter().map(|e| (e.clone(), c)));
            }
        }
        let note = style::highlight(ryucad::live::Style::Note);
        wanted.extend(self.marked.iter().map(|e| (e.clone(), note)));
        if let Some(me) = ws.myself() {
            wanted.extend(me.selection.iter().map(|e| (e.clone(), style::SELECTED)));
        }
        if let Some(h) = &self.hover {
            wanted.push((h.entity.clone(), style::HOVER));
        }
        // A mate chosen in the tree: its connectors' frames.
        let (frame_lines, _) = self.mate_frames(ws);
        let chosen = self.chosen.clone();
        let chosen_sketch = chosen
            .as_ref()
            .filter(|_| self.sketches)
            .and_then(|k| ws.model.sketches.get(k));
        let picker = self.picker(ws);
        let mut faces = Vec::new();
        // What the chosen feature made, in the solids that show it.
        if let Some(made) = ws
            .made
            .as_ref()
            .filter(|m| chosen.as_ref() == Some(&m.feature))
        {
            let c = linear(style::CHOSEN);
            for ((part, solid), names) in &made.faces {
                for n in names {
                    if let Some(Place::Face(k, i)) = picker.place(&format!("{part}/{solid}/{n}")) {
                        faces.push((k, i, c));
                    }
                }
            }
        }
        // A compare's new and changed faces, over the chosen feature so
        // choosing one still shows what changed in it.
        if let Some(c) = &ws.compare {
            for key in &c.new_bodies {
                let new = linear(style::NEW);
                faces.extend((0..picker.faces(key)).map(|i| (key.clone(), i, new)));
            }
            for (key, marks) in &c.faces {
                for (name, m) in marks {
                    let r = format!("{}/{}/{name}", key.0, key.1);
                    if let Some(Place::Face(k, i)) = picker.place(&r) {
                        let colour = match m {
                            FaceMark::New => style::NEW,
                            FaceMark::Changed => style::CHANGED,
                        };
                        faces.push((k, i, linear(colour)));
                    }
                }
            }
        }
        // Interference found: the faces each pair's shared volume touches,
        // but not over a compare, whose red means removed (#138).
        if let Some(i) = ws.interference.as_ref().filter(|_| ws.compare.is_none()) {
            let removed = linear(style::REMOVED);
            for r in i.faces() {
                if let Some(Place::Face(k, f)) = picker.place(r) {
                    faces.push((k, f, removed));
                }
            }
        }
        let mut lines = frame_lines;
        // A sketch chosen in the tree: all of it.
        if let Some(d) = chosen_sketch {
            let c = style::CHOSEN;
            lines.extend(d.curves.iter().map(|k| (Place::Edge(k.points.clone()), c)));
            lines.extend(d.dots.iter().map(|p| (Place::Vertex(p.at), c)));
        }
        for (e, c) in wanted {
            match picker.place(&e) {
                Some(Place::Face(k, i)) => faces.push((k, i, linear(c))),
                Some(p) => lines.push((p, c)),
                // An occurrence: every face of what it places.
                None => {
                    for key in ws.model.occurrence_bodies(&e) {
                        faces.extend((0..picker.faces(&key)).map(|i| (key.clone(), i, linear(c))));
                    }
                }
            }
        }
        (faces, lines)
    }

    /// The frames of the mate chosen in the tree, as both its connectors
    /// sit in the assembly as solved: each frame's x, y and z axes as
    /// lines from its origin, a tenth of the model's size long, and its
    /// name (`a` or `b`) at its origin.
    pub fn mate_frames(&mut self, ws: &Workspace) -> (Vec<(Place, Rgb)>, Vec<FrameName>) {
        let frames = self.chosen.as_ref().and_then(|k| {
            ws.model
                .parts
                .get(&k.0)?
                .feature(&k.1)?
                .mate
                .as_ref()?
                .frames
        });
        let Some(frames) = frames else {
            return (Vec::new(), Vec::new());
        };
        let length = self
            .bounds(ws)
            .map_or(0.01, |(lo, hi)| {
                let d = [0, 1, 2].map(|k| hi[k] - lo[k]);
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / 10.0
            })
            .max(1e-3);
        let mut lines = Vec::new();
        let mut names = Vec::new();
        for (name, f) in ["a", "b"].iter().zip(frames) {
            for (end, c) in f.axes(length).into_iter().zip(style::AXES) {
                lines.push((Place::Edge(vec![f.origin, end]), c));
            }
            names.push(((*name).to_owned(), f.origin));
        }
        (lines, names)
    }

    /// Highlight labels and where they go: the middle of what each
    /// highlight covers, in its colour.
    fn labels(&mut self, ws: &Workspace) -> Vec<(String, [f64; 3], Rgb)> {
        let picker = self.picker(ws);
        let mut out = Vec::new();
        for p in ws.people.all.values() {
            for h in &p.highlights {
                let Some(text) = &h.label else { continue };
                if let Some((lo, hi)) = picker.bounds_of(&h.entities) {
                    let mid = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
                    out.push((text.clone(), mid, style::highlight(h.style)));
                }
            }
        }
        out
    }

    /// Sketches' dimensions as labels where they aren't hidden behind a
    /// solid, in their states' colours; clicking one opens its value in
    /// the property panel.
    fn dimension_labels(
        &mut self,
        ui: &mut egui::Ui,
        ws: &mut Workspace,
        rect: egui::Rect,
        pr: &Projector,
    ) {
        let size = [f64::from(rect.width()), f64::from(rect.height())];
        let open = ws
            .dimension
            .as_ref()
            .map(|f| (f.part.clone(), f.sketch.clone(), f.constraint.clone()));
        let mut chosen = None;
        let drawings: Vec<_> = ws
            .model
            .drawn_sketches()
            .map(|(k, d)| (k.clone(), std::sync::Arc::clone(d)))
            .collect();
        for (key, d) in drawings {
            for l in &d.labels {
                let Some(n) = pr.ndc(l.at) else { continue };
                let s = egui::pos2(
                    rect.min.x + ((n[0] + 1.0) / 2.0 * size[0]) as f32,
                    rect.min.y + ((1.0 - n[1]) / 2.0 * size[1]) as f32,
                );
                if !rect.contains(s) || self.picker(ws).hidden(pr, l.at) {
                    continue;
                }
                let me = (key.0.clone(), key.1.clone(), l.constraint.clone());
                let color = match (open.as_ref() == Some(&me), l.state) {
                    (true, _) => style::color32(style::SELECTED),
                    (_, ConstraintState::Ok) => ui.visuals().text_color(),
                    (_, ConstraintState::Redundant) => style::color32(ryucad::render::SKETCH_OVER),
                    (_, ConstraintState::Conflicting) => {
                        style::color32(ryucad::render::SKETCH_FAILING)
                    }
                };
                let id = ui.scope_id().with(("dimension", &key, &l.constraint));
                egui::Area::new(id)
                    .fixed_pos(s)
                    .pivot(egui::Align2::CENTER_CENTER)
                    .constrain_to(rect)
                    .order(egui::Order::Middle)
                    .show(ui.ctx(), |ui| {
                        let text = egui::RichText::new(&l.text).color(color).small();
                        let r = ui
                            .add(egui::Button::new(text).small())
                            .on_hover_text(format!("{} {} of {}", l.ty, l.constraint, key.1));
                        if r.clicked() {
                            chosen = Some(me.clone());
                        }
                    });
            }
        }
        if let Some(c) = chosen {
            ws.open_dimension(Some(c));
        }
    }

    /// Draws the viewport into `ui`'s remaining space and handles its
    /// input. `gpu` is the window's wgpu state (`None` with no adapter).
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        ws: &mut Workspace,
        gpu: Option<&egui_wgpu::RenderState>,
    ) {
        let rect = ui.available_rect_before_wrap();
        self.rect = rect;
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        // A turn to a view moves on, and the session hears of each step.
        self.now = ui.input(|i| i.time);
        if self.nav.turn.is_some() {
            if self.nav.step(self.now) {
                ui.ctx().request_repaint();
            }
            ws.share_camera(&self.nav.camera);
        }
        // A view normal to a face, as the daemon worked it out, framed
        // for this viewport.
        if let Some(c) = ws.looked.take() {
            let points = self.picker(ws).points(None);
            if !points.is_empty() {
                let to = view::refit_points(&c, &points, self.aspect());
                self.turn(ws, to);
            }
        }
        // Meshes arrive a body at a time: frame what has come so far, and
        // again as more comes, until it has all come.
        if !self.framed
            && let Some(b) = self.bounds(ws)
        {
            if self.framing != Some(b) {
                self.framing = Some(b);
                let points = self.framing_points(ws);
                self.nav.fit(&points, self.aspect());
                ws.share_camera(&self.nav.camera);
            }
            self.framed = ws.idle();
        }
        // Following, the window shows the other's camera, and shares it as
        // its own: what it shows.
        if let Some(c) = ws.followed_camera()
            && *c != self.nav.camera
        {
            self.nav.camera = c.clone();
            ws.share_camera(&self.nav.camera);
        }
        let aspect = f64::from(rect.width().max(1.0) / rect.height().max(1.0));
        let size = [f64::from(rect.width()), f64::from(rect.height())];
        let local = |p: egui::Pos2| [f64::from(p.x - rect.min.x), f64::from(p.y - rect.min.y)];
        // What is under the mouse: the cube's widget, else the model.
        let pr = self.projector(ws, aspect);
        let over_cube = response.hover_pos().is_some_and(|p| self.on_cube(p));
        self.cube_hover = match response.hover_pos() {
            Some(p) if over_cube && !response.dragged() => {
                self.cube_widget().hit(&self.nav.camera, [p.x, p.y])
            }
            _ => None,
        };
        self.hover = match response.hover_pos() {
            Some(p) if !response.dragged() && !over_cube => {
                self.picker(ws).pick(&pr, size, local(p))
            }
            _ => None,
        };
        // Navigation.
        let (buttons, modifiers, wheel, fingers, pinch) = ui.input(|i| {
            let b = Buttons {
                primary: i.pointer.button_down(PointerButton::Primary),
                secondary: i.pointer.button_down(PointerButton::Secondary),
                middle: i.pointer.button_down(PointerButton::Middle),
            };
            let m = Modifiers {
                shift: i.modifiers.shift,
                ctrl: i.modifiers.command,
            };
            (
                b,
                m,
                i.smooth_scroll_delta.y,
                Fingers::of(&i.raw.events),
                i.zoom_delta(),
            )
        });
        // Turn about the model under `at`; from the background, about what
        // is in the middle of the view (the target may be well off the
        // model after zooming towards a point), else the model's middle.
        let pivot = |view: &mut View, ws: &Workspace, at: Option<egui::Pos2>| {
            let middle = [size[0] / 2.0, size[1] / 2.0];
            let picked = at
                .and_then(|p| view.picker(ws).pick(&pr, size, local(p)))
                .or_else(|| view.picker(ws).pick(&pr, size, middle))
                .map(|h| h.point);
            picked.or_else(|| {
                view.bounds(ws)
                    .map(|(lo, hi)| [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0))
            })
        };
        let length = self.handle_length(ws);
        if response.drag_started() {
            // Where the button went down: a drag starts only once the
            // pointer has moved a little, maybe onto the model.
            let pressed = ui.input(|i| i.pointer.press_origin());
            self.pivot = pivot(self, ws, pressed.or(response.interact_pointer_pos()));
            // A handle under the press is dragged, before anything else.
            if buttons.primary
                && let Some(p) = pressed.filter(|p| !self.on_cube(*p))
            {
                let at = local(p);
                if ws
                    .explodes
                    .adding
                    .as_mut()
                    .is_some_and(|a| a.grab(&pr, size, at, length))
                {
                    self.handle = Some(Handle::Step);
                } else if ws.sections.handles && ws.sections.grab(&pr, size, at, length) {
                    self.handle = Some(Handle::Section);
                }
            }
            // A left-drag on an assembly's instance moves it, where the
            // left button doesn't navigate (in ryucad's own style, with
            // Ctrl/Cmd); one that can't move says why.
            let moves = buttons.primary
                && (self.nav.style != crate::core::nav::Navigation::Ryucad || modifiers.ctrl);
            let on = pressed
                .filter(|p| !self.on_cube(*p))
                .and_then(|p| self.picker(ws).pick(&pr, size, local(p)))
                .filter(|h| h.rev.is_none() && matches!(h.kind, ryucad::entity::Target::Body(_)));
            if moves
                && self.handle.is_none()
                && let Some(h) = on
            {
                let key = (h.part.clone(), h.feature.clone());
                match ws.model.movable(&key) {
                    Ok((part, instance)) if ws.model.parts[&part].assembly => {
                        ws.drag_note = None;
                        self.grab = Some((part, instance, h.point));
                    }
                    Ok(_) => {}
                    Err(why) if ws.model.parts.get(&h.part).is_some_and(|p| p.assembly) => {
                        ws.drag_note = Some(why);
                    }
                    Err(_) => {}
                }
            }
        }
        if response.dragged()
            && let Some(handle) = self.handle
            && let Some(p) = response.interact_pointer_pos()
        {
            match handle {
                Handle::Section => ws.sections.drag(&pr, size, local(p), length),
                Handle::Step => {
                    if let Some(a) = ws.explodes.adding.as_mut() {
                        a.drag(&pr, size, local(p), length);
                    }
                    ws.sync_explode();
                }
            }
            ui.ctx().request_repaint();
        } else if response.dragged()
            && let Some((part, instance, grabbed)) = self.grab.clone()
            && let Some(p) = response.interact_pointer_pos()
        {
            // Across the plane through the point grabbed, facing the eye.
            let (_, _, f) = pr.camera.basis();
            let l = local(p);
            let (o, d) = pr.ray([2.0 * l[0] / size[0] - 1.0, 1.0 - 2.0 * l[1] / size[1]]);
            let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
            let along = dot(d, f);
            if along.abs() > 1e-9 {
                let t = dot(sub(grabbed, o), f) / along;
                let q = [0, 1, 2].map(|k| o[k] + d[k] * t);
                ws.drag_to(&part, &instance, sub(q, grabbed));
            }
        } else if response.dragged() {
            let g = self.nav.style.gesture(buttons, modifiers);
            if g != Gesture::None {
                let d = response.drag_delta();
                self.nav
                    .drag(g, [d.x, d.y], rect.height().max(1.0), self.pivot);
                self.moved(ws);
            }
        }
        if response.drag_stopped() && self.grab.take().is_some() {
            ws.drop_drag();
        }
        // A handle let go writes what it moved.
        if response.drag_stopped() {
            match self.handle.take() {
                Some(Handle::Section) => {
                    if let Some(planes) = ws.sections.release() {
                        ws.set_sections(planes);
                    }
                }
                Some(Handle::Step) => ws.finish_step(),
                None => {}
            }
        }
        // Two fingers on a trackpad pan, or with Shift orbit; a wheel (and
        // a pinch) zooms.
        if response.hovered() && fingers.delta != egui::Vec2::ZERO {
            let height = rect.height().max(1.0);
            let d = [-fingers.delta.x, -fingers.delta.y];
            if fingers.shift {
                if fingers.start || self.finger_pivot.is_none() {
                    self.finger_pivot = pivot(self, ws, response.hover_pos());
                }
                self.nav.drag(Gesture::Orbit, d, height, self.finger_pivot);
            } else {
                // The model follows the fingers, as content does.
                self.nav.drag(Gesture::Pan, [-d[0], -d[1]], height, None);
            }
            self.moved(ws);
        }
        if fingers.end {
            self.finger_pivot = None;
        }
        let scroll = if fingers.any { 0.0 } else { wheel };
        let pinched = (pinch - 1.0).abs() > f32::EPSILON;
        if response.hovered() && (scroll != 0.0 || pinched) {
            // Towards the model under the mouse; over the background,
            // towards where the mouse is at the depth of the model's
            // middle (unless that is behind the eye: then the target).
            let toward = response.hover_pos().and_then(|p| {
                let p = local(p);
                let hit = self.picker(ws).pick(&pr, size, p).map(|h| h.point);
                hit.or_else(|| {
                    let (lo, hi) = self.bounds(ws)?;
                    let middle = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
                    pr.ndc(middle)?;
                    let ndc = [2.0 * p[0] / size[0] - 1.0, 1.0 - 2.0 * p[1] / size[1]];
                    Some(pr.at_depth(ndc, middle))
                })
            });
            if pinched {
                self.nav.pinch(pinch, toward);
            } else {
                self.nav.scroll(scroll, toward);
            }
            self.moved(ws);
        }
        // Picking.
        // A face picked in a compare's ghost isn't in the working model:
        // it can't be selected, only described and its reference copied.
        let ghost = self.hover.as_ref().is_some_and(|h| h.rev.is_some());
        // In the Measure tool a pick of a face, edge or vertex is measured,
        // not selected.
        let measuring = ws.measure.on
            && self
                .hover
                .as_ref()
                .is_some_and(|h| matches!(h.kind, ryucad::entity::Target::Body(_)));
        // A click on the cube turns the view; it never picks.
        if response.clicked()
            && over_cube
            && let Some(hit) = self.cube_hover
        {
            self.cube_click(ws, hit);
        }
        let clicked = response.clicked() && !over_cube;
        if clicked && !ghost && measuring {
            let entity = self.hover.as_ref().expect("a pick").entity.clone();
            ws.measure_pick(&entity);
        } else if clicked && !ghost {
            let add = modifiers.shift || modifiers.ctrl;
            // Picking something else, or nothing, leaves the tree's choice.
            let keep = self
                .hover
                .as_ref()
                .is_some_and(|h| add || self.on_chosen(ws, &h.entity));
            if !keep && !(add && self.hover.is_none()) {
                self.unchoose(ws);
            }
            match (&self.hover, add) {
                (Some(h), false) => ws.select(vec![h.entity.clone()], SelectMode::Replace),
                (Some(h), true) => {
                    let mine = ws.myself().is_some_and(|m| m.selection.contains(&h.entity));
                    let mode = if mine {
                        SelectMode::Remove
                    } else {
                        SelectMode::Add
                    };
                    ws.select(vec![h.entity.clone()], mode);
                }
                (None, false) => ws.select(Vec::new(), SelectMode::Replace),
                (None, true) => {}
            }
        }
        if response.secondary_clicked() && !over_cube {
            self.menu = self.hover.clone();
        }
        // Visibility, with SolidWorks' keys by default: Tab hides the
        // occurrence (or solid) under the mouse, Shift+Tab shows those
        // hidden under it, Ctrl+Shift+Tab shows everything. The keys are
        // the viewport's commands' (`view.hide_under`, `view.show_under`,
        // `view.show_all`), taken by `take_keys` before egui could move
        // focus with them. Typing in a field keeps its keys.
        let tabs = std::mem::take(&mut self.tabs);
        self.hovered = response.hovered();
        if response.hovered() && !ui.ctx().text_edit_focused() {
            let all = tabs.contains(&"view.show_all");
            let show = tabs.contains(&"view.show_under");
            let hide = tabs.contains(&"view.hide_under");
            let at = response.hover_pos().map(local);
            if all {
                ws.set_visibility(Vec::new(), Vec::new(), Vec::new(), true);
            } else if show && let Some(p) = at {
                // Every hidden body the ray meets, and what picking would
                // find there (an edge or vertex within reach of the mouse).
                let hidden = Picker::of_hidden(&ws.model);
                let mut under = hidden.bodies_under(&pr, size, p);
                under.extend(
                    hidden
                        .pick(&pr, size, p)
                        .map(|h| (h.part.clone(), h.feature.clone())),
                );
                let mut leaves: Vec<String> = under.iter().map(|k| ws.model.leaf(k)).collect();
                leaves.sort();
                leaves.dedup();
                if !leaves.is_empty() {
                    ws.set_visibility(Vec::new(), leaves, Vec::new(), false);
                }
            } else if hide
                && let Some(h) = self.hover.as_ref().filter(|h| {
                    h.rev.is_none() && matches!(h.kind, ryucad::entity::Target::Body(_))
                })
            {
                let leaf = ws.model.leaf(&(h.part.clone(), h.feature.clone()));
                ws.set_visibility(vec![leaf], Vec::new(), Vec::new(), false);
                self.hover = None;
            }
        }
        // Drawing: what is in view, each at its level of detail, the
        // levels not here yet fetched nearest first.
        let (faces, lines) = self.painted(ws);
        let height = f64::from(rect.height() * ui.ctx().pixels_per_point());
        let (drawn, wanted) = crate::core::lod::choose(&ws.model, self.picker(ws), &pr, height);
        // Within what the window may keep on its adapter.
        let budget = ws
            .settings
            .get("display.gpu_memory")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(2 << 30);
        let (drawn, wanted) = crate::core::lod::within(&ws.model, &pr, drawn, wanted, budget);
        ws.fetch_levels(wanted);
        match gpu {
            Some(rs) => self.draw(ui, ws, rs, rect, &pr, (&faces, &drawn)),
            None => {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "No graphics adapter: use `ryucad render` to see the model",
                    egui::FontId::proportional(14.0),
                    ui.visuals().text_color(),
                );
            }
        }
        let painter = ui.painter_at(rect);
        let to_screen = |p: [f64; 3]| {
            pr.ndc(p).map(|n| {
                egui::pos2(
                    rect.min.x + ((n[0] + 1.0) / 2.0 * size[0]) as f32,
                    rect.min.y + ((1.0 - n[1]) / 2.0 * size[1]) as f32,
                )
            })
        };
        for (place, c) in &lines {
            let stroke = egui::Stroke::new(3.0, style::color32(*c));
            match place {
                Place::Edge(points) => {
                    for w in points.windows(2) {
                        if let (Some(a), Some(b)) = (to_screen(w[0]), to_screen(w[1])) {
                            painter.line_segment([a, b], stroke);
                        }
                    }
                }
                Place::Vertex(p) => {
                    if let Some(s) = to_screen(*p) {
                        painter.circle_filled(s, 4.5, style::color32(*c));
                    }
                }
                Place::Face(..) => {}
            }
        }
        if self.sketches {
            self.dimension_labels(ui, ws, rect, &pr);
        }
        // The active section plane's handles, a new step's arrows, and an
        // explode's trails from where occurrences are to where it puts
        // them.
        let accent = style::color32(style::SELECTED);
        let handle = egui::Stroke::new(2.5, accent);
        let arrow = |painter: &egui::Painter, a: &crate::core::gizmo::Arrow| {
            if let Some((b, t)) = a.on_screen(&pr, size) {
                let at =
                    |q: [f64; 2]| egui::pos2(rect.min.x + q[0] as f32, rect.min.y + q[1] as f32);
                painter.line_segment([at(b), at(t)], handle);
                painter.circle_filled(at(t), 4.5, accent);
            }
        };
        if ws.sections.handles
            && let Some((a, rings)) = ws.sections.gizmo(length)
        {
            arrow(&painter, &a);
            for r in rings {
                let pts: Vec<egui::Pos2> = r
                    .on_screen(&pr, size, 48)
                    .into_iter()
                    .map(|q| egui::pos2(rect.min.x + q[0] as f32, rect.min.y + q[1] as f32))
                    .collect();
                painter.add(egui::Shape::line(pts, egui::Stroke::new(1.5, accent)));
            }
        }
        if let Some(a) = &ws.explodes.adding {
            for x in a.arrows(length) {
                arrow(&painter, &x);
            }
        }
        if let Some(part) = ws.explodes.part.clone()
            && ws.explodes.current(&part).is_some()
        {
            let trail = egui::Stroke::new(1.0, style::color32(ryucad::render::TRAIL));
            for t in ws.explodes.trails(&part, &ws.model.middles(&part)) {
                let pts: Vec<egui::Pos2> = t.iter().filter_map(|p| to_screen(*p)).collect();
                painter.extend(egui::Shape::dashed_line(&pts, trail, 6.0, 4.0));
            }
        }
        // The Measure tool's dimension, a clearance shown, a centre of
        // mass: lines and marks over the model.
        let mut lines: Vec<([f64; 3], [f64; 3], Option<String>)> = Vec::new();
        if let Some(r) = ws.measure.reading.as_ref().filter(|_| ws.measure.on)
            && let Some((a, b)) = r.line
        {
            lines.push((a, b, r.label.clone()));
        }
        if let Some(c) = &ws.clearance
            && let Some(g) = c.shown.and_then(|k| c.gaps.get(k))
        {
            lines.push((g.points[0], g.points[1], Some(g.distance.clone())));
        }
        for (a, b, label) in lines {
            let (Some(sa), Some(sb)) = (to_screen(a), to_screen(b)) else {
                continue;
            };
            painter.line_segment([sa, sb], egui::Stroke::new(2.0, accent));
            painter.circle_filled(sa, 3.5, accent);
            painter.circle_filled(sb, 3.5, accent);
            if let Some(text) = label {
                let mid = sa + (sb - sa) * 0.5;
                painter.text(
                    mid + egui::vec2(6.0, -6.0),
                    egui::Align2::LEFT_BOTTOM,
                    text,
                    egui::FontId::proportional(13.0),
                    accent,
                );
            }
        }
        if let Some(c) = ws.mass.as_ref().and_then(|m| m.centre)
            && let Some(s) = to_screen(c)
        {
            let d = 6.0;
            let stroke = egui::Stroke::new(2.0, accent);
            painter.line_segment([s - egui::vec2(d, 0.0), s + egui::vec2(d, 0.0)], stroke);
            painter.line_segment([s - egui::vec2(0.0, d), s + egui::vec2(0.0, d)], stroke);
            painter.circle_stroke(s, d, stroke);
            painter.text(
                s + egui::vec2(d + 3.0, -d),
                egui::Align2::LEFT_BOTTOM,
                "centre of mass",
                egui::FontId::proportional(12.0),
                accent,
            );
        }
        let (_, frame_names) = self.mate_frames(ws);
        for (name, at) in frame_names {
            if let Some(s) = to_screen(at).filter(|s| rect.contains(*s)) {
                painter.text(
                    s + egui::vec2(6.0, -6.0),
                    egui::Align2::LEFT_BOTTOM,
                    name,
                    egui::FontId::proportional(13.0),
                    style::color32(style::CHOSEN),
                );
            }
        }
        for (i, (text, at, c)) in self.labels(ws).into_iter().enumerate() {
            let Some(s) = to_screen(at).filter(|s| rect.contains(*s)) else {
                continue;
            };
            egui::Area::new(ui.scope_id().with(("highlight label", i)))
                .fixed_pos(s + egui::vec2(8.0, -8.0))
                // Moved in from the edges rather than over the panels.
                .constrain_to(rect)
                .order(egui::Order::Foreground)
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(egui::RichText::new(text).color(style::color32(c)));
                    });
                });
        }
        // The cube and its menu, top right; the triad, bottom left.
        let dark = ui.visuals().dark_mode;
        if self.cube {
            let cube = self.cube_widget();
            crate::ui::cube::draw(&painter, &cube, &self.nav.camera, self.cube_hover, dark);
            let (lo, hi) = cube.menu();
            let button = ui
                .put(
                    egui::Rect::from_min_max(egui::pos2(lo[0], lo[1]), egui::pos2(hi[0], hi[1])),
                    egui::Button::new(egui::RichText::new("Views").small()),
                )
                .on_hover_text("Projection, isometric, zoom to fit and the named views");
            egui::Popup::menu(&button).show(|ui| self.views_menu(ui, ws));
        }
        if self.triad {
            crate::ui::cube::triad(
                &painter,
                egui::pos2(rect.min.x + 40.0, rect.max.y - 40.0),
                &self.nav.camera,
                dark,
            );
        }
        self.selector_ui(ui.ctx(), ws);
        if ws.lost {
            painter.rect_filled(rect, 0.0, egui::Color32::from_white_alpha(140));
        }
        // The registry's commands for what was right-clicked.
        let menu = self.menu.clone();
        let mut ran = None;
        response.context_menu(|ui| {
            use crate::core::command::Context;
            ran = match &menu {
                Some(h) => {
                    ui.label(describe(ws, h));
                    use crate::core::command::Pick;
                    use ryucad::entity::{EntityKind, Target};
                    let what = match (&h.rev, h.kind) {
                        (Some(_), _) => Context::Ghost,
                        (None, Target::Body(EntityKind::Face)) => Context::Entity(Pick::Face),
                        (None, Target::Body(EntityKind::Edge)) => Context::Entity(Pick::Edge),
                        (None, Target::Body(EntityKind::Vertex)) => Context::Entity(Pick::Vertex),
                        (None, _) => Context::Entity(Pick::Sketch),
                    };
                    crate::ui::commands::context_menu(ui, &self.commands, what, Some(&h.entity))
                }
                None => {
                    crate::ui::commands::context_menu(ui, &self.commands, Context::Viewport, None)
                }
            };
        });
        if ran.is_some() {
            self.ran = ran;
        }
        if let Some(h) = &self.hover {
            response.on_hover_text_at_pointer(describe(ws, h));
        }
    }

    /// The cube's menu: projection, isometric, zoom to fit, the named
    /// views.
    fn views_menu(&mut self, ui: &mut egui::Ui, ws: &mut Workspace) {
        let ortho = matches!(self.nav.camera.projection, Projection::Orthographic { .. });
        if ui.selectable_label(!ortho, "Perspective").clicked() {
            self.set_projection(ws, false);
            ui.close();
        }
        if ui.selectable_label(ortho, "Orthographic").clicked() {
            self.set_projection(ws, true);
            ui.close();
        }
        ui.separator();
        if ui.button("Isometric").clicked() {
            self.standard(ws, StandardView::Iso);
            ui.close();
        }
        if ui.button("Zoom to fit").clicked() {
            self.fit(ws, None);
            ui.close();
        }
        ui.separator();
        self.named_views(ui, ws);
    }

    /// A button per named view of the part shown; a view that doesn't
    /// read says why. Returns whether one was chosen.
    fn named_views(&mut self, ui: &mut egui::Ui, ws: &mut Workspace) -> bool {
        if ws.views.part.is_none() {
            ui.weak("Named views: show one part or assembly");
            return false;
        }
        if ws.views.views.is_empty() {
            ui.weak("No named views");
            return false;
        }
        let mut chosen = None;
        for v in &ws.views.views {
            match &v.camera {
                Ok(_) => {
                    if ui.button(&v.id).clicked() {
                        chosen = Some(v.id.clone());
                    }
                }
                Err(why) => {
                    ui.add_enabled(false, egui::Button::new(&v.id))
                        .on_disabled_hover_text(why);
                }
            }
        }
        if let Some(id) = &chosen {
            self.restore_view(ws, id);
            ui.close();
        }
        chosen.is_some()
    }

    /// The spacebar view selector, while open: a cube to click, the
    /// standard views, Normal to, the named views and Save view….
    fn selector_ui(&mut self, ctx: &egui::Context, ws: &mut Workspace) {
        let Some(sel) = self.selector.clone() else {
            return;
        };
        let mut close = false;
        let mut naming = sel.naming.clone();
        let area = egui::Area::new(egui::Id::unique("view selector"))
            .fixed_pos(egui::pos2(sel.at[0], sel.at[1]))
            .constrain_to(self.rect)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.strong("View selector");
                    let (r, painter) =
                        ui.allocate_painter(egui::vec2(150.0, 150.0), Sense::click());
                    let cube = Cube {
                        centre: [r.rect.center().x, r.rect.center().y],
                        half: 30.0,
                    };
                    let hover = r
                        .hover_pos()
                        .and_then(|p| cube.hit(&self.nav.camera, [p.x, p.y]));
                    crate::ui::cube::draw(
                        &painter,
                        &cube,
                        &self.nav.camera,
                        hover,
                        ui.visuals().dark_mode,
                    );
                    if r.clicked()
                        && let Some(h) = hover
                    {
                        self.cube_click(ws, h);
                        close = true;
                    }
                    ui.horizontal_wrapped(|ui| {
                        use StandardView as S;
                        for v in [
                            S::Front,
                            S::Back,
                            S::Left,
                            S::Right,
                            S::Top,
                            S::Bottom,
                            S::Iso,
                            S::Dimetric,
                            S::Trimetric,
                        ] {
                            let id = format!("view.{}", v.name());
                            let title = crate::core::command::get(&id).map_or("", |c| c.title);
                            if ui.button(title).clicked() {
                                self.standard(ws, v);
                                close = true;
                            }
                        }
                    });
                    let face = self.selected_face(ws);
                    if ui
                        .add_enabled(face.is_some(), egui::Button::new("Normal to"))
                        .on_disabled_hover_text("Select a planar face first")
                        .clicked()
                    {
                        self.normal_to(ws);
                        close = true;
                    }
                    ui.separator();
                    if self.named_views(ui, ws) {
                        close = true;
                    }
                    if ws.views.part.is_some() {
                        match &mut naming {
                            None => {
                                if ui.button("Save view…").clicked() {
                                    naming = Some(String::new());
                                }
                            }
                            Some(name) => {
                                ui.horizontal(|ui| {
                                    let field = ui.add(
                                        egui::TextEdit::singleline(name)
                                            .hint_text("view name")
                                            .desired_width(110.0),
                                    );
                                    field.request_focus();
                                    let ok = crate::core::views::valid_id(name);
                                    let save = ui.add_enabled(ok, egui::Button::new("Save"));
                                    let entered = field.lost_focus()
                                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                    if ok && (save.clicked() || entered) {
                                        ws.save_view(name, &self.nav.camera);
                                        close = true;
                                    }
                                });
                            }
                        }
                    }
                });
            });
        // A click elsewhere closes it, as a popup's does.
        if area.response.clicked_elsewhere() {
            close = true;
        }
        self.selector = if close {
            None
        } else {
            Some(crate::core::views::Selector { naming, ..sel })
        };
    }

    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        ws: &Workspace,
        rs: &egui_wgpu::RenderState,
        rect: egui::Rect,
        pr: &Projector,
        (faces, drawn): (&[PaintedFace], &[crate::core::lod::Drawn]),
    ) {
        let ppp = ui.ctx().pixels_per_point();
        let px = [
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
        ];
        let reference = self.reference(ws);
        let bounds = self.bounds(ws);
        let scene = self
            .scene
            .get_or_insert_with(|| Scene::new(&rs.device, &rs.adapter));
        scene.set_looks(&ws.model.looks);
        scene.sync_drawn(&rs.device, drawn, reference);
        scene.sync_ghosts(&rs.device, ws.compare.as_ref());
        let size = [f64::from(px[0]), f64::from(px[1])];
        let mut overlay = if self.sketches {
            crate::core::sketch::overlay(
                ws.model.drawn_sketches().map(|(_, d)| &**d),
                pr,
                size,
                f64::from(ppp),
            )
        } else {
            Vec::new()
        };
        // The origin's axes and planes, a tenth of the model's size.
        if self.origin {
            let length = match bounds {
                Some((lo, hi)) => ryucad::render::origin_size(&[lo, hi]),
                None => ryucad::render::origin_size(&[]),
            };
            overlay.extend(crate::core::sketch::origin(
                pr,
                size,
                f64::from(ppp),
                length,
            ));
        }
        let list = DrawList {
            reference,
            matrix: pr.matrix(reference),
            perspective: pr.line_pull(),
            light: scene::light(&pr.camera),
            edges: self.edges,
            faces,
            background: self.look.viewport_background(ui.visuals().dark_mode),
            overlay: &overlay,
        };
        scene.cut(scene::Cut::of(&ws.sections.planes, &pr.camera));
        let view = scene.render(&rs.device, &rs.queue, px, &list);
        let mut renderer = rs.renderer.write();
        let id = match self.texture {
            Some(id) => {
                renderer.update_egui_texture_from_wgpu_texture(
                    &rs.device,
                    view,
                    wgpu::FilterMode::Linear,
                    id,
                );
                id
            }
            None => *self.texture.insert(renderer.register_native_texture(
                &rs.device,
                view,
                wgpu::FilterMode::Linear,
            )),
        };
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter().image(id, rect, uv, egui::Color32::WHITE);
    }
}

/// This frame's scrolling by fingers on a trackpad: scrolls in points that
/// the platform says (or, not saying, implies) come from fingers or their
/// momentum. Wheels scroll in lines.
#[derive(Debug, Default)]
struct Fingers {
    /// Any arrived (a frame of them never zooms, even with no movement).
    any: bool,
    delta: egui::Vec2,
    /// They went down, or lifted.
    start: bool,
    end: bool,
    /// Shift was held: they orbit.
    shift: bool,
}

impl Fingers {
    fn of(events: &[egui::Event]) -> Fingers {
        let mut f = Fingers::default();
        for e in events {
            if let egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta,
                phase,
                source,
                modifiers,
            } = e
                && *source != egui::MouseWheelSource::Wheel
            {
                f.any = true;
                f.delta += *delta;
                f.shift |= modifiers.shift;
                f.start |= *phase == egui::TouchPhase::Start;
                f.end |= matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel);
            }
        }
        f
    }
}
