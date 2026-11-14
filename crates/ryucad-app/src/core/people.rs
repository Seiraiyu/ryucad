//! Who else is attached, and what each of them selects, looks at,
//! highlights and says they are doing: `ryu_app::people`' (ryueda 3.C),
//! with what a ryucad window shows ([`CadView`]).

use ryu_app::people::strings;
use ryucad::view::Camera;
use serde_json::Value;

pub use ryu_app::people::Highlight;

/// What else a participant's ryucad window shows.
#[derive(Clone, Debug, PartialEq)]
pub struct CadView {
    pub camera: Option<Camera>,
    /// The Git revision they compare the working files against.
    pub compare: Option<String>,
    /// The part or assembly their window shows; `None` for the project's
    /// model.
    pub focus: Option<String>,
    /// What their window hides: occurrences (`<asm>/<path>`) and solids
    /// (`<part>/<solid>`).
    pub hidden: Vec<String>,
    /// What their window cuts away.
    pub sections: Vec<ryucad::section::Plane>,
    /// The exploded view their window shows.
    pub explode: Option<crate::core::explode::Shown>,
    /// The motion their window shows.
    pub motion: Option<crate::core::motion::Shown>,
    /// The mode their window is in (design 374): an agent's is shown,
    /// never enforced.
    pub mode: ryucad::live::Mode,
}

impl ryu_app::people::View for CadView {
    fn from_json(v: &Value) -> Self {
        CadView {
            camera: serde_json::from_value(v["camera"].clone()).ok(),
            compare: v["compare"]["rev"].as_str().map(str::to_owned),
            focus: v["focus"].as_str().map(str::to_owned),
            hidden: strings(&v["hidden"]),
            sections: serde_json::from_value(v["sections"].clone()).unwrap_or_default(),
            mode: serde_json::from_value(v["mode"].clone()).unwrap_or_default(),
            explode: crate::core::explode::Shown::from_json(&v["explode"]),
            motion: crate::core::motion::Shown::from_json(&v["motion"]),
        }
    }
    fn followable(&self) -> bool {
        self.camera.is_some()
    }
}

/// One participant.
pub type Person = ryu_app::people::Person<CadView>;
/// Everyone attached, by ID.
pub type People = ryu_app::people::People<CadView>;

#[cfg(test)]
mod tests {
    use super::*;
    use ryucad::live::Style;
    use serde_json::json;

    #[test]
    fn a_person_shows_a_motion() {
        let v = serde_json::json!({"id": "p-1", "kind": "human", "name": "Dana",
            "motion": {"part": "asm", "motion": "lift", "fraction": 0.25, "playing": true}});
        let p = Person::from_json(&v).unwrap();
        assert_eq!(
            p.motion,
            Some(crate::core::motion::Shown {
                part: "asm".into(),
                motion: "lift".into(),
                fraction: 0.25,
                playing: true
            })
        );
        assert_eq!(p.explode, None);
    }

    #[test]
    fn notifications_carry_whole_participants() {
        let mut people = People::default();
        people.reset(&json!([
            {"id": "p-00000001", "kind": "human", "name": "Dana", "status": null,
             "selection": [], "camera": null, "highlights": []},
        ]));
        people.changed(&json!({"participant": {
            "id": "p-00000002", "kind": "agent", "name": "agent",
            "status": {"text": "drilling", "entities": ["p/a/0123"]},
            "selection": ["p/a/0123"],
            "camera": {"eye": [1.0, 0.0, 0.0], "target": [0.0, 0.0, 0.0], "up": [0.0, 0.0, 1.0],
                       "projection": {"perspective": {"fov_deg": 30.0}}},
            "highlights": [{"style": "pending", "label": "here", "entities": ["p/a/0123"]}],
            "mode": "annotate",
        }}));
        let agent = people.get("p-00000002").unwrap();
        assert_eq!(
            agent.status,
            Some(("drilling".into(), vec!["p/a/0123".into()]))
        );
        assert_eq!(agent.highlights[0].style, Style::Pending);
        assert_eq!(agent.camera.as_ref().unwrap().eye, [1.0, 0.0, 0.0]);
        assert_eq!(agent.mode, ryucad::live::Mode::Annotate);
        assert_eq!(
            people.get("p-00000001").unwrap().mode,
            ryucad::live::Mode::View
        );
        assert_eq!(people.others("p-00000001").count(), 1);
        people.changed(&json!({"participant": {"id": "p-00000002"}, "left": true}));
        assert!(people.get("p-00000002").is_none());
        assert_eq!(people.generation, 3);
    }
}
