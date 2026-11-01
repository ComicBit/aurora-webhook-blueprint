use crate::{
    config::{Config, Destination},
    model::*,
    security,
};
use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use std::{
    fs::File,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

const SCHEMA_VERSION: i64 = 1;
const MAX_ATTEMPTS: i64 = 10;
const MAX_PLANS: i64 = 100;
const WAL_HIGH_WATER: u64 = 64 * 1024 * 1024;
const WAL_CHECKPOINT_THRESHOLD: u64 = 8 * 1024 * 1024;
const SCHEMA: &str = r#"
CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE configurations(digest TEXT PRIMARY KEY, snapshot TEXT NOT NULL, approved_at INTEGER NOT NULL) STRICT;
CREATE TABLE events(
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 receipt_id TEXT NOT NULL UNIQUE,
 event_id TEXT NOT NULL UNIQUE,
 meeting_id TEXT,
 received_at INTEGER NOT NULL,
 expires_at INTEGER NOT NULL,
 metadata_expires_at INTEGER NOT NULL CHECK(metadata_expires_at >= expires_at),
 test INTEGER NOT NULL CHECK(test IN (0,1)),
 disposition TEXT NOT NULL CHECK(disposition IN ('received','held','deleted','expired')),
 digest TEXT NOT NULL,
 raw BLOB,
 CHECK(raw IS NULL OR length(raw) <= 8388608)
) STRICT;
CREATE INDEX events_expiry ON events(expires_at) WHERE raw IS NOT NULL;
CREATE INDEX events_metadata_expiry ON events(metadata_expires_at);
CREATE INDEX events_meeting ON events(meeting_id,received_at DESC,sequence DESC);
CREATE INDEX events_received ON events(received_at,sequence);
CREATE TRIGGER immutable_receipt BEFORE UPDATE OF receipt_id,event_id,digest,received_at,expires_at,metadata_expires_at ON events
BEGIN SELECT RAISE(ABORT,'immutable receipt'); END;
CREATE TRIGGER immutable_raw BEFORE UPDATE OF raw ON events
WHEN NEW.raw IS NOT NULL AND (OLD.raw IS NULL OR NEW.raw != OLD.raw)
BEGIN SELECT RAISE(ABORT,'immutable payload'); END;
CREATE TABLE deliveries(
 delivery_id TEXT PRIMARY KEY,
 receipt_id TEXT NOT NULL REFERENCES events(receipt_id) ON DELETE CASCADE,
 destination_id TEXT NOT NULL,
 destination_revision TEXT NOT NULL,
 destination_json TEXT NOT NULL,
 generation INTEGER NOT NULL CHECK(generation >= 0),
 state TEXT NOT NULL CHECK(state IN ('pending','running','completed','handed_off','needs_review','expired','deleted')),
 due_at INTEGER NOT NULL,
 attempts INTEGER NOT NULL DEFAULT 0,
 attempt_id TEXT,
 code TEXT,
 external_id TEXT,
 submission_state TEXT NOT NULL DEFAULT 'not_started' CHECK(submission_state IN ('not_started','in_flight','not_admitted','admitted','completed','unknown')),
 result BLOB CHECK(result IS NULL OR length(result) <= 65536),
 result_reserve INTEGER NOT NULL DEFAULT 65536 CHECK(result_reserve IN (0,65536)),
 UNIQUE(receipt_id,destination_id,generation)
) STRICT;
CREATE INDEX deliveries_due ON deliveries(state,due_at,destination_id);
CREATE INDEX deliveries_receipt ON deliveries(receipt_id);
CREATE UNIQUE INDEX one_running_per_destination ON deliveries(destination_id) WHERE state='running';
CREATE TABLE legacy_history(
 receipt_id TEXT PRIMARY KEY REFERENCES events(receipt_id) ON DELETE CASCADE,
 original_state TEXT NOT NULL,
 original_result_digest TEXT,
 result BLOB CHECK(result IS NULL OR length(result)<=65536)
) STRICT;
CREATE TRIGGER immutable_legacy_history BEFORE UPDATE OF receipt_id,original_state,original_result_digest ON legacy_history
BEGIN SELECT RAISE(ABORT,'immutable legacy history'); END;
CREATE TRIGGER immutable_legacy_result BEFORE UPDATE OF result ON legacy_history
WHEN NEW.result IS NOT NULL AND (OLD.result IS NULL OR NEW.result!=OLD.result)
BEGIN SELECT RAISE(ABORT,'immutable legacy result'); END;
CREATE TABLE plans(plan_id TEXT PRIMARY KEY, serialized TEXT NOT NULL, approve_before INTEGER NOT NULL, applied INTEGER NOT NULL DEFAULT 0 CHECK(applied IN (0,1))) STRICT;
CREATE INDEX plans_expiry ON plans(approve_before);
CREATE TABLE usage_counters(
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 logical_bytes INTEGER NOT NULL CHECK(logical_bytes>=0),
 records INTEGER NOT NULL CHECK(records>=0),
 pending INTEGER NOT NULL CHECK(pending>=0),
 running INTEGER NOT NULL CHECK(running>=0),
 plans INTEGER NOT NULL CHECK(plans>=0)
) STRICT;
INSERT INTO usage_counters VALUES(1,12288,0,0,0,0);
CREATE TRIGGER usage_event_insert AFTER INSERT ON events BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes+4096+coalesce(length(NEW.raw),0),records=records+1 WHERE singleton=1;
END;
CREATE TRIGGER usage_event_delete AFTER DELETE ON events BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes-4096-coalesce(length(OLD.raw),0),records=records-1 WHERE singleton=1;
END;
CREATE TRIGGER usage_event_payload AFTER UPDATE OF raw ON events BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes+coalesce(length(NEW.raw),0)-coalesce(length(OLD.raw),0) WHERE singleton=1;
 UPDATE deliveries SET result_reserve=0 WHERE receipt_id=NEW.receipt_id AND NEW.raw IS NULL AND result_reserve!=0;
END;
CREATE TRIGGER usage_delivery_insert AFTER INSERT ON deliveries BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes+4096+length(CAST(NEW.destination_json AS BLOB))+NEW.result_reserve,
 pending=pending+(NEW.state IN ('pending','running')),running=running+(NEW.state='running') WHERE singleton=1;
END;
CREATE TRIGGER usage_delivery_delete AFTER DELETE ON deliveries BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes-4096-length(CAST(OLD.destination_json AS BLOB))-OLD.result_reserve,
 pending=pending-(OLD.state IN ('pending','running')),running=running-(OLD.state='running') WHERE singleton=1;
END;
CREATE TRIGGER usage_delivery_update AFTER UPDATE OF destination_json,result_reserve,state ON deliveries BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes+length(CAST(NEW.destination_json AS BLOB))-length(CAST(OLD.destination_json AS BLOB))+NEW.result_reserve-OLD.result_reserve,
 pending=pending+(NEW.state IN ('pending','running'))-(OLD.state IN ('pending','running')),
 running=running+(NEW.state='running')-(OLD.state='running') WHERE singleton=1;
END;
CREATE TRIGGER usage_plan_insert AFTER INSERT ON plans BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes+4096+length(CAST(NEW.serialized AS BLOB)),plans=plans+1 WHERE singleton=1;
END;
CREATE TRIGGER usage_plan_delete AFTER DELETE ON plans BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes-4096-length(CAST(OLD.serialized AS BLOB)),plans=plans-1 WHERE singleton=1;
END;
CREATE TRIGGER usage_plan_update AFTER UPDATE OF serialized ON plans BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes+length(CAST(NEW.serialized AS BLOB))-length(CAST(OLD.serialized AS BLOB)) WHERE singleton=1;
END;
CREATE TRIGGER usage_config_insert AFTER INSERT ON configurations BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes+4096+length(CAST(NEW.snapshot AS BLOB)) WHERE singleton=1;
END;
CREATE TRIGGER usage_config_delete AFTER DELETE ON configurations BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes-4096-length(CAST(OLD.snapshot AS BLOB)) WHERE singleton=1;
END;
CREATE TRIGGER usage_config_update AFTER UPDATE OF snapshot ON configurations BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes+length(CAST(NEW.snapshot AS BLOB))-length(CAST(OLD.snapshot AS BLOB)) WHERE singleton=1;
END;
CREATE TRIGGER usage_legacy_insert AFTER INSERT ON legacy_history BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes+4096+coalesce(length(NEW.result),0) WHERE singleton=1;
END;
CREATE TRIGGER usage_legacy_delete AFTER DELETE ON legacy_history BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes-4096-coalesce(length(OLD.result),0) WHERE singleton=1;
END;
CREATE TRIGGER usage_legacy_result AFTER UPDATE OF result ON legacy_history BEGIN
 UPDATE usage_counters SET logical_bytes=logical_bytes+coalesce(length(NEW.result),0)-coalesce(length(OLD.result),0) WHERE singleton=1;
END;
CREATE VIEW receipt_metadata AS
 SELECT sequence,receipt_id,event_id,meeting_id,received_at,expires_at,metadata_expires_at,test,digest,
 CASE WHEN disposition='deleted' THEN 'deleted' WHEN expires_at<=unixepoch() THEN 'expired' ELSE disposition END AS disposition,
 raw IS NOT NULL AND expires_at>unixepoch() AS payload_available
 FROM events;
CREATE VIEW delivery_metadata AS
 SELECT d.delivery_id,d.receipt_id,e.event_id,d.destination_id,d.destination_revision,d.generation,
 CASE WHEN e.disposition='deleted' THEN 'deleted' WHEN e.expires_at<=unixepoch() THEN 'expired' ELSE d.state END AS state,
 d.due_at,d.attempts,d.code,e.expires_at,d.submission_state,d.external_id
 FROM deliveries d JOIN events e USING(receipt_id);
PRAGMA user_version=1;
"#;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Serve,
    Admin,
    ReadOnly,
    Apply,
}

pub struct Store {
    pub config: Config,
    connection: Connection,
    _owner_lock: Option<File>,
    mode: Mode,
    receiver_id: String,
    claim_cursor: usize,
}

fn id() -> String {
    Uuid::new_v4().to_string()
}
fn inspect_private(path: &Path, allow_missing: bool) -> Result<()> {
    let path = security::absolute_clean(path)?;
    let mut cursor = PathBuf::from("/");
    for component in path.components() {
        if let Component::Normal(part) = component {
            cursor.push(part);
            match std::fs::symlink_metadata(&cursor) {
                Ok(m) => {
                    ensure!(
                        !m.file_type().is_symlink(),
                        "managed path contains a symlink"
                    );
                    if cursor == path {
                        ensure!(
                            (m.is_dir() || (m.is_file() && m.nlink() == 1))
                                && m.permissions().mode() & 0o077 == 0,
                            "managed path must be private and not hard-linked"
                        );
                    } else {
                        ensure!(m.is_dir(), "managed path has a non-directory ancestor");
                    }
                }
                Err(e)
                    if allow_missing
                        && e.kind() == std::io::ErrorKind::NotFound
                        && cursor == path =>
                {
                    return Ok(());
                }
                Err(_) => bail!("cannot inspect managed path"),
            }
        }
    }
    Ok(())
}

impl Store {
    pub fn open(input: &Config, mode: Mode) -> Result<Self> {
        input.validate()?;
        let readonly = mode == Mode::ReadOnly;
        let owner = if readonly {
            inspect_private(&input.data_dir, false)?;
            None
        } else {
            Some(security::owner_lock(&input.data_dir)?)
        };
        if !readonly {
            inspect_private(&input.data_dir.join("receiver.lock"), false)?;
        }
        let path = input.data_dir.join("inbox.sqlite3");
        for name in [
            "inbox.sqlite3",
            "inbox.sqlite3-wal",
            "inbox.sqlite3-shm",
            "inbox.sqlite3-journal",
        ] {
            inspect_private(&input.data_dir.join(name), true)?;
        }
        let exists = path.try_exists()?;
        ensure!(
            exists || mode == Mode::Apply,
            "inbox is not initialized; apply configuration first"
        );
        if !exists {
            security::private_file(&path, true)?;
        }
        let flags = if readonly {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut connection =
            Connection::open_with_flags(&path, flags).context("cannot open inbox database")?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "recursive_triggers", "ON")?;
        let foreign_keys: i64 =
            connection.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
        ensure!(foreign_keys == 1, "SQLite foreign keys unavailable");
        let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        ensure!(
            version == SCHEMA_VERSION || (version == 0 && mode == Mode::Apply),
            "unsupported inbox schema"
        );
        if version == SCHEMA_VERSION {
            Self::usage(&connection).context("unsupported inbox accounting schema")?;
            connection.prepare("SELECT original_state,original_result_digest,result FROM legacy_history LIMIT 0").context("unsupported inbox legacy schema")?;
        }
        if !readonly {
            let journal: String =
                connection.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
            ensure!(
                journal.eq_ignore_ascii_case("wal"),
                "SQLite WAL unavailable"
            );
            connection.pragma_update(None, "synchronous", "FULL")?;
            connection.pragma_update(None, "fullfsync", "ON")?;
            connection.pragma_update(None, "checkpoint_fullfsync", "ON")?;
            connection.pragma_update(None, "secure_delete", "ON")?;
            connection.pragma_update(None, "wal_autocheckpoint", 256)?;
            // This trims reusable allocation after a WAL reset; the explicit
            // admission guard below handles readers that prevent that reset.
            connection.pragma_update(
                None,
                "journal_size_limit",
                WAL_CHECKPOINT_THRESHOLD as i64,
            )?;
            let sync: i64 = connection.pragma_query_value(None, "synchronous", |r| r.get(0))?;
            let full: i64 = connection.pragma_query_value(None, "fullfsync", |r| r.get(0))?;
            ensure!(
                sync == 2 && full == 1,
                "SQLite durable synchronization unavailable"
            );
        } else {
            connection.pragma_update(None, "query_only", "ON")?;
        }
        if !readonly {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if version == 0 {
                let count: i64 = tx.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                    [],
                    |r| r.get(0),
                )?;
                ensure!(count == 0, "refusing unrecognized database schema");
                tx.execute_batch(SCHEMA)?;
                tx.execute("INSERT INTO metadata VALUES('receiver_id',?1)", [id()])?;
            }
            if mode == Mode::Apply {
                let digest = input.digest()?;
                tx.execute("INSERT OR IGNORE INTO configurations(digest,snapshot,approved_at) VALUES(?1,?2,?3)",params![digest,serde_json::to_string(input)?,now()])?;
                tx.execute("INSERT INTO metadata(key,value) VALUES('active_config',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[digest])?;
                // Destination snapshots live with their intents. Retain only the active
                // full configuration so repeated administrative edits cannot grow forever.
                tx.execute("DELETE FROM configurations WHERE digest!=(SELECT value FROM metadata WHERE key='active_config')",[])?;
            }
            // Admission may already have happened before the previous process disappeared.
            // Even folder delivery requires an explicit retry to reconcile its receipt.
            tx.execute("UPDATE deliveries SET state='needs_review',submission_state='unknown',code='interrupted' WHERE state='running'",[])?;
            tx.commit()?;
        }
        let (receiver_id, active_digest, snapshot): (String,String,String)=connection.query_row(
            "SELECT r.value,a.value,c.snapshot FROM metadata r,metadata a JOIN configurations c ON c.digest=a.value WHERE r.key='receiver_id' AND a.key='active_config'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).context("inbox has no approved configuration")?;
        let config: Config =
            serde_json::from_str(&snapshot).context("invalid approved configuration")?;
        config.validate()?;
        ensure!(
            config.digest()? == active_digest && config.data_dir == input.data_dir,
            "approved configuration integrity failure"
        );
        if mode == Mode::Serve {
            ensure!(
                input.digest()? == active_digest,
                "configuration differs from approved snapshot; review and apply offline"
            );
        }
        for name in ["inbox.sqlite3", "inbox.sqlite3-wal", "inbox.sqlite3-shm"] {
            inspect_private(&input.data_dir.join(name), true)?;
        }
        if !readonly {
            File::open(&input.data_dir)?.sync_all()?;
        }
        Ok(Self {
            config,
            connection,
            _owner_lock: owner,
            mode,
            receiver_id,
            claim_cursor: 0,
        })
    }

    fn writable(&self) -> Result<()> {
        ensure!(self.mode != Mode::ReadOnly, "read-only inbox cannot mutate");
        Ok(())
    }
    fn admin(&self) -> Result<()> {
        ensure!(
            self.mode == Mode::Admin || self.mode == Mode::Apply,
            "operation requires offline administration"
        );
        Ok(())
    }
    fn headroom(&self, bytes: u64) -> Result<bool> {
        Ok(security::free_bytes(&self.config.data_dir)?
            >= self.config.min_free_bytes.saturating_add(bytes))
    }
    fn usage(tx: &Connection) -> Result<(u64, u64, u64)> {
        let (bytes, records, pending): (i64, i64, i64) = tx.query_row(
            "SELECT logical_bytes,records,pending FROM usage_counters WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok((bytes.try_into()?, records.try_into()?, pending.try_into()?))
    }
    fn fits(
        config: &Config,
        db: &Connection,
        bytes: u64,
        events: u64,
        pending: u64,
    ) -> Result<bool> {
        let (used, records, queued) = Self::usage(db)?;
        Ok(used.saturating_add(bytes) <= config.max_storage_bytes
            && records.saturating_add(events) <= u64::from(config.max_records)
            && queued.saturating_add(pending) <= u64::from(config.max_pending))
    }
    fn delivery_charge(destination: &Destination) -> Result<u64> {
        Ok(RECORD_CHARGE + MAX_RESULT as u64 + serde_json::to_string(destination)?.len() as u64)
    }
    fn add_delivery(
        tx: &Transaction<'_>,
        receipt: &str,
        destination: &Destination,
        generation: i64,
        at: i64,
    ) -> Result<()> {
        tx.execute("INSERT INTO deliveries(delivery_id,receipt_id,destination_id,destination_revision,destination_json,generation,state,due_at) VALUES(?1,?2,?3,?4,?5,?6,'pending',?7)",params![id(),receipt,destination.id,destination.revision()?,serde_json::to_string(destination)?,generation,at])?;
        Ok(())
    }

    fn checkpoint(&self, force: bool) -> Result<bool> {
        let bytes = match std::fs::metadata(self.config.data_dir.join("inbox.sqlite3-wal")) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(_) => bail!("cannot inspect inbox WAL"),
        };
        if !force && bytes < WAL_CHECKPOINT_THRESHOLD {
            return Ok(true);
        }
        // PASSIVE never waits for a reader or writer. A fully copied WAL can
        // be reused by the next write even before its allocated file shrinks.
        let (_busy, frames, copied): (i64, i64, i64) =
            self.connection
                .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?;
        Ok(bytes < WAL_HIGH_WATER || frames <= copied)
    }

    pub fn accept(&mut self, event: VerifiedEvent, at: i64) -> Result<Acceptance> {
        self.writable()?;
        // Look up retries before quota or expiry work. A retry never refreshes a deadline.
        let duplicate: Option<(String, bool)> = self
            .connection
            .query_row(
                "SELECT digest,raw IS NULL OR raw=?2 FROM events WHERE event_id=?1",
                params![event.event_id, event.body],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((digest, same_body)) = duplicate {
            return Ok(if digest == event.digest && same_body {
                Acceptance::Duplicate
            } else {
                Acceptance::Conflict
            });
        }
        if !self.checkpoint(false)? {
            return Ok(Acceptance::Full);
        }
        ensure!(event.body.len() <= MAX_BODY, "event exceeds body bound");
        let destinations: Vec<_> = if event.test {
            vec![]
        } else {
            self.config
                .destinations
                .iter()
                .filter(|d| d.enabled)
                .collect()
        };
        let charge = destinations
            .iter()
            .try_fold(event.body.len() as u64 + RECORD_CHARGE, |sum, d| {
                Ok::<_, anyhow::Error>(sum + Self::delivery_charge(d)?)
            })?;
        if !self.headroom(charge)? {
            return Ok(Acceptance::Full);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !Self::fits(&self.config, &tx, charge, 1, destinations.len() as u64)? {
            return Ok(Acceptance::Full);
        }
        let receipt = id();
        let expiry = at
            .checked_add(self.config.retention_seconds)
            .context("retention deadline overflow")?;
        let metadata_expiry = expiry
            .checked_add(self.config.metadata_retention_seconds)
            .context("metadata deadline overflow")?;
        tx.execute("INSERT INTO events(receipt_id,event_id,meeting_id,received_at,expires_at,metadata_expires_at,test,disposition,digest,raw) VALUES(?1,?2,?3,?4,?5,?6,?7,'received',?8,?9)",params![receipt,event.event_id,event.meeting_id,at,expiry,metadata_expiry,event.test,event.digest,event.body])?;
        for destination in destinations {
            Self::add_delivery(&tx, &receipt, destination, 0, at)?;
        }
        tx.commit()?;
        Ok(Acceptance::Accepted)
    }

    pub fn claim(&mut self, busy_destinations: &[String], at: i64) -> Result<Option<DeliveryJob>> {
        self.writable()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let running: i64 = tx.query_row(
            "SELECT running FROM usage_counters WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        if running >= self.config.max_active_deliveries as i64 {
            return Ok(None);
        }
        let mut selected = None;
        let mut next_cursor = self.claim_cursor;
        for offset in 0..self.config.destinations.len() {
            let index = (self.claim_cursor + offset) % self.config.destinations.len();
            let destination = &self.config.destinations[index];
            if !destination.enabled || busy_destinations.contains(&destination.id) {
                continue;
            }
            let row: Option<(String,String,String,Vec<u8>,i64)>=tx.query_row("SELECT d.delivery_id,e.receipt_id,e.event_id,e.raw,e.expires_at FROM deliveries d JOIN events e USING(receipt_id) WHERE d.state='pending' AND d.due_at<=?1 AND d.destination_id=?2 AND d.destination_revision=?3 AND e.raw IS NOT NULL AND e.expires_at>?1 AND e.test=0 AND NOT EXISTS(SELECT 1 FROM deliveries r WHERE r.destination_id=d.destination_id AND r.state='running') ORDER BY d.due_at,e.sequence,d.generation LIMIT 1",params![at,destination.id,destination.revision()?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
            if let Some((delivery_id, receipt_id, event_id, body, expires_at)) = row {
                let attempt_id = id();
                tx.execute("UPDATE deliveries SET state='running',submission_state='in_flight',attempt_id=?2,attempts=attempts+1,code=NULL WHERE delivery_id=?1",params![delivery_id,attempt_id])?;
                selected = Some(DeliveryJob {
                    delivery_id,
                    attempt_id,
                    receipt_id,
                    event_id,
                    destination: destination.clone(),
                    body: body.into(),
                    expires_at,
                });
                next_cursor = (index + 1) % self.config.destinations.len();
                break;
            }
        }
        tx.commit()?;
        self.claim_cursor = next_cursor;
        Ok(selected)
    }

    pub fn finish(
        &mut self,
        delivery_id: &str,
        attempt_id: &str,
        outcome: DeliveryOutcome,
        at: i64,
    ) -> Result<()> {
        self.writable()?;
        ensure!(
            outcome.code.len() <= 128
                && outcome
                    .code
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
            "invalid delivery diagnostic code"
        );
        ensure!(
            outcome
                .result
                .as_ref()
                .is_none_or(|r| r.len() <= MAX_RESULT),
            "delivery result exceeds bound"
        );
        ensure!(
            outcome
                .external_id
                .as_ref()
                .is_none_or(|v| v.len() <= 512 && !v.contains(['\n', '\r', '\0'])),
            "invalid external identifier"
        );
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row: Option<(String,Option<String>,i64,i64,bool)>=tx.query_row("SELECT d.state,d.attempt_id,d.attempts,e.expires_at,e.raw IS NOT NULL FROM deliveries d JOIN events e USING(receipt_id) WHERE d.delivery_id=?1",[delivery_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let Some((state, active, attempts, expiry, retained)) = row else {
            bail!("delivery not found");
        };
        ensure!(
            active.as_deref() == Some(attempt_id),
            "delivery attempt is not active"
        );
        let submission_state = match &outcome.kind {
            OutcomeKind::Completed => "completed",
            OutcomeKind::HandedOff => "admitted",
            OutcomeKind::NotAdmitted => "not_admitted",
            OutcomeKind::NeedsReview => "unknown",
        };
        if !retained || expiry <= at {
            tx.execute("UPDATE deliveries SET state=CASE WHEN state='deleted' THEN state ELSE 'expired' END,result=NULL,submission_state=?2,external_id=coalesce(?3,external_id),code=?4 WHERE delivery_id=?1",params![delivery_id,submission_state,outcome.external_id,outcome.code])?;
            tx.commit()?;
            return Ok(());
        }
        ensure!(
            state == "running" && active.as_deref() == Some(attempt_id),
            "delivery attempt is not active"
        );
        let (state, due) = match outcome.kind {
            OutcomeKind::Completed => ("completed", at),
            OutcomeKind::HandedOff => ("handed_off", at),
            OutcomeKind::NeedsReview => ("needs_review", at),
            OutcomeKind::NotAdmitted if attempts < MAX_ATTEMPTS => {
                ("pending", at.saturating_add((1i64 << attempts.min(9)) * 5))
            }
            OutcomeKind::NotAdmitted => ("needs_review", at),
        };
        tx.execute("UPDATE deliveries SET state=?2,due_at=?3,code=?4,result=?5,external_id=?6,submission_state=?7 WHERE delivery_id=?1",params![delivery_id,state,due,outcome.code,outcome.result,outcome.external_id,submission_state])?;
        tx.commit()?;
        Ok(())
    }

    pub fn expire(&mut self, at: i64) -> Result<()> {
        self.writable()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE deliveries SET state=CASE WHEN state='deleted' THEN state ELSE 'expired' END,result=NULL,submission_state=CASE WHEN submission_state='in_flight' THEN 'unknown' ELSE submission_state END,code=coalesce(code,'payload_expired') WHERE receipt_id IN (SELECT receipt_id FROM events WHERE expires_at<=?1 AND raw IS NOT NULL)",[at])?;
        tx.execute("UPDATE legacy_history SET result=NULL WHERE result IS NOT NULL AND receipt_id IN (SELECT receipt_id FROM events WHERE expires_at<=?1)",[at])?;
        tx.execute("UPDATE events SET raw=NULL,disposition=CASE WHEN disposition='deleted' THEN disposition ELSE 'expired' END WHERE expires_at<=?1 AND raw IS NOT NULL",[at])?;
        tx.execute("DELETE FROM events WHERE metadata_expires_at<=?1", [at])?;
        tx.execute("DELETE FROM plans WHERE approve_before<=?1", [at])?;
        tx.commit()?;
        self.checkpoint(true)?;
        Ok(())
    }

    pub fn status(&self, at: i64) -> Result<serde_json::Value> {
        let (bytes, records, pending) = Self::usage(&self.connection)?;
        let mut statement=self.connection.prepare("SELECT CASE WHEN e.expires_at<=?1 OR e.raw IS NULL THEN CASE WHEN e.disposition='deleted' THEN 'deleted' ELSE 'expired' END ELSE d.state END,count(*) FROM deliveries d JOIN events e USING(receipt_id) GROUP BY 1")?;
        let states = statement
            .query_map([at], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<std::collections::BTreeMap<_, _>>>()?;
        let mut event_statement=self.connection.prepare("SELECT CASE WHEN disposition='deleted' THEN 'deleted' WHEN expires_at<=?1 THEN 'expired' ELSE disposition END,count(*) FROM events GROUP BY 1")?;
        let event_states = event_statement
            .query_map([at], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<std::collections::BTreeMap<_, _>>>()?;
        let blocked: i64 = {
            let mut n = 0;
            let mut stmt=self.connection.prepare("SELECT destination_id,destination_revision,count(*) FROM deliveries d JOIN events e USING(receipt_id) WHERE d.state='pending' AND e.raw IS NOT NULL AND e.expires_at>?1 GROUP BY destination_id,destination_revision")?;
            for row in stmt.query_map([at], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })? {
                let (id, revision, count) = row?;
                if !self.config.destinations.iter().any(|d| {
                    d.enabled && d.id == id && d.revision().ok().as_deref() == Some(&revision)
                }) {
                    n += count;
                }
            }
            n
        };
        let mut stmt=self.connection.prepare("SELECT d.delivery_id,d.receipt_id,e.event_id,d.destination_id,d.destination_revision,d.generation,CASE WHEN e.disposition='deleted' THEN 'deleted' WHEN e.expires_at<=?1 THEN 'expired' ELSE d.state END,d.attempts,d.code,d.external_id,e.expires_at,e.raw IS NOT NULL AND e.expires_at>?1,d.submission_state FROM deliveries d JOIN events e USING(receipt_id) ORDER BY e.sequence DESC,d.destination_id,d.generation DESC LIMIT 1000")?;
        let deliveries=stmt.query_map([at],|r|Ok(serde_json::json!({"delivery_id":r.get::<_,String>(0)?,"receipt_id":r.get::<_,String>(1)?,"event_id":r.get::<_,String>(2)?,"destination_id":r.get::<_,String>(3)?,"destination_revision":r.get::<_,String>(4)?,"generation":r.get::<_,i64>(5)?,"state":r.get::<_,String>(6)?,"attempts":r.get::<_,i64>(7)?,"code":r.get::<_,Option<String>>(8)?,"external_id":r.get::<_,Option<String>>(9)?,"expires_at":r.get::<_,i64>(10)?,"payload_available":r.get::<_,bool>(11)?,"submission_state":r.get::<_,String>(12)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut legacy_statement=self.connection.prepare("SELECT h.receipt_id,e.event_id,h.original_state,e.expires_at,h.result IS NOT NULL AND e.expires_at>?1 AND e.disposition NOT IN ('deleted','expired') FROM legacy_history h JOIN events e USING(receipt_id) ORDER BY e.sequence DESC LIMIT 1000")?;
        let legacy_history=legacy_statement.query_map([at],|r|Ok(serde_json::json!({"receipt_id":r.get::<_,String>(0)?,"event_id":r.get::<_,String>(1)?,"original_state":r.get::<_,String>(2)?,"expires_at":r.get::<_,i64>(3)?,"result_available":r.get::<_,bool>(4)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(
            serde_json::json!({"receiver_id":self.receiver_id,"config_digest":self.config.digest()?,"logical_bytes":bytes,"max_storage_bytes":self.config.max_storage_bytes,"records":records,"event_states":event_states,"pending_inflight":pending,"blocked_revision":blocked,"states":states,"deliveries":deliveries,"delivery_limit":1000,"legacy_history":legacy_history,"legacy_history_limit":1000,"deduplication_horizon":"original payload deadline plus metadata_retention_seconds"}),
        )
    }

    pub fn list(&self, after: Option<&str>, limit: usize, at: i64) -> Result<Vec<EventRecord>> {
        ensure!((1..=1000).contains(&limit), "list limit must be 1..1000");
        let sequence = match after {
            Some(cursor) => self
                .connection
                .query_row(
                    "SELECT sequence FROM events WHERE receipt_id=?1",
                    [cursor],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .context("receipt cursor no longer available")?,
            None => 0,
        };
        let mut statement=self.connection.prepare("SELECT receipt_id,event_id,meeting_id,received_at,expires_at,metadata_expires_at,test,CASE WHEN disposition='deleted' THEN 'deleted' WHEN expires_at<=?3 THEN 'expired' ELSE disposition END,digest FROM events WHERE sequence>?1 ORDER BY sequence LIMIT ?2")?;
        Ok(statement
            .query_map(params![sequence, limit as i64, at], |r| {
                Ok(EventRecord {
                    receipt_id: r.get(0)?,
                    event_id: r.get(1)?,
                    meeting_id: r.get(2)?,
                    received_at: r.get(3)?,
                    expires_at: r.get(4)?,
                    metadata_expires_at: r.get(5)?,
                    test: r.get(6)?,
                    disposition: r.get(7)?,
                    digest: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn raw(&self, event_id: &str, at: i64) -> Result<Vec<u8>> {
        self.connection
            .query_row(
                "SELECT raw FROM events WHERE event_id=?1 AND expires_at>?2 AND raw IS NOT NULL",
                params![event_id, at],
                |r| r.get(0),
            )
            .optional()?
            .context("retained event body not found")
    }
    pub fn result(&self, delivery_id: &str, at: i64) -> Result<Vec<u8>> {
        self.connection.query_row("SELECT d.result FROM deliveries d JOIN events e USING(receipt_id) WHERE d.delivery_id=?1 AND e.expires_at>?2 AND e.raw IS NOT NULL AND d.result IS NOT NULL",params![delivery_id,at],|r|r.get(0)).optional()?.context("retained delivery result not found")
    }
    pub fn legacy_result(&self, event_id: &str, at: i64) -> Result<Vec<u8>> {
        self.connection.query_row("SELECT h.result FROM legacy_history h JOIN events e USING(receipt_id) WHERE e.event_id=?1 AND e.expires_at>?2 AND e.disposition NOT IN ('deleted','expired') AND h.result IS NOT NULL",params![event_id,at],|r|r.get(0)).optional()?.context("retained legacy result not found")
    }
    pub fn delete(&mut self, event_id: &str, _at: i64) -> Result<()> {
        self.admin()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE events SET raw=NULL,disposition='deleted' WHERE event_id=?1",
            [event_id],
        )?;
        ensure!(changed == 1, "event not found");
        tx.execute("UPDATE deliveries SET state='deleted',result=NULL,submission_state=CASE WHEN submission_state='in_flight' THEN 'unknown' ELSE submission_state END,code='deleted' WHERE receipt_id=(SELECT receipt_id FROM events WHERE event_id=?1)",[event_id])?;
        tx.execute("UPDATE legacy_history SET result=NULL WHERE receipt_id=(SELECT receipt_id FROM events WHERE event_id=?1)",[event_id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn retry(
        &mut self,
        delivery_id: &str,
        acknowledge_duplicate_risk: bool,
        at: i64,
    ) -> Result<()> {
        self.admin()?;
        ensure!(
            acknowledge_duplicate_risk,
            "explicit duplicate-risk acknowledgement is required"
        );
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row: Option<(String,String,String,String,bool)>=tx.query_row("SELECT d.receipt_id,d.destination_id,d.destination_revision,d.state,e.raw IS NOT NULL AND e.expires_at>?2 AND e.test=0 FROM deliveries d JOIN events e USING(receipt_id) WHERE d.delivery_id=?1",params![delivery_id,at],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let (receipt, destination_id, revision, state, retained) =
            row.context("delivery not found")?;
        ensure!(
            retained,
            "delivery payload is unavailable or is a test event"
        );
        ensure!(
            matches!(state.as_str(), "completed" | "handed_off" | "needs_review"),
            "only terminal deliveries can be explicitly repeated"
        );
        let destination = self
            .config
            .destinations
            .iter()
            .find(|d| d.id == destination_id && d.enabled)
            .context("destination is not enabled")?;
        ensure!(
            destination.revision()? == revision,
            "destination revision differs; retry cannot redirect an existing intent"
        );
        let charge = Self::delivery_charge(destination)?;
        ensure!(
            security::free_bytes(&self.config.data_dir)?
                >= self.config.min_free_bytes.saturating_add(charge),
            "insufficient storage headroom"
        );
        let active: i64=tx.query_row("SELECT count(*) FROM deliveries WHERE receipt_id=?1 AND destination_id=?2 AND state IN ('pending','running')",params![receipt,destination_id],|r|r.get(0))?;
        ensure!(active == 0, "delivery already has active work");
        ensure!(
            Self::fits(&self.config, &tx, charge, 0, 1)?,
            "inbox quota exceeded"
        );
        let generation: i64 = tx.query_row(
            "SELECT max(generation)+1 FROM deliveries WHERE receipt_id=?1 AND destination_id=?2",
            params![receipt, destination_id],
            |r| r.get(0),
        )?;
        Self::add_delivery(&tx, &receipt, destination, generation, at)?;
        tx.commit()?;
        Ok(())
    }

    pub fn preview(
        &mut self,
        dest_id: &str,
        start: i64,
        end: i64,
        all_revisions: bool,
        at: i64,
    ) -> Result<BackfillPlan> {
        self.admin()?;
        ensure!(
            start < end,
            "historical interval must be nonempty and half-open"
        );
        let destination = self
            .config
            .destinations
            .iter()
            .find(|d| d.id == dest_id && d.enabled)
            .context("destination is not enabled")?;
        let mut plan = BackfillPlan {
            format_version: 1,
            plan_id: id(),
            receiver_id: self.receiver_id.clone(),
            config_digest: self.config.digest()?,
            destination_id: dest_id.into(),
            destination_revision: destination.revision()?,
            created_at: at,
            approve_before: at.saturating_add(900),
            start_received_at: start,
            end_received_at: end,
            all_revisions,
            events: vec![],
            unavailable: 0,
        };
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM plans WHERE approve_before<=?1", [at])?;
        let plans: i64 = tx.query_row(
            "SELECT plans FROM usage_counters WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        ensure!(plans < MAX_PLANS, "too many outstanding previews");
        // Choose the latest receipt in the requested range before checking availability.
        // A newer unavailable revision must not resurrect an older revision implicitly.
        {
            let mut stmt=tx.prepare("SELECT e.receipt_id,e.event_id,e.digest,e.expires_at,e.raw IS NOT NULL AND e.expires_at>?3 AND e.disposition!='deleted',EXISTS(SELECT 1 FROM deliveries d WHERE d.receipt_id=e.receipt_id AND d.destination_id=?4 AND d.generation=0) FROM events e WHERE e.received_at>=?1 AND e.received_at<?2 AND e.test=0 AND (?5 OR e.meeting_id IS NULL OR NOT EXISTS(SELECT 1 FROM events n WHERE n.meeting_id=e.meeting_id AND n.test=0 AND n.received_at>=?1 AND n.received_at<?2 AND (n.received_at>e.received_at OR (n.received_at=e.received_at AND n.sequence>e.sequence)))) ORDER BY e.sequence")?;
            let mut rows = stmt.query(params![start, end, at, dest_id, all_revisions])?;
            while let Some(row) = rows.next()? {
                let available: bool = row.get(4)?;
                let already_routed: bool = row.get(5)?;
                if !available {
                    plan.unavailable += 1;
                    continue;
                }
                if already_routed {
                    continue;
                }
                ensure!(
                    plan.events.len() < 1000,
                    "preview exceeds 1000 intents; narrow the receipt interval"
                );
                let selection = Selection {
                    receipt_id: row.get(0)?,
                    event_id: row.get(1)?,
                    digest: row.get(2)?,
                    expires_at: row.get(3)?,
                };
                plan.approve_before = plan.approve_before.min(selection.expires_at);
                plan.events.push(selection);
            }
        }
        let serialized = serde_json::to_string(&plan)?;
        ensure!(
            security::free_bytes(&self.config.data_dir)?
                >= self
                    .config
                    .min_free_bytes
                    .saturating_add(serialized.len() as u64 + RECORD_CHARGE),
            "insufficient storage headroom"
        );
        ensure!(
            Self::fits(
                &self.config,
                &tx,
                serialized.len() as u64 + RECORD_CHARGE,
                0,
                0
            )?,
            "inbox quota exceeded"
        );
        tx.execute(
            "INSERT INTO plans(plan_id,serialized,approve_before) VALUES(?1,?2,?3)",
            params![plan.plan_id, serialized, plan.approve_before],
        )?;
        tx.commit()?;
        Ok(plan)
    }

    pub fn approve(&mut self, plan: &BackfillPlan, at: i64) -> Result<usize> {
        self.admin()?;
        ensure!(plan.events.len() <= 1000, "preview exceeds intent bound");
        ensure!(
            plan.format_version == 1 && plan.receiver_id == self.receiver_id,
            "preview belongs to a different inbox or format"
        );
        ensure!(
            plan.config_digest == self.config.digest()?,
            "approved configuration changed since preview"
        );
        ensure!(at < plan.approve_before, "preview expired");
        let destination = self
            .config
            .destinations
            .iter()
            .find(|d| d.id == plan.destination_id && d.enabled)
            .context("destination is not enabled")?;
        ensure!(
            destination.revision()? == plan.destination_revision,
            "destination changed since preview"
        );
        let charge = Self::delivery_charge(destination)? * plan.events.len() as u64;
        ensure!(self.headroom(charge)?, "insufficient storage headroom");
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (stored, applied): (String, bool) = tx
            .query_row(
                "SELECT serialized,applied FROM plans WHERE plan_id=?1",
                [&plan.plan_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .context("stored preview not found")?;
        ensure!(
            stored == serde_json::to_string(plan)?,
            "preview does not match the frozen local selection"
        );
        if applied {
            return Ok(0);
        }
        let mut selected = Vec::new();
        for selection in &plan.events {
            let matches: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE receipt_id=?1 AND event_id=?2 AND digest=?3 AND expires_at=?4 AND expires_at>?5 AND raw IS NOT NULL AND test=0 AND disposition!='deleted')",params![selection.receipt_id,selection.event_id,selection.digest,selection.expires_at,at],|r|r.get(0))?;
            ensure!(matches, "preview payload identity or availability changed");
            let exists: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM deliveries WHERE receipt_id=?1 AND destination_id=?2 AND generation=0)",params![selection.receipt_id,plan.destination_id],|r|r.get(0))?;
            if !exists {
                selected.push(&selection.receipt_id);
            }
        }
        let count = selected.len();
        ensure!(
            Self::fits(
                &self.config,
                &tx,
                Self::delivery_charge(destination)? * count as u64,
                0,
                count as u64
            )?,
            "inbox quota exceeded"
        );
        for receipt in selected {
            Self::add_delivery(&tx, receipt, destination, 0, at)?;
        }
        tx.execute(
            "UPDATE plans SET applied=1 WHERE plan_id=?1",
            [&plan.plan_id],
        )?;
        tx.commit()?;
        Ok(count)
    }

    pub fn import_legacy(&mut self, row: LegacyRecord, at: i64) -> Result<()> {
        use sha2::{Digest, Sha256};
        self.admin()?;
        ensure!(
            row.format_version == 1 && !row.event_id.is_empty() && row.event_id.len() <= 256,
            "invalid legacy identity"
        );
        ensure!(row.received_at <= row.expires_at, "invalid legacy deadline");
        ensure!(
            row.digest.len() == 64
                && row.digest.bytes().all(|c| c.is_ascii_hexdigit())
                && row.digest == row.digest.to_ascii_lowercase(),
            "invalid legacy digest"
        );
        ensure!(
            matches!(
                row.state.as_str(),
                "received"
                    | "held"
                    | "deleted"
                    | "expired"
                    | "pending"
                    | "running"
                    | "completed"
                    | "failed"
                    | "delivered"
                    | "needs_review"
                    | "done"
                    | "test"
            ),
            "invalid legacy state"
        );
        let raw = match row.body_base64 {
            Some(encoded) => {
                ensure!(
                    encoded.len() <= MAX_BODY.div_ceil(3) * 4,
                    "legacy body exceeds bound"
                );
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .context("invalid legacy body encoding")?;
                ensure!(
                    bytes.len() <= MAX_BODY && hex::encode(Sha256::digest(&bytes)) == row.digest,
                    "legacy body digest mismatch"
                );
                Some(bytes)
            }
            None => None,
        };
        let result = match row.result_base64 {
            Some(encoded) => {
                ensure!(
                    encoded.len() <= MAX_RESULT.div_ceil(3) * 4,
                    "legacy result exceeds bound"
                );
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .context("invalid legacy result encoding")?;
                ensure!(bytes.len() <= MAX_RESULT, "legacy result exceeds bound");
                Some(bytes)
            }
            None => None,
        };
        let result_digest = result
            .as_ref()
            .map(|bytes| hex::encode(Sha256::digest(bytes)));
        let (raw, meeting, body_test) = match raw {
            Some(bytes) => {
                let event = crate::intake::validate_body(bytes, &row.event_id)?;
                (Some(event.body), Some(event.meeting_id), event.test)
            }
            None => (None, None, false),
        };
        let test = row.test || row.state == "test" || body_test;
        let metadata_expiry = row
            .expires_at
            .checked_add(self.config.metadata_retention_seconds)
            .context("legacy metadata deadline overflow")?;
        ensure!(
            metadata_expiry > at,
            "legacy record is beyond the finite metadata horizon"
        );
        let deleted = row.state == "deleted";
        let expired = row.expires_at <= at || row.state == "expired";
        let retained = if !deleted && !expired { raw } else { None };
        let retained_result = if !deleted && !expired { result } else { None };
        let charge = 2 * RECORD_CHARGE
            + retained.as_ref().map_or(0, |r| r.len() as u64)
            + retained_result.as_ref().map_or(0, |r| r.len() as u64);
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        type LegacyIdentity = (String, i64, i64, bool, Option<String>, Option<String>);
        let existing: Option<LegacyIdentity>=tx.query_row("SELECT e.digest,e.received_at,e.expires_at,e.test,h.original_state,h.original_result_digest FROM events e LEFT JOIN legacy_history h USING(receipt_id) WHERE e.event_id=?1",[&row.event_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
        if let Some((digest, received, expiry, was_test, state, old_result_digest)) = existing {
            ensure!(
                digest == row.digest
                    && received == row.received_at
                    && expiry == row.expires_at
                    && was_test == test
                    && state.as_deref() == Some(row.state.as_str())
                    && old_result_digest == result_digest,
                "legacy record conflicts with existing receipt"
            );
            return Ok(());
        }
        ensure!(
            security::free_bytes(&self.config.data_dir)?
                >= self.config.min_free_bytes.saturating_add(charge),
            "insufficient storage headroom"
        );
        ensure!(
            Self::fits(&self.config, &tx, charge, 1, 0)?,
            "inbox quota exceeded"
        );
        let disposition = if deleted {
            "deleted"
        } else if expired {
            "expired"
        } else {
            "held"
        };
        let receipt = id();
        tx.execute("INSERT INTO events(receipt_id,event_id,meeting_id,received_at,expires_at,metadata_expires_at,test,disposition,digest,raw) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![receipt,row.event_id,meeting,row.received_at,row.expires_at,metadata_expiry,test,disposition,row.digest,retained])?;
        tx.execute("INSERT INTO legacy_history(receipt_id,original_state,original_result_digest,result) VALUES(?1,?2,?3,?4)",params![receipt,row.state,result_digest,retained_result])?;
        tx.commit()?;
        Ok(())
    }

    pub fn backup(&self, destination: &Path) -> Result<()> {
        self.admin()?;
        let destination = security::absolute_clean(destination)?;
        let parent = destination.parent().context("backup path has no parent")?;
        inspect_private(parent, false)?;
        // Reserve a private inode without overwriting even an empty existing file.
        security::write_private_new(&destination, &[])?;
        let mut target =
            Connection::open_with_flags(&destination, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        {
            let backup = rusqlite::backup::Backup::new(&self.connection, &mut target)?;
            backup.run_to_completion(64, Duration::from_millis(10), None)?;
        }
        let integrity: String = target.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        ensure!(integrity == "ok", "backup integrity check failed");
        target.close().map_err(|(_, e)| e)?;
        File::open(&destination)?.sync_all()?;
        File::open(parent)?.sync_all()?;
        inspect_private(&destination, false)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DestinationKind, FolderFormat};
    use sha2::{Digest, Sha256};

    struct Inbox {
        _root: tempfile::TempDir,
        config: Config,
    }
    impl Inbox {
        fn new(destinations: usize) -> Self {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().canonicalize().unwrap();
            let config = Config {
                data_dir: path.join("inbox"),
                min_free_bytes: 0,
                retention_seconds: 100,
                metadata_retention_seconds: 300,
                destinations: (0..destinations)
                    .map(|n| Destination {
                        id: format!("dest{n}"),
                        enabled: true,
                        kind: DestinationKind::Folder {
                            root: path.join(format!("export{n}")),
                            formats: vec![FolderFormat::OriginalJson],
                        },
                    })
                    .collect(),
                ..Default::default()
            };
            Self {
                _root: root,
                config,
            }
        }
        fn open(&self) -> Store {
            Store::open(&self.config, Mode::Apply).unwrap()
        }
    }
    fn event(event_id: &str, meeting_id: &str, test: bool) -> VerifiedEvent {
        let body =
            format!("{{\"id\":\"{event_id}\",\"meeting\":\"{meeting_id}\",\"test\":{test}}}")
                .into_bytes();
        VerifiedEvent {
            event_id: event_id.into(),
            meeting_id: meeting_id.into(),
            test,
            digest: hex::encode(Sha256::digest(&body)),
            body,
        }
    }
    fn finish(store: &mut Store, job: &DeliveryJob, kind: OutcomeKind, at: i64) {
        store
            .finish(
                &job.delivery_id,
                &job.attempt_id,
                DeliveryOutcome {
                    kind,
                    code: "synthetic".into(),
                    result: Some(b"result".to_vec()),
                    external_id: None,
                },
                at,
            )
            .unwrap();
    }
    fn assert_usage(store: &Store) {
        let db = &store.connection;
        let events: i64 = db
            .query_row(
                "SELECT coalesce(sum(4096+coalesce(length(raw),0)),0) FROM events",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let deliveries: i64=db.query_row("SELECT coalesce(sum(4096+length(CAST(destination_json AS BLOB))+result_reserve),0) FROM deliveries",[],|r|r.get(0)).unwrap();
        let plans: i64 = db
            .query_row(
                "SELECT coalesce(sum(4096+length(CAST(serialized AS BLOB))),0) FROM plans",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let configs: i64 = db
            .query_row(
                "SELECT coalesce(sum(4096+length(CAST(snapshot AS BLOB))),0) FROM configurations",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let legacy: i64 = db
            .query_row(
                "SELECT coalesce(sum(4096+coalesce(length(result),0)),0) FROM legacy_history",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let records: i64 = db
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap();
        let pending: i64 = db
            .query_row(
                "SELECT count(*) FROM deliveries WHERE state IN ('pending','running')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let running: i64 = db
            .query_row(
                "SELECT count(*) FROM deliveries WHERE state='running'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let plan_count: i64 = db
            .query_row("SELECT count(*) FROM plans", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            Store::usage(db).unwrap(),
            (
                (12288 + events + deliveries + plans + configs + legacy) as u64,
                records as u64,
                pending as u64
            )
        );
        assert_eq!(
            db.query_row(
                "SELECT running,plans FROM usage_counters WHERE singleton=1",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            )
            .unwrap(),
            (running, plan_count)
        );
    }
    fn delivery_count(store: &Store) -> i64 {
        assert_usage(store);
        store
            .connection
            .query_row("SELECT count(*) FROM deliveries", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn duplicates_precede_capacity_and_never_route_new_destinations() {
        let mut inbox = Inbox::new(0);
        let mut store = inbox.open();
        assert_eq!(
            store.accept(event("one", "meeting", false), 10).unwrap(),
            Acceptance::Accepted
        );
        let receipt = store.list(None, 10, 11).unwrap().remove(0);
        drop(store);
        inbox.config.destinations = vec![Destination {
            id: "dest0".into(),
            enabled: true,
            kind: DestinationKind::Folder {
                root: inbox.config.data_dir.parent().unwrap().join("export0"),
                formats: vec![FolderFormat::OriginalJson],
            },
        }];
        inbox.config.max_pending = 1;
        let mut store = inbox.open();
        assert_eq!(
            store.accept(event("one", "meeting", false), 20).unwrap(),
            Acceptance::Duplicate
        );
        assert_eq!(delivery_count(&store), 0);
        let unchanged = store.list(None, 10, 20).unwrap().remove(0);
        assert_eq!(unchanged.receipt_id, receipt.receipt_id);
        assert_eq!(unchanged.expires_at, 110);
        assert_eq!(
            store.accept(event("one", "changed", false), 20).unwrap(),
            Acceptance::Conflict
        );
    }

    #[test]
    fn all_destinations_and_event_are_atomic_on_admission_full() {
        let mut inbox = Inbox::new(2);
        inbox.config.max_pending = 1;
        let mut store = inbox.open();
        assert_eq!(
            store.accept(event("one", "meeting", false), 10).unwrap(),
            Acceptance::Full
        );
        assert!(store.list(None, 10, 10).unwrap().is_empty());
        assert_eq!(delivery_count(&store), 0);
    }

    #[test]
    fn test_events_never_dispatch_or_backfill() {
        let inbox = Inbox::new(2);
        let mut store = inbox.open();
        assert_eq!(
            store.accept(event("test", "meeting", true), 10).unwrap(),
            Acceptance::Accepted
        );
        assert_eq!(delivery_count(&store), 0);
        assert!(store.claim(&[], 11).unwrap().is_none());
        let plan = store.preview("dest0", 0, 20, true, 11).unwrap();
        assert!(plan.events.is_empty());
        assert_eq!(store.approve(&plan, 12).unwrap(), 0);
    }

    #[test]
    fn raw_and_results_expire_without_sweep_and_late_finish_cannot_restore() {
        let inbox = Inbox::new(1);
        let mut store = inbox.open();
        store.accept(event("one", "meeting", false), 10).unwrap();
        let job = store.claim(&[], 11).unwrap().unwrap();
        assert!(store.raw("one", 109).is_ok());
        assert!(store.raw("one", 110).is_err());
        finish(&mut store, &job, OutcomeKind::Completed, 110);
        assert!(store.result(&job.delivery_id, 110).is_err());
        let result: Option<Vec<u8>> = store
            .connection
            .query_row("SELECT result FROM deliveries", [], |r| r.get(0))
            .unwrap();
        assert!(result.is_none());
        store.expire(110).unwrap();
        assert_eq!(
            store.accept(event("one", "meeting", false), 120).unwrap(),
            Acceptance::Duplicate
        );
        store.expire(410).unwrap();
        assert!(store.list(None, 10, 410).unwrap().is_empty());
        assert_eq!(
            store.accept(event("one", "meeting", false), 411).unwrap(),
            Acceptance::Accepted
        );
    }

    #[test]
    fn read_only_coexists_with_owner_and_rejects_every_mutation() {
        let inbox = Inbox::new(1);
        let mut store = inbox.open();
        store.accept(event("one", "meeting", false), 10).unwrap();
        let mut reader = Store::open(&inbox.config, Mode::ReadOnly).unwrap();
        assert!(reader.raw("one", 11).is_ok());
        assert!(reader.status(11).is_ok());
        assert!(Store::open(&inbox.config, Mode::Admin).is_err());
        assert!(reader.accept(event("two", "meeting", false), 11).is_err());
        assert!(reader.claim(&[], 11).is_err());
        assert!(reader.expire(11).is_err());
        assert!(reader.delete("one", 11).is_err());
        assert!(reader.preview("dest0", 0, 20, false, 11).is_err());
        assert!(reader.retry("missing", true, 11).is_err());
    }

    #[test]
    fn config_snapshot_is_authoritative_and_changed_revision_cannot_redirect() {
        let mut inbox = Inbox::new(1);
        let mut store = inbox.open();
        store.accept(event("one", "meeting", false), 10).unwrap();
        drop(store);
        if let DestinationKind::Folder { formats, .. } = &mut inbox.config.destinations[0].kind {
            formats.push(FolderFormat::Markdown);
        }
        assert!(Store::open(&inbox.config, Mode::Serve).is_err());
        let admin = Store::open(&inbox.config, Mode::Admin).unwrap();
        assert_eq!(
            admin.config.destinations[0].revision().unwrap(),
            Destination {
                id: "dest0".into(),
                enabled: true,
                kind: DestinationKind::Folder {
                    root: inbox.config.data_dir.parent().unwrap().join("export0"),
                    formats: vec![FolderFormat::OriginalJson]
                }
            }
            .revision()
            .unwrap()
        );
        drop(admin);
        let mut store = inbox.open();
        assert!(store.claim(&[], 11).unwrap().is_none());
        assert_eq!(store.status(11).unwrap()["blocked_revision"], 1);
        store.accept(event("two", "meeting", false), 12).unwrap();
        assert_eq!(store.claim(&[], 13).unwrap().unwrap().event_id, "two");
    }

    #[test]
    fn restart_marks_inflight_needs_review_without_automatic_retry() {
        let inbox = Inbox::new(1);
        let mut store = inbox.open();
        store.accept(event("one", "meeting", false), 10).unwrap();
        let job = store.claim(&[], 11).unwrap().unwrap();
        drop(store);
        let mut store = Store::open(&inbox.config, Mode::Admin).unwrap();
        assert!(store.claim(&[], 12).unwrap().is_none());
        assert!(store.retry(&job.delivery_id, false, 12).is_err());
        store.retry(&job.delivery_id, true, 12).unwrap();
        let repeated = store.claim(&[], 13).unwrap().unwrap();
        assert_ne!(job.delivery_id, repeated.delivery_id);
        assert_eq!(job.receipt_id, repeated.receipt_id);
        assert_eq!(repeated.expires_at, job.expires_at);
    }

    #[test]
    fn one_inflight_per_destination_but_independent_destinations_progress() {
        let inbox = Inbox::new(2);
        let mut store = inbox.open();
        store.accept(event("one", "meeting", false), 10).unwrap();
        store.accept(event("two", "meeting", false), 11).unwrap();
        let first = store.claim(&[], 12).unwrap().unwrap();
        let second = store.claim(&[], 12).unwrap().unwrap();
        assert_ne!(first.destination.id, second.destination.id);
        assert!(store.claim(&[], 12).unwrap().is_none());
        finish(&mut store, &first, OutcomeKind::Completed, 13);
        assert_eq!(
            store.claim(&[], 14).unwrap().unwrap().destination.id,
            first.destination.id
        );
    }

    #[test]
    fn proven_nonadmission_retries_backoff_and_stop_at_ten() {
        let mut inbox = Inbox::new(1);
        inbox.config.retention_seconds = 100000;
        let mut store = inbox.open();
        store.accept(event("one", "meeting", false), 10).unwrap();
        let mut at = 11;
        for attempt in 1..=10 {
            let job = store.claim(&[], at).unwrap().unwrap();
            finish(&mut store, &job, OutcomeKind::NotAdmitted, at);
            assert!(store.claim(&[], at).unwrap().is_none());
            if attempt < 10 {
                at += (1i64 << attempt.min(9)) * 5;
            }
        }
        assert!(store.claim(&[], at + 10000).unwrap().is_none());
        let state: String = store
            .connection
            .query_row("SELECT state FROM deliveries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "needs_review");
    }

    #[test]
    fn preview_freezes_exact_half_open_receipt_selection_and_rejects_edits() {
        let mut inbox = Inbox::new(0);
        let mut store = inbox.open();
        store.accept(event("outside", "other", false), 9).unwrap();
        store.accept(event("old", "meeting", false), 10).unwrap();
        store.accept(event("latest", "meeting", false), 11).unwrap();
        store.accept(event("end", "other", false), 20).unwrap();
        drop(store);
        inbox.config.destinations = vec![Destination {
            id: "archive".into(),
            enabled: true,
            kind: DestinationKind::Folder {
                root: inbox.config.data_dir.parent().unwrap().join("archive"),
                formats: vec![FolderFormat::OriginalJson],
            },
        }];
        let mut store = inbox.open();
        let plan = store.preview("archive", 10, 20, false, 21).unwrap();
        assert_eq!(plan.events.len(), 1);
        assert_eq!(plan.events[0].event_id, "latest");
        assert_eq!(plan.approve_before, 111);
        let mut forged = plan.clone();
        forged.events.clear();
        assert!(store.approve(&forged, 22).is_err());
        store
            .accept(event("arrived_after_preview", "meeting", false), 22)
            .unwrap();
        assert_eq!(store.approve(&plan, 23).unwrap(), 1);
        assert_eq!(store.approve(&plan, 24).unwrap(), 0);
        assert_eq!(delivery_count(&store), 2);
    }

    #[test]
    fn deleted_latest_revision_does_not_implicitly_select_older_body() {
        let inbox = Inbox::new(1);
        let mut store = inbox.open();
        store.accept(event("old", "meeting", false), 10).unwrap();
        store.accept(event("new", "meeting", false), 11).unwrap();
        store.delete("new", 12).unwrap();
        let plan = store.preview("dest0", 0, 20, false, 13).unwrap();
        assert!(plan.events.is_empty());
        assert_eq!(plan.unavailable, 1);
        assert!(store.raw("new", 13).is_err());
        assert_eq!(
            store.accept(event("new", "meeting", false), 14).unwrap(),
            Acceptance::Duplicate
        );
    }

    #[test]
    fn approval_is_atomic_if_any_frozen_body_becomes_unavailable() {
        let mut inbox = Inbox::new(0);
        let mut store = inbox.open();
        store.accept(event("one", "one", false), 10).unwrap();
        store.accept(event("two", "two", false), 11).unwrap();
        drop(store);
        inbox.config.destinations = vec![Destination {
            id: "archive".into(),
            enabled: true,
            kind: DestinationKind::Folder {
                root: inbox.config.data_dir.parent().unwrap().join("archive"),
                formats: vec![FolderFormat::OriginalJson],
            },
        }];
        let mut store = inbox.open();
        let plan = store.preview("archive", 0, 20, true, 12).unwrap();
        store.delete("two", 13).unwrap();
        assert!(store.approve(&plan, 14).is_err());
        assert_eq!(delivery_count(&store), 0);
    }

    #[test]
    fn backup_is_private_consistent_and_never_overwrites() {
        let inbox = Inbox::new(0);
        let mut store = inbox.open();
        store.accept(event("one", "meeting", false), 10).unwrap();
        let backup = inbox.config.data_dir.join("backup.sqlite3");
        store.backup(&backup).unwrap();
        assert!(store.backup(&backup).is_err());
        let copied =
            Connection::open_with_flags(&backup, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            copied
                .query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            std::fs::metadata(backup).unwrap().permissions().mode() & 0o077,
            0
        );
    }

    #[test]
    fn legacy_import_preserves_exact_payload_and_deadline_without_outbound_jobs() {
        let inbox = Inbox::new(1);
        let mut store = inbox.open();
        let body = include_bytes!("../fixtures/meeting.completed.json").to_vec();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let event_id = parsed["id"].as_str().unwrap().to_string();
        let row = LegacyRecord {
            format_version: 1,
            event_id: event_id.clone(),
            received_at: 10,
            expires_at: 110,
            state: "done".into(),
            digest: hex::encode(Sha256::digest(&body)),
            body_base64: Some(base64::engine::general_purpose::STANDARD.encode(&body)),
            result_base64: Some(
                base64::engine::general_purpose::STANDARD.encode(b"historical result"),
            ),
            test: false,
        };
        store.import_legacy(row, 20).unwrap();
        assert_eq!(store.raw(&event_id, 20).unwrap(), body);
        assert_eq!(
            store.legacy_result(&event_id, 20).unwrap().as_slice(),
            b"historical result"
        );
        assert_eq!(
            store.status(20).unwrap()["legacy_history"][0]["original_state"],
            "done"
        );
        assert_eq!(
            store.status(20).unwrap()["legacy_history"][0]["result_available"],
            true
        );
        let record = store.list(None, 10, 20).unwrap().remove(0);
        assert_eq!(record.disposition, "held");
        assert_eq!(record.received_at, 10);
        assert_eq!(record.expires_at, 110);
        assert_eq!(delivery_count(&store), 0);
        assert!(store.claim(&[], 21).unwrap().is_none());
        let plan = store.preview("dest0", 10, 11, false, 21).unwrap();
        assert_eq!(plan.events.len(), 1);
        assert_eq!(plan.approve_before, 110);
        assert_eq!(store.approve(&plan, 22).unwrap(), 1);
    }

    #[test]
    fn historical_results_survive_restart_but_not_expiry_deletion_or_reimport() {
        let inbox = Inbox::new(1);
        let mut store = inbox.open();
        let result = vec![0xff; MAX_RESULT];
        let row = |event_id: &str| LegacyRecord {
            format_version: 1,
            event_id: event_id.into(),
            received_at: 10,
            expires_at: 110,
            state: "done".into(),
            digest: "a".repeat(64),
            body_base64: None,
            result_base64: Some(base64::engine::general_purpose::STANDARD.encode(&result)),
            test: false,
        };
        let baseline = Store::usage(&store.connection).unwrap().0;
        store.import_legacy(row("retained"), 20).unwrap();
        assert_eq!(
            Store::usage(&store.connection).unwrap().0 - baseline,
            2 * RECORD_CHARGE + MAX_RESULT as u64
        );
        store.import_legacy(row("deleted"), 20).unwrap();
        assert_eq!(delivery_count(&store), 0);
        drop(store);
        let mut store = Store::open(&inbox.config, Mode::Admin).unwrap();
        let reader = Store::open(&inbox.config, Mode::ReadOnly).unwrap();
        assert_eq!(reader.legacy_result("retained", 109).unwrap(), result);
        assert!(reader.legacy_result("retained", 110).is_err());
        drop(reader);
        assert!(store.raw("retained", 21).is_err());
        let mut conflicting = row("retained");
        conflicting.result_base64 =
            Some(base64::engine::general_purpose::STANDARD.encode(b"different"));
        assert!(store.import_legacy(conflicting, 21).is_err());
        assert_eq!(store.legacy_result("retained", 21).unwrap(), result);
        store.delete("deleted", 21).unwrap();
        assert!(store.legacy_result("deleted", 21).is_err());
        store.import_legacy(row("deleted"), 22).unwrap();
        assert!(store.legacy_result("deleted", 22).is_err());
        assert_usage(&store);
        assert!(store.legacy_result("retained", 110).is_err());
        store.expire(110).unwrap();
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT count(*) FROM legacy_history WHERE result IS NOT NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        store.import_legacy(row("retained"), 111).unwrap();
        assert!(store.legacy_result("retained", 111).is_err());
        let status = store.status(111).unwrap();
        assert!(status["legacy_history"].as_array().unwrap().iter().all(
            |record| record["original_state"] == "done" && record["result_available"] == false
        ));
        assert_eq!(delivery_count(&store), 0);
        assert_usage(&store);
        store.expire(410).unwrap();
        assert_eq!(
            store
                .connection
                .query_row("SELECT count(*) FROM legacy_history", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(Store::usage(&store.connection).unwrap().0, baseline);
        assert_usage(&store);
    }

    #[test]
    fn legacy_missing_body_and_previously_test_receipts_never_gain_jobs() {
        let inbox = Inbox::new(1);
        let mut store = inbox.open();
        for (event_id, state, test) in [
            ("missing", "held", false),
            ("old_test", "test", false),
            ("deleted", "deleted", false),
        ] {
            store
                .import_legacy(
                    LegacyRecord {
                        format_version: 1,
                        event_id: event_id.into(),
                        received_at: 10,
                        expires_at: 110,
                        state: state.into(),
                        digest: "a".repeat(64),
                        body_base64: None,
                        result_base64: None,
                        test,
                    },
                    20,
                )
                .unwrap();
        }
        assert_eq!(delivery_count(&store), 0);
        let plan = store.preview("dest0", 0, 20, true, 21).unwrap();
        assert!(plan.events.is_empty());
        assert_eq!(plan.unavailable, 2);
        assert_eq!(store.approve(&plan, 22).unwrap(), 0);
        assert!(store.raw("missing", 22).is_err());
        assert!(
            store
                .list(None, 10, 22)
                .unwrap()
                .iter()
                .find(|e| e.event_id == "old_test")
                .unwrap()
                .test
        );
    }

    #[test]
    fn result_reservation_and_record_charge_are_retained_until_original_deadline() {
        let inbox = Inbox::new(1);
        let mut store = inbox.open();
        let before = Store::usage(&store.connection).unwrap().0;
        let body_size = event("one", "meeting", false).body.len() as u64;
        let destination_charge = Store::delivery_charge(&inbox.config.destinations[0]).unwrap();
        store.accept(event("one", "meeting", false), 10).unwrap();
        let admitted = Store::usage(&store.connection).unwrap().0;
        assert_eq!(
            admitted - before,
            RECORD_CHARGE + body_size + destination_charge
        );
        let job = store.claim(&[], 11).unwrap().unwrap();
        finish(&mut store, &job, OutcomeKind::Completed, 12);
        assert_eq!(Store::usage(&store.connection).unwrap().0, admitted);
        store.expire(110).unwrap();
        assert_eq!(
            Store::usage(&store.connection).unwrap().0,
            admitted - body_size - MAX_RESULT as u64
        );
    }

    #[test]
    fn counters_follow_retries_deletion_cascade_and_plan_garbage_collection() {
        let inbox = Inbox::new(1);
        let mut store = inbox.open();
        assert_usage(&store);
        store.accept(event("one", "meeting", false), 10).unwrap();
        store.accept(event("two", "other", false), 10).unwrap();
        assert_usage(&store);
        let job = store.claim(&[], 11).unwrap().unwrap();
        assert_usage(&store);
        finish(&mut store, &job, OutcomeKind::NeedsReview, 12);
        store.retry(&job.delivery_id, true, 13).unwrap();
        assert_usage(&store);
        store.delete("one", 14).unwrap();
        assert_usage(&store);
        store.expire(110).unwrap();
        assert_usage(&store);
        store.expire(410).unwrap();
        assert_usage(&store);
        assert_eq!(delivery_count(&store), 0);
        let plan = store.preview("dest0", 0, 20, false, 411).unwrap();
        assert_usage(&store);
        store.approve(&plan, 412).unwrap();
        assert_usage(&store);
        store.expire(plan.approve_before).unwrap();
        assert_usage(&store);
    }

    #[test]
    fn round_robin_serves_every_destination_under_continuous_early_backlog() {
        let inbox = Inbox::new(8);
        let mut store = inbox.open();
        for number in 0..3 {
            store
                .accept(event(&format!("event{number}"), "meeting", false), 10)
                .unwrap();
        }
        let mut seen = std::collections::HashSet::new();
        for _ in 0..8 {
            let job = store.claim(&[], 11).unwrap().unwrap();
            seen.insert(job.destination.id.clone());
            finish(&mut store, &job, OutcomeKind::Completed, 12);
        }
        assert_eq!(seen.len(), 8);
        assert_usage(&store);
    }

    #[test]
    fn pinned_reader_pauses_new_admission_at_wal_high_water_but_not_claims_or_duplicates() {
        let inbox = Inbox::new(1);
        let mut store = inbox.open();
        store.accept(event("first", "meeting", false), 10).unwrap();
        let reader = Connection::open_with_flags(
            inbox.config.data_dir.join("inbox.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        reader
            .execute_batch("BEGIN; SELECT sequence FROM events;")
            .unwrap();
        let mut paused = false;
        for number in 0..100 {
            let mut incoming = event(&format!("large{number}"), "meeting", false);
            incoming.body.resize(1024 * 1024, b' ');
            incoming.digest = hex::encode(Sha256::digest(&incoming.body));
            if store.accept(incoming, 10).unwrap() == Acceptance::Full {
                paused = true;
                break;
            }
        }
        assert!(paused);
        assert!(
            std::fs::metadata(inbox.config.data_dir.join("inbox.sqlite3-wal"))
                .unwrap()
                .len()
                >= WAL_HIGH_WATER
        );
        assert_eq!(
            store.accept(event("first", "meeting", false), 11).unwrap(),
            Acceptance::Duplicate
        );
        assert!(store.claim(&[], 11).unwrap().is_some());
        reader.execute_batch("COMMIT").unwrap();
        store.expire(20).unwrap();
        assert_eq!(
            store
                .accept(event("resumed", "meeting", false), 20)
                .unwrap(),
            Acceptance::Accepted
        );
        assert_usage(&store);
    }

    #[test]
    fn expiry_removes_content_without_erasing_observed_external_admission() {
        let inbox = Inbox::new(1);
        let mut store = inbox.open();
        store
            .accept(event("external", "meeting", false), 10)
            .unwrap();
        let job = store.claim(&[], 11).unwrap().unwrap();
        store
            .finish(
                &job.delivery_id,
                &job.attempt_id,
                DeliveryOutcome {
                    kind: OutcomeKind::HandedOff,
                    code: "runs_admitted".into(),
                    result: None,
                    external_id: Some("synthetic_run".into()),
                },
                12,
            )
            .unwrap();
        store.expire(job.expires_at).unwrap();
        let status = store.status(job.expires_at).unwrap();
        let delivery = &status["deliveries"][0];
        assert_eq!(delivery["state"], "expired");
        assert_eq!(delivery["submission_state"], "admitted");
        assert_eq!(delivery["external_id"], "synthetic_run");
        assert!(store.raw("external", job.expires_at).is_err());
        assert!(
            store
                .finish(
                    &job.delivery_id,
                    "stale-attempt",
                    DeliveryOutcome {
                        kind: OutcomeKind::Completed,
                        code: "late".into(),
                        result: Some(b"must not resurrect".to_vec()),
                        external_id: None,
                    },
                    job.expires_at + 1
                )
                .is_err()
        );
    }

    #[test]
    fn schema_newer_than_binary_and_database_symlinks_fail_closed() {
        let inbox = Inbox::new(0);
        let store = inbox.open();
        store
            .connection
            .pragma_update(None, "user_version", 2)
            .unwrap();
        drop(store);
        assert!(Store::open(&inbox.config, Mode::Admin).is_err());
        let other = Inbox::new(0);
        security::private_directory(&other.config.data_dir).unwrap();
        std::os::unix::fs::symlink(
            inbox.config.data_dir.join("inbox.sqlite3"),
            other.config.data_dir.join("inbox.sqlite3"),
        )
        .unwrap();
        assert!(Store::open(&other.config, Mode::Apply).is_err());
    }
}
