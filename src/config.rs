use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
    pub retention_seconds: i64,
    pub metadata_retention_seconds: i64,
    pub max_storage_bytes: u64,
    pub max_pending: u32,
    pub max_records: u32,
    pub max_concurrent_requests: usize,
    pub max_active_deliveries: usize,
    pub max_connections: usize,
    pub min_free_bytes: u64,
    pub destinations: Vec<Destination>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            listen: "127.0.0.1:8787".parse().unwrap(),
            data_dir: "data".into(),
            retention_seconds: 604800,
            metadata_retention_seconds: 2592000,
            max_storage_bytes: 256 << 20,
            max_pending: 1000,
            max_records: 100000,
            max_concurrent_requests: 8,
            max_active_deliveries: 4,
            max_connections: 64,
            min_free_bytes: 256 << 20,
            destinations: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Destination {
    pub id: String,
    pub enabled: bool,
    #[serde(flatten)]
    pub kind: DestinationKind,
}
impl<'de> Deserialize<'de> for Destination {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            id: String,
            #[serde(default)]
            enabled: bool,
            kind: String,
            root: Option<PathBuf>,
            formats: Option<Vec<FolderFormat>>,
            agent: Option<AgentConfig>,
            remote: Option<RemoteConfig>,
        }
        let input = Input::deserialize(deserializer)?;
        let invalid = || serde::de::Error::custom("destination fields do not match its kind");
        let kind = match input.kind.as_str() {
            "folder" if input.agent.is_none() && input.remote.is_none() => {
                DestinationKind::Folder {
                    root: input.root.ok_or_else(invalid)?,
                    formats: input.formats.unwrap_or_else(default_formats),
                }
            }
            "agent"
                if input.root.is_none() && input.formats.is_none() && input.remote.is_none() =>
            {
                DestinationKind::Agent {
                    agent: input.agent.ok_or_else(invalid)?,
                }
            }
            "remote"
                if input.root.is_none() && input.formats.is_none() && input.agent.is_none() =>
            {
                DestinationKind::Remote {
                    remote: input.remote.ok_or_else(invalid)?,
                }
            }
            _ => return Err(invalid()),
        };
        Ok(Self {
            id: input.id,
            enabled: input.enabled,
            kind,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DestinationKind {
    Folder {
        root: PathBuf,
        #[serde(default = "default_formats")]
        formats: Vec<FolderFormat>,
    },
    Agent {
        agent: AgentConfig,
    },
    Remote {
        remote: RemoteConfig,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FolderFormat {
    OriginalJson,
    Markdown,
}
fn default_formats() -> Vec<FolderFormat> {
    vec![FolderFormat::OriginalJson, FolderFormat::Markdown]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    pub runtime: AgentRuntime,
    pub mode: AgentMode,
    pub binary: PathBuf,
    pub workspace: PathBuf,
    pub profile_home: PathBuf,
    pub task: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub disclosure_approved: bool,
    #[serde(default)]
    pub authority_approved: bool,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub base_url_env: Option<String>,
    #[serde(default)]
    pub credential_env: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntime {
    Claude,
    Codex,
    Hermes,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    NewTask,
    Continue,
    CloudFollowup,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteConfig {
    pub runtime: RemoteRuntime,
    pub mode: RemoteMode,
    pub url_env: String,
    pub bearer_env: String,
    pub task: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default = "remote_timeout")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub disclosure_approved: bool,
    #[serde(default)]
    pub authority_approved: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRuntime {
    Hermes,
    GrokBot,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteMode {
    NewTask,
    ExistingNotification,
}
fn default_timeout() -> u64 {
    600
}
fn remote_timeout() -> u64 {
    30
}

pub fn fingerprint<T: Serialize>(value: &T) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}
impl Destination {
    pub fn revision(&self) -> Result<String> {
        fingerprint(self)
    }
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        use std::io::Read;
        let mut raw = Vec::new();
        std::fs::File::open(path)
            .context("cannot open configuration")?
            .take(65537)
            .read_to_end(&mut raw)?;
        ensure!(raw.len() <= 65536, "configuration exceeds 64KiB");
        let mut config: Self = serde_json::from_slice(&raw).context("invalid configuration")?;
        if config.data_dir.is_relative() {
            config.data_dir = std::env::current_dir()?
                .join(path)
                .parent()
                .context("configuration has no parent")?
                .join(&config.data_dir);
        }
        // Normalize dot components, but never resolve away a symlink before checking it.
        config.data_dir = crate::security::absolute_clean(&config.data_dir)?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 1, "unsupported configuration schema");
        ensure!(
            self.listen.ip().is_loopback() && self.listen.port() != 0,
            "listen must be a literal loopback address and port"
        );
        ensure!(self.data_dir.is_absolute(), "data_dir must be absolute");
        ensure!(
            (1..=315360000).contains(&self.retention_seconds),
            "invalid retention_seconds"
        );
        ensure!(
            (300..=315360000).contains(&self.metadata_retention_seconds),
            "invalid metadata_retention_seconds"
        );
        ensure!(
            ((9u64 << 20)..=(1u64 << 40)).contains(&self.max_storage_bytes),
            "invalid storage budget"
        );
        ensure!(
            (1..=100000).contains(&self.max_pending) && (1..=1000000).contains(&self.max_records),
            "invalid queue/record limits"
        );
        ensure!(
            (1..=64).contains(&self.max_concurrent_requests)
                && (1..=16).contains(&self.max_active_deliveries),
            "invalid concurrency"
        );
        ensure!(
            (1..=1024).contains(&self.max_connections),
            "invalid connection limit"
        );
        ensure!(self.destinations.len() <= 16, "at most 16 destinations");
        let mut ids = std::collections::HashSet::new();
        let mut roots: Vec<PathBuf> = vec![];
        for d in &self.destinations {
            ensure!(
                !d.id.is_empty()
                    && d.id.len() <= 128
                    && d.id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
                "invalid destination id"
            );
            ensure!(ids.insert(&d.id), "duplicate destination id");
            match &d.kind {
                DestinationKind::Folder { root, formats } => {
                    let clean = crate::security::absolute_clean(root)?;
                    ensure!(
                        !formats.is_empty() && formats.len() <= 2,
                        "folder needs one or both formats"
                    );
                    ensure!(
                        formats.len() != 2 || formats[0] != formats[1],
                        "duplicate folder format"
                    );
                    ensure!(
                        !clean.starts_with(&self.data_dir) && !self.data_dir.starts_with(&clean),
                        "export and database roots must not overlap"
                    );
                    ensure!(
                        roots
                            .iter()
                            .all(|r| !r.starts_with(&clean) && !clean.starts_with(r)),
                        "folder roots must not overlap"
                    );
                    roots.push(clean);
                }
                DestinationKind::Agent { agent } => {
                    ensure!(
                        agent.binary.is_absolute()
                            && agent.workspace.is_absolute()
                            && agent.profile_home.is_absolute(),
                        "agent paths must be absolute"
                    );
                    ensure!(
                        !agent.task.trim().is_empty() && agent.task.len() <= 16384,
                        "agent task must be 1..16384 bytes"
                    );
                    ensure!(
                        (1..=3600).contains(&agent.timeout_seconds),
                        "invalid agent deadline"
                    );
                    if let Some(name) = &agent.credential_env {
                        ensure!(env_name(name), "invalid agent credential reference");
                    }
                    if let Some(name) = &agent.base_url_env {
                        ensure!(env_name(name), "invalid model endpoint reference");
                    }
                    if agent.runtime == AgentRuntime::Claude {
                        ensure!(
                            agent.credential_env.is_some(),
                            "Claude requires an explicit credential environment reference"
                        );
                    }
                    if agent.mode != AgentMode::NewTask {
                        ensure!(
                            agent.session_id.as_ref().is_some_and(|v| !v.is_empty()
                                && v.len() <= 256
                                && !v.contains(['\n', '\r'])),
                            "continuation needs an exact session id"
                        );
                    }
                    if agent.mode == AgentMode::CloudFollowup {
                        ensure!(
                            agent.runtime == AgentRuntime::Claude,
                            "cloud_followup is a Claude mode"
                        );
                    }
                    if agent.runtime == AgentRuntime::Hermes {
                        ensure!(
                            agent.mode == AgentMode::NewTask,
                            "Hermes existing conversations use the official remote Runs adapter"
                        );
                        ensure!(
                            agent.model.as_ref().is_some_and(|v| !v.trim().is_empty())
                                && agent
                                    .provider
                                    .as_ref()
                                    .is_some_and(|v| !v.trim().is_empty() && v != "auto"),
                            "Hermes requires explicit model/provider"
                        );
                    }
                    if d.enabled {
                        ensure!(
                            agent.disclosure_approved && agent.authority_approved,
                            "agent activation requires disclosure and authority review"
                        );
                    }
                }
                DestinationKind::Remote { remote } => {
                    ensure!(
                        env_name(&remote.url_env)
                            && env_name(&remote.bearer_env)
                            && remote.url_env != remote.bearer_env,
                        "invalid remote secret references"
                    );
                    ensure!(
                        !remote.task.trim().is_empty()
                            && remote.task.len() <= 16384
                            && (1..=600).contains(&remote.timeout_seconds),
                        "invalid remote task/deadline"
                    );
                    if remote.runtime == RemoteRuntime::Hermes {
                        ensure!(
                            remote.model.as_ref().is_some_and(|v| !v.trim().is_empty())
                                && remote
                                    .provider
                                    .as_ref()
                                    .is_some_and(|v| !v.trim().is_empty() && v != "auto"),
                            "Hermes requires explicit model/provider"
                        );
                    }
                    if remote.runtime == RemoteRuntime::GrokBot {
                        ensure!(
                            remote.mode == RemoteMode::ExistingNotification,
                            "Grok Bot targets an existing routine, not the model API"
                        );
                    }
                    if remote.mode == RemoteMode::ExistingNotification
                        && remote.runtime == RemoteRuntime::Hermes
                    {
                        ensure!(
                            remote.session_id.as_ref().is_some_and(|v| !v.is_empty()),
                            "Hermes notification requires exact API-owned session"
                        );
                    }
                    if d.enabled {
                        ensure!(
                            remote.disclosure_approved && remote.authority_approved,
                            "remote activation requires disclosure and authority review"
                        );
                    }
                }
            }
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<String> {
        fingerprint(self)
    }
}
pub fn env_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && !name.starts_with("AURORA_")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prevents_public_bind_and_overlapping_managed_roots() {
        let mut c = Config {
            data_dir: "/private/data".into(),
            ..Default::default()
        };
        c.listen = "0.0.0.0:8787".parse().unwrap();
        assert!(c.validate().is_err());
        c.listen = "127.0.0.1:8787".parse().unwrap();
        c.destinations = vec![
            Destination {
                id: "one".into(),
                enabled: true,
                kind: DestinationKind::Folder {
                    root: "/private/notes".into(),
                    formats: default_formats(),
                },
            },
            Destination {
                id: "two".into(),
                enabled: true,
                kind: DestinationKind::Folder {
                    root: "/private/notes/nested".into(),
                    formats: default_formats(),
                },
            },
        ];
        assert!(c.validate().is_err());
    }
}
