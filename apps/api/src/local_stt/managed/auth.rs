//! Per-launch secrets stay in memory and cross only the child's inherited stdin.
use reqwest::{header::HeaderValue, RequestBuilder, Response};
use tokio::io::AsyncWriteExt;

#[derive(Clone)]
pub struct WorkerAuth {
    capability: String,
    instance: String,
}

impl std::fmt::Debug for WorkerAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkerAuth([redacted])")
    }
}

fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

impl WorkerAuth {
    pub(super) fn new() -> Self {
        Self {
            capability: secret(),
            instance: secret(),
        }
    }

    pub(super) async fn send(&self, pipe: &mut tokio::process::ChildStdin) -> std::io::Result<()> {
        pipe.write_all(format!("{}\n{}\n", self.capability, self.instance).as_bytes())
            .await?;
        pipe.shutdown().await
    }

    pub(super) fn redact(&self, line: &str) -> String {
        line.trim_end()
            .replace(&self.capability, "[redacted]")
            .replace(&self.instance, "[redacted]")
    }

    pub(crate) fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        let mut value = HeaderValue::from_str(&format!("Bearer {}", self.capability))
            .expect("generated ASCII capability");
        value.set_sensitive(true);
        request.header(reqwest::header::AUTHORIZATION, value)
    }

    pub(crate) fn matches(&self, response: &Response) -> bool {
        response
            .headers()
            .get("x-writer-instance")
            .is_some_and(|value| value.as_bytes() == self.instance.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_launches_and_debug_do_not_disclose_capabilities() {
        let a = WorkerAuth::new();
        let b = WorkerAuth::new();
        assert_ne!(a.capability, b.capability);
        assert_ne!(a.instance, b.instance);
        assert!(!format!("{a:?}").contains(&a.capability));
        assert_eq!(
            a.redact(&format!("{} {}\n", a.capability, a.instance)),
            "[redacted] [redacted]"
        );
        let request = a
            .authorize(reqwest::Client::new().get("http://127.0.0.1/health"))
            .build()
            .unwrap();
        assert!(request.headers()[reqwest::header::AUTHORIZATION].is_sensitive());
        assert!(!format!("{request:?}").contains(&a.capability));
    }
}
