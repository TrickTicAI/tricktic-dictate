//! Gemini Files' resumable protocol only; generation/multipart requests are never replayed here.
use super::*;
use crate::recordings::disk::{read_json, write_json};
use serde::{Deserialize, Serialize};
const CHUNK: usize = 8 * 1024 * 1024;
#[derive(Deserialize, Serialize)]
struct Checkpoint {
    binding: String,
    location: String,
    name: String,
    expires_at_ms: u64,
    // Persist the validated final asset before returning it to generation.
    file: Option<Value>,
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
impl CloudProvider {
    fn upload_url(&self, location: &str) -> Result<reqwest::Url, ApiError> {
        let url = reqwest::Url::parse(location).map_err(|_| invalid_response())?;
        let endpoint = reqwest::Url::parse(&self.endpoint).map_err(|_| invalid_response())?;
        if location.len() > 8192
            || url.origin() != endpoint.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid_response());
        }
        Ok(url)
    }
    pub(super) async fn resumable_file(
        &self,
        upload: &mut Upload,
        credential: &HeaderValue,
        digest: [u8; 32],
    ) -> Result<Value, ApiError> {
        let client = self.client.as_ref().ok_or_else(invalid_response)?;
        let binding = format!(
            "{:x}",
            Sha256::digest(
                [
                    digest.as_slice(),
                    self.endpoint.as_bytes(),
                    b"gemini-files-v1"
                ]
                .concat()
            )
        );
        let mut cp = read_json::<Checkpoint>(&upload.checkpoint)
            .await
            .ok()
            .filter(|cp| cp.binding == binding && cp.expires_at_ms > now());
        if let Some(checkpoint) = cp.as_ref() {
            self.upload_url(&checkpoint.location)?;
            self.file_url(&checkpoint.name)?;
            if checkpoint.file.is_some() {
                let response = client
                    .get(self.file_url(&checkpoint.name)?)
                    .header("x-goog-api-key", credential.clone())
                    .send()
                    .await
                    .map_err(transport_error)?;
                if response.status() == reqwest::StatusCode::NOT_FOUND {
                    cp = None;
                } else {
                    return response_json(response).await;
                }
            }
        }
        let mut offset = 0;
        if let Some(checkpoint) = cp.as_ref() {
            let response = client
                .post(self.upload_url(&checkpoint.location)?)
                .header("x-goog-api-key", credential.clone())
                .header("X-Goog-Upload-Command", "query")
                .header("content-length", 0)
                .send()
                .await
                .map_err(transport_error)?;
            if matches!(response.status().as_u16(), 404 | 410)
                || response
                    .headers()
                    .get("X-Goog-Upload-Status")
                    .is_some_and(|s| s == "cancelled")
            {
                cp = None;
            } else {
                if !response.status().is_success() {
                    response_json(response).await?;
                    return Err(invalid_response());
                }
                let status = response
                    .headers()
                    .get("X-Goog-Upload-Status")
                    .and_then(|s| s.to_str().ok())
                    .unwrap_or("")
                    .to_owned();
                offset = response
                    .headers()
                    .get("X-Goog-Upload-Size-Received")
                    .and_then(|s| s.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .filter(|n| *n <= upload.len)
                    .ok_or_else(invalid_response)?;
                if status == "final" {
                    // A lost finalize response never issues another finalize. The requested asset name is known before start.
                    let file = response_json(
                        client
                            .get(self.file_url(&checkpoint.name)?)
                            .header("x-goog-api-key", credential.clone())
                            .send()
                            .await
                            .map_err(transport_error)?,
                    )
                    .await?;
                    let checkpoint = cp.as_mut().unwrap();
                    checkpoint.file =
                        Some(json!({"name":file["name"],"uri":file["uri"],"state":file["state"]}));
                    write_json(&upload.checkpoint, checkpoint).await?;
                    return Ok(file);
                }
                if status != "active" {
                    return Err(invalid_response());
                }
            }
        }
        if cp.is_none() {
            offset = 0;
            let endpoint = reqwest::Url::parse(&self.endpoint).map_err(|_| invalid_response())?;
            let mut start = endpoint.clone();
            start.set_path(&format!(
                "/upload{}/files",
                endpoint.path().trim_end_matches('/')
            ));
            let name = format!("files/writer-{}", uuid::Uuid::new_v4().simple());
            let response = client
                .post(start)
                .header("x-goog-api-key", credential.clone())
                .header("X-Goog-Upload-Protocol", "resumable")
                .header("X-Goog-Upload-Command", "start")
                .header("X-Goog-Upload-Header-Content-Length", upload.len)
                .header("X-Goog-Upload-Header-Content-Type", upload.format.mime)
                .json(&json!({"file":{"name":name,"display_name":"Writer dictation"}}))
                .send()
                .await
                .map_err(transport_error)?;
            if !response.status().is_success() {
                response_json(response).await?;
                return Err(invalid_response());
            }
            let location = response
                .headers()
                .get("X-Goog-Upload-URL")
                .and_then(|s| s.to_str().ok())
                .ok_or_else(invalid_response)?
                .to_owned();
            self.upload_url(&location)?;
            cp = Some(Checkpoint {
                binding,
                location,
                name,
                expires_at_ms: now() + 47 * 3600 * 1000,
                file: None,
            });
            // The URL is a capability: private disk only, never diagnostics or public job/recording JSON.
            write_json(&upload.checkpoint, cp.as_ref().unwrap()).await?;
        }
        let mut cp = cp.unwrap();

        while offset < upload.len {
            let count = (upload.len - offset).min(CHUNK as u64) as usize;
            let bytes = upload.range(offset, count).await?;
            let response = client
                .post(self.upload_url(&cp.location)?)
                .header("x-goog-api-key", credential.clone())
                .header("X-Goog-Upload-Offset", offset)
                .header("X-Goog-Upload-Command", "upload")
                .header("content-length", count)
                .body(bytes)
                .send()
                .await
                .map_err(transport_error)?;
            if !response.status().is_success() {
                response_json(response).await?;
                return Err(invalid_response());
            }
            if response
                .headers()
                .get("X-Goog-Upload-Status")
                .is_none_or(|s| s != "active")
            {
                return Err(invalid_response());
            }
            offset += count as u64;
        }
        // Ensure the exact source has not changed since the identity pass before finalizing.
        if upload.digest(credential).await? != digest {
            return Err(storage_error());
        }
        let response = client
            .post(self.upload_url(&cp.location)?)
            .header("x-goog-api-key", credential.clone())
            .header("X-Goog-Upload-Offset", offset)
            .header("X-Goog-Upload-Command", "finalize")
            .header("content-length", 0)
            .send()
            .await
            .map_err(transport_error)?;
        let file = response_json(response)
            .await?
            .get("file")
            .cloned()
            .ok_or_else(invalid_response)?;
        // Some test/legacy receivers assign their own file names; remember the actual validated name.
        let name = file["name"].as_str().ok_or_else(invalid_response)?;
        let url = self.file_url(name)?;
        if file["uri"].as_str() != Some(url.as_str()) {
            return Err(invalid_response());
        }
        cp.name = name.to_owned();
        cp.file = Some(json!({"name":file["name"],"uri":file["uri"],"state":file["state"]}));
        write_json(&upload.checkpoint, &cp).await?;
        Ok(file)
    }
}
#[cfg(test)]
mod tests;
