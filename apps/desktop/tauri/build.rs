fn main() {
    // A published desktop update must retain account login and deletion support.
    println!("cargo:rerun-if-env-changed=WRITER_PUBLIC_RELEASE");
    println!("cargo:rerun-if-env-changed=WRITER_ACCOUNT_SERVICE_URL");
    if std::env::var_os("WRITER_PUBLIC_RELEASE").is_some() {
        assert!(
            std::env::var_os("CARGO_FEATURE_GOOGLE_SIGN_IN").is_some(),
            "Desktop releases require the google-sign-in feature."
        );
        for name in [
            "WRITER_GOOGLE_PROJECT_ID",
            "WRITER_FIREBASE_API_KEY",
            "WRITER_GOOGLE_DESKTOP_CLIENT_ID",
            "WRITER_ACCOUNT_SERVICE_URL",
        ] {
            println!("cargo:rerun-if-env-changed={name}");
            assert!(
                std::env::var(name).is_ok_and(|v| !v.is_empty()),
                "Desktop release account configuration is incomplete: {name}"
            );
        }
    }
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target == "windows" || target == "macos" {
        tauri_build::build();
    }
    if target == "windows" {
        // Tauri's application manifest is attached to the main binary, not
        // integration-test executables. The real updater fixture links Tauri's
        // TaskDialogIndirect import, which requires Common Controls v6 even
        // though the fixture never opens a window.
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-tests=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'");
    }
}
