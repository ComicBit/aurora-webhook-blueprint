use anyhow::{Result, anyhow, ensure};
use axum::http::{HeaderMap, header::AUTHORIZATION};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::{Deserialize, Deserializer, Serialize, de};
use sha2::{Digest, Sha256};
use std::{borrow::Cow, collections::BTreeMap, fmt, marker::PhantomData};

pub(crate) const CHUNK_SIZE: usize = 1 << 20;
pub(crate) const MANIFEST_LIMIT: usize = 1 << 20;
const TOKEN_LIMIT: usize = 8 << 10;
const ISSUER: &str = "https://issuer.example.test/aurora";
const AUDIENCE: &str = "aurora-company-sync-local";
const TOKEN_TYPE: &str = "aurora-upload-local+jwt";
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "Object<EnterpriseConfigFields>")]
pub(crate) struct EnterpriseConfig {
    pub(crate) org_id: String,
    pub(crate) receiver_id: String,
    pub(crate) max_artifact_bytes: u64,
    pub(crate) max_storage_bytes: u64,
    pub(crate) max_records: u32,
    pub(crate) max_concurrent_requests: usize,
    pub(crate) upload_ttl_seconds: i64,
    pub(crate) retention_seconds: i64,
    pub(crate) metadata_retention_seconds: i64,
    pub(crate) public_keys: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnterpriseConfigFields {
    org_id: String,
    receiver_id: String,
    max_artifact_bytes: u64,
    max_storage_bytes: u64,
    max_records: u32,
    max_concurrent_requests: usize,
    upload_ttl_seconds: i64,
    retention_seconds: i64,
    metadata_retention_seconds: i64,
    #[serde(deserialize_with = "deserialize_pins")]
    public_keys: BTreeMap<String, String>,
}

impl From<Object<EnterpriseConfigFields>> for EnterpriseConfig {
    fn from(Object(fields): Object<EnterpriseConfigFields>) -> Self {
        Self {
            org_id: fields.org_id,
            receiver_id: fields.receiver_id,
            max_artifact_bytes: fields.max_artifact_bytes,
            max_storage_bytes: fields.max_storage_bytes,
            max_records: fields.max_records,
            max_concurrent_requests: fields.max_concurrent_requests,
            upload_ttl_seconds: fields.upload_ttl_seconds,
            retention_seconds: fields.retention_seconds,
            metadata_retention_seconds: fields.metadata_retention_seconds,
            public_keys: fields.public_keys,
        }
    }
}

impl EnterpriseConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        self.pins()?.try_for_each(|pin| pin.map(|_| ()))
    }

    fn pins(&self) -> Result<impl Iterator<Item = Result<(&str, [u8; 32])>> + '_> {
        ensure!(
            valid_name(&self.org_id) && valid_name(&self.receiver_id),
            "invalid Enterprise org or receiver ID"
        );
        ensure!(
            (1..=(1 << 40)).contains(&self.max_storage_bytes)
                && (1..=self.max_storage_bytes).contains(&self.max_artifact_bytes),
            "invalid Enterprise storage or artifact limit"
        );
        ensure!(
            (1..=1_000_000).contains(&self.max_records)
                && (1..=64).contains(&self.max_concurrent_requests),
            "invalid Enterprise record or concurrency limit"
        );
        ensure!(
            (1..=315_360_000).contains(&self.upload_ttl_seconds)
                && (1..=315_360_000).contains(&self.retention_seconds)
                && (300..=315_360_000).contains(&self.metadata_retention_seconds),
            "invalid Enterprise upload or retention duration"
        );
        ensure!(
            (1..=32).contains(&self.public_keys.len()),
            "Enterprise requires 1 to 32 pinned keys"
        );
        Ok(self.public_keys.iter().map(|(id, encoded)| {
            ensure!(valid_name(id), "invalid Enterprise key ID");
            let key = decode_fixed(encoded).ok_or_else(|| anyhow!("invalid Enterprise key pin"))?;
            Ok((id.as_str(), key))
        }))
    }
}

fn deserialize_pins<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error> {
    struct PinsVisitor;
    impl<'de> de::Visitor<'de> for PinsVisitor {
        type Value = BTreeMap<String, String>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a map of unique key IDs to key pins")
        }

        fn visit_map<M: de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut pins = BTreeMap::new();
            while let Some((id, key)) = map.next_entry()? {
                if pins.insert(id, key).is_some() {
                    return Err(de::Error::custom("duplicate Enterprise key ID"));
                }
            }
            Ok(pins)
        }
    }
    deserializer.deserialize_map(PinsVisitor)
}

// Derived struct deserializers also accept positional sequences; the wire format
// only accepts objects. Forward map access without materializing an intermediate map.
struct Object<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct ObjectVisitor<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> de::Visitor<'de> for ObjectVisitor<T> {
            type Value = Object<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object")
            }

            fn visit_map<M: de::MapAccess<'de>>(
                self,
                map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                T::deserialize(de::value::MapAccessDeserializer::new(map)).map(Object)
            }
        }
        deserializer.deserialize_map(ObjectVisitor(PhantomData))
    }
}

fn deserialize_optional_object<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    Ok(Option::<Object<T>>::deserialize(deserializer)?.map(|Object(value)| value))
}

pub(crate) struct Verifier {
    org_id: String,
    receiver_id: String,
    keys: BTreeMap<String, [u8; 32]>,
}

impl Verifier {
    pub(crate) fn new(config: &EnterpriseConfig) -> Result<Self> {
        let keys = config
            .pins()?
            .map(|pin| pin.map(|(id, key)| (id.to_owned(), key)))
            .collect::<Result<_>>()?;
        Ok(Self {
            org_id: config.org_id.clone(),
            receiver_id: config.receiver_id.clone(),
            keys,
        })
    }

    pub(crate) fn verify(&self, headers: &HeaderMap, at: i64) -> Option<Proof> {
        let mut values = headers.get_all(AUTHORIZATION).iter();
        let value = values.next()?.to_str().ok()?;
        let (scheme, token) = value.split_once(' ')?;
        if values.next().is_some()
            || !scheme.eq_ignore_ascii_case("Bearer")
            || token.len() > TOKEN_LIMIT
        {
            return None;
        }
        let mut pieces = token.splitn(4, '.');
        let protected = pieces.next()?;
        let payload = pieces.next()?;
        let signature = pieces.next()?;
        if pieces.next().is_some() {
            return None;
        }
        let mut scratch = [0; TOKEN_LIMIT / 4 * 3];
        let protected = decode(protected, &mut scratch)?;
        let Object(protected): Object<ProtectedHeader<'_>> =
            serde_json::from_slice(protected).ok()?;
        if protected.alg != "EdDSA" || protected.typ != TOKEN_TYPE {
            return None;
        }
        let key = self.keys.get(protected.kid.as_ref())?;
        let signature_bytes = decode_fixed::<64>(signature)?;
        let signed_bytes = &token.as_bytes()[..token.len() - signature.len() - 1];
        UnparsedPublicKey::new(&ED25519, key)
            .verify(signed_bytes, &signature_bytes)
            .ok()?;
        let payload = decode(payload, &mut scratch)?;
        let Object(claims): Object<Claims<'_>> = serde_json::from_slice(payload).ok()?;
        if claims.v != 1
            || claims.iss != ISSUER
            || claims.aud != AUDIENCE
            || claims.org_id != self.org_id
            || claims.receiver_id != self.receiver_id
            || !valid_uuid(&claims.user_id)
            || !valid_uuid(&claims.session_id)
            || !valid_uuid(&claims.event_id)
            || !valid_uuid(&claims.jti)
            || !valid_hash(&claims.manifest_sha256)
            || (claims.scopes.has(Scope::Check) && claims.manifest_sha256 != EMPTY_SHA256)
            || claims.iat > claims.nbf
            || claims.nbf >= claims.exp
            || claims.exp.checked_sub(claims.iat)? > 300
            || claims.nbf > at.saturating_add(30)
            || claims.exp <= at
        {
            return None;
        }
        Some(Proof {
            org_id: claims.org_id,
            receiver_id: claims.receiver_id,
            user_id: claims.user_id,
            session_id: claims.session_id,
            event_id: claims.event_id,
            manifest_sha256: claims.manifest_sha256,
            jti: claims.jti,
            scopes: claims.scopes,
            iat: claims.iat,
            nbf: claims.nbf,
            exp: claims.exp,
        })
    }
}

fn decode<'a>(value: &str, out: &'a mut [u8]) -> Option<&'a [u8]> {
    let len = URL_SAFE_NO_PAD.decode_slice(value, out).ok()?;
    Some(&out[..len])
}

fn decode_fixed<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != (N * 4).div_ceil(3) {
        return None;
    }
    let mut decoded = [0; N];
    if decode(value, &mut decoded)?.len() != N {
        return None;
    }
    Some(decoded)
}

fn valid_name(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn lowercase_hex(b: u8) -> bool {
    matches!(b, b'0'..=b'9' | b'a'..=b'f')
}

pub(crate) fn valid_uuid(value: &str) -> bool {
    let mut non_nil = false;
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => {
                non_nil |= b != b'0';
                lowercase_hex(b)
            }
        })
        && non_nil
}

pub(crate) fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(lowercase_hex)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtectedHeader<'a> {
    #[serde(borrow)]
    alg: Cow<'a, str>,
    #[serde(borrow)]
    typ: Cow<'a, str>,
    #[serde(borrow)]
    kid: Cow<'a, str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims<'a> {
    v: u8,
    #[serde(borrow)]
    iss: Cow<'a, str>,
    #[serde(borrow)]
    aud: Cow<'a, str>,
    org_id: String,
    receiver_id: String,
    user_id: String,
    session_id: String,
    event_id: String,
    manifest_sha256: String,
    scopes: Scopes,
    jti: String,
    iat: i64,
    nbf: i64,
    exp: i64,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
#[repr(u8)]
enum Scope {
    Manifest = 1,
    Audio = 2,
    Outcome = 4,
    Check = 8,
}

struct Scopes(u8);

impl Scopes {
    fn has(&self, scope: Scope) -> bool {
        self.0 & scope as u8 != 0
    }
}

impl<'de> Deserialize<'de> for Scopes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct ScopesVisitor;
        impl<'de> de::Visitor<'de> for ScopesVisitor {
            type Value = Scopes;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an array of distinct Enterprise scopes")
            }

            fn visit_seq<S: de::SeqAccess<'de>>(
                self,
                mut seq: S,
            ) -> std::result::Result<Self::Value, S::Error> {
                let mut scopes = Scopes(0);
                while let Some(scope) = seq.next_element()? {
                    if scopes.has(scope) {
                        return Err(de::Error::custom("duplicate Enterprise scope"));
                    }
                    scopes.0 |= scope as u8;
                }
                if scopes.0 == 0 || (scopes.has(Scope::Check) && scopes.0 != Scope::Check as u8) {
                    return Err(de::Error::custom("invalid Enterprise scopes"));
                }
                Ok(scopes)
            }
        }
        deserializer.deserialize_seq(ScopesVisitor)
    }
}

pub(crate) struct Proof {
    pub(crate) org_id: String,
    pub(crate) receiver_id: String,
    pub(crate) user_id: String,
    pub(crate) session_id: String,
    pub(crate) event_id: String,
    pub(crate) manifest_sha256: String,
    pub(crate) jti: String,
    scopes: Scopes,
    iat: i64,
    nbf: i64,
    pub(crate) exp: i64,
}

impl Proof {
    pub(crate) fn allows_manifest(&self) -> bool {
        self.scopes.has(Scope::Manifest)
    }

    pub(crate) fn allows_audio(&self) -> bool {
        self.scopes.has(Scope::Audio)
    }

    pub(crate) fn allows_outcome(&self) -> bool {
        self.scopes.has(Scope::Outcome)
    }

    pub(crate) fn allows_check(&self) -> bool {
        self.scopes.has(Scope::Check)
    }

    pub(crate) fn binding_digest(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"aurora-enterprise-local-v1-replay\0");
        // Validated IDs and hashes cannot contain the field separator. jti is the lookup key.
        for field in [
            &self.org_id,
            &self.receiver_id,
            &self.user_id,
            &self.session_id,
            &self.event_id,
            &self.manifest_sha256,
        ] {
            digest.update(field.as_bytes());
            digest.update([0]);
        }
        digest.update([self.scopes.0]);
        digest.update(self.iat.to_be_bytes());
        digest.update(self.nbf.to_be_bytes());
        digest.update(self.exp.to_be_bytes());
        digest.finalize().into()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    version: u8,
    pub(crate) session_id: String,
    pub(crate) event_id: String,
    pub(crate) revision: i32,
    #[serde(default, deserialize_with = "deserialize_optional_object")]
    text: Option<Text>,
    #[serde(default, deserialize_with = "deserialize_optional_object")]
    pub(crate) audio: Option<Audio>,
    pub(crate) outcome: Option<Outcome>,
}

impl Manifest {
    pub(crate) fn parse(raw: &[u8]) -> Result<Self> {
        // In particular, do not surface serde's errors: unknown field names can contain user text.
        let Object(manifest): Object<Self> =
            serde_json::from_slice(raw).map_err(|_| anyhow!("invalid Enterprise manifest"))?;
        ensure!(
            manifest.version == 1
                && valid_uuid(&manifest.session_id)
                && valid_uuid(&manifest.event_id)
                && manifest.revision > 0
                && manifest.audio.as_ref().is_none_or(Audio::valid)
                && (manifest.text.as_ref().is_some_and(Text::nonempty)
                    || manifest.audio.is_some()
                    || manifest.outcome.is_some()),
            "invalid Enterprise manifest"
        );
        Ok(manifest)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Audio {
    pub(crate) artifact_id: String,
    pub(crate) length: u64,
    pub(crate) sha256: String,
    pub(crate) media_type: String,
}

impl Audio {
    fn valid(&self) -> bool {
        valid_uuid(&self.artifact_id)
            && self.length > 0
            && valid_hash(&self.sha256)
            && matches!(
                self.media_type.as_str(),
                "audio/mp4"
                    | "audio/m4a"
                    | "audio/aac"
                    | "audio/mpeg"
                    | "audio/wav"
                    | "audio/x-wav"
                    | "audio/ogg"
                    | "audio/flac"
                    | "audio/webm"
            )
    }
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
pub(crate) enum Outcome {
    Won,
    NotWon,
    Unknown,
    Skipped,
}

impl Outcome {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Won => "won",
            Self::NotWon => "not_won",
            Self::Unknown => "unknown",
            Self::Skipped => "skipped",
        }
    }
}

// The receiver stores the original manifest, not a second copy of meeting content.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    title: Option<TextString>,
    transcript: Option<TextString>,
    summary: Option<TextString>,
    action_items: Option<ActionItems>,
}

impl Text {
    fn nonempty(&self) -> bool {
        self.title.is_some()
            || self.transcript.is_some()
            || self.summary.is_some()
            || self.action_items.is_some()
    }
}

struct TextString;

impl<'de> Deserialize<'de> for TextString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct StringVisitor;
        impl de::Visitor<'_> for StringVisitor {
            type Value = TextString;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a string")
            }

            fn visit_str<E: de::Error>(self, _: &str) -> std::result::Result<Self::Value, E> {
                Ok(TextString)
            }
        }
        deserializer.deserialize_str(StringVisitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionItem {
    #[serde(rename = "text")]
    _text: TextString,
    #[serde(rename = "completed")]
    _completed: bool,
}

struct ActionItems;

impl<'de> Deserialize<'de> for ActionItems {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct ItemsVisitor;
        impl<'de> de::Visitor<'de> for ItemsVisitor {
            type Value = ActionItems;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an array of action items")
            }

            fn visit_seq<S: de::SeqAccess<'de>>(
                self,
                mut seq: S,
            ) -> std::result::Result<Self::Value, S::Error> {
                while seq.next_element::<Object<ActionItem>>()?.is_some() {}
                Ok(ActionItems)
            }
        }
        deserializer.deserialize_seq(ItemsVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const CONFIG_JSON: &str = include_str!("../fixtures/enterprise-local/config.json");
    const MANIFEST_JSON: &[u8] = include_bytes!("../fixtures/enterprise-local/manifest.json");
    const VECTORS_JSON: &str = include_str!("../fixtures/enterprise-local/proof-vectors.json");

    fn config_json() -> Value {
        let mut config: Value = serde_json::from_str(CONFIG_JSON).unwrap();
        config["enterprise_local_v1"].take()
    }

    fn authorization(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        headers
    }

    #[test]
    fn verifies_openssl_proofs_and_binds_expiry_but_not_pin_or_scope_order() {
        let config = serde_json::from_value(config_json()).unwrap();
        let verifier = Verifier::new(&config).unwrap();
        let vectors: Value = serde_json::from_str(VECTORS_JSON).unwrap();
        let at = vectors["at"].as_i64().unwrap();
        let verify =
            |name: &str| verifier.verify(&authorization(vectors[name].as_str().unwrap()), at);
        let upload = verify("upload").unwrap();
        let manifest = Manifest::parse(MANIFEST_JSON).unwrap();
        assert_eq!(upload.user_id, "20000000-0000-4000-8000-000000000001");
        assert_eq!(upload.session_id, manifest.session_id);
        assert_eq!(upload.event_id, manifest.event_id);
        assert_eq!(
            upload.manifest_sha256,
            hex::encode(Sha256::digest(MANIFEST_JSON))
        );
        assert_eq!(manifest.outcome.unwrap().as_str(), "won");
        assert!(upload.allows_manifest() && upload.allows_outcome());
        assert!(!upload.allows_audio() && !upload.allows_check());
        assert_eq!(
            upload.binding_digest(),
            verify("rotated_upload").unwrap().binding_digest()
        );
        assert_ne!(
            upload.binding_digest(),
            verify("changed_expiry").unwrap().binding_digest()
        );

        let check = verify("check").unwrap();
        assert!(check.allows_check());
        assert!(!check.allows_manifest() && !check.allows_audio() && !check.allows_outcome());
    }

    #[test]
    fn rejects_signed_invalid_claims_unpinned_keys_and_ambiguous_authorization() {
        let mut config: EnterpriseConfig = serde_json::from_value(config_json()).unwrap();
        let verifier = Verifier::new(&config).unwrap();
        let vectors: Value = serde_json::from_str(VECTORS_JSON).unwrap();
        let at = vectors["at"].as_i64().unwrap();
        for name in [
            "duplicate_scope",
            "mixed_check",
            "invalid_header",
            "invalid_claim_shape",
        ] {
            let headers = authorization(vectors[name].as_str().unwrap());
            assert!(verifier.verify(&headers, at).is_none(), "{name} accepted");
        }

        let token = vectors["upload"].as_str().unwrap();
        let mut headers = authorization(token);
        assert!(verifier.verify(&headers, 1_760_000_300).is_none());
        assert!(verifier.verify(&headers, 1_759_999_969).is_none());
        assert!(verifier.verify(&headers, 1_759_999_970).is_some());
        headers.append(AUTHORIZATION, headers[AUTHORIZATION].clone());
        assert!(verifier.verify(&headers, at).is_none());

        let start = token.rfind('.').unwrap() + 1;
        let mut tampered = token.to_owned();
        let different = if token.as_bytes()[start] == b'A' {
            "B"
        } else {
            "A"
        };
        tampered.replace_range(start..start + 1, different);
        assert!(verifier.verify(&authorization(&tampered), at).is_none());

        config.public_keys.remove("fixture-rotation");
        let rotated = authorization(vectors["rotated_upload"].as_str().unwrap());
        assert!(
            Verifier::new(&config)
                .unwrap()
                .verify(&rotated, at)
                .is_none()
        );
        config.receiver_id = "another-receiver".into();
        assert!(
            Verifier::new(&config)
                .unwrap()
                .verify(&authorization(token), at)
                .is_none()
        );
    }

    #[test]
    fn validates_strict_config_and_pin_limits() {
        let config = config_json();
        let raw = config.to_string();
        let duplicate = raw.replace("\"fixture-rotation\":", "\"fixture-ed25519\":");
        assert!(serde_json::from_str::<EnterpriseConfig>(&duplicate).is_err());
        let mut with_extra = config.clone();
        with_extra["enabled"] = Value::Bool(true);
        assert!(serde_json::from_value::<EnterpriseConfig>(with_extra).is_err());

        let mut config: EnterpriseConfig = serde_json::from_value(config).unwrap();
        config.validate().unwrap();
        config.max_artifact_bytes = config.max_storage_bytes + 1;
        assert!(config.validate().is_err());
        config.max_artifact_bytes = 1;
        config
            .public_keys
            .get_mut("fixture-ed25519")
            .unwrap()
            .push('=');
        assert!(config.validate().is_err());
    }

    #[test]
    fn manifest_objects_and_types_are_strict_but_null_is_absence() {
        let body = |fields| {
            format!(
                r#"{{"version":1,"session_id":"30000000-0000-4000-8000-000000000001","event_id":"40000000-0000-4000-8000-000000000001","revision":1,{fields}}}"#
            )
        };
        let raw = body(r#""text":null,"audio":null,"outcome":"not_won""#);
        assert_eq!(
            Manifest::parse(raw.as_bytes())
                .unwrap()
                .outcome
                .unwrap()
                .as_str(),
            "not_won"
        );
        for fields in [
            r#""text":{"title":null,"transcript":null,"summary":null,"action_items":null}"#,
            r#""text":{"transcript":false}"#,
            r#""text":["synthetic",null,null,null]"#,
            r#""text":{"action_items":[["synthetic",false]]}"#,
            r#""text":{"action_items":[{"text":"synthetic"}]}"#,
            r#""text":{"title":"synthetic","transcript":"a","transcript":"b"}"#,
            r#""outcome":{"won":null}"#,
            r#""outcome":"sold""#,
            r#""outcome":"won","user_id":"synthetic""#,
            r#""audio":["60000000-0000-4000-8000-000000000001",1,"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","audio/wav"]"#,
            r#""audio":{"artifact_id":"60000000-0000-4000-8000-000000000001","length":0,"sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","media_type":"audio/wav"}"#,
        ] {
            assert!(
                Manifest::parse(body(fields).as_bytes()).is_err(),
                "{fields}"
            );
        }
    }
}
