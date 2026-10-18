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
    /// The camera has framed the model once.
    framed: bool,
    pivot: Option<[f64; 3]>,
    /// What the context menu was opened on.
    menu: Option<Hit>,
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
            pivot: None,
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

    /// Looks from a standard view at the whole model, and tells the session.
    pub fn standard(&mut self, ws: &mut Workspace, v: StandardView) {
        if let Some((lo, hi)) = self.bounds(ws) {
            self.nav.standard(v, lo, hi);
            self.moved(ws);
        }
    }

    /// Frames the whole model, or one body.
    pub fn fit(&mut self, ws: &mut Workspace, body: Option<&BodyKey>) {
        let b = match body {
            Some(k) => self.picker(ws).body_bounds(k),
            None => self.bounds(ws),
        };
        if let Some((lo, hi)) = b {
            self.nav.fit(lo, hi);
            self.moved(ws);
        }
    }

    /// Frames the given entities.
    pub fn fit_entities(&mut self, ws: &mut Workspace, entities: &[String]) {
        if let Some((lo, hi)) = self.picker(ws).bounds_of(entities) {
            self.nav.fit(lo, hi);
            self.moved(ws);
        }
    }

    /// The camera moved by this window's hand: following stops, and the
    /// session hears of it.
    fn moved(&mut self, ws: &mut Workspace) {
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
        if !self.framed
            && let Some((lo, hi)) = self.bounds(ws)
        {
            self.framed = true;
            self.nav.fit(lo, hi);
            ws.share_camera(&self.nav.camera);
        }
        if let Some(c) = ws.followed_camera() {
            self.nav.camera = c.clone();
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
        let (buttons, modifiers, scroll) = ui.input(|i| {
            let b = Buttons {
                primary: i.pointer.button_down(PointerButton::Primary),
                secondary: i.pointer.button_down(PointerButton::Secondary),
                middle: i.pointer.button_down(PointerButton::Middle),
            };
            let m = Modifiers {
                shift: i.modifiers.shift,
                ctrl: i.modifiers.command,
            };
            (b, m, i.smooth_scroll_delta.y)
        });
        if response.drag_started() {
            self.pivot = response
                .interact_pointer_pos()
                .and_then(|p| self.picker(ws).pick(&pr, size, local(p)))
                .map(|h| h.point);
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
        if response.hovered() && scroll != 0.0 {
            let toward = response
                .hover_pos()
                .and_then(|p| self.picker(ws).pick(&pr, size, local(p)));
            self.nav.scroll(scroll, toward.map(|h| h.point));
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
