//! Saved file transport: fixed-size reads, exact content lengths, no base64/JSON audio copies.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::stream::try_unfold;
use std::{io, pin::Pin};
use tokio::io::{AsyncRead, AsyncSeek, AsyncSeekExt};

const CHUNK: usize = 48 * 1024; // Multiple of three: only the final base64 block is padded.
trait Reader: AsyncRead + AsyncSeek + Send + Unpin {}
impl<T: AsyncRead + AsyncSeek + Send + Unpin> Reader for T {}

pub(super) struct Upload {
    reader: Option<Box<dyn Reader>>,
    prefix: Vec<u8>,
    expected_hash: Option<(Sha256, [u8; 32])>,
    pub len: u64,
    pub checkpoint: std::path::PathBuf,
    pub content_hash: Option<[u8; 32]>,
    pub format: AudioFormat,
    // Keep a disposable silence-prepared file alive through upload and generation.
    _prepared: Option<crate::local_stt::silence::PreparedCopy>,
    _normalized: Option<normalize::Normalized>,
}
impl Upload {
    pub async fn open(
        audio: &SavedAudio,
        format: AudioFormat,
        limit: u64,
    ) -> Result<Self, ApiError> {
        let original = tokio::fs::File::open(&audio.path)
            .await
            .map_err(|_| storage_error())?;
        let metadata = original.metadata().await.map_err(|_| storage_error())?;
        if !metadata.is_file() || metadata.len() != audio.metadata.byte_length {
            // Retain the previous size gate for an oversized manifest without reading it.
            return Err(if audio.metadata.byte_length > limit {
                size_error()
            } else {
                error(
                    ErrorCode::AudioNotSaved,
                    "Saved audio size does not match its manifest.",
                    false,
                )
            });
        }
        let normalized_file = if audio.prepared.is_none() {
            normalize::normalize(audio).await
        } else {
            None
        };
        let prepared = crate::local_stt::silence::prepared_copy(
            normalized_file.as_ref().map_or(audio, |n| &n.audio),
        )
        .await
        .ok();
        let mut normalized = None;
        let mut prefix = Vec::new();
        let mut chosen_format = format;
        if let Some(pcm) = prepared.as_ref().and_then(|p| p.audio.prepared.as_ref()) {
            if pcm.byte_length > 0
                && pcm.byte_length % 2 == 0
                && pcm.byte_length <= 256 * 1024 * 1024
            {
                if let Ok(file) = tokio::fs::File::open(&pcm.path).await {
                    if file
                        .metadata()
                        .await
                        .map(|m| m.len() == pcm.byte_length)
                        .unwrap_or(false)
                    {
                        prefix = wav_header(pcm.byte_length)?;
                        normalized = Some((Box::new(file) as Box<dyn Reader>, pcm.byte_length));
                    }
                }
            }
        }
        let is_prepared = normalized.is_some();
        let (mut reader, length) = normalized.unwrap_or((Box::new(original), metadata.len()));
        let len = length + prefix.len() as u64;
        if len == 0 || len > limit {
            return Err(size_error());
        }
        if is_prepared {
            chosen_format = AudioFormat::parse(&ProviderId::Gemini, "audio/wav")?;
        }
        let mut signature = [0; 12];
        if prefix.is_empty() {
            let count = len.min(signature.len() as u64) as usize;
            reader
                .read_exact(&mut signature[..count])
                .await
                .map_err(|_| storage_error())?;
            if !chosen_format.matches(&signature[..count]) {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "Audio container does not match its declared MIME type.",
                    false,
                ));
            }
            reader
                .seek(std::io::SeekFrom::Start(0))
                .await
                .map_err(|_| storage_error())?;
        }
        crate::latency::detail(
            audio.metadata.id,
            "cloud_upload",
            if is_prepared {
                "prepared_16k_mono"
            } else {
                "original"
            },
        );
        Ok(Self {
            reader: Some(reader),
            prefix,
            expected_hash: None,
            len,
            checkpoint: audio.path.with_extension("gemini-upload.json"),
            content_hash: None,
            format: chosen_format,
            _prepared: prepared,
            _normalized: normalized_file,
        })
    }
    pub async fn digest(&mut self, credential: &HeaderValue) -> Result<[u8; 32], ApiError> {
        let reader = self.reader.as_mut().ok_or_else(storage_error)?;
        reader
            .seek(std::io::SeekFrom::Start(0))
            .await
            .map_err(|_| storage_error())?;
        let mut hash = Sha256::new();
        hash.update(credential.as_bytes());
        hash.update(self.format.mime);
        hash.update(&self.prefix);
        let mut content_hash = Sha256::new();
        content_hash.update(&self.prefix);
        let mut buffer = [0; CHUNK];
        let mut count = self.prefix.len() as u64;
        loop {
            let size = reader
                .read(&mut buffer)
                .await
                .map_err(|_| storage_error())?;
            if size == 0 {
                break;
            }
            count += size as u64;
            if count > self.len {
                return Err(storage_error());
            }
            hash.update(&buffer[..size]);
            content_hash.update(&buffer[..size]);
        }
        if count != self.len {
            return Err(storage_error());
        }
        reader
            .seek(std::io::SeekFrom::Start(0))
            .await
            .map_err(|_| storage_error())?;
        self.content_hash = Some(content_hash.finalize().into());
        let digest = hash.finalize().into();
        let mut initial = Sha256::new();
        initial.update(credential.as_bytes());
        initial.update(self.format.mime);
        self.expected_hash = Some((initial, digest));
        Ok(digest)
    }
    /// Bounded Files chunks; the reader remains open so a resumed suffix can seek safely.
    pub async fn range(&mut self, offset: u64, count: usize) -> Result<Vec<u8>, ApiError> {
        if offset.saturating_add(count as u64) > self.len {
            return Err(storage_error());
        }
        let mut bytes = vec![0; count];
        let prefix_offset = (offset as usize).min(self.prefix.len());
        let head = (self.prefix.len() - prefix_offset).min(count);
        bytes[..head].copy_from_slice(&self.prefix[prefix_offset..prefix_offset + head]);
        let reader = self.reader.as_mut().ok_or_else(storage_error)?;
        reader
            .seek(std::io::SeekFrom::Start(
                offset.saturating_sub(self.prefix.len() as u64),
            ))
            .await
            .map_err(|_| storage_error())?;
        reader
            .read_exact(&mut bytes[head..])
            .await
            .map_err(|_| storage_error())?;
        Ok(bytes)
    }
    pub async fn body(&mut self) -> Result<reqwest::Body, ApiError> {
        self.stream(None).await
    }
    pub async fn inline_body(
        &mut self,
        prefix: Vec<u8>,
        suffix: Vec<u8>,
    ) -> Result<reqwest::Body, ApiError> {
        self.stream(Some((prefix, suffix))).await
    }
    async fn stream(
        &mut self,
        envelope: Option<(Vec<u8>, Vec<u8>)>,
    ) -> Result<reqwest::Body, ApiError> {
        let file = self.reader.take().ok_or_else(storage_error)?;
        let reader: Pin<Box<dyn AsyncRead + Send>> =
            Box::pin(std::io::Cursor::new(self.prefix.clone()).chain(file));
        // Prefetch at most one chunk for tiny clips. This also allows immediate HTTP
        // quota/auth responses without interleaving file I/O into multipart transmission.
        if self.len <= CHUNK as u64 && envelope.is_none() {
            let mut reader = reader;
            let mut bytes = vec![0; self.len as usize];
            reader
                .read_exact(&mut bytes)
                .await
                .map_err(|_| storage_error())?;
            let mut extra = [0];
            if reader.read(&mut extra).await.map_err(|_| storage_error())? != 0 {
                return Err(storage_error());
            }
            if let Some((mut hash, expected)) = self.expected_hash.take() {
                hash.update(&bytes);
                if <[u8; 32]>::from(hash.finalize()) != expected {
                    return Err(storage_error());
                }
            }
            return Ok(reqwest::Body::from(bytes));
        }
        let inline = envelope.is_some();
        let (head, tail) = envelope.unwrap_or_default();
        let state = (
            reader,
            self.len,
            Some(head),
            Some(tail),
            self.expected_hash.take(),
        );
        let stream = try_unfold(
            state,
            move |(mut reader, remaining, mut head, mut tail, mut hash)| async move {
                if let Some(bytes) = head.take().filter(|b| !b.is_empty()) {
                    return Ok::<_, io::Error>(Some((
                        bytes,
                        (reader, remaining, head, tail, hash),
                    )));
                }
                if remaining > 0 {
                    let count = remaining.min(CHUNK as u64) as usize;
                    let mut bytes = vec![0; count];
                    reader.read_exact(&mut bytes).await?;
                    if let Some((hasher, _)) = hash.as_mut() {
                        hasher.update(&bytes);
                    }
                    // Verify EOF before releasing the last chunk. A changed length fails this attempt.
                    if remaining == count as u64 {
                        if let Some((hasher, expected)) = hash.take() {
                            if <[u8; 32]>::from(hasher.finalize()) != expected {
                                return Err(io::Error::other("Saved audio changed."));
                            }
                        }
                        let mut extra = [0];
                        if reader.read(&mut extra).await? != 0 {
                            return Err(io::Error::other("Saved audio changed."));
                        }
                    }
                    let bytes = if inline {
                        STANDARD.encode(bytes).into_bytes()
                    } else {
                        bytes
                    };
                    return Ok(Some((
                        bytes,
                        (reader, remaining - count as u64, head, tail, hash),
                    )));
                }
                if let Some(bytes) = tail.take().filter(|b| !b.is_empty()) {
                    return Ok(Some((bytes, (reader, remaining, head, tail, hash))));
                }
                Ok(None)
            },
        );
        Ok(reqwest::Body::wrap_stream(stream))
    }
}
fn wav_header(length: u64) -> Result<Vec<u8>, ApiError> {
    let size = u32::try_from(length).map_err(|_| size_error())?;
    let mut wav = crate::local_stt::silence::wav(&[]);
    wav[4..8].copy_from_slice(&(size + 36).to_le_bytes());
    wav[40..44].copy_from_slice(&size.to_le_bytes());
    Ok(wav)
}
pub(super) fn gemini_body(options: &TranscriptionOptions, audio: Value) -> Value {
    json!({
        "systemInstruction": {"parts": [{"text": preservation_prompt(options)}]},
        "contents": [{"role": "user", "parts": [{"text": "Transcribe the speech in this audio."}, audio]}],
        "generationConfig": {"temperature": 0, "candidateCount": 1, "responseMimeType": "text/plain"}
    })
}
pub(super) fn inline_envelope(
    options: &TranscriptionOptions,
    mime: &str,
) -> Result<(Vec<u8>, Vec<u8>), ApiError> {
    // Serialize the actual escaped UTF-8 prompt/config once, replacing only the audio string.
    let body = serde_json::to_string(&gemini_body(
        options,
        json!({"inlineData": {"mimeType": mime, "data": "WRITER_AUDIO_PLACEHOLDER"}}),
    ))
    .map_err(|_| invalid_response())?;
    let (prefix, suffix) = body
        .split_once("\"data\":\"WRITER_AUDIO_PLACEHOLDER\"")
        .ok_or_else(invalid_response)?;
    Ok((
        format!("{prefix}\"data\":\"").into_bytes(),
        format!("\"{suffix}").into_bytes(),
    ))
}
pub(super) fn inline_size(length: u64, prefix: usize, suffix: usize) -> Result<u64, ApiError> {
    length
        .checked_add(2)
        .and_then(|n| (n / 3).checked_mul(4))
        .and_then(|n| n.checked_add(prefix as u64))
        .and_then(|n| n.checked_add(suffix as u64))
        .ok_or_else(size_error)
}
