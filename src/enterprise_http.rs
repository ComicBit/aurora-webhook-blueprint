use crate::{
    enterprise_auth::{CHUNK_SIZE, MANIFEST_LIMIT, Verifier, valid_hash, valid_uuid},
    enterprise_store::{EnterpriseStore, Failure, Receipt, Request},
    model,
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, State, rejection::PathRejection},
    http::{HeaderMap, Method, Request as HttpRequest, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{post, put},
};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, oneshot},
    task::JoinHandle,
};

const BODY_TIMEOUT: Duration = Duration::from_secs(15);
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(1);

struct Job {
    request: Request,
    proof: crate::enterprise_auth::Proof,
    reply: oneshot::Sender<Result<Receipt, Failure>>,
    _permit: OwnedSemaphorePermit,
}

#[derive(Clone)]
struct App {
    tx: mpsc::SyncSender<Job>,
    verifier: Arc<Verifier>,
    permits: Arc<Semaphore>,
}

pub(crate) struct Background {
    tx: mpsc::SyncSender<Job>,
    task: JoinHandle<()>,
}

impl Background {
    pub(crate) async fn shutdown(self) -> anyhow::Result<()> {
        drop(self.tx);
        self.task.await?;
        Ok(())
    }
}

pub(crate) fn start(
    mut store: EnterpriseStore,
    intake: Option<(Verifier, usize)>,
) -> (Router, Background) {
    let (tx, rx) = mpsc::sync_channel::<Job>(intake.as_ref().map_or(1, |(_, slots)| *slots));
    let task = tokio::task::spawn_blocking(move || {
        let mut maintenance = Instant::now();
        let mut cleanup_failed = false;
        loop {
            if Instant::now() >= maintenance {
                let failed = store.expire(model::now()).is_err();
                if failed && !cleanup_failed {
                    eprintln!("Enterprise retention cleanup unavailable; will retry");
                } else if !failed && cleanup_failed {
                    eprintln!("Enterprise retention cleanup recovered");
                }
                cleanup_failed = failed;
                maintenance = Instant::now() + MAINTENANCE_INTERVAL;
            }
            match rx.recv_timeout(maintenance.saturating_duration_since(Instant::now())) {
                Ok(job) => {
                    let _ = job
                        .reply
                        .send(store.execute(job.request, job.proof, model::now()));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    let router = intake.map_or_else(Router::new, |(verifier, slots)| {
        Router::new()
            .route("/v1/enterprise/check", post(check))
            .route("/v1/enterprise/events/{event_id}", post(event).get(event))
            .route("/v1/enterprise/events/{event_id}/finalize", post(finalize))
            .route(
                "/v1/enterprise/events/{event_id}/audio/{artifact_id}/chunks/{index}",
                put(chunk),
            )
            .with_state(App {
                tx: tx.clone(),
                verifier: Arc::new(verifier),
                permits: Arc::new(Semaphore::new(slots)),
            })
    });
    (router, Background { tx, task })
}

#[derive(Clone, Copy)]
enum Operation<'a> {
    Check,
    Admit(&'a str),
    Status(&'a str),
    Chunk(&'a str, &'a str, &'a str),
    Finalize(&'a str),
}

async fn check(State(app): State<App>, request: HttpRequest<Body>) -> Response {
    handle(app, Operation::Check, request).await
}

async fn event(
    State(app): State<App>,
    path: Result<Path<String>, PathRejection>,
    request: HttpRequest<Body>,
) -> Response {
    let Ok(Path(event)) = path else {
        return error(StatusCode::BAD_REQUEST, "invalid_path");
    };
    let operation = if request.method() == Method::POST {
        Operation::Admit(&event)
    } else {
        Operation::Status(&event)
    };
    handle(app, operation, request).await
}

async fn finalize(
    State(app): State<App>,
    path: Result<Path<String>, PathRejection>,
    request: HttpRequest<Body>,
) -> Response {
    let Ok(Path(event)) = path else {
        return error(StatusCode::BAD_REQUEST, "invalid_path");
    };
    handle(app, Operation::Finalize(&event), request).await
}

async fn chunk(
    State(app): State<App>,
    path: Result<Path<(String, String, String)>, PathRejection>,
    request: HttpRequest<Body>,
) -> Response {
    let Ok(Path((event, artifact, index))) = path else {
        return error(StatusCode::BAD_REQUEST, "invalid_path");
    };
    handle(app, Operation::Chunk(&event, &artifact, &index), request).await
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

fn matches_digest(body: &[u8], expected: &str) -> bool {
    let mut encoded = [0; 64];
    hex::encode_to_slice(Sha256::digest(body), &mut encoded).expect("fixed SHA-256 output length");
    encoded == expected.as_bytes()
}

fn protected(mut response: Response) -> Response {
    use axum::http::HeaderValue;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn error(status: StatusCode, code: &'static str) -> Response {
    let mut response = protected((status, code).into_response());
    if status == StatusCode::TOO_MANY_REQUESTS {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, "1".parse().unwrap());
    }
    response
}

async fn handle(app: App, op: Operation<'_>, request: HttpRequest<Body>) -> Response {
    let Ok(permit) = app.permits.try_acquire_owned() else {
        return error(StatusCode::TOO_MANY_REQUESTS, "enterprise_busy");
    };
    let (parts, body) = request.into_parts();
    let headers = &parts.headers;
    let unauthorized = || error(StatusCode::UNAUTHORIZED, "unauthorized");
    let invalid_path = || error(StatusCode::BAD_REQUEST, "invalid_path");
    let Some(proof) = app.verifier.verify(headers, model::now()) else {
        return unauthorized();
    };
    let check = matches!(op, Operation::Check);
    if (check && !proof.allows_check()) || (!check && !proof.allows_manifest()) {
        return unauthorized();
    }
    let (limit, content_type) = match op {
        Operation::Check => (0, None),
        Operation::Admit(_) => (MANIFEST_LIMIT, Some("application/json")),
        Operation::Chunk(_, _, _) => {
            if !proof.allows_audio() {
                return unauthorized();
            }
            (CHUNK_SIZE, Some("application/octet-stream"))
        }
        Operation::Status(_) | Operation::Finalize(_) => (0, None),
    };
    if let Operation::Admit(event)
    | Operation::Chunk(event, _, _)
    | Operation::Status(event)
    | Operation::Finalize(event) = op
    {
        if !valid_uuid(event) {
            return invalid_path();
        }
        if event != proof.event_id {
            return unauthorized();
        }
    }
    if headers.contains_key(header::CONTENT_ENCODING)
        || content_type.is_some_and(|expected| {
            !single_header(headers, "content-type").is_some_and(|actual| {
                actual
                    .split(';')
                    .next()
                    .is_some_and(|media| media.trim().eq_ignore_ascii_case(expected))
            })
        })
    {
        return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type");
    }
    let over_limit = || {
        if limit == 0 {
            error(StatusCode::BAD_REQUEST, "unexpected_body")
        } else {
            error(StatusCode::PAYLOAD_TOO_LARGE, "body_limit")
        }
    };
    if single_header(headers, "content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > limit)
    {
        return over_limit();
    }
    let body = match tokio::time::timeout(BODY_TIMEOUT, to_bytes(body, limit)).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => return over_limit(),
        Err(_) => return error(StatusCode::REQUEST_TIMEOUT, "body_timeout"),
    };
    if proof.exp <= model::now() {
        return unauthorized();
    }
    let wrong_hash = || error(StatusCode::UNPROCESSABLE_ENTITY, "hash_mismatch");
    let request = match op {
        Operation::Check => return protected(StatusCode::NO_CONTENT.into_response()),
        Operation::Admit(_) => {
            if !matches_digest(&body, &proof.manifest_sha256) {
                return wrong_hash();
            }
            let Ok(manifest) = crate::enterprise_auth::Manifest::parse(&body) else {
                return error(StatusCode::BAD_REQUEST, "invalid_manifest");
            };
            if manifest.session_id != proof.session_id
                || manifest.event_id != proof.event_id
                || (manifest.audio.is_some() && !proof.allows_audio())
                || (manifest.outcome.is_some() && !proof.allows_outcome())
            {
                return unauthorized();
            }
            Request::Admit {
                manifest,
                raw: body,
            }
        }
        Operation::Status(_) => Request::Status,
        Operation::Finalize(_) => Request::Finalize,
        Operation::Chunk(_, artifact_id, index) => {
            if !valid_uuid(artifact_id)
                || index.is_empty()
                || !index.bytes().all(|b| b.is_ascii_digit())
                || (index.starts_with('0') && index.len() > 1)
            {
                return invalid_path();
            }
            let Ok(index) = index.parse() else {
                return invalid_path();
            };
            let Some(digest) =
                single_header(headers, "x-aurora-chunk-sha256").filter(|hex| valid_hash(hex))
            else {
                return error(StatusCode::BAD_REQUEST, "invalid_chunk_digest");
            };
            if body.is_empty() {
                return error(StatusCode::BAD_REQUEST, "invalid_chunk");
            }
            if !matches_digest(&body, digest) {
                return wrong_hash();
            }
            Request::Chunk {
                artifact_id: artifact_id.into(),
                index,
                body,
            }
        }
    };
    let (reply, result) = oneshot::channel();
    let unavailable = || error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable");
    match app.tx.try_send(Job {
        request,
        proof,
        reply,
        _permit: permit,
    }) {
        Ok(()) => {}
        Err(mpsc::TrySendError::Full(_)) => {
            return error(StatusCode::TOO_MANY_REQUESTS, "enterprise_busy");
        }
        Err(mpsc::TrySendError::Disconnected(_)) => return unavailable(),
    }
    match result.await {
        Ok(Ok(receipt)) => protected(
            (
                if receipt.admitted {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                Json(receipt),
            )
                .into_response(),
        ),
        Ok(Err(Failure::Unauthorized)) => unauthorized(),
        Ok(Err(Failure::NotFound)) => error(StatusCode::NOT_FOUND, "not_found"),
        Ok(Err(Failure::Conflict)) => error(StatusCode::CONFLICT, "upload_conflict"),
        Ok(Err(Failure::Gone)) => error(StatusCode::GONE, "upload_expired"),
        Ok(Err(Failure::HashMismatch)) => wrong_hash(),
        Ok(Err(Failure::Full)) => error(StatusCode::INSUFFICIENT_STORAGE, "storage_full"),
        Ok(Err(Failure::TooLarge)) => error(StatusCode::PAYLOAD_TOO_LARGE, "artifact_limit"),
        Ok(Err(Failure::InvalidChunk)) => error(StatusCode::BAD_REQUEST, "invalid_chunk"),
        Ok(Err(Failure::Unavailable(cause))) => {
            // Do not log values from request bodies, credentials, or private paths.
            drop(cause);
            unavailable()
        }
        Err(_) => unavailable(),
    }
}
