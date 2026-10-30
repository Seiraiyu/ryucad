//! Approval rules (`approvals` in `ryupdm.json`, format 2) and how
//! reviews meet them. [`judge`] is the one ryupdm rule the hosted service
//! links: the service's verdict on a pull request and a clone's
//! verification of a release tag ([`crate::attest`]) agree by
//! construction.
