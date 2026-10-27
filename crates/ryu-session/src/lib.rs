//! What every ryu product's session daemon shares. A product (ryucad,
//! ryueda) is a marker type implementing [`Product`]; the modules here are
//! generic over it, and the product names their types for itself
//! (`ryucad::endpoint::Endpoint` is `Endpoint<ryucad::Cad>`), so its own
//! callers never see the parameter.
//!
//! Nothing here reads a product's files or knows its requests: those stay
//! the product's, behind the trait.

pub mod activity;
pub mod endpoint;
pub mod fresh;
pub mod live;
pub mod notify;
pub mod prefs;
pub mod rpc;

use serde::Serialize;
use serde_json::{Map, Value};

/// A product built on these sessions: its name, the variables that move
/// its folders, and what its live sessions hold.
pub trait Product: Sized + Send + Sync + 'static {
    /// The product's name: its binary, and the folder and socket names its
    /// daemons use (`ryucad`). Two products never share a daemon.
    const NAME: &'static str;
    /// The variable that overrides the runtime folder (tests keep their
    /// daemons apart with it): `RYUCAD_RUNTIME_DIR`.
    const RUNTIME_DIR_VAR: &'static str;
    /// What participants select, highlight and work on: an entity
    /// reference, written as a string.
    type Target: Clone + PartialEq + Serialize + std::fmt::Debug + Send + Sync;
    /// What else a participant's window shows (ryucad's camera, compare,
    /// focus and hidden set), from its `Default`.
    type View: Default + Clone + std::fmt::Debug + Send + Sync;
    /// What subscribers were last told about the project, from its
    /// `Default`.
    type Seen: Default + std::fmt::Debug + Send + Sync;
    /// What a subscription can ask for.
    type Topic: notify::Topics + std::fmt::Debug;
    /// Adds a participant's view to its JSON, beside `id`, `kind`, `name`,
    /// `status`, `selection` and `highlights`.
    fn view_json(view: &Self::View, into: &mut Map<String, Value>);
}

/// A product for this crate's tests.
#[cfg(test)]
pub(crate) mod testing {
    use serde::{Deserialize, Serialize};
    use serde_json::{Map, Value, json};

    pub struct Test;

    /// A product's topics, as ryucad's were when these tests were written.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Topic {
        Model,
        Project,
        Participants,
        Activity,
        Merge,
        Pdm,
    }

    impl crate::notify::Topics for Topic {
        const ALL: &'static [Topic] = &[
            Topic::Model,
            Topic::Project,
            Topic::Participants,
            Topic::Activity,
            Topic::Merge,
            Topic::Pdm,
        ];
        const PARTICIPANTS: Topic = Topic::Participants;
        const ACTIVITY: Topic = Topic::Activity;
    }

    impl crate::Product for Test {
        const NAME: &'static str = "ryutest";
        const RUNTIME_DIR_VAR: &'static str = "RYUTEST_RUNTIME_DIR";
        type Target = String;
        type View = Option<String>;
        type Seen = ();
        type Topic = Topic;

        fn view_json(view: &Option<String>, into: &mut Map<String, Value>) {
            into.insert("camera".into(), json!(view));
        }
    }
}
