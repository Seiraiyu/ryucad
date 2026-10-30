//! Attestations: what the hosted service signs when a release record
//! becomes official under approval rules, carried as the message of the
//! annotated tag `ryupdm/<part>/<label>` (`…/<label>.obsolete` for an
//! obsolete record). A clone verifies them offline against the keys this
//! build trusts ([`HOSTED_KEYS`] and `ryupdm.json`'s `approvals.keys`).
//!
//! The message is the attestation's canonical JSON, then
//! `-----BEGIN RYUPDM SIGNATURE-----`, `{"ed25519": "<128 hex>", "key":
//! "<16 hex>"}` on one line and `-----END RYUPDM SIGNATURE-----`. The
//! signature is Ed25519 over `ryupdm.attestation.1\0` followed by the
//! canonical bytes, so the same attestation and key give the same tag.
