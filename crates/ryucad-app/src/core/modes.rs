//! The window's mode (design 374): View, which every window and every
//! reconnection starts in and which changes nothing; Annotate, where
//! every model edit becomes a proposed change for the agent
//! (`annotation.propose`) and comments are made; and Direct edit, which
//! writes as the window always has. What a request needs is the daemon's
//! own rule (`Request::needs`, `annotation::propose::PROPOSABLE`), so the
//! window never sends what the daemon would refuse, and its controls say
//! which mode would allow them.

pub use ryucad::live::Mode;
use ryucad::proto::Request;

/// What the window does with a request in its mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Sent as it is: it writes nothing, or the mode allows it.
    Send,
    /// Sent as a proposal (`annotation.propose`): a model edit in Annotate.
    Propose,
    /// Not sent: why, naming the mode that allows it.
    Refuse(&'static str),
}

const VIEW_ANNOTATES: &str = "Annotate or Direct edit does this: View changes nothing.";
const VIEW_WRITES: &str = "Direct edit does this: View changes nothing.";
const ANNOTATE_WRITES: &str =
    "Direct edit does this: in Annotate only model edits can be suggested.";

fn proposable(method: &str) -> bool {
    ryucad::annotation::propose::PROPOSABLE.contains(&method)
}

/// What the window does with `req` in `mode`.
pub fn gate(mode: Mode, req: &Request) -> Gate {
    let Some(needs) = req.needs() else {
        return Gate::Send;
    };
    if needs.contains(&mode) {
        return Gate::Send;
    }
    match mode {
        Mode::Annotate if proposable(&req.method()) => Gate::Propose,
        Mode::View if req.annotates() => Gate::Refuse(VIEW_ANNOTATES),
        Mode::View => Gate::Refuse(VIEW_WRITES),
        _ => Gate::Refuse(ANNOTATE_WRITES),
    }
}

/// What a control that writes by `method` does in `mode`: whether it is
/// enabled, and its tooltip, before its request is built. Only for
/// controls that write (preferences, cameras, sections and selections
/// write nothing a mode guards, and never ask).
pub fn control(mode: Mode, method: &str) -> Gate {
    let annotates = method.starts_with("annotation.") && method != "annotation.apply";
    match mode {
        Mode::Edit => Gate::Send,
        Mode::Annotate if annotates => Gate::Send,
        Mode::Annotate if proposable(method) => Gate::Propose,
        Mode::Annotate => Gate::Refuse(ANNOTATE_WRITES),
        Mode::View if annotates => Gate::Refuse(VIEW_ANNOTATES),
        Mode::View => Gate::Refuse(VIEW_WRITES),
    }
}

/// The mode's name as its button and bar say it.
pub fn title(mode: Mode) -> &'static str {
    match mode {
        Mode::View => "View only",
        Mode::Annotate => "Annotate",
        Mode::Edit => "Direct edit",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(method: &str, params: serde_json::Value) -> Request {
        Request::from_rpc(method, Some(params)).unwrap()
    }

    #[test]
    fn the_window_gates_requests_as_the_daemon_judges_them() {
        use Gate::{Propose, Send};
        let edit = req(
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {}}),
        );
        let undo = req("undo", json!({}));
        let reply = req("annotation.reply", json!({"id": "aaaaaaaaaa", "text": "t"}));
        let apply = req("annotation.apply", json!({"id": "aaaaaaaaaa"}));
        let read = req("feature.list", json!({"part": "p"}));
        let pref = req(
            "config.set",
            json!({"key": "display.edges", "value": false}),
        );
        let unit = req("config.set", json!({"key": "default_unit", "value": "in"}));
        let release = req("release.propose", json!({"part": "p"}));
        // (request, View, Annotate); Direct edit always sends. `None` is
        // refused.
        let table: [(&Request, Option<Gate>, Option<Gate>); 8] = [
            (&edit, None, Some(Propose)),
            (&undo, None, None),
            (&reply, None, Some(Send)),
            (&apply, None, None),
            (&read, Some(Send), Some(Send)),
            (&pref, Some(Send), Some(Send)),
            (&unit, None, None),
            (&release, None, None),
        ];
        for (r, view, annotate) in table {
            let g = |m| match gate(m, r) {
                Gate::Refuse(_) => None,
                g => Some(g),
            };
            assert_eq!(g(Mode::View), view, "{} in View", r.method());
            assert_eq!(g(Mode::Annotate), annotate, "{} in Annotate", r.method());
            assert_eq!(g(Mode::Edit), Some(Send), "{} in Direct edit", r.method());
            // A control that sends it says the same before it is built.
            if r.needs().is_some() {
                for m in [Mode::View, Mode::Annotate, Mode::Edit] {
                    assert_eq!(gate(m, r), control(m, &r.method()), "{} {m:?}", r.method());
                }
            }
        }
    }

    #[test]
    fn a_refusal_names_the_mode_that_allows_it() {
        let Gate::Refuse(why) = control(Mode::View, "annotation.reply") else {
            panic!("refused");
        };
        assert!(why.contains("Annotate"), "{why}");
        let Gate::Refuse(why) = control(Mode::Annotate, "undo") else {
            panic!("refused");
        };
        assert!(why.contains("Direct edit"), "{why}");
        assert_eq!(control(Mode::Annotate, "view.save"), Gate::Propose);
    }
}
