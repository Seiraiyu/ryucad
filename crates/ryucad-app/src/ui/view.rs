//! The viewport: the 3D view with navigation, hover and picking. A click
//! sets this window's selection through the daemon; everyone's selections
//! and highlights are painted over the model in their colours.

use crate::core::model::BodyKey;
use crate::core::nav::{Buttons, Gesture, Modifiers, Nav, Projector};
use crate::core::pick::{Hit, Picker, Place};
use crate::core::workspace::Workspace;
use crate::ui::scene::{self, DrawList, PaintedFace, Scene, linear};
use crate::ui::style::{self, Rgb};
use eframe::egui::{self, PointerButton, Sense};
use eframe::{egui_wgpu, wgpu};
use ryucad::live::SelectMode;
use ryucad::view::{self, StandardView};
use std::collections::BTreeMap;

/// What the window's viewport keeps between frames.
pub struct View {
    pub nav: Nav,
    /// Draw B-rep edges (`display.edges`).
    pub edges: bool,
    /// The feature chosen in the tree: its body is painted.
    pub chosen: Option<BodyKey>,
    pub hover: Option<Hit>,
    /// Where the viewport was last drawn, in points.
    pub rect: egui::Rect,
    picker: Option<Picker>,
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
}

/// A body's points that frame it from one direction
/// (`view::framing_points`): a few hundred of its mesh's thousands.
struct Framers {
    hash: String,
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
/// (cylinder)".
pub fn describe(ws: &Workspace, hit: &Hit) -> String {
    let ty = ws
        .part(&hit.part)
        .and_then(|p| p.feature(&hit.feature))
        .and_then(|f| f.ty.clone())
        .unwrap_or_default();
    format!("{} of {} ({ty})", hit.kind.as_str(), hit.feature)
}

impl View {
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
            chosen: None,
            hover: None,
            rect: egui::Rect::NOTHING,
            picker: None,
            reference: None,
            scene: None,
            texture: None,
            framed: false,
            framing: None,
            framers: BTreeMap::new(),
            pivot: None,
            finger_pivot: None,
            menu: None,
        }
    }

    /// The model prepared for picking, rebuilt when it changes.
    pub fn picker(&mut self, ws: &Workspace) -> &Picker {
        if self
            .picker
            .as_ref()
            .is_none_or(|p| p.generation != ws.model.generation)
        {
            self.picker = Some(Picker::new(&ws.model));
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

    /// Looks from a standard view at the whole model, and tells the session.
    pub fn standard(&mut self, ws: &mut Workspace, v: StandardView) {
        let points = self.picker(ws).points(None);
        if !points.is_empty() {
            self.nav.standard(v, &points, self.aspect());
            self.moved(ws);
        }
    }

    /// The points that frame the whole model as the camera looks: each
    /// body's few, kept while its mesh and the camera's direction stay.
    /// While a model loads it is framed again as each batch of meshes
    /// comes: over all their points, each frame would cost more than the
    /// last.
    fn framing_points(&mut self, ws: &Workspace) -> Vec<[f64; 3]> {
        let c = &self.nav.camera;
        let (back, up) = (unit(sub(c.eye, c.target)), unit(c.up));
        self.framers.retain(|k, _| ws.model.meshes.contains_key(k));
        let mut out = Vec::new();
        for (key, body) in &ws.model.meshes {
            let kept = self.framers.get(key).is_some_and(|f| {
                f.hash == body.hash && same_way(f.back, back) && same_way(f.up, up)
            });
            if !kept {
                let points = self.picker(ws).points(Some(key));
                let framers = Framers {
                    hash: body.hash.clone(),
                    back,
                    up,
                    points: view::framing_points(&points, back, up),
                };
                self.framers.insert(key.clone(), framers);
            }
            out.extend_from_slice(&self.framers[key].points);
        }
        out
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

    /// Frames the given entities.
    pub fn fit_entities(&mut self, ws: &mut Workspace, entities: &[String]) {
        let points = self.picker(ws).points_of(entities);
        if !points.is_empty() {
            self.nav.fit(&points, self.aspect());
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
    /// chosen feature, other people's selections, highlights, this
    /// window's selection, then the hover.
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
        if let Some(me) = ws.myself() {
            wanted.extend(me.selection.iter().map(|e| (e.clone(), style::SELECTED)));
        }
        if let Some(h) = &self.hover {
            wanted.push((h.entity.clone(), style::HOVER));
        }
        let chosen = self.chosen.clone();
        let picker = self.picker(ws);
        let mut faces = Vec::new();
        if let Some(k) = chosen {
            let c = linear(style::CHOSEN);
            faces.extend((0..picker.faces(&k)).map(|i| (k.clone(), i, c)));
        }
        let mut lines = Vec::new();
        for (e, c) in wanted {
            match picker.place(&e) {
                Some(Place::Face(k, i)) => faces.push((k, i, linear(c))),
                Some(p) => lines.push((p, c)),
                None => {}
            }
        }
        (faces, lines)
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
        // What is under the mouse.
        let pr = self.projector(ws, aspect);
        self.hover = match response.hover_pos() {
            Some(p) if !response.dragged() => self.picker(ws).pick(&pr, size, local(p)),
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
        if response.drag_started() {
            // Where the button went down: a drag starts only once the
            // pointer has moved a little, maybe onto the model.
            let pressed = ui.input(|i| i.pointer.press_origin());
            self.pivot = pivot(self, ws, pressed.or(response.interact_pointer_pos()));
        }
        if response.dragged() {
            let g = self.nav.style.gesture(buttons, modifiers);
            if g != Gesture::None {
                let d = response.drag_delta();
                self.nav
                    .drag(g, [d.x, d.y], rect.height().max(1.0), self.pivot);
                self.moved(ws);
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
        if response.clicked() {
            let add = modifiers.shift || modifiers.ctrl;
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
        if response.secondary_clicked() {
            self.menu = self.hover.clone();
        }
        // Drawing.
        let (faces, lines) = self.painted(ws);
        match gpu {
            Some(rs) => self.draw(ui, ws, rs, rect, &pr, &faces),
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
        if ws.lost {
            painter.rect_filled(rect, 0.0, egui::Color32::from_white_alpha(140));
        }
        let menu = self.menu.clone();
        response.context_menu(|ui| match &menu {
            Some(h) => {
                ui.label(describe(ws, h));
                if ui.button("Copy reference").clicked() {
                    ui.ctx().copy_text(h.entity.clone());
                    ui.close();
                }
                if ui.button("Select").clicked() {
                    ws.select(vec![h.entity.clone()], SelectMode::Replace);
                    ui.close();
                }
            }
            None => {
                if ui.button("Zoom to fit").clicked() {
                    self.fit(ws, None);
                    ui.close();
                }
            }
        });
        if let Some(h) = &self.hover {
            response.on_hover_text_at_pointer(describe(ws, h));
        }
    }

    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        ws: &Workspace,
        rs: &egui_wgpu::RenderState,
        rect: egui::Rect,
        pr: &Projector,
        faces: &[PaintedFace],
    ) {
        let ppp = ui.ctx().pixels_per_point();
        let px = [
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
        ];
        let reference = self.reference(ws);
        let scene = self
            .scene
            .get_or_insert_with(|| Scene::new(&rs.device, &rs.adapter));
        scene.sync(&rs.device, &ws.model, reference);
        let list = DrawList {
            reference,
            matrix: pr.matrix(reference),
            perspective: pr.line_pull(),
            light: scene::light(&pr.camera),
            edges: self.edges,
            faces,
            background: crate::ui::style::viewport_background(ui.visuals().dark_mode),
        };
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
