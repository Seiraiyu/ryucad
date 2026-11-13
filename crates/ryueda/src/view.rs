//! What a participant's window shows besides its selection (ryu-session's
//! `Product::View` for ryueda): a compare against a Git revision (plan
//! 4.B), and a block's sheet as one of its layers through a camera (plan
//! 3.B). Session state, never written to the project.

use crate::error::{Code, Error, Result};
use ryusch::geom::Rect;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What a participant shows.
#[derive(Clone, Debug, Default)]
pub struct View {
    /// The Git revision the participant compares the working files
    /// against, as given (`HEAD`, a branch, a commit); `None` when it
    /// isn't comparing.
    pub compare: Option<String>,
    /// The design whose layer is drawn, when a design places the block.
    pub design: Option<String>,
    pub block: Option<String>,
    /// The sheet's ID.
    pub sheet: Option<String>,
    /// The layer drawn (an instance or stack path) when one was asked.
    pub layer: Option<String>,
    pub camera: Option<Camera>,
    /// Who set the view last: the participant itself (`view.set`), or
    /// the one whose `view.show` moved it. A window that doesn't follow
    /// (plan 3.D) may leave another's change.
    pub shown_by: Option<String>,
}

/// Where a window looks, in the sheet's frame (nanometres, y up): the
/// window's centre, and how many nanometres a point spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    pub centre_nm: [i64; 2],
    pub zoom_nm_per_pt: i64,
}

/// The window a framing camera is worked out for, in points: a camera
/// set for one window frames about the same in another.
pub const FRAME_PT: [i64; 2] = [800, 500];
/// The closest a framing camera comes: a grid step (1.27 mm) is 127
/// points.
pub const MIN_ZOOM: i64 = 10_000;

impl Camera {
    /// The camera that frames `r` with four grid steps round it in a
    /// [`FRAME_PT`] window, never closer than [`MIN_ZOOM`].
    pub fn framing(r: Rect) -> Camera {
        let g = r.grow(4 * ryusch::geom::GRID);
        let up = |len: i64, pt: i64| (len + pt - 1) / pt;
        Camera {
            centre_nm: g.center(),
            zoom_nm_per_pt: up(g.width(), FRAME_PT[0])
                .max(up(g.height(), FRAME_PT[1]))
                .max(MIN_ZOOM),
        }
    }

    /// Refuses a zoom that isn't positive.
    pub fn check(&self) -> Result<()> {
        if self.zoom_nm_per_pt > 0 {
            return Ok(());
        }
        Err(Error::new(
            Code::InvalidParam,
            "a camera's zoom is a positive number of nanometres a point",
        )
        .with("param", "camera"))
    }
}

/// What a participant's JSON says of its view besides its compare:
/// `design`, `block`, `sheet`, `layer`, `camera` and `shown_by`, null
/// where unset.
pub fn to_json(v: &View) -> Value {
    json!({
        "design": v.design,
        "block": v.block,
        "sheet": v.sheet,
        "layer": v.layer,
        "camera": v.camera,
        "shown_by": v.shown_by,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_session::live::{Kind, Live};
    use serde_json::json;

    #[test]
    fn a_camera_frames_a_box_in_a_window_of_frame_pt() {
        // 8 x 4 grid steps, grown by four steps each way: 16 x 12 steps.
        let r = Rect::new([0, 0], [10_160_000, 5_080_000]);
        let c = Camera::framing(r);
        assert_eq!(c.centre_nm, [5_080_000, 2_540_000]);
        // 20.32 mm over 800 points, 15.24 mm over 500: the larger.
        assert_eq!(c.zoom_nm_per_pt, 30_480);
        // A point's box: its four grid steps each way (10.16 mm) over 500
        // points, never closer than MIN_ZOOM.
        let point = Camera::framing(Rect::new([0, 0], [0, 0]));
        assert_eq!(point.zoom_nm_per_pt, 20_320);
        assert!(point.zoom_nm_per_pt >= MIN_ZOOM);
        assert!(
            Camera {
                centre_nm: [0, 0],
                zoom_nm_per_pt: 0
            }
            .check()
            .is_err()
        );
    }

    #[test]
    fn a_participants_view_is_in_its_json() {
        let mut live = Live::<crate::Eda>::new();
        let id = live.hello(Kind::Human, Some("Dana"), None).unwrap();
        let v = live.get(&id).unwrap().to_json();
        for k in [
            "compare", "design", "block", "sheet", "layer", "camera", "shown_by",
        ] {
            assert_eq!(v[k], serde_json::Value::Null, "{k}");
        }
        live.get_mut(&id).unwrap().view = View {
            block: Some("top".into()),
            sheet: Some("s1".into()),
            camera: Some(Camera {
                centre_nm: [1, 2],
                zoom_nm_per_pt: 10_000,
            }),
            shown_by: Some(id.clone()),
            ..View::default()
        };
        let v = live.get(&id).unwrap().to_json();
        assert_eq!((&v["block"], &v["sheet"]), (&json!("top"), &json!("s1")));
        assert_eq!(
            v["camera"],
            json!({"centre_nm": [1, 2], "zoom_nm_per_pt": 10_000})
        );
        assert_eq!(v["shown_by"], json!(id));
    }
}
