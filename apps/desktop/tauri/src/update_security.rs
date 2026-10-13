//! Authenticated desktop release contract; exercised on Linux without a WebView.
#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
use base64::{engine::general_purpose::STANDARD, Engine};
use minisign_verify::{PublicKey, Signature};
use reqwest::{Client, Url};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Write, path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, String>;
pub const MAX_FEED: usize = 64 * 1024;
pub const MAX_ARTIFACT: usize = 300 * 1024 * 1024;
const MAX_LIFETIME: u64 = 30 * 24 * 60 * 60;
/// Signed metadata binds a release to this app. Copies installed before the
/// identity change (0.39 and earlier) only read `writer_release`, bound to
/// [`LEGACY_APP`]; this client reads only [`FEED_FIELD`], bound to [`APP`]. The
/// publisher keeps both envelopes in every entry (docs/release/identity.md).
pub const APP: &str = "com.tricktic.dictate";
pub const LEGACY_APP: &str = "com.metrix.writer";
pub const FEED_FIELD: &str = "dictate_release";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub payload: String,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub app: String,
    pub platform: String,
    pub version: String,
    pub sha256: String,
    pub size: u64,
    pub issued_at: u64,
    pub expires_at: u64,
}

#[derive(Clone)]
pub struct Release {
    pub envelope: Envelope,
    pub manifest: Manifest,
    pub url: Url,
}

fn invalid() -> String {
    "The update could not be authenticated. Check again or use a verified manual release.".into()
}

pub fn verify_signature(bytes: &[u8], encoded: &str, public_key: &str) -> Result<()> {
    let key = STANDARD.decode(public_key.trim()).map_err(|_| invalid())?;
    let signature = STANDARD.decode(encoded.trim()).map_err(|_| invalid())?;
    let key = PublicKey::decode(std::str::from_utf8(&key).map_err(|_| invalid())?)
        .map_err(|_| invalid())?;
    let signature = Signature::decode(std::str::from_utf8(&signature).map_err(|_| invalid())?)
        .map_err(|_| invalid())?;
    // Matches Tauri's verifier, including verification of the trusted comment.
    key.verify(bytes, &signature, true).map_err(|_| invalid())
}

impl Envelope {
    pub fn authenticate(&self, key: &str) -> Result<Manifest> {
        self.authenticate_for(key, &[APP])
    }

    /// Either identity's envelope: the rollback floor and retained-archive
    /// harnesses, which may hold a Metrix Writer release.
    pub fn authenticate_any(&self, key: &str) -> Result<Manifest> {
        self.authenticate_for(key, &[APP, LEGACY_APP])
    }

    fn authenticate_for(&self, key: &str, apps: &[&str]) -> Result<Manifest> {
        if self.payload.len() > 16 * 1024 || self.signature.len() > 4096 {
            return Err(invalid());
        }
        let bytes = STANDARD.decode(&self.payload).map_err(|_| invalid())?;
        verify_signature(&bytes, &self.signature, key)?;
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if manifest.schema != 1
            || !apps.contains(&manifest.app.as_str())
            || !["windows-x86_64", "darwin-aarch64"].contains(&manifest.platform.as_str())
            || manifest.size == 0
            || manifest.size > MAX_ARTIFACT as u64
            || manifest.sha256.len() != 64
            || !manifest
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || manifest.expires_at <= manifest.issued_at
            || manifest.expires_at - manifest.issued_at > MAX_LIFETIME
        {
            return Err(invalid());
        }
        let version = Version::parse(&manifest.version).map_err(|_| invalid())?;
        if !version.pre.is_empty() || !version.build.is_empty() {
            return Err(invalid());
        }
        Ok(manifest)
    }
}

impl Manifest {
    pub fn validate(
        &self,
        installed: &str,
        platform: &str,
        now: u64,
        previous: Option<&Self>,
    ) -> Result<()> {
        let version = Version::parse(&self.version).map_err(|_| invalid())?;
        if self.platform != platform
            || version < Version::parse(installed).map_err(|_| invalid())?
        {
            return Err(concat!(
                "The update has the wrong platform or would roll ",
                product_name!(),
                " back."
            )
            .into());
        }
        if self.issued_at > now.saturating_add(300) || self.expires_at <= now {
            return Err("Update metadata has expired or the system clock is incorrect. Check again; dictation remains available.".into());
        }
        if let Some(old) = previous {
            if old.platform != self.platform
                || version < Version::parse(&old.version).map_err(|_| invalid())?
                || self.issued_at < old.issued_at
                || (self.version == old.version
                    && (self.sha256 != old.sha256 || self.size != old.size))
            {
                return Err(
                    "The update feed replayed or replaced a previously verified release.".into(),
                );
            }
        }
        Ok(())
    }

    pub fn verify_bytes(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 != self.size || format!("{:x}", Sha256::digest(bytes)) != self.sha256
        {
            return Err(
                "The downloaded artifact does not match its signed release metadata.".into(),
            );
        }
        Ok(())
    }
}

pub fn allowed_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || url.host_str() != Some("updates.tricktic.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.path().starts_with("/writer/")
    {
        return Err(concat!(
            "The update destination is not an approved ",
            product_name!(),
            " HTTPS address."
        )
        .into());
    }
    Ok(url)
}

pub fn channel_url(endpoint: &str, platform: &str) -> Result<Url> {
    if !["windows-x86_64", "darwin-aarch64"].contains(&platform) {
        return Err(
            "Automatic updates are not available for this operating system or processor.".into(),
        );
    }
    allowed_url(&endpoint.replace("{{platform}}", platform))
}

pub fn response_error(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        404 => "The requested update feed or download has not been published (HTTP 404). Try again later.".into(),
        401 | 403 => format!("The update server denied access (HTTP {}). Try again later or contact release support.", status.as_u16()),
        429 => "The update server received too many requests (HTTP 429). Wait a moment before checking again.".into(),
        500..=599 => format!("The update server is temporarily unavailable (HTTP {}). Try again later.", status.as_u16()),
        300..=399 => concat!("The update server returned a redirect. ", product_name!(), " requires a direct, verified download address.").into(),
        204 => "The update server returned no release metadata (HTTP 204). Try again later.".into(),
        206 => "The update server returned an incomplete response (HTTP 206). Try again.".into(),
        _ => format!("The update server returned an unexpected response (HTTP {}).", status.as_u16()),
    }
}

fn connection_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "The update server did not respond in time. Try again.".into()
    } else if error.is_connect() {
        concat!(product_name!(), " could not connect securely to the update server. Check access to updates.tricktic.com, including any VPN or proxy.").into()
    } else {
        concat!(
            product_name!(),
            " could not complete the update request. Try again."
        )
        .into()
    }
}

pub fn client() -> Result<Client> {
    Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(15 * 60))
        .build()
        .map_err(|_| invalid())
}

/// The ceiling is enforced before every append, including chunked responses.
/// Dropping the future on cancellation releases all staged bytes and the socket.
pub async fn fetch(
    client: &Client,
    url: Url,
    limit: usize,
    cancel: &CancellationToken,
    mut progress: impl FnMut(usize),
) -> Result<Vec<u8>> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("Update download canceled.".into()),
        result = async {
            let mut response = client.get(url).send().await.map_err(connection_error)?;
            // Never follow redirects, accept partial ranges, or treat a 204 as
            // authenticated evidence that the installed version is current.
            if response.status() != reqwest::StatusCode::OK {
                return Err(response_error(response.status()));
            }
            if response.content_length().is_some_and(|n| n > limit as u64) {
                return Err("Update response exceeds its byte limit.".into());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| "Update transfer interrupted.")? {
                if chunk.len() > limit.saturating_sub(bytes.len()) {
                    return Err("Update response exceeds its byte limit.".into());
                }
                bytes.try_reserve_exact(chunk.len()).map_err(|_| "Not enough memory to stage the update.")?;
                bytes.extend_from_slice(&chunk);
                progress(bytes.len());
            }
            Ok(bytes)
        } => result,
    }
}

pub fn release_from_feed(feed: &serde_json::Value, platform: &str, key: &str) -> Result<Release> {
    let platforms = feed["platforms"]
        .as_object()
        .ok_or("The update server returned malformed release metadata. Try again later.")?;
    let entry = platforms.get(platform).ok_or_else(|| {
        let system = match platform {
            "darwin-aarch64" => "macOS (Apple Silicon)",
            "windows-x86_64" => "Windows (64-bit)",
            _ => "this operating system or processor",
        };
        format!(
            concat!(
                "This update feed has no release for {system}. Your installed ",
                product_name!(),
                " is unchanged. Try again after a compatible release is published."
            ),
            system = system
        )
    })?;
    let envelope: Envelope =
        serde_json::from_value(entry[FEED_FIELD].clone()).map_err(|_| {
            concat!(
                "This channel needs signed ",
                product_name!(),
                " release metadata. Check later or use a verified manual release."
            )
        })?;
    let manifest = envelope.authenticate(key)?;
    if feed["version"].as_str() != Some(&manifest.version) || manifest.platform != platform {
        return Err(invalid());
    }
    let url = allowed_url(entry["url"].as_str().ok_or_else(invalid)?)?;
    let filename = if platform == "windows-x86_64" {
        "setup.exe"
    } else {
        "Metrix.Writer.app.tar.gz"
    };
    if url.path()
        != format!(
            "/writer/releases/{}/{}/{filename}",
            manifest.version, manifest.sha256
        )
    {
        return Err(invalid());
    }
    Ok(Release {
        envelope,
        manifest,
        url,
    })
}

pub fn previous(path: &Path, key: &str) -> Result<Option<Manifest>> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Could not read the update rollback record.".into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FEED as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() > MAX_FEED {
        return Err(invalid());
    }
    let envelope: Envelope = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    // Expiration does not erase the highest authenticated version ever seen. A
    // record carried over from a Metrix Writer install is the same publisher's
    // signature over the same releases, so it remains the floor.
    envelope.authenticate_any(key).map(Some)
}

pub fn remember(path: &Path, envelope: &Envelope) -> Result<()> {
    let parent = path.parent().ok_or_else(invalid)?;
    std::fs::create_dir_all(parent).map_err(|_| invalid())?;
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|_| invalid())?;
    staged
        .write_all(&serde_json::to_vec(envelope).map_err(|_| invalid())?)
        .map_err(|_| invalid())?;
    staged.as_file().sync_all().map_err(|_| invalid())?;
    staged
        .persist(path)
        .map_err(|_| "Could not save the update rollback record.")?;
    Ok(())
}

#[cfg(test)]
#[path = "update_security_tests.rs"]
mod tests;
