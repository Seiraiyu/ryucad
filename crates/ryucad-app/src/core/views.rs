//! The named views of the part the window shows (`view.list`), for the
//! cube's menu and the spacebar view selector, and the selector's state.

use ryucad::error::Error;
use ryucad::view::Camera;
use serde_json::Value;

/// One named view: its camera, or why its file doesn't read.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedView {
    pub id: String,
    pub camera: Result<Camera, String>,
}

/// The named views of one part, as last read, and the last save's
/// failure.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NamedViews {
    /// The part they are of; `None` when the window shows several parts
    /// (or none), which have no one part's views.
    pub part: Option<String>,
    pub views: Vec<NamedView>,
    pub error: Option<Error>,
}

impl NamedViews {
    /// `part`'s views from a `view.list` of it.
    pub fn of(part: &str, v: &Value) -> NamedViews {
        let views = v["views"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|x| x["part"] == part)
            .filter_map(|x| {
                let id = x["view"].as_str()?.to_owned();
                let camera = match serde_json::from_value::<Camera>(x["camera"].clone()) {
                    Ok(c) => Ok(c),
                    Err(_) => Err(x["error"]["message"]
                        .as_str()
                        .unwrap_or("the view doesn't read")
                        .to_owned()),
                };
                Some(NamedView { id, camera })
            })
            .collect();
        NamedViews {
            part: Some(part.to_owned()),
            views,
            error: None,
        }
    }

    /// The camera of view `id`, if it reads.
    pub fn camera(&self, id: &str) -> Option<&Camera> {
        self.views
            .iter()
            .find(|v| v.id == id)
            .and_then(|v| v.camera.as_ref().ok())
    }
}

/// Whether `id` may name a view: a part ID that isn't a standard view's
/// name.
pub fn valid_id(id: &str) -> bool {
    ryucad::doc::views::check_id(id).is_ok()
}

/// The spacebar view selector, while open: where it opened (in points),
/// and the name being typed for Save view….
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selector {
    pub at: [f32; 2],
    pub naming: Option<String>,
}

impl Selector {
    pub fn at(at: [f32; 2]) -> Selector {
        Selector { at, naming: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_parts_views_from_view_list() {
        let v = json!({
            "views": [
                {"part": "p", "view": "detail",
                 "camera": {"eye": [0.1, -0.1, 0.1], "target": [0.0, 0.0, 0.0], "up": [0.0, 0.0, 1.0],
                            "projection": {"perspective": {"fov_deg": 30.0}}}},
                {"part": "p", "view": "broken",
                 "error": {"code": "invalid_json", "message": "parts/p/views/broken.json: bad"}},
                {"part": "q", "view": "other",
                 "camera": {"eye": [1.0, 0.0, 0.0], "target": [0.0, 0.0, 0.0], "up": [0.0, 0.0, 1.0],
                            "projection": {"perspective": {"fov_deg": 30.0}}}}
            ],
            "revisions": {"p": "r1"}
        });
        let n = NamedViews::of("p", &v);
        assert_eq!(n.part.as_deref(), Some("p"));
        assert_eq!(
            n.views.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
            ["detail", "broken"]
        );
        assert_eq!(n.camera("detail").unwrap().eye, [0.1, -0.1, 0.1]);
        assert_eq!(n.camera("broken"), None);
        assert_eq!(
            n.views[1].camera,
            Err("parts/p/views/broken.json: bad".to_owned())
        );
        assert!(valid_id("detail") && !valid_id("front") && !valid_id("Two Words"));
    }
}
