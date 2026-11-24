CREATE TABLE enterprise_sessions (
    org_id TEXT NOT NULL,
    receiver_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    latest_revision INTEGER NOT NULL CHECK (latest_revision BETWEEN 1 AND 2147483647),
    PRIMARY KEY (org_id, receiver_id, session_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE enterprise_receipts (
    event_id TEXT PRIMARY KEY NOT NULL,
    org_id TEXT NOT NULL,
    receiver_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision BETWEEN 1 AND 2147483647),
    manifest_sha256 TEXT NOT NULL CHECK (length(manifest_sha256) = 64),
    manifest BLOB,
    outcome TEXT CHECK (outcome IN ('won', 'not_won', 'unknown', 'skipped')),
    storage_id TEXT UNIQUE CHECK (length(storage_id) = 32),
    artifact_id TEXT,
    audio_sha256 TEXT CHECK (length(audio_sha256) = 64),
    audio_length INTEGER NOT NULL CHECK (audio_length >= 0),
    received_bytes INTEGER NOT NULL DEFAULT 0 CHECK (received_bytes BETWEEN 0 AND audio_length),
    state TEXT NOT NULL CHECK (state IN ('uploading', 'complete', 'expired')),
    admitted_at INTEGER NOT NULL,
    completed_at INTEGER,
    expires_at INTEGER NOT NULL,
    purge_at INTEGER NOT NULL CHECK (purge_at > expires_at),
    completed_ttl INTEGER NOT NULL CHECK (completed_ttl > 0),
    payload_bytes INTEGER NOT NULL CHECK (payload_bytes >= 0),
    FOREIGN KEY (org_id, receiver_id, session_id)
        REFERENCES enterprise_sessions (org_id, receiver_id, session_id),
    UNIQUE (org_id, receiver_id, session_id, revision),
    CHECK ((state = 'expired') = (manifest IS NULL)),
    CHECK (state != 'expired' OR outcome IS NULL),
    CHECK (state != 'uploading' OR (audio_length > 0 AND completed_at IS NULL)),
    CHECK (state != 'complete' OR (received_bytes = audio_length AND completed_at IS NOT NULL)),
    CHECK (state = 'expired' OR payload_bytes = length(manifest) + audio_length),
    CHECK (payload_bytes >= audio_length),
    CHECK (
        (audio_length = 0 AND storage_id IS NULL AND artifact_id IS NULL AND audio_sha256 IS NULL)
        OR (audio_length > 0 AND storage_id IS NOT NULL AND artifact_id IS NOT NULL AND audio_sha256 IS NOT NULL)
    )
) STRICT;

CREATE INDEX enterprise_live_expiry ON enterprise_receipts (expires_at) WHERE state != 'expired';
CREATE INDEX enterprise_pending_cleanup ON enterprise_receipts (event_id) WHERE state = 'expired' AND payload_bytes > 0;
CREATE INDEX enterprise_tombstone_expiry ON enterprise_receipts (purge_at) WHERE state = 'expired' AND payload_bytes = 0;
CREATE UNIQUE INDEX enterprise_one_upload ON enterprise_receipts (org_id, receiver_id, session_id) WHERE state = 'uploading';

-- A proof may first be used before its event exists, and its independent expiry
-- must survive deletion of any associated event. No user identifiers are stored here.
CREATE TABLE enterprise_replay (
    jti TEXT PRIMARY KEY NOT NULL,
    binding BLOB NOT NULL CHECK (length(binding) = 32),
    expires_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
CREATE INDEX enterprise_replay_expiry ON enterprise_replay (expires_at);

CREATE TABLE enterprise_usage (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    payload_bytes INTEGER NOT NULL CHECK (payload_bytes >= 0),
    unwritten_bytes INTEGER NOT NULL CHECK (unwritten_bytes BETWEEN 0 AND payload_bytes),
    records INTEGER NOT NULL CHECK (records >= 0),
    replay_records INTEGER NOT NULL CHECK (replay_records >= 0)
) STRICT;
INSERT INTO enterprise_usage VALUES (1, 0, 0, 0, 0);

CREATE TRIGGER enterprise_receipt_insert AFTER INSERT ON enterprise_receipts BEGIN
    UPDATE enterprise_usage SET
        payload_bytes = payload_bytes + NEW.payload_bytes,
        unwritten_bytes = unwritten_bytes + NEW.audio_length - NEW.received_bytes,
        records = records + 1 WHERE singleton = 1;
END;
CREATE TRIGGER enterprise_receipt_update
AFTER UPDATE OF payload_bytes, audio_length, received_bytes ON enterprise_receipts BEGIN
    UPDATE enterprise_usage SET
        payload_bytes = payload_bytes - OLD.payload_bytes + NEW.payload_bytes,
        unwritten_bytes = unwritten_bytes - (OLD.audio_length - OLD.received_bytes)
            + (NEW.audio_length - NEW.received_bytes) WHERE singleton = 1;
END;
CREATE TRIGGER enterprise_receipt_delete AFTER DELETE ON enterprise_receipts BEGIN
    UPDATE enterprise_usage SET
        payload_bytes = payload_bytes - OLD.payload_bytes,
        unwritten_bytes = unwritten_bytes - (OLD.audio_length - OLD.received_bytes),
        records = records - 1 WHERE singleton = 1;
    DELETE FROM enterprise_sessions
        WHERE (org_id, receiver_id, session_id) = (OLD.org_id, OLD.receiver_id, OLD.session_id)
        AND NOT EXISTS (
            SELECT 1 FROM enterprise_receipts
            WHERE (org_id, receiver_id, session_id) = (OLD.org_id, OLD.receiver_id, OLD.session_id)
        );
END;
CREATE TRIGGER enterprise_replay_insert AFTER INSERT ON enterprise_replay BEGIN
    UPDATE enterprise_usage SET replay_records = replay_records + 1 WHERE singleton = 1;
END;
CREATE TRIGGER enterprise_replay_delete AFTER DELETE ON enterprise_replay BEGIN
    UPDATE enterprise_usage SET replay_records = replay_records - 1 WHERE singleton = 1;
END;
