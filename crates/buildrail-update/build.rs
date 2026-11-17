//! Rebuild when the embedded update public key changes.
//!
//! `src/keys.rs` reads `BUILDRAIL_UPDATE_PUBLIC_KEY` with `option_env!`;
//! without this line Cargo would keep a stale (or missing) key after the
//! variable changes.
fn main() {
    println!("cargo:rerun-if-env-changed=BUILDRAIL_UPDATE_PUBLIC_KEY");
}
