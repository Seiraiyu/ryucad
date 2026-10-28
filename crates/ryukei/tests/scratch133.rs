use std::time::Instant;

fn run(data: &[u8], old: bool) -> (Result<usize, String>, std::time::Duration) {
    unsafe {
        if old {
            std::env::set_var("S133_OLD", "1")
        } else {
            std::env::remove_var("S133_OLD")
        }
    }
    let t = Instant::now();
    let r = std::panic::catch_unwind(|| ryukei_testkit::along_bytes::carried(data));
    let out = match r {
        Ok(Some((b, _))) => Ok(b.faces().count()),
        Ok(None) => Err("none".into()),
        Err(e) => Err(format!("panic {:?}", e.downcast_ref::<String>())),
    };
    (out, t.elapsed())
}

#[test]
fn scratch133() {
    let mut x: u64 = std::env::var("SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let n: usize = std::env::var("N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let mut stats = std::collections::BTreeMap::<String, usize>::new();
    let mut tn = std::time::Duration::ZERO;
    let mut to = std::time::Duration::ZERO;
    let mut done = 0;
    while done < n {
        let mut data = [0u8; 16];
        for d in &mut data {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *d = (x >> 33) as u8;
        }
        data[11] &= !1; // choose a sweep along a path
        let (new, a) = run(&data, false);
        if new == Err("none".into()) && a.as_millis() < 1 {
            continue;
        }
        eprintln!("NEW {data:?} {new:?} {a:?}");
        let (old, b) = if std::env::var_os("SKIPOLD").is_some() {
            (new.clone(), a)
        } else {
            run(&data, true)
        };
        done += 1;
        tn += a;
        to += b;
        eprintln!("CASE {data:?} new {new:?} {a:?} old {old:?} {b:?}");
        let key = format!("new={:?} old={:?}", new.is_ok(), old.is_ok());
        *stats.entry(key.clone()).or_default() += 1;
        if new.is_ok() != old.is_ok() || new.is_err() && new != Err("none".into()) {
            eprintln!("DIFF {data:?} new {new:?} {a:?} old {old:?} {b:?}");
        }
    }
    eprintln!("{stats:?} new {tn:?} old {to:?}");
}
