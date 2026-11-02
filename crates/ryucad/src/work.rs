//! What ryucad hands a kernel call: its request's cancel (plan 15.A). A
//! cancel stops a heal, a boolean or a mesh at its next charge, not only
//! between them, and the request answers `cancelled` as at a boundary,
//! keeping nothing the kernel left unfinished.

use crate::error::{Code, Error};
use ryu_session::job::Job;
use ryukei::work::Work;

/// The work for a kernel call `job` makes: its cancel flag, no budget.
pub fn work(job: &Job) -> Work<'_> {
    match job.kernel_cancel() {
        Some(flag) => Work::unlimited().with_cancel(flag),
        None => Work::unlimited(),
    }
}

/// The request's `cancelled` when the kernel stopped for its cancel, else
/// nothing (the caller maps the error as before).
pub fn stopped(e: &ryukei::Error, job: &Job) -> Option<Error> {
    match e {
        ryukei::Error::Cancelled => Some(
            job.check()
                .err()
                .unwrap_or_else(|| Error::new(Code::Cancelled, "cancelled inside a kernel call")),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn a_kernel_cancel_answers_as_the_job_does() {
        let flag = Arc::new(AtomicBool::new(false));
        let job = Job::new("t", "p", "rebuild", flag.clone());
        assert!(stopped(&ryukei::Error::Degenerate, &job).is_none());
        flag.store(true, Ordering::SeqCst);
        let e = stopped(&ryukei::Error::Cancelled, &job).expect("cancelled");
        assert_eq!(e.code, Code::Cancelled);
        assert_eq!(e.details["token"], "t");
        // With no job (the flag can't be set), still a typed cancel.
        let e = stopped(&ryukei::Error::Cancelled, &Job::default()).unwrap();
        assert_eq!(e.code, Code::Cancelled);
    }

    #[test]
    fn the_work_carries_the_jobs_flag() {
        let flag = Arc::new(AtomicBool::new(true));
        let job = Job::new("t", "p", "rebuild", flag);
        let w = work(&job);
        let (b, _) = ryukei::primitive::block(
            ryukei::name::OpKey(1),
            0.01,
            0.01,
            0.01,
            &ryukei::geom::Rigid::IDENTITY,
        )
        .unwrap();
        assert_eq!(
            ryukei::check::check_intersections_with(&b, &w),
            Err(ryukei::Error::Cancelled)
        );
    }
}
