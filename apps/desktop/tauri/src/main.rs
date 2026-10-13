#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#[macro_use]
mod product;
mod account;
mod identity;
#[cfg(any(windows, target_os = "macos"))]
mod capture;
#[cfg(any(windows, target_os = "macos"))]
mod microphones;
#[cfg(any(windows, target_os = "macos", test))]
mod notices;
#[cfg(any(windows, target_os = "macos", test))]
mod panel_health;
#[cfg(any(windows, target_os = "macos"))]
mod shell;
mod storage;
#[cfg(any(windows, target_os = "macos", test))]
mod tray_mark;
mod update_security;
#[cfg(any(windows, target_os = "macos", test))]
mod window_memory;
#[cfg(any(windows, target_os = "macos"))]
fn main() {
    shell::run();
}
#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!(concat!(
        product_name!(),
        " desktop currently targets Windows and macOS; use the browser client on Linux."
    ));
}

#[cfg(test)]
mod product_tests {
    #[test]
    fn product_name_matches_tauri_config() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["productName"], product_name!());
    }
    #[test]
    fn legal_links_match_the_web_constants() {
        let web = include_str!("../../../web/src/product.ts");
        for (name, value) in [
            ("LEGAL_BASE", crate::product::LEGAL_BASE),
            ("SUPPORT_EMAIL", crate::product::SUPPORT_EMAIL),
        ] {
            assert!(
                web.contains(&format!("export const {name} = '{value}';")),
                "{name}"
            );
        }
    }
}
