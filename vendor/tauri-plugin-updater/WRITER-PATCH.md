# Writer's updater 2.11.0 patch

Source: crates.io `tauri-plugin-updater` 2.11.0, archive checksum
`b28d8cabdeb0564f03ae261963de4bc3d98321cd3d213e76a81b7d344e5df606`.
Upstream: <https://github.com/tauri-apps/plugins-workspace/tree/updater-v2.11.0/plugins/updater>.
Licenses and required build/permission inputs are retained verbatim. Unneeded
changelog and development lockfile are omitted.

Only `src/updater.rs` is changed:

- `Updater::update_from_response(Value)` factors the existing handle construction
  out of `check()`. Writer supplies its already bounded and authenticated feed;
  this method performs no networking. Upstream `check()` still delegates through
  the same construction code after its existing parsing.
- `Update::verify(&[u8])` exposes the existing `verify_signature` call, so Writer's
  bounded downloader and install-time guard retain the exact Tauri verification.

This avoids a second unbounded fetch to construct an `Update` whose installation
context is private. Upstream extraction, installer launch, shutdown, and artifact
signature algorithms are unchanged. Writer never invokes upstream `check()` or
`download()`; the plugin has no WebView command permissions in Writer.

When updating this dependency, reapply/review these two small APIs, compare against
upstream, rerun the cross-language security fixtures and native updater tests,
and run native Clippy. Do not remove the patch until equivalent APIs are available
upstream. Cargo's root patch is intentional; no registry cache is modified.
