//! The Branches tab and Git's merge: what the window asks the daemon for it (an `impl Workspace`
//! block) and, from 27.F's Task 7 on, what it does with the answers.
//! Its fields live on `core::branches::Branches`.

use super::*;

impl Workspace {
    /// Reads the Branches tab's branches (`branch.list`) and worktrees
    /// (`worktree.list`) again; the worktrees' previews follow, one at a
    /// time.
    pub fn load_branches(&mut self) {
        self.branches.branches_at = Some(Instant::now());
        self.branches.branches_waiting = true;
        self.send(Request::BranchList {}, Pending::BranchList);
        self.send(Request::WorktreeList {}, Pending::Branches);
    }

    /// Reads the worktrees again after this project changed, once the tab
    /// has read them: this worktree's state moved, so its previews are
    /// made again.
    pub(super) fn reload_branches(&mut self) {
        if self.branches.list.is_some() && !self.branches.branches_waiting {
            self.load_branches();
        }
    }

    /// The Branches tab is shown: read the worktrees again when the last
    /// reading is older than [`crate::core::branches::BRANCHES_EVERY`].
    /// Returns when to look again.
    pub fn branches_shown(&mut self, now: Instant) -> Duration {
        let every = crate::core::branches::BRANCHES_EVERY;
        let due = self.branches.branches_at.map_or(now, |t| t + every);
        if now >= due && !self.branches.branches_waiting {
            self.load_branches();
            return every;
        }
        due.saturating_duration_since(now)
            .max(Duration::from_millis(100))
    }

    /// Asks for the next merge preview the tab lacks, if none is on its
    /// way.
    pub(super) fn ask_preview(&mut self) {
        let Some((path, key)) = self.branches.next_preview() else {
            return;
        };
        self.branches.previewing = Some((path.clone(), key.clone()));
        let rev = crate::core::branches::compare_rev(&path);
        self.send(Request::MergePreview { rev }, Pending::Preview(path, key));
    }

    /// Opens the worktree at `path` in a new window of this app, or brings
    /// the windows already on it to the front: what `ryucad open` does.
    pub fn open_worktree(&mut self, path: &str) {
        let dir = Path::new(path);
        let app = std::env::current_exe().ok();
        if let Err(e) = ryucad::client::open_app(dir, Some(dir), app.as_deref()) {
            self.fail(e);
        }
    }

    /// Opens the Check out dialog for `branch`, naming every other
    /// participant attached to this daemon (question 436).
    pub fn ask_checkout(&mut self, branch: &str) {
        let me = self.link.participant.clone();
        let attached = self
            .session
            .people
            .others(&me)
            .map(|p| p.name.clone())
            .collect();
        self.branches.checkout = self.branches.checkout_of(branch, attached);
    }

    /// Checks out the dialog's branch in this folder (`branch.switch`),
    /// guarded by the project revision the list was read at; the dialog
    /// closes when it is done, or says why it was refused.
    pub fn confirm_checkout(&mut self) {
        let Some(c) = self.branches.checkout.as_mut().filter(|c| !c.busy) else {
            return;
        };
        c.busy = true;
        c.refused = None;
        let req = Request::BranchSwitch {
            branch: c.branch.clone(),
            expected_revision: c.expected.clone(),
        };
        self.send(req, Pending::Switch);
    }

    /// Settles a merge conflict by `choice` (`ours`, `theirs`, or `repair`
    /// with `repair`), against the revision of its part the window last
    /// saw. The answer brings the new status; a failure is kept in the
    /// merge's `failed`.
    pub fn resolve(&mut self, conflict: &str, choice: &str, repair: Option<&str>) {
        let Some(m) = &mut self.branches.merge else {
            return;
        };
        let Some(c) = m.conflict(conflict) else {
            return;
        };
        let req = crate::core::merge::resolution(c, choice, repair, &self.model.parts);
        m.resolving = true;
        m.failed = None;
        self.send(req, Pending::Resolve);
    }
}
