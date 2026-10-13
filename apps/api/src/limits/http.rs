//! Bound body bytes/idle time even before multipart headers have been parsed.
use super::{busy, Limits};
use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use http_body::{Frame, SizeHint};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    sync::Semaphore,
    time::{Instant, Sleep},
};

pub(crate) struct Admission {
    slots: Semaphore,
    idle: Duration,
    upload_bytes: usize,
}
impl Admission {
    pub fn new(limits: &Limits, max_upload: u64, device: bool) -> Arc<Self> {
        Arc::new(Self {
            slots: Semaphore::new(
                if device {
                    limits.device_uploads
                } else {
                    limits.uploads
                }
                .saturating_mul(4),
            ),
            idle: limits.idle,
            upload_bytes: usize::try_from(max_upload)
                .unwrap_or(usize::MAX)
                .saturating_add(64 * 1024),
        })
    }
    pub fn environment() -> Result<Arc<Self>, crate::transcription::ApiError> {
        let limits = Limits::from_env()?;
        let max_upload = match std::env::var("WRITER_MAX_UPLOAD_BYTES") {
            Ok(value) => value
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| busy("Invalid WRITER_MAX_UPLOAD_BYTES configuration."))?,
            Err(_) => 1024 * 1024 * 1024,
        };
        Ok(Self::new(&limits, max_upload, false))
    }
}
pub(crate) async fn admit(
    State(config): State<Arc<Admission>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Ok(_permit) = config.slots.try_acquire() else {
        return crate::jobs::response_error(busy("Request slots are full; retry later."));
    };
    let upload = request.uri().path() == "/api/v1/recordings"
        && request.method() == axum::http::Method::POST;
    let limit = if upload {
        config.upload_bytes
    } else {
        2 * 1024 * 1024
    };
    let body = std::mem::replace(request.body_mut(), Body::empty());
    *request.body_mut() = Body::new(http_body_util::Limited::new(
        IdleBody {
            body,
            timer: Box::pin(tokio::time::sleep(config.idle)),
            idle: config.idle,
        },
        limit,
    ));
    next.run(request).await
}
struct IdleBody {
    body: Body,
    timer: Pin<Box<Sleep>>,
    idle: Duration,
}
impl http_body::Body for IdleBody {
    type Data = Bytes;
    type Error = axum::BoxError;
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let this = self.get_mut();
        // Check first so a stream of empty frames cannot keep a request alive.
        if this.timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Some(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request body idle deadline",
            )
            .into())));
        }
        match Pin::new(&mut this.body).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if frame.data_ref().is_some_and(|d| !d.is_empty()) {
                    this.timer.as_mut().reset(Instant::now() + this.idle);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error.into()))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}
