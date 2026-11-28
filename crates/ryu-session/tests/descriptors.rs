//! #1133: a detached child keeps none of its starter's descriptors: a
//! pipe it was never given doesn't stay open while it runs. A test binary
//! of its own, so no other test's spawn can take the pipe while it is open.

#![cfg(unix)]

use std::io::Read;
use std::os::fd::FromRawFd;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn a_detached_child_keeps_no_inherited_descriptor() {
    let mut fds = [0; 2];
    // Not close-on-exec: as a pipe that reached this process by accident.
    // SAFETY: a plain pipe(2) into an array of two.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let mut cmd = Command::new("sleep");
    cmd.arg("10");
    let mut child = ryu_session::client::spawn_detached(&mut cmd).unwrap();
    // SAFETY: our own write end, closed once.
    unsafe { libc::close(fds[1]) };
    // SAFETY: our own read end, owned by the File from here.
    let mut read = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let start = Instant::now();
    let n = read.read(&mut [0u8; 1]).unwrap();
    let took = start.elapsed();
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(n, 0);
    assert!(
        took < Duration::from_secs(5),
        "the child held the pipe open for {took:?}"
    );
}
