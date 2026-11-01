use crate::model::{MAX_BODY, VerifiedEvent};
use anyhow::{Result, ensure};
use axum::http::HeaderMap;
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use subtle::ConstantTimeEq;

pub struct Secrets {
    key: Vec<u8>,
    bearer: Option<Vec<u8>>,
}
impl Secrets {
    pub fn load() -> Result<Self> {
        let secret = std::env::var("AURORA_WEBHOOK_SECRET")
            .map_err(|_| anyhow::anyhow!("AURORA_WEBHOOK_SECRET is required"))?;
        let key = STANDARD
            .decode(secret.strip_prefix("whsec_").unwrap_or(&secret))
            .map_err(|_| anyhow::anyhow!("invalid webhook signing key"))?;
        ensure!(key.len() == 32, "webhook signing key must encode 32 bytes");
        let bearer = std::env::var("AURORA_BEARER_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());
        if let Some(v) = &bearer {
            ensure!(
                (32..=4096).contains(&v.len()) && !v.bytes().any(|b| b.is_ascii_whitespace()),
                "invalid bearer token"
            );
        }
        Ok(Self {
            key,
            bearer: bearer.map(|s| format!("Bearer {s}").into_bytes()),
        })
    }
}
fn unique<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() || value.is_empty() {
        None
    } else {
        Some(value)
    }
}
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && id.bytes().all(|b| (33..=126).contains(&b))
}
pub fn verify<'a>(
    headers: &'a HeaderMap,
    body: &[u8],
    secrets: &Secrets,
    now: i64,
) -> Result<&'a str> {
    let id = unique(headers, "webhook-id").ok_or_else(|| anyhow::anyhow!("unauthorized"))?;
    ensure!(valid_id(id), "unauthorized");
    let timestamp =
        unique(headers, "webhook-timestamp").ok_or_else(|| anyhow::anyhow!("unauthorized"))?;
    ensure!(
        timestamp.len() <= 20 && timestamp.bytes().all(|b| b.is_ascii_digit()),
        "unauthorized"
    );
    let seconds = timestamp
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("unauthorized"))?;
    ensure!(seconds >= 0 && seconds.abs_diff(now) <= 300, "unauthorized");
    let signatures =
        unique(headers, "webhook-signature").ok_or_else(|| anyhow::anyhow!("unauthorized"))?;
    ensure!(signatures.len() <= 8192, "unauthorized");
    if let Some(expected) = &secrets.bearer {
        let actual =
            unique(headers, "authorization").ok_or_else(|| anyhow::anyhow!("unauthorized"))?;
        let a = Sha256::digest(actual.as_bytes());
        let b = Sha256::digest(expected);
        ensure!(bool::from(a.ct_eq(&b)), "unauthorized");
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(&secrets.key)
        .map_err(|_| anyhow::anyhow!("unauthorized"))?;
    mac.update(id.as_bytes());
    mac.update(b".");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    let mut matched = 0u8;
    let mut count = 0;
    for entry in signatures.split_ascii_whitespace() {
        count += 1;
        ensure!(count <= 8, "unauthorized");
        let mut candidate = [0u8; 33];
        if let Some(encoded) = entry.strip_prefix("v1,")
            && let Ok(length) = STANDARD.decode_slice(encoded, &mut candidate)
            && length == 32
        {
            matched |= candidate[..32].ct_eq(expected.as_slice()).unwrap_u8();
        }
    }
    ensure!(matched == 1, "unauthorized");
    Ok(id)
}

#[derive(Deserialize)]
#[serde(transparent)]
struct Nullable<T>(Option<T>);
fn required_nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Nullable<T>, D::Error> {
    Option::<T>::deserialize(d).map(Nullable)
}
#[derive(Default)]
enum Field<T> {
    #[default]
    Absent,
    Null,
    Value(T),
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Field<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(d)? {
            Some(v) => Self::Value(v),
            None => Self::Null,
        })
    }
}
#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(borrow)]
    id: Cow<'a, str>,
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
    #[serde(borrow)]
    schema_version: Cow<'a, str>,
    #[serde(borrow)]
    timestamp: Cow<'a, str>,
    #[serde(default)]
    test: Field<bool>,
    #[serde(default, borrow)]
    related_event_id: Field<Cow<'a, str>>,
    #[serde(borrow)]
    data: Data<'a>,
}
#[derive(Deserialize)]
struct Data<'a> {
    #[serde(borrow)]
    content_scope: Cow<'a, str>,
    #[serde(borrow)]
    meeting: Meeting<'a>,
    #[serde(borrow)]
    speakers: Vec<Speaker<'a>>,
    #[serde(borrow)]
    source: Source<'a>,
    #[serde(default, borrow)]
    transcript: Field<Transcript<'a>>,
    #[serde(default, borrow)]
    summary: Field<Summary<'a>>,
    #[serde(default, borrow)]
    action_items: Field<Vec<Action<'a>>>,
}
#[derive(Deserialize)]
struct Meeting<'a> {
    #[serde(borrow)]
    id: Cow<'a, str>,
    #[serde(borrow)]
    title: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "required_nullable")]
    started_at: Nullable<Cow<'a, str>>,
    #[serde(borrow, deserialize_with = "required_nullable")]
    ended_at: Nullable<Cow<'a, str>>,
    #[serde(deserialize_with = "required_nullable")]
    duration_seconds: Nullable<f64>,
    #[serde(borrow)]
    languages: Vec<Cow<'a, str>>,
}
#[derive(Deserialize)]
struct Speaker<'a> {
    #[serde(borrow)]
    id: Cow<'a, str>,
    #[serde(borrow)]
    label: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "required_nullable")]
    name: Nullable<Cow<'a, str>>,
}
#[derive(Deserialize)]
struct Source<'a> {
    #[serde(borrow)]
    app: Cow<'a, str>,
    #[serde(borrow)]
    app_version: Cow<'a, str>,
    #[serde(borrow)]
    platform: Cow<'a, str>,
}
#[derive(Deserialize)]
struct Transcript<'a> {
    #[serde(borrow)]
    text: Cow<'a, str>,
    #[serde(borrow)]
    turns: Vec<Turn<'a>>,
}
#[derive(Deserialize)]
struct Turn<'a> {
    #[serde(borrow, deserialize_with = "required_nullable")]
    speaker_id: Nullable<Cow<'a, str>>,
    start_seconds: f64,
    end_seconds: f64,
    #[serde(borrow)]
    text: Cow<'a, str>,
}
#[derive(Deserialize)]
struct Summary<'a> {
    #[serde(borrow)]
    text: Cow<'a, str>,
}
#[derive(Deserialize)]
struct Action<'a> {
    #[serde(borrow)]
    text: Cow<'a, str>,
    completed: bool,
}
fn timestamp(value: &str) -> bool {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).is_ok()
}
fn number(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}
// A lexical ceiling is applied before deserialization, then serde validates the grammar.
fn structure_limit(raw: &[u8]) -> Result<()> {
    let (mut depth, mut nodes, mut string, mut escape, mut scalar) =
        (0usize, 0usize, false, false, false);
    for &b in raw {
        if string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                string = false;
            }
            continue;
        }
        match b {
            b'"' => {
                string = true;
                nodes += 1;
                scalar = false;
            }
            b'{' | b'[' => {
                depth += 1;
                nodes += 1;
                scalar = false;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                scalar = false;
            }
            b',' | b':' | b' ' | b'\r' | b'\n' | b'\t' => {
                scalar = false;
            }
            _ => {
                if !scalar {
                    nodes += 1;
                    scalar = true;
                }
            }
        }
        ensure!(
            depth <= 64 && nodes <= 100000,
            "event structure exceeds limits"
        );
    }
    Ok(())
}

pub fn validate_body(body: Vec<u8>, id: &str) -> Result<VerifiedEvent> {
    ensure!(body.len() <= MAX_BODY, "event exceeds body limit");
    structure_limit(&body)?;
    let e: Envelope<'_> =
        serde_json::from_slice(&body).map_err(|_| anyhow::anyhow!("invalid event envelope"))?;
    ensure!(
        e.id == id && valid_id(id) && e.kind == "meeting.completed",
        "invalid event identity/type"
    );
    let parts: Vec<_> = e.schema_version.split('.').collect();
    ensure!(
        (2..=3).contains(&parts.len())
            && parts[0] == "1"
            && parts[1..].iter().all(|p| !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u32>().is_ok()),
        "unsupported event schema"
    );
    ensure!(timestamp(&e.timestamp), "invalid event timestamp");
    let test = match &e.test {
        Field::Absent => false,
        Field::Value(v) => *v,
        Field::Null => anyhow::bail!("invalid test flag"),
    };
    match &e.related_event_id {
        Field::Absent => {}
        Field::Value(v) => ensure!(valid_id(v), "invalid related identity"),
        Field::Null => anyhow::bail!("invalid related identity"),
    }
    let d = &e.data;
    let m = &d.meeting;
    ensure!(
        !m.id.trim().is_empty() && m.id.len() <= 256,
        "invalid meeting identity"
    );
    ensure!(
        m.started_at.0.as_deref().is_none_or(timestamp)
            && m.ended_at.0.as_deref().is_none_or(timestamp)
            && m.duration_seconds.0.is_none_or(number),
        "invalid meeting metadata"
    );
    for s in &d.speakers {
        ensure!(
            !s.id.trim().is_empty() && !s.label.trim().is_empty(),
            "invalid speaker"
        );
        let _ = &s.name.0;
    }
    ensure!(
        d.source.app == "Aurora"
            && !d.source.app_version.trim().is_empty()
            && matches!(d.source.platform.as_ref(), "ios" | "macos"),
        "invalid Aurora source"
    );
    let (transcript, summary) = match d.content_scope.as_ref() {
        "transcript" => (true, false),
        "summary" => (false, true),
        "transcript_and_summary" => (true, true),
        _ => anyhow::bail!("invalid content scope"),
    };
    match (&d.transcript, transcript) {
        (Field::Value(t), true) => {
            for turn in &t.turns {
                ensure!(
                    number(turn.start_seconds) && number(turn.end_seconds),
                    "invalid transcript offset"
                );
                let _ = (&turn.speaker_id.0, &turn.text);
            }
            let _ = &t.text;
        }
        (Field::Absent, false) => {}
        _ => anyhow::bail!("invalid transcript scope"),
    }
    match (&d.summary, summary) {
        (Field::Value(s), true) => {
            let _ = &s.text;
        }
        (Field::Absent, false) => {}
        _ => anyhow::bail!("invalid summary scope"),
    }
    match (&d.action_items, summary) {
        (Field::Value(items), true) => {
            for item in items {
                let _ = (&item.text, item.completed);
            }
        }
        (Field::Absent, false) => {}
        _ => anyhow::bail!("invalid action-item scope"),
    }
    let _ = (&m.title, &m.languages);
    let event_id = e.id.to_string();
    let meeting_id = m.id.to_string();
    let digest = hex::encode(Sha256::digest(&body));
    drop(e);
    Ok(VerifiedEvent {
        event_id,
        meeting_id,
        test,
        body,
        digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIXTURE: &[u8] = include_bytes!("../fixtures/meeting.completed.json");
    #[test]
    fn signed_bytes_alternatives_and_freshness() {
        let secrets = Secrets {
            key: vec![42; 32],
            bearer: None,
        };
        let id = "evt_synthetic";
        let mut h = HeaderMap::new();
        h.insert("webhook-id", id.parse().unwrap());
        h.insert("webhook-timestamp", "1000".parse().unwrap());
        let mut mac = Hmac::<Sha256>::new_from_slice(&secrets.key).unwrap();
        mac.update(b"evt_synthetic.1000.");
        mac.update(FIXTURE);
        let sig = STANDARD.encode(mac.finalize().into_bytes());
        h.insert(
            "webhook-signature",
            format!("v1,invalid v1,{sig}").parse().unwrap(),
        );
        assert!(verify(&h, FIXTURE, &secrets, 1300).is_ok());
        assert!(verify(&h, FIXTURE, &secrets, 1301).is_err());
        assert!(verify(&h, b"changed", &secrets, 1000).is_err());
        h.append("webhook-id", "duplicate".parse().unwrap());
        assert!(verify(&h, FIXTURE, &secrets, 1000).is_err());
    }
    #[test]
    fn rejects_null_excluded_fields_and_duplicate_identity() {
        let original: serde_json::Value = serde_json::from_slice(FIXTURE).unwrap();
        let id = original["id"].as_str().unwrap();
        assert!(validate_body(FIXTURE.to_vec(), id).is_ok());
        let mut v = original.clone();
        v["data"]["content_scope"] = "transcript".into();
        v["data"].as_object_mut().unwrap().remove("summary");
        v["data"].as_object_mut().unwrap().remove("action_items");
        assert!(validate_body(serde_json::to_vec(&v).unwrap(), id).is_ok());
        v["data"]["summary"] = serde_json::Value::Null;
        assert!(validate_body(serde_json::to_vec(&v).unwrap(), id).is_err());
        let duplicate = format!(
            "{{\"id\":\"other\",{}",
            String::from_utf8_lossy(&FIXTURE[1..])
        );
        assert!(validate_body(duplicate.into_bytes(), id).is_err());
        let mut v = original.clone();
        v["data"]["meeting"]
            .as_object_mut()
            .unwrap()
            .remove("started_at");
        assert!(validate_body(serde_json::to_vec(&v).unwrap(), id).is_err());
    }
    #[test]
    fn escaped_meeting_text_preserves_raw_evidence() {
        let mut event: serde_json::Value = serde_json::from_slice(FIXTURE).unwrap();
        event["data"]["meeting"]["title"] = "A \"quoted\" meeting\nwith a second line".into();
        event["data"]["transcript"]["text"] = "first\nsecond\tthird\\literal".into();
        event["data"]["speakers"][0]["name"] = "Renée \"R\"".into();
        event["data"]["summary"]["text"] = "<script>not executable</script>\n```".into();
        event["additive_future_field"] = serde_json::json!({"value":true});
        let raw = serde_json::to_vec(&event).unwrap();
        let parsed = validate_body(raw.clone(), event["id"].as_str().unwrap()).unwrap();
        assert_eq!(parsed.body, raw);
    }
}
