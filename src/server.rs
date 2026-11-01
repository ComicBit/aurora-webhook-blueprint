use crate::{
    config::Config,
    intake::{self, Secrets},
    model::{self, Acceptance, DeliveryJob, DeliveryOutcome, MAX_BODY, OutcomeKind, VerifiedEvent},
    store::{Mode, Store},
};
use anyhow::{Context, Result};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::{
    sync::{Semaphore, mpsc, oneshot},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

enum Command {
    Accept(VerifiedEvent, oneshot::Sender<Result<Acceptance>>),
    Claim(Vec<String>, oneshot::Sender<Result<Option<DeliveryJob>>>),
    Finish(String, String, DeliveryOutcome, oneshot::Sender<Result<()>>),
    Expire(oneshot::Sender<Result<()>>),
}
#[derive(Clone)]
struct Database {
    tx: mpsc::Sender<Command>,
}
impl Database {
    async fn accept(&self, event: VerifiedEvent) -> Result<Acceptance> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::Accept(event, tx))
            .await
            .context("database owner unavailable")?;
        rx.await.context("database acceptance interrupted")?
    }
    async fn claim(&self, busy: Vec<String>) -> Result<Option<DeliveryJob>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::Claim(busy, tx))
            .await
            .context("database owner unavailable")?;
        rx.await.context("database claim interrupted")?
    }
    async fn finish(&self, id: String, attempt: String, outcome: DeliveryOutcome) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::Finish(id, attempt, outcome, tx))
            .await
            .context("database owner unavailable")?;
        rx.await.context("database settlement interrupted")?
    }
    async fn expire(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Command::Expire(tx))
            .await
            .context("database owner unavailable")?;
        rx.await.context("database maintenance interrupted")?
    }
}
#[derive(Clone)]
struct App {
    db: Database,
    secrets: Arc<Secrets>,
    permits: Arc<Semaphore>,
}
fn response(status: StatusCode, code: &'static str) -> Response {
    let mut response = (status, code).into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    if status == StatusCode::SERVICE_UNAVAILABLE {
        response
            .headers_mut()
            .insert("retry-after", "30".parse().unwrap());
    }
    response
}
async fn receive(State(app): State<App>, request: Request<Body>) -> Response {
    let Ok(_permit) = app.permits.try_acquire() else {
        return response(StatusCode::SERVICE_UNAVAILABLE, "receiver_busy");
    };
    if request.headers().contains_key("content-encoding") {
        return response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "content_encoding_not_supported",
        );
    }
    if !request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
        })
    {
        return response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "application_json_required",
        );
    }
    if request
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|v| v > MAX_BODY as u64)
    {
        return response(StatusCode::PAYLOAD_TOO_LARGE, "body_limit");
    }
    let (headers, body) = {
        let (parts, body) = request.into_parts();
        (parts.headers, body)
    };
    let body = match tokio::time::timeout(Duration::from_secs(15), to_bytes(body, MAX_BODY)).await {
        Ok(Ok(v)) => v,
        Ok(Err(_)) => return response(StatusCode::PAYLOAD_TOO_LARGE, "invalid_or_oversized_body"),
        Err(_) => return response(StatusCode::REQUEST_TIMEOUT, "body_timeout"),
    };
    let id = match intake::verify(&headers, &body, &app.secrets, model::now()) {
        Ok(id) => id,
        Err(_) => return response(StatusCode::UNAUTHORIZED, "unauthorized"),
    };
    let event = match intake::validate_body(body.into(), id) {
        Ok(e) => e,
        Err(_) => return response(StatusCode::BAD_REQUEST, "invalid_event"),
    };
    match app.db.accept(event).await {
        Ok(Acceptance::Accepted | Acceptance::Duplicate) => response(StatusCode::NO_CONTENT, ""),
        Ok(Acceptance::Conflict) => response(StatusCode::CONFLICT, "event_conflict"),
        Ok(Acceptance::Full) => response(StatusCode::SERVICE_UNAVAILABLE, "inbox_full"),
        Err(_) => response(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable"),
    }
}
async fn worker(db: Database, limit: usize, cancel: CancellationToken) -> Result<()> {
    let mut jobs = JoinSet::new();
    let mut busy = HashSet::<String>::new();
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    let mut last_expire = 0;
    let operation:Result<()>=async {
    loop {
        tokio::select! {
            _=cancel.cancelled()=>return Ok(()),
            done=jobs.join_next(),if !jobs.is_empty()=>{
                let task=done.context("worker disappeared")?;
                let destination=task.context("delivery task stopped")??;
                busy.remove(&destination);
            }
            _=tick.tick()=>{
                let now=model::now();if now!=last_expire {db.expire().await?;last_expire=now;}
                while jobs.len()<limit {
                    let Some(job)=db.claim(busy.iter().cloned().collect()).await? else {break};
                    busy.insert(job.destination.id.clone());let db=db.clone();let cancellation=cancel.clone();
                    jobs.spawn(async move {
                        let remaining=job.expires_at.saturating_sub(model::now());
                        let outcome=if remaining<=0 {DeliveryOutcome{kind:OutcomeKind::NeedsReview,code:"deadline_before_submission".into(),result:None,external_id:None}} else {
                            tokio::select! {
                                _=cancellation.cancelled()=>DeliveryOutcome{kind:OutcomeKind::NeedsReview,code:"shutdown_after_claim".into(),result:None,external_id:None},
                                run=tokio::time::timeout(Duration::from_secs(remaining as u64),crate::dispatch::run(&job))=>match run {
                                    Ok(outcome)=>outcome,
                                    Err(_)=>DeliveryOutcome{kind:OutcomeKind::NeedsReview,code:"retention_deadline".into(),result:None,external_id:None},
                                }
                            }
                        };
                        let destination=job.destination.id.clone();
                        db.finish(job.delivery_id,job.attempt_id,outcome).await?;
                        Ok::<String,anyhow::Error>(destination)
                    });
                }
            }
        }
    }
    }.await;
    // Even fatal database/scheduler errors must reap local process guards before main exits.
    cancel.cancel();
    let drain = async {
        let mut failure = None;
        while let Some(result) = jobs.join_next().await {
            if let Err(error) = result.map_err(anyhow::Error::from).and_then(|v| v)
                && failure.is_none()
            {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    };
    let settlement = match tokio::time::timeout(Duration::from_secs(25), drain).await {
        Ok(result) => result,
        Err(_) => {
            jobs.abort_all();
            while jobs.join_next().await.is_some() {}
            Err(anyhow::anyhow!(
                "worker shutdown deadline exceeded; unfinished admissions require review"
            ))
        }
    };
    operation.and(settlement)
}

pub async fn serve(config: Config, secrets: Secrets) -> Result<()> {
    let (tx, mut rx) = mpsc::channel(32);
    let (ready_tx, ready_rx) = oneshot::channel();
    let dbconfig = config.clone();
    let dbtask = tokio::task::spawn_blocking(move || -> Result<()> {
        let mut store = match Store::open(&dbconfig, Mode::Serve) {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return Ok(());
            }
        };
        let _ = ready_tx.send(Ok(()));
        while let Some(command) = rx.blocking_recv() {
            match command {
                Command::Accept(e, reply) => {
                    let _ = reply.send(store.accept(e, model::now()));
                }
                Command::Claim(b, reply) => {
                    let _ = reply.send(store.claim(&b, model::now()));
                }
                Command::Finish(id, attempt, outcome, reply) => {
                    let _ = reply.send(store.finish(&id, &attempt, outcome, model::now()));
                }
                Command::Expire(reply) => {
                    let _ = reply.send(store.expire(model::now()));
                }
            }
        }
        Ok(())
    });
    ready_rx.await.context("database startup interrupted")??;
    let db = Database { tx };
    let cancellation = CancellationToken::new();
    let _cancel_on_exit = cancellation.clone().drop_guard();
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .context("cannot bind loopback listener")?;
    let app = Router::new()
        .route("/webhooks/aurora", post(receive))
        .route(
            "/healthz",
            get(|| async { response(StatusCode::OK, "ready") }),
        )
        .with_state(App {
            db: db.clone(),
            secrets: Arc::new(secrets),
            permits: Arc::new(Semaphore::new(config.max_concurrent_requests)),
        });
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut worker = tokio::spawn(worker(
        db.clone(),
        config.max_active_deliveries,
        cancellation.clone(),
    ));
    let connections = Arc::new(Semaphore::new(config.max_connections));
    let mut tasks = JoinSet::new();
    println!("listening on {} (private ingress only)", config.listen);
    let mut worker_exited = false;
    let mut outcome = Ok(());
    loop {
        tokio::select! {
            _=tokio::signal::ctrl_c()=>break,
            _=term.recv()=>break,
            result=&mut worker=>{worker_exited=true;outcome=match result {Ok(v)=>v,Err(_)=>Err(anyhow::anyhow!("worker stopped unexpectedly"))};break},
            Some(_)=tasks.join_next(),if !tasks.is_empty()=>{},
            accepted=listener.accept()=>{
                let(stream,_)=match accepted {
                    Ok(value)=>value,
                    Err(_)=>{outcome=Err(anyhow::anyhow!("listener failed"));break;}
                };
                let Ok(permit)=connections.clone().try_acquire_owned() else {drop(stream);continue};
                let service=TowerToHyperService::new(app.clone());let token=cancellation.clone();
                tasks.spawn(async move {
                    let _permit=permit;
                    let mut builder=hyper::server::conn::http1::Builder::new();
                    builder.timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(5)).max_headers(100).max_buf_size(16384);
                    let connection=builder.serve_connection(TokioIo::new(stream),service);tokio::pin!(connection);
                    tokio::select! {
                        _=&mut connection=>{},
                        _=token.cancelled()=>{connection.as_mut().graceful_shutdown();let _=tokio::time::timeout(Duration::from_secs(20),&mut connection).await;},
                    }
                });
            }
        }
    }
    cancellation.cancel();
    drop(listener);
    drop(app);
    while tasks.join_next().await.is_some() {}
    if !worker_exited {
        worker.await.context("worker shutdown failed")??;
    }
    drop(db);
    dbtask.await.context("database owner shutdown failed")??;
    outcome
}
