//! About's legal links and the bundled license notices (MW-RELEASE-4).
//!
//! The page names a link or a notice; it never supplies a URL or a file path
//! that is opened as given. URLs come from `product.rs` and notice paths from
//! `notices/index.json`, which scripts/desktop/notices.mjs writes at build time.
use crate::product::{LEGAL_BASE, SUPPORT_EMAIL};
use crate::storage::Result;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

/// Largest notice the view reads. The Rust notices are the biggest, near 2 MB.
const MAX_NOTICE_BYTES: u64 = 32 * 1024 * 1024;

pub fn link(kind: &str) -> Option<String> {
    Some(match kind {
        "privacy" | "terms" | "security" => format!("{LEGAL_BASE}/{kind}"),
        "support" => format!("mailto:{SUPPORT_EMAIL}"),
        // Where each cloud provider issues API keys ("Get a key" in Settings).
        "key-openai" => "https://platform.openai.com/api-keys".into(),
        "key-gemini" => "https://aistudio.google.com/apikey".into(),
        "key-grok" => "https://console.x.ai".into(),
        "key-openrouter" => "https://openrouter.ai/keys".into(),
        _ => return None,
    })
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Notice {
    pub path: String,
    pub title: String,
    pub group: String,
    /// `text` is readable in the app; `source` is source code shipped beside it.
    pub kind: String,
}

#[derive(Deserialize)]
struct Index {
    version: u32,
    notices: Vec<Notice>,
}

fn contained(path: &str) -> bool {
    path.starts_with("notices/")
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// The notices this build ships. A build without an index (a developer run)
/// has none, which the view says instead of showing an error.
pub fn index(resources: &Path) -> Result<Vec<Notice>> {
    let bytes = match std::fs::read(resources.join("notices/index.json")) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("The license list is unreadable. Repair the installation.".into()),
    };
    let index: Index = serde_json::from_slice(&bytes)
        .map_err(|_| "The license list is damaged. Repair the installation.")?;
    if index.version != 1
        || index
            .notices
            .iter()
            .any(|n| !contained(&n.path) || !matches!(n.kind.as_str(), "text" | "source"))
    {
        return Err("The license list is damaged. Repair the installation.".into());
    }
    Ok(index.notices)
}

pub fn read(resources: &Path, path: &str) -> Result<String> {
    if !index(resources)?
        .iter()
        .any(|n| n.path == path && n.kind == "text")
    {
        return Err("That notice is not part of this installation.".into());
    }
    let file = resources.join(path);
    let missing = || "This notice is missing. Repair the installation.".to_string();
    if std::fs::metadata(&file).map_err(|_| missing())?.len() > MAX_NOTICE_BYTES {
        return Err("This notice is too large to show. It is in the notices folder.".into());
    }
    Ok(String::from_utf8_lossy(&std::fs::read(file).map_err(|_| missing())?).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resources(index: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("notices/source")).unwrap();
        std::fs::write(dir.path().join("notices/index.json"), index).unwrap();
        std::fs::write(dir.path().join("notices/WEB-NOTICES.txt"), "react MIT").unwrap();
        std::fs::write(
            dir.path().join("notices/source/ffmpeg.tar.xz"),
            [0xfd, 0x37],
        )
        .unwrap();
        std::fs::write(dir.path().join("secret.txt"), "private").unwrap();
        dir
    }

    #[test]
    fn links_are_fixed_and_unknown_kinds_are_refused() {
        assert_eq!(
            link("privacy").unwrap(),
            "https://legal.tricktic.com/tricktic-dictate/privacy"
        );
        assert_eq!(link("support").unwrap(), "mailto:contact@tricktic.com");
        assert_eq!(
            link("key-openrouter").unwrap(),
            "https://openrouter.ai/keys"
        );
        assert!(link("key-local").is_none());
        assert!(link("https://evil.invalid").is_none());
        assert!(link("").is_none());
    }

    #[test]
    fn only_indexed_text_notices_are_read() {
        let dir = resources(
            r#"{"version":1,"notices":[
                {"path":"notices/WEB-NOTICES.txt","title":"Web","group":"Application","kind":"text"},
                {"path":"notices/source/ffmpeg.tar.xz","title":"FFmpeg","group":"Runtime","kind":"source"}]}"#,
        );
        assert_eq!(index(dir.path()).unwrap().len(), 2);
        assert_eq!(
            read(dir.path(), "notices/WEB-NOTICES.txt").unwrap(),
            "react MIT"
        );
        for refused in [
            "notices/source/ffmpeg.tar.xz",
            "notices/../secret.txt",
            "secret.txt",
            "notices/index.json",
        ] {
            assert!(read(dir.path(), refused).is_err(), "{refused}");
        }
        std::fs::remove_file(dir.path().join("notices/WEB-NOTICES.txt")).unwrap();
        assert!(read(dir.path(), "notices/WEB-NOTICES.txt")
            .unwrap_err()
            .contains("missing"));
    }

    #[test]
    fn an_index_that_escapes_the_notices_folder_is_refused() {
        for bad in [
            r#"{"version":1,"notices":[{"path":"notices/../secret.txt","title":"x","group":"x","kind":"text"}]}"#,
            r#"{"version":1,"notices":[{"path":"secret.txt","title":"x","group":"x","kind":"text"}]}"#,
            r#"{"version":1,"notices":[{"path":"notices/WEB-NOTICES.txt","title":"x","group":"x","kind":"script"}]}"#,
            r#"{"version":2,"notices":[]}"#,
            "not json",
        ] {
            let dir = resources(bad);
            assert!(index(dir.path()).is_err(), "{bad}");
            assert!(read(dir.path(), "secret.txt").is_err());
        }
        let empty = tempfile::tempdir().unwrap();
        assert!(index(empty.path()).unwrap().is_empty());
    }
}
