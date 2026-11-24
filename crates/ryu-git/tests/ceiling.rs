//! `GIT_CEILING_DIRECTORIES` fences the search for a repository (#1004).
//! Its own test binary: it sets the variable, which no other test in this
//! process may read while it does.

use ryu_git::discover_upwards;

#[test]
fn a_ceiling_stops_the_search_for_a_repository_above() {
    let outer = tempfile::tempdir().unwrap();
    let ok = ryu_git::git_env::isolated()
        .arg("init")
        .arg("-q")
        .arg(outer.path())
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let fenced = outer.path().join("fence");
    let project = fenced.join("project");
    std::fs::create_dir_all(&project).unwrap();
    assert!(discover_upwards(&project, true).is_some());
    assert!(ryu_git::drivers::discover(&project, false).is_some());
    // SAFETY: the only test in this binary; nothing else reads the
    // environment while it is set.
    unsafe { std::env::set_var("GIT_CEILING_DIRECTORIES", &fenced) };
    let isolated = discover_upwards(&project, true);
    let plain = ryu_git::drivers::discover(&project, false);
    // A ceiling that isn't above the folder is ignored, as Git ignores it.
    unsafe { std::env::set_var("GIT_CEILING_DIRECTORIES", project.join("elsewhere")) };
    let unrelated = discover_upwards(&project, true);
    unsafe { std::env::remove_var("GIT_CEILING_DIRECTORIES") };
    assert!(isolated.is_none());
    assert!(plain.is_none());
    assert!(unrelated.is_some());
}
