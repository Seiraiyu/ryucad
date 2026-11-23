//! #799: a window started by `ryucad open` from a shell whose job is
//! killed on close (Windows Terminal's tabs, some agents' shells) must
//! outlive the job, so `spawn_detached` asks to break away. A test binary
//! of its own: this process joins jobs.
#![cfg(windows)]

use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// This process in a new kill-on-close job, with `flags` besides.
fn join_job(flags: u32) -> HANDLE {
    // SAFETY: plain Win32 calls on a job this test makes and on this
    // process. The job stays open until the process ends, so it kills
    // nothing of the runner's.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        assert!(!job.is_null());
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | flags;
        assert_ne!(
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                u32::try_from(std::mem::size_of_val(&info)).unwrap(),
            ),
            0
        );
        assert_ne!(AssignProcessToJobObject(job, GetCurrentProcess()), 0);
        job
    }
}

fn in_job(child: &std::process::Child, job: HANDLE) -> bool {
    let mut inside = 0;
    // SAFETY: the child's own handle, open while `child` is.
    unsafe {
        assert_ne!(IsProcessInJob(child.as_raw_handle(), job, &mut inside), 0);
    }
    inside != 0
}

fn pause() -> std::process::Command {
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/c", "ping", "-n", "30", "127.0.0.1"]);
    cmd
}

fn end(mut child: std::process::Child) {
    child.kill().ok();
    child.wait().ok();
}

/// One test: a process can't leave a job, so the order matters. A job
/// that allows breakaway first, then one nested in it that doesn't.
#[test]
fn a_detached_child_breaks_away_from_its_starters_job_when_it_may() {
    // Whether whatever runs this test lets its children break away at
    // all: a runner's own job may not, and then only the fallback can be
    // seen.
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let free = match pause().creation_flags(CREATE_BREAKAWAY_FROM_JOB).spawn() {
        Ok(c) => {
            end(c);
            true
        }
        Err(e) if e.raw_os_error() == Some(5) => false,
        Err(e) => panic!("{e}"),
    };
    let job = join_job(JOB_OBJECT_LIMIT_BREAKAWAY_OK);
    let child = ryu_session::client::spawn_detached(&mut pause()).unwrap();
    if free {
        assert!(
            !in_job(&child, job),
            "the detached child is still in its starter's job"
        );
    } else {
        eprintln!("this runner's job refuses breakaway: only the fallback is tested");
    }
    end(child);

    // A job that refuses breakaway: the child is still started, inside
    // it, as before #799.
    let strict = join_job(0);
    let child = ryu_session::client::spawn_detached(&mut pause()).unwrap();
    assert!(in_job(&child, strict));
    end(child);
}
