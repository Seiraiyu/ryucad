//! The command palette's matching (design 199, Command registry >
//! Palette): fuzzy over a command's title, category and id, recently run
//! commands first. What `@` searches is the product's. Pure: a window
//! draws it. Moved from ryucad-app by ryueda 3.C.

use crate::command::{Command, Kind};

/// How many recently run commands the palette remembers.
pub const RECENT: usize = 8;

/// How well `query` matches `text`: its characters in order, ignoring
/// case, `None` if they aren't all there. Higher is better: runs of
/// characters together, matches at the start of a word, and a match at
/// the very start score more; a long text scores a little less.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().collect();
    let mut s = 0;
    let mut qi = 0;
    let mut prev: Option<usize> = None;
    for (i, c) in t.iter().enumerate() {
        if qi == q.len() {
            break;
        }
        if c.to_lowercase().eq(std::iter::once(q[qi])) {
            let word_start = i == 0 || !t[i - 1].is_alphanumeric();
            s += 1;
            if word_start {
                s += 8;
            }
            if i == 0 {
                s += 4;
            }
            if prev.is_some_and(|p| p + 1 == i) {
                s += 5;
            }
            prev = Some(i);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    // The query whole, as typed, counts most: at a word's start most.
    let lower: Vec<char> = t.iter().flat_map(|c| c.to_lowercase()).collect();
    if lower.len() == t.len() {
        let at = (0..=lower.len().saturating_sub(q.len())).find(|i| lower[*i..].starts_with(&q));
        if let Some(i) = at {
            s += 20;
            if i == 0 || !t[i - 1].is_alphanumeric() {
                s += 20;
            }
        }
    }
    Some(s * 16 - t.len() as i32)
}

/// A command's best score over its title, category and id.
fn command_score<C>(query: &str, c: &Command<C>) -> Option<i32> {
    [
        score(query, c.title).map(|s| s + 2),
        score(query, c.category.title()),
        score(query, c.id),
    ]
    .into_iter()
    .flatten()
    .max()
}

/// The commands of `registry` matching `query`, best first; recently run
/// commands (`recent`, newest first) before the rest, and with an empty
/// query in that order then the registry's. Disabled commands stay in:
/// the window shows them greyed with their reason.
pub fn commands<C>(
    registry: &'static [Command<C>],
    query: &str,
    recent: &[&'static str],
) -> Vec<&'static Command<C>> {
    let mut found: Vec<(bool, usize, i32, usize, &'static Command<C>)> = registry
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == Kind::Action || c.kind == Kind::Toggle)
        // A context row's own copy of a command the menus list under the
        // same title (`view.fit_row` beside `view.fit`) is one action to
        // whoever types its name: listed once (#519).
        .filter(|(_, c)| {
            c.menu
                || !registry
                    .iter()
                    .any(|o| o.menu && o.id != c.id && o.title == c.title)
        })
        .filter_map(|(i, c)| {
            let s = command_score(query, c)?;
            let r = recent.iter().position(|id| *id == c.id);
            Some((r.is_none(), r.unwrap_or(0), -s, i, c))
        })
        .collect();
    found.sort_by_key(|(not_recent, r, s, i, _)| (*not_recent, *r, *s, *i));
    found.into_iter().map(|x| x.4).collect()
}

/// Notes a command as just run.
pub fn ran(recent: &mut Vec<&'static str>, id: &'static str) {
    recent.retain(|r| *r != id);
    recent.insert(0, id);
    recent.truncate(RECENT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_is_in_order_and_ignores_case() {
        assert!(score("zf", "Zoom to fit").is_some());
        assert!(score("fz", "Zoom to fit").is_none());
        assert!(score("ZOOM", "zoom to fit").is_some());
        assert_eq!(score("", "anything"), Some(0));
        // Word starts and runs beat scattered letters.
        assert!(score("zoom", "Zoom to fit") > score("zoom", "Zap of a room"));
        assert!(score("fit", "Zoom to fit") > score("fit", "Find it"));
    }

    #[test]
    fn recent_commands_come_first_and_choices_never_show() {
        use crate::command::{Category, cmd};
        const TOY: &[Command<()>] = &[
            cmd("view.fit", "Zoom to fit", Category::View),
            cmd("view.front", "Front", Category::View),
            cmd("file.recent", "Open recent", Category::File).choices(),
        ];
        let ids = |q: &str, r: &[&'static str]| -> Vec<&str> {
            commands(TOY, q, r).iter().map(|c| c.id).collect()
        };
        assert_eq!(ids("", &[]), ["view.fit", "view.front"]);
        assert_eq!(ids("", &["view.front"]), ["view.front", "view.fit"]);
        assert_eq!(ids("zf", &[]), ["view.fit"]);
        let mut recent = vec!["view.fit"];
        ran(&mut recent, "view.front");
        assert_eq!(recent, ["view.front", "view.fit"]);
    }
}
