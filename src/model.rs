use serde::{Deserialize, Serialize};

pub const MAX_BODY: usize = 8 * 1024 * 1024;
pub const MAX_RESULT: usize = 64 * 1024;
pub const MAX_STDERR: usize = 16 * 1024;
pub const RECORD_CHARGE: u64 = 4096;

pub struct VerifiedEvent {
    pub event_id: String,
    pub meeting_id: String,
    pub test: bool,
    pub body: Vec<u8>,
    pub digest: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Acceptance {
    Accepted,
    Duplicate,
    Conflict,
    Full,
}

pub struct DeliveryJob {
    pub delivery_id: String,
    pub attempt_id: String,
    pub receipt_id: String,
    pub event_id: String,
    pub destination: crate::config::Destination,
    pub body: bytes::Bytes,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    Completed,
    HandedOff,
    NotAdmitted,
    NeedsReview,
}

pub struct DeliveryOutcome {
    pub kind: OutcomeKind,
    // Fixed, connector-owned diagnostic codes, never provider stderr or transcript text.
    pub code: String,
    pub result: Option<Vec<u8>>,
    pub external_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub receipt_id: String,
    pub event_id: String,
    pub meeting_id: Option<String>,
    pub received_at: i64,
    pub expires_at: i64,
    pub metadata_expires_at: i64,
    pub test: bool,
    pub disposition: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Selection {
    pub receipt_id: String,
    pub event_id: String,
    pub digest: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackfillPlan {
    pub format_version: u32,
    pub plan_id: String,
    pub receiver_id: String,
    pub config_digest: String,
    pub destination_id: String,
    pub destination_revision: String,
    pub created_at: i64,
    pub approve_before: i64,
    pub start_received_at: i64,
    pub end_received_at: i64,
    pub all_revisions: bool,
    pub events: Vec<Selection>,
    pub unavailable: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRecord {
    pub format_version: u32,
    pub event_id: String,
    pub received_at: i64,
    pub expires_at: i64,
    pub state: String,
    pub digest: String,
    pub body_base64: Option<String>,
    pub result_base64: Option<String>,
    pub test: bool,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
