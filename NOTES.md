# Bug burn-down notes

- 2026-10-27, lap 1: no issue carries a `burn` label (the label doesn't exist in
  Seiraiyu/ryucad), so the lap took open bug #123 as the burn-down target.
- #123: `feature.list` now reads the session's (cached) rebuild of the part, so a
  feature whose file loads but doesn't build is `ok: false` with its error, and
  features after it read `dependency_failed`. Cost: a cold `--no-daemon` list now
  rebuilds the part. The issue's `feature edit` warning and `assembly tree`
  notes were left for later.
