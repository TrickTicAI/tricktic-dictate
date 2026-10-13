/// The public display name. It must equal `productName` in tauri.conf.json.
/// The bundle identifier and the names derived from it are in `identity.rs`;
/// see docs/release/identity.md.
// Probes that include only some modules may not use it on every target.
#[allow(unused_macros)]
macro_rules! product_name {
    () => {
        "TrickTic Dictate"
    };
}

/// Public legal pages and support address. They must equal `LEGAL_BASE` and
/// `SUPPORT_EMAIL` in apps/web/src/product.ts; see docs/release/legal/README.md.
#[allow(dead_code)]
pub const LEGAL_BASE: &str = "https://legal.tricktic.com/tricktic-dictate";
#[allow(dead_code)]
pub const SUPPORT_EMAIL: &str = "contact@tricktic.com";
