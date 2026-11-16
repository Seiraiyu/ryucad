//! The hosted review service's client for every ryu product (design
//! 667): a thin client of its API (`/api/v1`, design 65), so an agent
//! reads checks and renders and comments with the CLI it models with.
//! Moved out of ryucad unchanged; ryucad and ryueda name themselves
//! through [`Product`]. CLI only: it talks to a service, not a session.
//!
//! The service's error codes become `review_*` codes, so they are stable
//! here too; one this build doesn't know is `review_failed` with the
//! service's code in the details.
