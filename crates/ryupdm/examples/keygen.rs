//! Makes the hosted service's Ed25519 signing key (design 261). The seed
//! (64 hex) goes to stdout, for a pipe into `gh secret set`; the public
//! key's line for `attest::HOSTED_KEYS` goes to stderr. Nothing is
//! written to disk.
//!
//! ```sh
//! cargo run -q -p ryupdm --features testing --example keygen \
//!   | gh secret set RYUPDM_SIGNING_KEY --repo Seiraiyu/ryupdm-service
//! ```

fn main() {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("a random source");
    let public = ryupdm::attest::public_key(&seed);
    eprintln!(
        "(\"{}\", \"{}\"),",
        ryupdm::attest::key_id(&public),
        ryupdm::attest::hex(&public)
    );
    // No newline: the secret is exactly the 64 hex digits.
    print!("{}", ryupdm::attest::hex(&seed));
}
