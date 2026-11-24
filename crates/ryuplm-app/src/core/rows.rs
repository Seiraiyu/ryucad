//! The Items list as windows of `item.list` rows (design 938, The
//! window): the rows the tile shows and [`AHEAD`] beyond on each side,
//! asked again when the filter, the sort or the catalogue's revision
//! moves, one ask in flight, the last filter winning. Nothing here grows
//! with the catalogue: a frame reads at most the rows held.

use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// How many rows beyond those shown are read, each way.
pub const AHEAD: usize = 200;

/// How long the list must rest after scrolling before a window is asked
/// for: a window asked mid-scroll is still out when a search is typed, and
/// the search's window queues behind it on the daemon (at 100,000 items
/// each costs a freshness scan, ryu-session's #1018).
pub const PAUSE: Duration = Duration::from_millis(80);

/// The list's filter and sort, as `view.set` and `item.list` take them.
pub type Filter = ryuplm::view::List;

/// One item as the list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub number: String,
    pub name: Option<String>,
    pub class: Option<String>,
    pub unit: String,
    pub make_buy: Option<String>,
    pub state: String,
    pub label: Option<String>,
    pub errors: u32,
    pub warnings: u32,
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

impl Row {
    pub fn read(v: &Value) -> Option<Row> {
        Some(Row {
            id: text(&v["id"])?,
            number: text(&v["number"])?,
            name: text(&v["name"]),
            class: text(&v["class"]),
            unit: text(&v["unit"]).unwrap_or_else(|| "each".into()),
            make_buy: text(&v["make_buy"]),
            state: text(&v["state"]).unwrap_or_default(),
            label: text(&v["label"]),
            errors: v["errors"].as_u64().unwrap_or(0) as u32,
            warnings: v["warnings"].as_u64().unwrap_or(0) as u32,
        })
    }

    /// `item/<id>`.
    pub fn reference(&self) -> String {
        format!("item/{}", self.id)
    }
}

/// One `item.list` asked: what it was asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub filter: Filter,
    pub revision: Option<String>,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Default)]
pub struct Rows {
    /// The filter the list shows.
    pub filter: Filter,
    /// The catalogue's revision, as the window last heard it.
    pub revision: Option<String>,
    /// How many items match the held rows' filter: `item.list`'s `total`.
    pub total: Option<usize>,
    /// The held rows start at this index.
    pub offset: usize,
    pub rows: Vec<Row>,
    /// The filter and revision the held rows were read at.
    held: Option<(Filter, Option<String>)>,
    /// The ask in flight.
    out: Option<Ask>,
    /// The rows the tile showed at its last frame: first and count.
    want: (usize, usize),
    /// The last read failed: the tile greys what it holds.
    pub error: bool,
    /// When the rows shown last moved (a scroll).
    scrolled: Option<Instant>,
}

impl Rows {
    /// The tile shows rows `first..first + count`.
    pub fn view(&mut self, first: usize, count: usize) {
        self.view_at(first, count, Instant::now());
    }

    /// [`Rows::view`] at `now`.
    pub fn view_at(&mut self, first: usize, count: usize, now: Instant) {
        let want = (first, count.max(1));
        if want != self.want {
            self.want = want;
            self.scrolled = Some(now);
        }
    }

    /// The catalogue's revision is `revision` (`project.changed`'s or
    /// `project.info`'s `catalogue`).
    pub fn moved(&mut self, revision: &str) {
        self.revision = Some(revision.to_owned());
    }

    /// A new filter or sort: the list goes back to its top.
    pub fn filter(&mut self, f: Filter) {
        if f != self.filter {
            self.filter = f;
            self.want.0 = 0;
        }
    }

    fn covered(&self) -> bool {
        let (first, count) = self.want;
        let end = self.total.map_or(first + count, |t| (first + count).min(t));
        self.held.as_ref() == Some(&(self.filter.clone(), self.revision.clone()))
            && first >= self.offset
            && end <= self.offset + self.rows.len()
    }

    /// What to ask now, if anything: nothing while one is out or while
    /// the rows held cover the rows shown at this filter and revision.
    /// It counts as asked.
    pub fn ask(&mut self) -> Option<Ask> {
        self.ask_at(Instant::now())
    }

    /// [`Rows::ask`] at `now`. Rows held at this filter and revision that
    /// only scrolled out of view wait for the list to rest ([`PAUSE`]); a
    /// new filter, sort or revision, or nothing held, asks at once.
    pub fn ask_at(&mut self, now: Instant) -> Option<Ask> {
        if self.out.is_some() || self.covered() {
            return None;
        }
        let current = self.held.as_ref() == Some(&(self.filter.clone(), self.revision.clone()));
        if current
            && self
                .scrolled
                .is_some_and(|t| now.saturating_duration_since(t) < PAUSE)
        {
            return None;
        }
        let (first, count) = self.want;
        let offset = first.saturating_sub(AHEAD);
        let a = Ask {
            filter: self.filter.clone(),
            revision: self.revision.clone(),
            offset,
            limit: first - offset + count + AHEAD,
        };
        self.out = Some(a.clone());
        Some(a)
    }

    /// `item.list`'s answer to `a`.
    pub fn answered(&mut self, a: &Ask, v: &Value) {
        self.out = None;
        self.error = false;
        self.total = v["total"].as_u64().map(|t| t as usize);
        self.offset = a.offset;
        self.rows = v["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Row::read)
            .collect();
        self.held = Some((a.filter.clone(), a.revision.clone()));
    }

    /// `a` was refused, or the link went: the rows held stay, greyed,
    /// and the next ask tries again.
    pub fn failed(&mut self, a: &Ask) {
        if self.out.as_ref() == Some(a) {
            self.out = None;
        }
        self.error = true;
    }

    /// Forgets the ask in flight (the link was lost with it).
    pub fn lost(&mut self) {
        self.out = None;
        self.held = None;
    }

    /// Row `i` of the list, when held.
    pub fn get(&self, i: usize) -> Option<&Row> {
        i.checked_sub(self.offset).and_then(|k| self.rows.get(k))
    }

    /// How long until a scrolled list may ask, when it waits to rest.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        let t = self.scrolled?;
        Some(PAUSE.saturating_sub(now.saturating_duration_since(t))).filter(|d| !d.is_zero())
    }

    /// Whether the rows held were read at the filter and revision shown:
    /// not a previous filter's, still on screen until the new ones come.
    pub fn current(&self) -> bool {
        self.held.as_ref() == Some(&(self.filter.clone(), self.revision.clone()))
    }

    /// Whether an ask is out.
    pub fn busy(&self) -> bool {
        self.out.is_some()
    }

    /// `item.list`'s params for `a`.
    pub fn params(a: &Ask) -> Value {
        let mut p = json!({"offset": a.offset, "limit": a.limit});
        for (k, v) in [
            ("text", &a.filter.text),
            ("class", &a.filter.class),
            ("state", &a.filter.state),
            ("faults", &a.filter.faults),
            ("source", &a.filter.source),
            ("sort", &a.filter.sort),
        ] {
            if let Some(v) = v.as_ref().filter(|v| !v.is_empty()) {
                p[k] = json!(v);
            }
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn answer(a: &Ask, total: usize) -> Value {
        let end = (a.offset + a.limit).min(total);
        let rows: Vec<Value> = (a.offset..end)
            .map(|k| {
                json!({"id": format!("{k:013}"), "number": format!("N-{k}"), "name": null,
                            "class": null, "unit": "each", "make_buy": null, "state": "in_work",
                            "errors": 0, "warnings": u32::from(k % 7 == 0)})
            })
            .collect();
        json!({"schema": 1, "total": total, "offset": a.offset, "count": rows.len(), "items": rows})
    }

    #[test]
    fn the_rows_in_view_and_200_beyond_one_ask_at_a_time() {
        let mut r = Rows::default();
        r.moved("rev-1");
        r.view(0, 30);
        let a = r.ask().expect("the first rows");
        assert_eq!((a.offset, a.limit), (0, 230));
        assert_eq!(r.ask(), None, "one in flight");
        r.answered(&a, &answer(&a, 1000));
        assert_eq!(r.total, Some(1000));
        assert_eq!(r.get(7).unwrap().number, "N-7");
        assert_eq!(r.get(7).unwrap().warnings, 1);
        r.view(10, 30);
        assert_eq!(r.ask(), None, "held");
        r.view(500, 30);
        assert_eq!(r.ask(), None, "a scroll waits for the list to rest");
        let b = r.ask_at(Instant::now() + PAUSE).unwrap();
        assert_eq!((b.offset, b.limit), (300, 430));
        assert!(r.get(0).is_some(), "the old rows stay until the new arrive");
        r.answered(&b, &answer(&b, 1000));
        assert!(r.get(0).is_none() && r.get(520).is_some());
        // Past the end: what there is.
        r.view(990, 30);
        let c = r.ask_at(Instant::now() + PAUSE).unwrap();
        r.answered(&c, &answer(&c, 1000));
        r.view(990, 30);
        assert_eq!(r.ask(), None);
        assert_eq!(Rows::params(&c)["offset"], 790);
    }

    #[test]
    fn a_new_filter_or_revision_asks_again_and_the_last_wins() {
        let mut r = Rows::default();
        r.moved("rev-1");
        r.view(0, 30);
        let a = r.ask().unwrap();
        // Typed while the first is out: held, not sent.
        r.filter(Filter {
            text: Some("b".into()),
            ..Default::default()
        });
        r.filter(Filter {
            text: Some("bo".into()),
            ..Default::default()
        });
        assert_eq!(r.ask(), None);
        r.answered(&a, &answer(&a, 1000));
        let b = r.ask().expect("the filter typed since");
        assert_eq!(b.filter.text.as_deref(), Some("bo"));
        assert_eq!(b.offset, 0, "a new filter starts at the top");
        assert_eq!(Rows::params(&b)["text"], "bo");
        r.answered(&b, &answer(&b, 40));
        assert_eq!(r.ask(), None);
        r.moved("rev-2");
        let c = r.ask().expect("the catalogue moved");
        assert_eq!(c.revision.as_deref(), Some("rev-2"));
        r.failed(&c);
        assert!(r.error);
        assert!(r.get(3).is_some(), "a failed read keeps the rows, greyed");
    }

    #[test]
    fn a_sort_and_filters_go_as_item_list_takes_them() {
        let a = Ask {
            filter: Filter {
                class: Some("fastener".into()),
                state: Some("released".into()),
                faults: Some("errors".into()),
                sort: Some("number".into()),
                source: Some("erp".into()),
                text: None,
            },
            revision: None,
            offset: 0,
            limit: 230,
        };
        let p = Rows::params(&a);
        assert_eq!(
            p,
            json!({"class": "fastener", "state": "released", "faults": "errors",
                   "source": "erp", "sort": "number", "offset": 0, "limit": 230})
        );
        assert!(ryuplm::proto::Request::from_rpc("item.list", Some(p)).is_ok());
    }

    #[test]
    fn a_scroll_waits_to_rest_and_a_search_never_does() {
        let t0 = Instant::now();
        let mut r = Rows::default();
        r.moved("rev-1");
        r.view_at(0, 30, t0);
        let a = r.ask_at(t0).expect("nothing held: at once");
        r.answered(&a, &answer(&a, 100_000));
        // Scrolled far: nothing asked mid-scroll.
        r.view_at(5_000, 30, t0);
        assert_eq!(r.ask_at(t0 + Duration::from_millis(10)), None);
        assert_eq!(
            r.wait(t0 + Duration::from_millis(10)),
            Some(Duration::from_millis(70))
        );
        // A search typed meanwhile asks at once, from the top.
        r.filter(Filter {
            text: Some("bolt".into()),
            ..Default::default()
        });
        assert!(!r.current(), "the held rows are the old filter's");
        let b = r
            .ask_at(t0 + Duration::from_millis(10))
            .expect("a new filter asks at once");
        assert_eq!((b.offset, b.filter.text.as_deref()), (0, Some("bolt")));
    }
}
