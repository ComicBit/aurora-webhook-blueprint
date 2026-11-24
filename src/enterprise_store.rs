//! Local Enterprise receipts and media. The caller holds the inbox owner lock and
//! serializes all calls; the consumer worker never writes these tables or files.
use crate::{
    config::Config,
    enterprise_auth::{CHUNK_SIZE, EnterpriseConfig, MANIFEST_LIMIT, Manifest, Outcome, Proof},
    exports::{open_root, open_trusted_root},
    security::absolute_clean,
};
use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use rustix::{
    fs::{self, AtFlags, Mode, OFlags},
    io::Errno,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

const MEDIA_DIRECTORY: &str = "enterprise-media";
const RECORD_CHARGE: u64 = 4096; // Receipt, indexes, and at most one session row.
const REPLAY_CHARGE: u64 = 1024; // Bounded replay row/index plus write headroom.
const EXPIRY_PAYLOAD_BATCH: i64 = 8; // At most eight 1 MiB manifests erased per tick.
const CLEANUP_BATCH: i64 = 64;
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const FILE_FLAGS: OFlags = OFlags::NOFOLLOW
    .union(OFlags::CLOEXEC)
    .union(OFlags::NONBLOCK);

pub(crate) enum Request {
    Admit {
        manifest: Manifest,
        raw: Bytes,
    },
    Status,
    Chunk {
        artifact_id: String,
        index: u64,
        body: Bytes,
    },
    Finalize,
}

#[derive(Debug)]
pub(crate) enum Failure {
    Unauthorized,
    NotFound,
    Conflict,
    Gone,
    HashMismatch,
    TooLarge,
    InvalidChunk,
    Full,
    Unavailable(anyhow::Error),
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        if error.chain().any(|source| {
            source
                .downcast_ref::<rusqlite::Error>()
                .is_some_and(|error| {
                    error.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull)
                })
                || source
                    .downcast_ref::<io::Error>()
                    .is_some_and(|error| error.raw_os_error().is_some_and(full_errno))
                || source
                    .downcast_ref::<Errno>()
                    .is_some_and(|error| matches!(*error, Errno::NOSPC | Errno::DQUOT))
        }) {
            Self::Full
        } else {
            Self::Unavailable(error)
        }
    }
}

impl From<rusqlite::Error> for Failure {
    fn from(error: rusqlite::Error) -> Self {
        anyhow::Error::from(error).into()
    }
}

fn full_errno(errno: i32) -> bool {
    matches!(errno, libc::ENOSPC | libc::EDQUOT)
}

fn with_sqlite_errno(db: &Connection, failure: Failure) -> Failure {
    if let Failure::Unavailable(error) = &failure
        && error.chain().any(|source| {
            source
                .downcast_ref::<rusqlite::Error>()
                .is_some_and(|error| {
                    matches!(
                        error.sqlite_error_code(),
                        Some(
                            rusqlite::ErrorCode::SystemIoFailure | rusqlite::ErrorCode::CannotOpen
                        )
                    )
                })
        })
    {
        // The live connection is exclusively used by this writer. Bundled SQLite
        // reports EDQUOT as IOERR, not SQLITE_FULL, on Unix.
        let errno = unsafe { rusqlite::ffi::sqlite3_system_errno(db.handle()) };
        if full_errno(errno) {
            return Failure::Full;
        }
    }
    failure
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Uploading,
    Complete,
    Expired,
}

#[derive(Debug, Serialize)]
pub(crate) struct Receipt {
    event_id: String,
    session_id: String,
    revision: i32,
    state: State,
    artifact_id: Option<String>,
    received_bytes: u64,
    expires_at: i64,
    #[serde(skip)]
    pub(crate) admitted: bool,
}

struct Event {
    receipt: Receipt,
    org_id: String,
    receiver_id: String,
    user_id: String,
    manifest_sha256: String,
    storage_id: Option<String>,
    audio_length: u64,
    audio_sha256: Option<String>,
    purge_at: i64,
    completed_ttl: i64,
}

impl Event {
    fn same_upload(&self, proof: &Proof) -> bool {
        self.org_id == proof.org_id
            && self.receiver_id == proof.receiver_id
            && self.user_id == proof.user_id
            && self.receipt.session_id == proof.session_id
            && self.manifest_sha256 == proof.manifest_sha256
    }

    fn media_names(&self) -> Result<MediaNames> {
        MediaNames::new(self.storage_id.as_deref().context("missing media name")?)
    }
}

#[derive(Serialize)]
struct ExportReceipt<'a> {
    format_version: u32,
    #[serde(flatten)]
    receipt: &'a Receipt,
    org_id: &'a str,
    receiver_id: &'a str,
    user_id: &'a str,
    manifest_sha256: &'a str,
    audio_sha256: Option<&'a str>,
}

struct MediaNames {
    part: String,
    published: String,
}

impl MediaNames {
    fn new(id: &str) -> Result<Self> {
        ensure!(
            id.len() == 32
                && id
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "invalid managed media name"
        );
        Ok(Self {
            part: format!("{id}.part"),
            published: format!("{id}.bin"),
        })
    }
}

struct Usage {
    payload_bytes: u64,
    unwritten_bytes: u64,
    records: u64,
    replay_records: u64,
}

pub(crate) struct EnterpriseStore {
    db: Connection,
    settings: Option<EnterpriseConfig>,
    data_dir: PathBuf,
    data: OwnedFd,
    media: Option<OwnedFd>,
    min_free_bytes: u64,
    cleanup_cursor: String,
}

impl EnterpriseStore {
    pub(crate) fn open(config: &Config) -> Result<Self> {
        let data_dir = absolute_clean(&config.data_dir)?;
        let data = open_root(&data_dir, false)?;
        // Do not open/close separate data-file descriptors: POSIX can release a
        // sibling SQLite connection's process-wide locks when any such fd closes.
        check_private_file(&data, "inbox.sqlite3")?.context("missing inbox database")?;
        for name in [
            "inbox.sqlite3-wal",
            "inbox.sqlite3-shm",
            "inbox.sqlite3-journal",
        ] {
            check_private_file(&data, name)?;
        }
        let db = Connection::open_with_flags(
            data_dir.join("inbox.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        db.pragma_update(None, "synchronous", "FULL")?;
        db.pragma_update(None, "fullfsync", "ON")?;
        db.pragma_update(None, "checkpoint_fullfsync", "ON")?;
        db.pragma_update(None, "secure_delete", "ON")?;
        db.pragma_update(None, "wal_autocheckpoint", 256)?;
        db.pragma_update(None, "journal_size_limit", 8 * 1024 * 1024)?;
        let journal: String = db.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        let sync: i64 = db.pragma_query_value(None, "synchronous", |row| row.get(0))?;
        let foreign_keys: i64 = db.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
        ensure!(
            journal.eq_ignore_ascii_case("wal") && sync == 2 && foreign_keys == 1,
            "durable enterprise database settings unavailable"
        );
        let settings = config.enterprise_local_v1.clone();
        let media = match fs::openat(&data, MEDIA_DIRECTORY, DIRECTORY_FLAGS, Mode::empty()) {
            Ok(fd) => Some(fd),
            Err(Errno::NOENT) if settings.is_none() => None,
            Err(Errno::NOENT) => {
                fs::mkdirat(&data, MEDIA_DIRECTORY, Mode::from_raw_mode(0o700))?;
                Some(fs::openat(
                    &data,
                    MEDIA_DIRECTORY,
                    DIRECTORY_FLAGS,
                    Mode::empty(),
                )?)
            }
            Err(error) => return Err(error.into()),
        };
        if let Some(dir) = &media {
            let stat = fs::fstat(dir)?;
            ensure!(
                stat.st_uid == rustix::process::geteuid().as_raw()
                    && stat.st_mode & 0o077 == 0
                    && stat.st_dev == fs::fstat(&data)?.st_dev,
                "enterprise media directory must be private and on the inbox filesystem"
            );
            // Covers a previous interrupted mkdir as well as this invocation.
            fs::fsync(&data)?;
        }
        Ok(Self {
            db,
            settings,
            data_dir,
            data,
            media,
            min_free_bytes: config.min_free_bytes,
            cleanup_cursor: String::new(),
        })
    }

    pub(crate) fn execute(
        &mut self,
        request: Request,
        proof: Proof,
        at: i64,
    ) -> Result<Receipt, Failure> {
        let result = self.execute_inner(request, proof, at);
        result.map_err(|failure| with_sqlite_errno(&self.db, failure))
    }

    fn execute_inner(
        &mut self,
        request: Request,
        proof: Proof,
        at: i64,
    ) -> Result<Receipt, Failure> {
        let settings = self.settings.as_ref().ok_or(Failure::NotFound)?;
        if proof.exp <= at
            || proof.org_id != settings.org_id
            || proof.receiver_id != settings.receiver_id
            || !proof.allows_manifest()
            || match &request {
                Request::Admit { manifest, .. } => {
                    (manifest.audio.is_some() && !proof.allows_audio())
                        || (manifest.outcome.is_some() && !proof.allows_outcome())
                }
                Request::Chunk { .. } => !proof.allows_audio(),
                _ => false,
            }
        {
            return Err(Failure::Unauthorized);
        }
        if let Request::Admit { manifest, raw } = &request {
            if manifest.event_id != proof.event_id || manifest.session_id != proof.session_id {
                return Err(Failure::Unauthorized);
            }
            if raw.len() > MANIFEST_LIMIT {
                return Err(Failure::TooLarge);
            }
        }
        if !self.writable_wal()? {
            return Err(Failure::Full);
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prepare = || -> Result<Prepared, Failure> {
            prune_replay(&tx, at)?;
            bind_proof(&tx, settings, &self.data, self.min_free_bytes, &proof)?;
            let Some(event) = find_event(&tx, &proof.event_id)? else {
                return match request {
                    Request::Admit { manifest, raw } => admit(
                        &tx,
                        settings,
                        &self.data,
                        self.min_free_bytes,
                        proof,
                        manifest,
                        raw,
                        at,
                    )
                    .map(Prepared::Receipt),
                    _ => Err(Failure::NotFound),
                };
            };
            if !event.same_upload(&proof) {
                return Err(Failure::Conflict);
            }
            if matches!(request, Request::Finalize)
                && event.audio_length > 0
                && !proof.allows_audio()
            {
                return Err(Failure::Unauthorized);
            }
            if event.receipt.state == State::Expired || event.receipt.expires_at <= at {
                if event.receipt.state != State::Expired {
                    // The tombstone is durable before the caller can observe 410.
                    tx.execute("UPDATE enterprise_receipts SET state='expired',manifest=NULL,outcome=NULL WHERE event_id=?1", [&proof.event_id])?;
                }
                return Err(Failure::Gone);
            }
            match request {
                Request::Admit { raw, .. } => {
                    let exact: bool = tx.query_row(
                        "SELECT manifest=?2 FROM enterprise_receipts WHERE event_id=?1",
                        params![proof.event_id, raw.as_ref()],
                        |row| row.get(0),
                    )?;
                    if !exact {
                        return Err(Failure::Conflict);
                    }
                    Ok(Prepared::Receipt(event.receipt))
                }
                Request::Status => Ok(Prepared::Receipt(event.receipt)),
                Request::Chunk {
                    artifact_id,
                    index,
                    body,
                } => Ok(Prepared::Chunk(event, artifact_id, index, body)),
                Request::Finalize if event.receipt.state == State::Complete => {
                    Ok(Prepared::Receipt(event.receipt))
                }
                Request::Finalize => Ok(Prepared::Finalize(event)),
            }
        };
        let prepared = match prepare() {
            // Keep verified replay bindings for semantic conflicts, missing and
            // expired events. Authorization or storage failures roll back.
            Err(error @ (Failure::Unauthorized | Failure::Unavailable(_) | Failure::Full)) => {
                return Err(error);
            }
            prepared => prepared,
        };
        tx.commit()?;
        match prepared? {
            Prepared::Receipt(receipt) => Ok(receipt),
            Prepared::Chunk(event, artifact_id, index, body) => {
                self.chunk(event, &artifact_id, index, &body)
            }
            Prepared::Finalize(event) => self.finalize(event, at),
        }
    }

    fn chunk(
        &self,
        mut event: Event,
        artifact_id: &str,
        index: u64,
        body: &[u8],
    ) -> Result<Receipt, Failure> {
        if event.receipt.artifact_id.as_deref() != Some(artifact_id) {
            return Err(Failure::NotFound);
        }
        let chunk_size = CHUNK_SIZE as u64;
        let offset = index.checked_mul(chunk_size).ok_or(Failure::Conflict)?;
        let received = event.receipt.received_bytes;
        if offset >= event.audio_length || offset > received {
            return Err(Failure::Conflict);
        }
        let length = (event.audio_length - offset).min(chunk_size);
        if body.len() as u64 != length {
            return Err(Failure::InvalidChunk);
        }
        let media = self
            .media
            .as_ref()
            .context("missing enterprise media directory")?;
        let names = event.media_names()?;
        if offset < received {
            let mut file = if received == event.audio_length {
                match open_media(media, &names.published, OFlags::RDONLY)? {
                    Some(file) => {
                        exact_length(&file, received)?;
                        file
                    }
                    None if event.receipt.state == State::Uploading => {
                        recovery_part(media, &names.part, received)?
                    }
                    None => {
                        return Err(anyhow::anyhow!("missing published enterprise media").into());
                    }
                }
            } else {
                recovery_part(media, &names.part, received)?
            };
            if !matches_bytes(&mut file, offset, body)? {
                return Err(Failure::Conflict);
            }
            return Ok(event.receipt);
        }
        let usage = usage(&self.db)?;
        let prior_file = open_part(media, &names.part, received)?;
        if !has_headroom(
            &self.data,
            self.min_free_bytes,
            usage.unwritten_bytes.saturating_add(RECORD_CHARGE),
        )? {
            return Err(Failure::Full);
        }
        let mut file = match prior_file {
            Some(file) => file,
            None => new_private_file(media, &names.part)?,
        };
        append_chunk(media, &mut file, received, body)?;
        // If interrupted here, open_part truncates the suffix to the last
        // committed offset before any append or retry comparison.
        if !self.writable_wal()? {
            return Err(Failure::Full);
        }
        let next = received + length;
        let changed = self.db.execute(
            "UPDATE enterprise_receipts SET received_bytes=?2 WHERE event_id=?1 AND state='uploading' AND received_bytes=?3",
            params![event.receipt.event_id, next as i64, received as i64],
        )?;
        ensure_changed(changed)?;
        event.receipt.received_bytes = next;
        Ok(event.receipt)
    }

    fn finalize(&self, mut event: Event, at: i64) -> Result<Receipt, Failure> {
        if event.receipt.received_bytes != event.audio_length {
            return Err(Failure::Conflict);
        }
        let media = self
            .media
            .as_ref()
            .context("missing enterprise media directory")?;
        let names = event.media_names()?;
        let expected = event
            .audio_sha256
            .as_deref()
            .context("missing enterprise media digest")?;
        let expires_at = deadline(at, event.completed_ttl)?;
        let purge_at = deadline(expires_at, event.purge_at - event.receipt.expires_at)?;
        if !verify_and_publish(media, &names, event.audio_length, expected)? {
            return Err(Failure::HashMismatch);
        }
        // The hash and publication hold no SQLite transaction. A crash before
        // this update leaves a private final name that a repeated finalize verifies.
        if !self.writable_wal()? {
            return Err(Failure::Full);
        }
        let changed = self.db.execute(
            "UPDATE enterprise_receipts SET state='complete',completed_at=?2,expires_at=?3,purge_at=?4 WHERE event_id=?1 AND state='uploading' AND received_bytes=audio_length",
            params![event.receipt.event_id, at, expires_at, purge_at],
        )?;
        ensure_changed(changed)?;
        event.receipt.state = State::Complete;
        event.receipt.expires_at = expires_at;
        Ok(event.receipt)
    }

    fn writable_wal(&self) -> Result<bool> {
        let bytes = check_private_file(&self.data, "inbox.sqlite3-wal")?
            .map_or(0, |stat| stat.st_size.max(0) as u64);
        crate::store::checkpoint_wal(&self.db, bytes, false)
    }

    pub(crate) fn expire(&mut self, at: i64) -> Result<()> {
        ensure!(self.writable_wal()?, "enterprise WAL headroom unavailable");
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE enterprise_receipts SET state='expired',manifest=NULL,outcome=NULL WHERE event_id IN (
                SELECT event_id FROM enterprise_receipts WHERE state!='expired' AND expires_at<=?1
                ORDER BY expires_at LIMIT ?2
            )",
            params![at, EXPIRY_PAYLOAD_BATCH],
        )?;
        prune_replay(&tx, at)?;
        tx.commit()?;

        let mut batch = self.pending_cleanup(&self.cleanup_cursor)?;
        if batch.is_empty() && !self.cleanup_cursor.is_empty() {
            self.cleanup_cursor.clear();
            batch = self.pending_cleanup("")?;
        }
        let mut error = None;
        let mut unlink_attempted = false;
        if let Some((last, _)) = batch.last() {
            self.cleanup_cursor.clone_from(last);
            batch.retain(|(_, storage_id)| {
                let Some(id) = storage_id else {
                    return true;
                };
                unlink_attempted = true;
                if let Err(cause) = self.unlink_media(id) {
                    // Hold failed reservations for retry, but still clean
                    // unrelated expired payloads on the same pass.
                    error.get_or_insert(cause);
                    return false;
                }
                true
            });
        }
        if unlink_attempted {
            // Also sync when a prior interrupted attempt already removed the names.
            fs::fsync(self.media.as_ref().unwrap_or(&self.data))?;
        }
        ensure!(self.writable_wal()?, "enterprise WAL headroom unavailable");
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let released = batch.len();
        for (event_id, _) in batch {
            release_payload(&tx, &event_id)?;
        }
        tx.execute(
            "DELETE FROM enterprise_receipts WHERE event_id IN (
                SELECT event_id FROM enterprise_receipts
                WHERE state='expired' AND payload_bytes=0 AND purge_at<=?1
                ORDER BY purge_at LIMIT ?2
            )",
            params![at, CLEANUP_BATCH],
        )?;
        tx.commit()?;
        if released > 0 {
            eprintln!("Enterprise retention released {released} expired payload(s)");
        }
        error.map_or(Ok(()), |cause| {
            Err(cause.context("enterprise expiry cleanup failed"))
        })
    }

    fn pending_cleanup(&self, cursor: &str) -> Result<Vec<(String, Option<String>)>> {
        let mut statement = self.db.prepare_cached(
            "SELECT event_id,storage_id FROM enterprise_receipts WHERE state='expired' AND payload_bytes>0 AND event_id>?1 ORDER BY event_id LIMIT ?2",
        )?;
        Ok(statement
            .query_map(params![cursor, CLEANUP_BATCH], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    fn unlink_media(&self, id: &str) -> Result<()> {
        let names = MediaNames::new(id)?;
        if let Some(dir) = &self.media {
            for name in [&names.part, &names.published] {
                if check_private_file(dir, name)?.is_some() {
                    fs::unlinkat(dir, name.as_str(), AtFlags::empty())?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn export(&self, event_id: &str, output: &Path, at: i64) -> Result<()> {
        let event = find_event(&self.db, event_id)?.context("enterprise event not found")?;
        ensure!(
            event.receipt.state == State::Complete && event.receipt.expires_at > at,
            "enterprise event is not retained and complete"
        );
        let manifest: Vec<u8> = self.db.query_row(
            "SELECT manifest FROM enterprise_receipts WHERE event_id=?1",
            [event_id],
            |row| row.get(0),
        )?;
        ensure!(
            matches_digest(Sha256::digest(&manifest).into(), &event.manifest_sha256)?,
            "enterprise manifest digest mismatch"
        );
        let receipt = serde_json::to_vec_pretty(&ExportReceipt {
            format_version: 1,
            receipt: &event.receipt,
            org_id: &event.org_id,
            receiver_id: &event.receiver_id,
            user_id: &event.user_id,
            manifest_sha256: &event.manifest_sha256,
            audio_sha256: event.audio_sha256.as_deref(),
        })?;
        let output = absolute_clean(output)?;
        ensure!(
            !output.starts_with(&self.data_dir),
            "enterprise export must be outside the managed data directory"
        );
        let parent = open_trusted_root(output.parent().context("missing export parent")?)?;
        let leaf = output
            .file_name()
            .context("missing export directory name")?;
        fs::mkdirat(&parent, leaf, Mode::from_raw_mode(0o700))
            .context("enterprise export requires a new directory")?;
        let dir = fs::openat(&parent, leaf, DIRECTORY_FLAGS, Mode::empty())?;
        let result = (|| -> Result<()> {
            fs::fsync(&parent)?;
            write_private(&dir, "manifest.json", &manifest)?;
            if let Some(expected) = event.audio_sha256.as_deref() {
                let media = self
                    .media
                    .as_ref()
                    .context("missing enterprise media directory")?;
                let names = event.media_names()?;
                let mut source = open_media(media, &names.published, OFlags::RDONLY)?
                    .context("missing published enterprise media")?;
                let mut target = new_private_file(&dir, "audio.bin")?;
                let digest = stream_digest(&mut source, &mut target, event.audio_length)?;
                ensure!(
                    matches_digest(digest, expected)?,
                    "enterprise media digest mismatch"
                );
                target.sync_all()?;
            }
            fs::fsync(&dir)?;
            write_private(&dir, ".receipt.tmp", &receipt)?;
            fs::renameat_with(
                &dir,
                ".receipt.tmp",
                &dir,
                "receipt.json",
                fs::RenameFlags::NOREPLACE,
            )?;
            fs::fsync(&dir)?;
            Ok(())
        })();
        if result.is_err() {
            // Only this invocation's freshly created directory was ever opened.
            for name in ["receipt.json", ".receipt.tmp", "audio.bin", "manifest.json"] {
                let _ = fs::unlinkat(&dir, name, AtFlags::empty());
            }
            let _ = fs::fsync(&dir);
            let _ = fs::unlinkat(&parent, leaf, AtFlags::REMOVEDIR);
            let _ = fs::fsync(&parent);
        }
        result
    }
}

fn prune_replay(db: &Connection, at: i64) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM enterprise_replay WHERE jti IN (
            SELECT jti FROM enterprise_replay WHERE expires_at<=?1
            ORDER BY expires_at LIMIT ?2
        )",
        params![at, CLEANUP_BATCH],
    )?;
    Ok(())
}

fn release_payload(db: &Connection, event_id: &str) -> Result<()> {
    let changed = db.execute(
        "UPDATE enterprise_receipts SET payload_bytes=0,storage_id=NULL,artifact_id=NULL,audio_sha256=NULL,audio_length=0,received_bytes=0 WHERE event_id=?1 AND state='expired' AND payload_bytes>0",
        [event_id],
    )?;
    ensure_changed(changed)
}

enum Prepared {
    Receipt(Receipt),
    Chunk(Event, String, u64, Bytes),
    Finalize(Event),
}

fn find_event(db: &Connection, event_id: &str) -> rusqlite::Result<Option<Event>> {
    db.prepare_cached(
        "SELECT e.event_id,e.session_id,e.revision,e.state,e.artifact_id,e.received_bytes,e.expires_at,e.org_id,e.receiver_id,s.user_id,e.manifest_sha256,e.storage_id,e.audio_length,e.audio_sha256,e.purge_at,e.completed_ttl FROM enterprise_receipts e JOIN enterprise_sessions s USING (org_id,receiver_id,session_id) WHERE e.event_id=?1",
    )?.query_row([event_id], |row| {
        let state = match row.get_ref(3)?.as_str()? {
            "uploading" => State::Uploading,
            "complete" => State::Complete,
            "expired" => State::Expired,
            _ => return Err(rusqlite::Error::InvalidQuery),
        };
        Ok(Event {
            receipt: Receipt {
                event_id: row.get(0)?,
                session_id: row.get(1)?,
                revision: row.get(2)?,
                state,
                artifact_id: row.get(4)?,
                received_bytes: nonnegative_int(row, 5)?,
                expires_at: row.get(6)?,
                admitted: false,
            },
            org_id: row.get(7)?,
            receiver_id: row.get(8)?,
            user_id: row.get(9)?,
            manifest_sha256: row.get(10)?,
            storage_id: row.get(11)?,
            audio_length: nonnegative_int(row, 12)?,
            audio_sha256: row.get(13)?,
            purge_at: row.get(14)?,
            completed_ttl: row.get(15)?,
        })
    }).optional()
}

fn usage(db: &Connection) -> rusqlite::Result<Usage> {
    db.prepare_cached("SELECT payload_bytes,unwritten_bytes,records,replay_records FROM enterprise_usage WHERE singleton=1")?
        .query_row([], |row| Ok(Usage {
            payload_bytes: nonnegative_int(row, 0)?,
            unwritten_bytes: nonnegative_int(row, 1)?,
            records: nonnegative_int(row, 2)?,
            replay_records: nonnegative_int(row, 3)?,
        }))
}

fn nonnegative_int(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value = row.get::<_, i64>(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

fn replay_cap(settings: &EnterpriseConfig) -> u64 {
    // Allow credential churn for resumable uploads without letting valid proofs
    // consume an unbounded independent metadata budget. Live rows are never evicted.
    let by_records = (u64::from(settings.max_records) * 16).clamp(64, 65_536);
    by_records.min((settings.max_storage_bytes / (8 * REPLAY_CHARGE)).max(1))
}

fn bind_proof(
    db: &Connection,
    settings: &EnterpriseConfig,
    data: &OwnedFd,
    min_free_bytes: u64,
    proof: &Proof,
) -> Result<(), Failure> {
    let bound = proof.binding_digest();
    let same: Option<bool> = db
        .prepare_cached("SELECT binding=?2 FROM enterprise_replay WHERE jti=?1")?
        .query_row(params![proof.jti, bound.as_slice()], |row| row.get(0))
        .optional()?;
    if let Some(same) = same {
        return if same { Ok(()) } else { Err(Failure::Conflict) };
    }
    let used = usage(db)?;
    // A previous uncommitted suffix can occupy part of the still-reserved audio.
    // Allow a fresh proof to reach chunk recovery before counting that reservation.
    if used.replay_records >= replay_cap(settings)
        || !has_headroom(data, min_free_bytes, REPLAY_CHARGE)?
    {
        return Err(Failure::Full);
    }
    db.execute(
        "INSERT INTO enterprise_replay (jti,binding,expires_at) VALUES (?1,?2,?3)",
        params![proof.jti, bound.as_slice(), proof.exp],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn admit(
    db: &Connection,
    settings: &EnterpriseConfig,
    data: &OwnedFd,
    min_free_bytes: u64,
    proof: Proof,
    manifest: Manifest,
    raw: Bytes,
    at: i64,
) -> Result<Receipt, Failure> {
    let owner: Option<(String, i32)> = db.query_row(
        "SELECT user_id,latest_revision FROM enterprise_sessions WHERE (org_id,receiver_id,session_id)=(?1,?2,?3)",
        params![proof.org_id, proof.receiver_id, proof.session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    if let Some((owner, revision)) = owner {
        if owner != proof.user_id || manifest.revision <= revision {
            return Err(Failure::Conflict);
        }
        let uploading: bool = db.query_row(
            "SELECT EXISTS (SELECT 1 FROM enterprise_receipts WHERE (org_id,receiver_id,session_id)=(?1,?2,?3) AND state='uploading' AND expires_at>?4)",
            params![proof.org_id, proof.receiver_id, proof.session_id, at],
            |row| row.get(0),
        )?;
        if uploading {
            return Err(Failure::Conflict);
        }
    }
    let audio_length = manifest.audio.as_ref().map_or(0, |audio| audio.length);
    if audio_length > settings.max_artifact_bytes {
        return Err(Failure::TooLarge);
    }
    let payload_bytes = raw.len() as u64 + audio_length;
    let used = usage(db)?;
    if used.records >= u64::from(settings.max_records)
        || used
            .payload_bytes
            .saturating_add(payload_bytes)
            .saturating_add(RECORD_CHARGE.saturating_mul(used.records + 1))
            > settings.max_storage_bytes
        || !has_headroom(
            data,
            min_free_bytes,
            used.unwritten_bytes
                .saturating_add(payload_bytes)
                .saturating_add(RECORD_CHARGE),
        )?
    {
        return Err(Failure::Full);
    }
    let status = if manifest.audio.is_some() {
        "uploading"
    } else {
        "complete"
    };
    let completed_at = manifest.audio.is_none().then_some(at);
    let expires_at = deadline(
        at,
        if completed_at.is_some() {
            settings.retention_seconds
        } else {
            settings.upload_ttl_seconds
        },
    )?;
    let purge_at = deadline(expires_at, settings.metadata_retention_seconds)?;
    let storage_id = manifest
        .audio
        .as_ref()
        .map(|_| Uuid::new_v4().simple().to_string());
    let outcome = manifest.outcome.as_ref().map(Outcome::as_str);
    let (artifact_id, audio_sha256) = manifest
        .audio
        .map(|audio| (Some(audio.artifact_id), Some(audio.sha256)))
        .unwrap_or_default();
    db.execute(
        "INSERT INTO enterprise_sessions (org_id,receiver_id,session_id,user_id,latest_revision) VALUES (?1,?2,?3,?4,?5) ON CONFLICT (org_id,receiver_id,session_id) DO UPDATE SET latest_revision=excluded.latest_revision",
        params![proof.org_id, proof.receiver_id, proof.session_id, proof.user_id, manifest.revision],
    )?;
    db.execute(
        "UPDATE enterprise_receipts SET state='expired',manifest=NULL,outcome=NULL WHERE (org_id,receiver_id,session_id)=(?1,?2,?3) AND state='uploading' AND expires_at<=?4",
        params![proof.org_id, proof.receiver_id, proof.session_id, at],
    )?;
    db.execute(
        "INSERT INTO enterprise_receipts (event_id,org_id,receiver_id,session_id,revision,manifest_sha256,manifest,outcome,storage_id,artifact_id,audio_sha256,audio_length,state,admitted_at,completed_at,expires_at,purge_at,completed_ttl,payload_bytes) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
        params![proof.event_id, proof.org_id, proof.receiver_id, proof.session_id, manifest.revision, proof.manifest_sha256, raw.as_ref(), outcome, storage_id, artifact_id, audio_sha256, audio_length as i64, status, at, completed_at, expires_at, purge_at, settings.retention_seconds, payload_bytes as i64],
    )?;
    Ok(Receipt {
        event_id: proof.event_id,
        session_id: proof.session_id,
        revision: manifest.revision,
        state: if completed_at.is_some() {
            State::Complete
        } else {
            State::Uploading
        },
        artifact_id,
        received_bytes: 0,
        expires_at,
        admitted: true,
    })
}

fn deadline(at: i64, ttl: i64) -> Result<i64> {
    at.checked_add(ttl).context("enterprise clock out of range")
}

fn ensure_changed(changed: usize) -> Result<()> {
    ensure!(changed == 1, "enterprise receipt transition failed");
    Ok(())
}

fn has_headroom(data: &OwnedFd, min_free_bytes: u64, needed: u64) -> Result<bool> {
    let stats = fs::fstatvfs(data)?;
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize) >= min_free_bytes.saturating_add(needed))
}

fn trusted_file(stat: &fs::Stat) -> Result<()> {
    ensure!(
        fs::FileType::from_raw_mode(stat.st_mode).is_file()
            && stat.st_uid == rustix::process::geteuid().as_raw()
            && stat.st_mode & 0o077 == 0
            && stat.st_nlink == 1,
        "enterprise file must be private, regular and not hard-linked"
    );
    Ok(())
}

fn check_private_file(dir: &OwnedFd, name: &str) -> Result<Option<fs::Stat>> {
    match fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            trusted_file(&stat)?;
            Ok(Some(stat))
        }
        Err(Errno::NOENT) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn open_media(dir: &OwnedFd, name: &str, flags: OFlags) -> Result<Option<File>> {
    match fs::openat(dir, name, flags | FILE_FLAGS, Mode::empty()) {
        Ok(fd) => {
            trusted_file(&fs::fstat(&fd)?)?;
            Ok(Some(File::from(fd)))
        }
        Err(Errno::NOENT) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn new_private_file(dir: &OwnedFd, name: &str) -> Result<File> {
    let fd = fs::openat(
        dir,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | FILE_FLAGS,
        Mode::from_raw_mode(0o600),
    )?;
    trusted_file(&fs::fstat(&fd)?)?;
    Ok(File::from(fd))
}

fn write_private(dir: &OwnedFd, name: &str, body: &[u8]) -> Result<()> {
    let mut file = new_private_file(dir, name)?;
    file.write_all(body)?;
    file.sync_all()?;
    Ok(())
}

fn trim_uncommitted(file: &File, committed: u64) -> Result<()> {
    let actual = file.metadata()?.len();
    ensure!(
        actual >= committed,
        "enterprise media is shorter than its receipt"
    );
    if actual > committed {
        file.set_len(committed)?;
        file.sync_all()?;
        eprintln!("Enterprise resume discarded an uncommitted media suffix");
    }
    Ok(())
}

fn recovery_part(dir: &OwnedFd, name: &str, committed: u64) -> Result<File> {
    let file = open_media(dir, name, OFlags::RDWR)?.context("missing enterprise partial media")?;
    trim_uncommitted(&file, committed)?;
    Ok(file)
}

fn open_part(dir: &OwnedFd, name: &str, committed: u64) -> Result<Option<File>> {
    let file = open_media(dir, name, OFlags::RDWR)?;
    if let Some(file) = &file {
        trim_uncommitted(file, committed)?;
    } else {
        ensure!(committed == 0, "missing enterprise partial media");
    }
    Ok(file)
}

fn append_chunk(dir: &OwnedFd, file: &mut File, offset: u64, body: &[u8]) -> Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(body)?;
    file.sync_all()?;
    if offset == 0 {
        fs::fsync(dir)?;
    }
    Ok(())
}

fn matches_bytes(file: &mut File, offset: u64, body: &[u8]) -> Result<bool> {
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = [0; 16384];
    for expected in body.chunks(buffer.len()) {
        let actual = &mut buffer[..expected.len()];
        file.read_exact(actual)?;
        if actual != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn exact_length(file: &File, length: u64) -> Result<()> {
    ensure!(
        file.metadata()?.len() == length,
        "enterprise media length mismatch"
    );
    Ok(())
}

fn stream_digest(source: &mut File, target: &mut impl Write, length: u64) -> Result<[u8; 32]> {
    exact_length(source, length)?;
    let mut reader = source.take(length);
    let mut buffer = [0; 65536];
    let mut hash = Sha256::new();
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        target.write_all(&buffer[..count])?;
    }
    ensure!(
        reader.limit() == 0,
        "enterprise media ended before its declared length"
    );
    Ok(hash.finalize().into())
}

fn matches_digest(digest: [u8; 32], expected: &str) -> Result<bool> {
    let mut decoded = [0; 32];
    hex::decode_to_slice(expected, &mut decoded).context("invalid persisted enterprise digest")?;
    Ok(digest == decoded)
}

fn verify_and_publish(
    dir: &OwnedFd,
    names: &MediaNames,
    length: u64,
    expected: &str,
) -> Result<bool> {
    let published = open_media(dir, &names.published, OFlags::RDONLY)?;
    let already_published = published.is_some();
    let mut file = match published {
        Some(file) => {
            ensure!(
                check_private_file(dir, &names.part)?.is_none(),
                "unexpected enterprise partial media"
            );
            file
        }
        None => recovery_part(dir, &names.part, length)?,
    };
    if !matches_digest(stream_digest(&mut file, &mut io::sink(), length)?, expected)? {
        return Ok(false);
    }
    if !already_published {
        fs::renameat_with(
            dir,
            names.part.as_str(),
            dir,
            names.published.as_str(),
            fs::RenameFlags::NOREPLACE,
        )?;
    }
    fs::fsync(dir)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        enterprise_auth::Verifier,
        store::{Mode as StoreMode, Store},
    };
    use axum::http::{HeaderMap, header::AUTHORIZATION};
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use serde_json::json;

    struct Fixture {
        config: Config,
        verifier: Verifier,
        key: Ed25519KeyPair,
        _owner: Store,
        temp: tempfile::TempDir,
    }

    impl Fixture {
        fn new(budget: u64) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let mut config = Config {
                data_dir: temp.path().canonicalize().unwrap().join("data"),
                min_free_bytes: 0,
                ..Config::default()
            };
            let key = Ed25519KeyPair::from_seed_unchecked(&[37; 32]).unwrap();
            let settings = EnterpriseConfig {
                org_id: "test-org".into(),
                receiver_id: "test-receiver".into(),
                max_artifact_bytes: budget,
                max_storage_bytes: budget,
                max_records: 8,
                max_concurrent_requests: 1,
                upload_ttl_seconds: 30,
                retention_seconds: 120,
                metadata_retention_seconds: 300,
                public_keys: [("fixture".into(), URL_SAFE_NO_PAD.encode(key.public_key()))].into(),
            };
            let verifier = Verifier::new(&settings).unwrap();
            config.enterprise_local_v1 = Some(settings);
            let owner = Store::open(&config, StoreMode::Apply).unwrap();
            Self {
                config,
                verifier,
                key,
                _owner: owner,
                temp,
            }
        }

        fn store(&self) -> EnterpriseStore {
            EnterpriseStore::open(&self.config).unwrap()
        }

        fn proof(&self, upload: &Upload, jti: u128, issued_at: i64) -> Proof {
            let header = URL_SAFE_NO_PAD
                .encode(br#"{"alg":"EdDSA","typ":"aurora-upload-local+jwt","kid":"fixture"}"#);
            let claims = URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&json!({
                    "v": 1,
                    "iss": "https://issuer.example.test/aurora",
                    "aud": "aurora-company-sync-local",
                    "org_id": "test-org",
                    "receiver_id": "test-receiver",
                    "user_id": Uuid::from_u128(1).to_string(),
                    "session_id": upload.session,
                    "event_id": upload.event,
                    "manifest_sha256": hex::encode(Sha256::digest(&upload.raw)),
                    "scopes": ["manifest", "audio", "outcome"],
                    "jti": Uuid::from_u128(jti).to_string(),
                    "iat": issued_at,
                    "nbf": issued_at,
                    "exp": issued_at + 90,
                }))
                .unwrap(),
            );
            let signing_input = format!("{header}.{claims}");
            let signature = URL_SAFE_NO_PAD.encode(self.key.sign(signing_input.as_bytes()));
            let mut headers = HeaderMap::new();
            headers.insert(
                AUTHORIZATION,
                format!("Bearer {signing_input}.{signature}")
                    .parse()
                    .unwrap(),
            );
            self.verifier.verify(&headers, issued_at).unwrap()
        }
    }

    const ARTIFACT: &str = "00000000-0000-0000-0000-0000000000a0";

    struct Upload {
        event: String,
        session: String,
        raw: Bytes,
    }

    impl Upload {
        fn new(id: u128, audio: Option<&[u8]>) -> Self {
            let event = Uuid::from_u128(id).to_string();
            let session = Uuid::from_u128(id + 100).to_string();
            let raw = Bytes::from(
                serde_json::to_vec(&json!({
                    "version": 1,
                    "event_id": event,
                    "session_id": session,
                    "revision": 1,
                    "outcome": "won",
                    "audio": audio.map(|bytes| json!({
                        "artifact_id": ARTIFACT,
                        "length": bytes.len(),
                        "sha256": hex::encode(Sha256::digest(bytes)),
                        "media_type": "audio/wav"
                    })),
                }))
                .unwrap(),
            );
            Self {
                event,
                session,
                raw,
            }
        }

        fn admit(&self) -> Request {
            Request::Admit {
                manifest: Manifest::parse(&self.raw).unwrap(),
                raw: self.raw.clone(),
            }
        }
    }

    fn chunk(index: u64, bytes: Bytes) -> Request {
        Request::Chunk {
            artifact_id: ARTIFACT.into(),
            index,
            body: bytes,
        }
    }

    #[test]
    fn restarts_trim_uncommitted_chunk_and_reconcile_published_audio() {
        let fixture = Fixture::new(4 * CHUNK_SIZE as u64);
        let all = Bytes::from(vec![7; CHUNK_SIZE + 3]);
        let upload = Upload::new(2, Some(&all));
        let mut store = fixture.store();
        assert!(
            store
                .execute(upload.admit(), fixture.proof(&upload, 10, 100), 100)
                .unwrap()
                .admitted
        );
        let first = all.slice(..CHUNK_SIZE);
        let tail = all.slice(CHUNK_SIZE..);
        let receipt = store
            .execute(
                chunk(0, first.clone()),
                fixture.proof(&upload, 10, 100),
                101,
            )
            .unwrap();
        assert_eq!(receipt.received_bytes, CHUNK_SIZE as u64);
        let event = find_event(&store.db, &upload.event).unwrap().unwrap();
        let names = event.media_names().unwrap();
        let media = store.media.as_ref().unwrap();
        let mut file = recovery_part(media, &names.part, receipt.received_bytes).unwrap();
        // Stop after the file is durable, before SQLite advances the offset.
        append_chunk(
            media,
            &mut file,
            receipt.received_bytes,
            b"interrupted tail",
        )
        .unwrap();
        drop(file);
        drop(store);

        let mut reduced = fixture.config.clone();
        reduced
            .enterprise_local_v1
            .as_mut()
            .unwrap()
            .max_artifact_bytes = 1;
        let mut store = EnterpriseStore::open(&reduced).unwrap();
        assert!(
            !store
                .execute(upload.admit(), fixture.proof(&upload, 11, 102), 102)
                .unwrap()
                .admitted
        );
        assert_eq!(
            store
                .execute(Request::Status, fixture.proof(&upload, 11, 102), 102)
                .unwrap()
                .received_bytes,
            CHUNK_SIZE as u64
        );
        assert_eq!(
            store
                .execute(chunk(1, tail.clone()), fixture.proof(&upload, 11, 102), 103)
                .unwrap()
                .received_bytes,
            all.len() as u64
        );
        let media = store.media.as_ref().unwrap();
        // Stop after rename and directory fsync, before the completion receipt.
        assert!(
            verify_and_publish(
                media,
                &names,
                all.len() as u64,
                event.audio_sha256.as_deref().unwrap()
            )
            .unwrap()
        );
        drop(store);

        let mut store = fixture.store();
        assert_eq!(
            store
                .execute(Request::Status, fixture.proof(&upload, 12, 104), 104)
                .unwrap()
                .state,
            State::Uploading
        );
        let complete = store
            .execute(Request::Finalize, fixture.proof(&upload, 12, 104), 105)
            .unwrap();
        assert_eq!(complete.state, State::Complete);
        assert_eq!(complete.expires_at, 225);
        let repeat = store
            .execute(Request::Finalize, fixture.proof(&upload, 12, 104), 106)
            .unwrap();
        assert_eq!(repeat.expires_at, complete.expires_at);
        assert!(
            store
                .execute(chunk(1, tail), fixture.proof(&upload, 12, 104), 107)
                .is_ok()
        );
        assert!(matches!(
            store.execute(
                chunk(1, Bytes::from_static(b"bad")),
                fixture.proof(&upload, 12, 104),
                108
            ),
            Err(Failure::Conflict)
        ));

        let output = fixture.temp.path().canonicalize().unwrap().join("export");
        store.export(&upload.event, &output, 109).unwrap();
        assert_eq!(
            std::fs::read(output.join("manifest.json")).unwrap(),
            upload.raw.as_ref()
        );
        assert_eq!(
            std::fs::read(output.join("audio.bin")).unwrap(),
            all.as_ref()
        );
        let receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(receipt["user_id"], Uuid::from_u128(1).to_string());
        assert!(store.export(&upload.event, &output, 109).is_err());
        assert!(
            store
                .export(&upload.event, &fixture.config.data_dir.join("inside"), 109)
                .is_err()
        );
    }

    #[test]
    fn expired_reservation_recovers_after_unlink_even_when_disabled() {
        let fixture = Fixture::new(64 << 10);
        let audio = Bytes::from(vec![8; 48 << 10]);
        let upload = Upload::new(3, Some(&audio));
        let other = Upload::new(4, Some(&audio));
        let mut store = fixture.store();
        store
            .execute(upload.admit(), fixture.proof(&upload, 20, 100), 100)
            .unwrap();
        store
            .execute(
                chunk(0, audio.clone()),
                fixture.proof(&upload, 20, 100),
                101,
            )
            .unwrap();
        assert!(matches!(
            store.execute(other.admit(), fixture.proof(&other, 21, 102), 102),
            Err(Failure::Full)
        ));
        assert!(matches!(
            store.execute(Request::Status, fixture.proof(&upload, 20, 100), 130),
            Err(Failure::Gone)
        ));
        assert!(matches!(
            store.execute(other.admit(), fixture.proof(&other, 21, 102), 130),
            Err(Failure::Full)
        ));
        let event = find_event(&store.db, &upload.event).unwrap().unwrap();
        let cleared: bool = store.db.query_row(
            "SELECT manifest IS NULL AND outcome IS NULL FROM enterprise_receipts WHERE event_id=?1",
            [&upload.event],
            |row| row.get(0),
        ).unwrap();
        assert!(cleared);
        // Stop after unlink and directory fsync, before release_payload.
        store
            .unlink_media(event.storage_id.as_deref().unwrap())
            .unwrap();
        fs::fsync(store.media.as_ref().unwrap()).unwrap();
        assert!(usage(&store.db).unwrap().payload_bytes >= audio.len() as u64);
        drop(store);

        let mut disabled = fixture.config.clone();
        disabled.enterprise_local_v1 = None;
        let mut store = EnterpriseStore::open(&disabled).unwrap();
        store.expire(131).unwrap();
        assert_eq!(usage(&store.db).unwrap().payload_bytes, 0);
        drop(store);

        let mut store = fixture.store();
        assert!(
            store
                .execute(other.admit(), fixture.proof(&other, 21, 102), 132)
                .unwrap()
                .admitted
        );
        store.expire(430).unwrap();
        assert!(matches!(
            store.execute(Request::Status, fixture.proof(&upload, 22, 430), 430),
            Err(Failure::NotFound)
        ));
    }

    #[test]
    fn replay_capacity_does_not_evict_live_bindings() {
        let fixture = Fixture::new(16 << 10);
        let original = Upload::new(5, None);
        let other = Upload::new(6, None);
        let mut store = fixture.store();
        for jti in [30, 31] {
            assert!(matches!(
                store.execute(Request::Status, fixture.proof(&original, jti, 100), 100),
                Err(Failure::NotFound)
            ));
        }
        assert!(matches!(
            store.execute(Request::Status, fixture.proof(&other, 30, 100), 101),
            Err(Failure::Conflict)
        ));
        assert!(matches!(
            store.execute(Request::Status, fixture.proof(&original, 30, 100), 101),
            Err(Failure::NotFound)
        ));
        assert!(matches!(
            store.execute(other.admit(), fixture.proof(&other, 32, 101), 101),
            Err(Failure::Full)
        ));
        assert!(matches!(
            store.execute(Request::Status, fixture.proof(&original, 30, 100), 190),
            Err(Failure::Unauthorized)
        ));
        assert!(
            store
                .execute(other.admit(), fixture.proof(&other, 33, 190), 190)
                .unwrap()
                .admitted
        );
    }

    #[test]
    fn full_space_errors_are_classified_without_disclosing_details() {
        for errno in [libc::ENOSPC, libc::EDQUOT] {
            let error =
                anyhow::Error::new(io::Error::from_raw_os_error(errno)).context("durable write");
            assert!(matches!(Failure::from(error), Failure::Full));
        }
        for errno in [Errno::NOSPC, Errno::DQUOT] {
            assert!(matches!(
                Failure::from(anyhow::Error::new(errno)),
                Failure::Full
            ));
        }
        let fixture = Fixture::new(16 << 10);
        let upload = Upload::new(7, None);
        let mut store = fixture.store();
        store.min_free_bytes = u64::MAX;
        assert!(matches!(
            store.execute(upload.admit(), fixture.proof(&upload, 40, 100), 100),
            Err(Failure::Full)
        ));
        let usage = usage(&store.db).unwrap();
        assert_eq!(usage.records, 0);
        assert_eq!(usage.replay_records, 0);
    }
}
