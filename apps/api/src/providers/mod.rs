//! Provider implementations receive only server-resolved audio and owned options.
pub mod cloud;

use crate::transcription::{
    ApiError, ExecutionLocation, ProviderId, RecordingMetadata, Transcript, TranscriptionOptions,
};
use async_trait::async_trait;
use std::{path::PathBuf, sync::Arc};

/// Internal only: never deserialize this path from an HTTP request.
#[derive(Clone, Debug)]
pub struct PreparedAudio {
    pub path: PathBuf,
    /// Coherent committed prefix of mono 16 kHz signed little-endian PCM.
    pub byte_length: u64,
}

#[derive(Clone, Debug)]
pub struct SavedAudio {
    pub metadata: RecordingMetadata,
    pub path: PathBuf,
    pub prepared: Option<PreparedAudio>,
}

#[derive(Clone, Debug)]
pub struct ProviderInfo {
    pub id: ProviderId,
    pub execution_location: ExecutionLocation,
    pub configured: bool,
    pub available: bool,
}

#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    fn info(&self) -> ProviderInfo;
    /// Only adapter failures qualify. Admission, configuration and storage errors
    /// are handled separately and never spend automatic retries.
    fn automatic_retry(&self, error: &ApiError) -> bool {
        error.retryable
            && matches!(
                error.code,
                crate::transcription::ErrorCode::ProviderTimeout
                    | crate::transcription::ErrorCode::ProviderFailed
                    | crate::transcription::ErrorCode::RateLimited
            )
    }
    /// A managed worker restart may outlast the backoff. Waiting for it must not
    /// consume both retries before the model can accept saved audio again.
    fn automatic_retry_ready(&self) -> bool {
        true
    }
    fn clear_preview(&self) {}
    async fn preview(
        &self,
        _audio: &SavedAudio,
        _options: &TranscriptionOptions,
    ) -> Result<String, ApiError> {
        Err(ApiError::unavailable("Live preview"))
    }
    async fn transcribe(
        &self,
        audio: &SavedAudio,
        options: &TranscriptionOptions,
    ) -> Result<Transcript, ApiError>;
}

/// Credentials and the managed worker come from the process-wide setup; an unavailable
/// setup store degrades to explicit environment configuration only.
pub fn adapters() -> Vec<Arc<dyn TranscriptionProvider>> {
    match crate::desktop_settings::global() {
        Ok(setup) => {
            let mut adapters = cloud::adapters_with(setup.credentials.clone());
            adapters.extend(crate::local_stt::adapters_with(Some(&setup)));
            adapters
        }
        Err(_) => {
            let mut adapters = cloud::adapters();
            adapters.extend(crate::local_stt::adapters_with(None));
            adapters
        }
    }
}

pub struct UnavailableProvider(pub ProviderInfo);

#[async_trait]
impl TranscriptionProvider for UnavailableProvider {
    fn info(&self) -> ProviderInfo {
        self.0.clone()
    }
    async fn transcribe(
        &self,
        _: &SavedAudio,
        _: &TranscriptionOptions,
    ) -> Result<Transcript, ApiError> {
        Err(ApiError::unavailable("Transcription provider"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        recordings::{AudioReader, RecordingStore},
        transcription::{ContractCatalog, ErrorCode, Recording},
    };
    use uuid::Uuid;

    struct FixtureStore(Recording);
    #[async_trait]
    impl RecordingStore for FixtureStore {
        async fn save(&self, _: RecordingMetadata, _: AudioReader) -> Result<Recording, ApiError> {
            Err(ApiError::unavailable("Fixture ingestion"))
        }
        async fn get(&self, id: Uuid) -> Result<Recording, ApiError> {
            assert_eq!(id, self.0.metadata.id);
            Ok(self.0.clone())
        }
        async fn audio(&self, id: Uuid) -> Result<SavedAudio, ApiError> {
            let recording = self.get(id).await?;
            Ok(SavedAudio {
                prepared: None,
                metadata: recording.metadata,
                path: PathBuf::from("fixture-only.webm"),
            })
        }
        async fn delete(&self, _: Uuid) -> Result<(), ApiError> {
            panic!("Transcription must not delete retained audio")
        }
    }

    struct FixtureProvider(Transcript);
    #[async_trait]
    impl TranscriptionProvider for FixtureProvider {
        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                id: ProviderId::Local,
                execution_location: ExecutionLocation::SelfHostedServer,
                configured: true,
                available: true,
            }
        }
        async fn transcribe(
            &self,
            audio: &SavedAudio,
            _: &TranscriptionOptions,
        ) -> Result<Transcript, ApiError> {
            assert_eq!(audio.metadata.byte_length, 4096);
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn provider_and_persistence_are_independently_injectable_without_keys() {
        let fixture: ContractCatalog =
            serde_json::from_str(include_str!("../../../../packages/contracts/examples.json"))
                .unwrap();
        let crate::transcription::JobStatus::Succeeded { result } = fixture.job.data.status else {
            panic!()
        };
        let store: Arc<dyn RecordingStore> = Arc::new(FixtureStore(fixture.recording.data));
        let provider: Arc<dyn TranscriptionProvider> = Arc::new(FixtureProvider(result.transcript));
        let id = fixture.transcription_request.data.recording_id;
        let audio = store.audio(id).await.unwrap();
        let options = fixture.transcription_request.data.options;
        let transcript = provider.transcribe(&audio, &options).await.unwrap();
        assert_eq!(transcript.raw_text, "שלום Metrix");
        let unavailable: Arc<dyn TranscriptionProvider> =
            Arc::new(UnavailableProvider(ProviderInfo {
                id: ProviderId::Openai,
                execution_location: ExecutionLocation::Cloud,
                configured: true,
                available: false,
            }));
        let error = unavailable.transcribe(&audio, &options).await.unwrap_err();
        assert!(matches!(error.code, ErrorCode::NotImplemented));
        assert_eq!(store.get(id).await.unwrap().metadata.byte_length, 4096);
    }
}
