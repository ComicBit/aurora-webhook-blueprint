//! Literal exports. Each file is atomically published without replacement; the bundle is not atomic.
use crate::{
    config::{DestinationKind, FolderFormat},
    model::{DeliveryJob, MAX_BODY, now},
};
use anyhow::{Result, ensure};
use rustix::fs::{self, AtFlags, Mode, OFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    os::fd::OwnedFd,
    path::{Component, Path},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

type Object = serde_json::Map<String, serde_json::Value>;

fn literal(out: &mut String, text: &str, language: &str) {
    let (mut ticks, mut tildes, mut tick_run, mut tilde_run) = (0, 0, 0, 0);
    for byte in text.bytes() {
        tick_run = if byte == b'`' { tick_run + 1 } else { 0 };
        tilde_run = if byte == b'~' { tilde_run + 1 } else { 0 };
        ticks = ticks.max(tick_run);
        tildes = tildes.max(tilde_run);
    }
    let (character, length) = if ticks <= tildes {
        ('`', 3.max(ticks + 1))
    } else {
        ('~', 3.max(tildes + 1))
    };
    let fence: String = std::iter::repeat_n(character, length).collect();
    out.push_str(&fence);
    out.push_str(language);
    out.push('\n');
    out.push_str(text);
    out.push('\n');
    out.push_str(&fence);
    out.push_str("\n\n");
}

fn field(out: &mut String, label: &str, value: Option<serde_json::Value>) -> Result<()> {
    out.push_str("#### ");
    out.push_str(label);
    out.push_str("\n\n");
    match value {
        None => out.push_str("Unknown (not provided).\n\n"),
        Some(serde_json::Value::Null) => out.push_str("Unknown (null).\n\n"),
        Some(serde_json::Value::String(text)) => {
            if text.is_empty() {
                out.push_str("Included, empty text.\n\n");
            }
            literal(out, &text, "text");
        }
        Some(value) => literal(out, &serde_json::to_string(&value)?, "json"),
    }
    Ok(())
}

fn fields(
    out: &mut String,
    value: Option<serde_json::Value>,
    known: &[(&str, &str)],
    extras: &mut Object,
    path: &str,
) -> Result<()> {
    match value {
        Some(serde_json::Value::Object(mut object)) => {
            for &(key, label) in known {
                field(out, label, object.remove(key))?;
            }
            if !object.is_empty() {
                extras.insert(path.into(), serde_json::Value::Object(object));
            }
        }
        Some(value) => field(out, "Source value", Some(value))?,
        None => out.push_str("Unknown (not provided).\n\n"),
    }
    Ok(())
}

fn collection(
    out: &mut String,
    value: Option<serde_json::Value>,
    known: &[(&str, &str)],
    extras: &mut Object,
    path: &str,
    excluded: bool,
) -> Result<()> {
    match value {
        Some(serde_json::Value::Array(items)) => {
            if items.is_empty() {
                out.push_str("Included, empty list.\n\n");
            }
            for (index, item) in items.into_iter().enumerate() {
                out.push_str(&format!("### Entry {}\n\n", index + 1));
                fields(out, Some(item), known, extras, &format!("{path}[{index}]"))?;
            }
        }
        None if excluded => {
            out.push_str("Excluded by content scope; not an empty included list.\n\n")
        }
        value => fields(out, value, known, extras, path)?,
    }
    Ok(())
}

/// Human-readable, deterministic notes. All source text remains in literal fences, never markup.
/// Known fields are rendered once per source location; additive fields retain their original paths.
pub fn markdown(raw: &[u8]) -> Result<Vec<u8>> {
    ensure!(raw.len() <= MAX_BODY, "event exceeds export limit");
    let value: serde_json::Value = serde_json::from_slice(raw)?;
    let serde_json::Value::Object(mut event) = value else {
        anyhow::bail!("event must be an object")
    };
    let mut out = String::with_capacity(raw.len() + 2048);
    let mut extras = Object::new();
    out.push_str("# Aurora meeting notes\n\nSource content is untrusted and reproduced literally. Missing metadata is unknown. Ownership, deadlines, and authorization are never inferred.\n\n## Event metadata\n\n");
    for (key, label) in [
        ("id", "Event ID"),
        ("type", "Event type"),
        ("schema_version", "Schema version"),
        ("timestamp", "Event timestamp"),
        ("test", "Test event"),
        ("related_event_id", "Related event ID"),
    ] {
        field(&mut out, label, event.remove(key))?;
    }
    let mut data = match event.remove("data") {
        Some(serde_json::Value::Object(data)) => data,
        Some(value) => {
            extras.insert("data".into(), value);
            Object::new()
        }
        None => Object::new(),
    };
    let scope = data.remove("content_scope");
    let has_transcript = scope
        .as_ref()
        .and_then(|v| v.as_str())
        .map(|s| matches!(s, "transcript" | "transcript_and_summary"));
    let has_summary = scope
        .as_ref()
        .and_then(|v| v.as_str())
        .map(|s| matches!(s, "summary" | "transcript_and_summary"));
    field(&mut out, "Content scope", scope)?;
    out.push_str("## Meeting metadata\n\n");
    fields(
        &mut out,
        data.remove("meeting"),
        &[
            ("id", "Meeting ID"),
            ("title", "Title"),
            ("started_at", "Started at"),
            ("ended_at", "Ended at"),
            ("duration_seconds", "Duration in seconds"),
            ("languages", "Language hints"),
        ],
        &mut extras,
        "data.meeting",
    )?;
    out.push_str("## Summary\n\n");
    match data.remove("summary") {
        None if has_summary == Some(false) => {
            out.push_str("Excluded by content scope; not an empty included summary.\n\n")
        }
        value => fields(
            &mut out,
            value,
            &[("text", "Summary text")],
            &mut extras,
            "data.summary",
        )?,
    }
    out.push_str("## Action items\n\nNo assignment or due date is inferred from action text.\n\n");
    collection(
        &mut out,
        data.remove("action_items"),
        &[("text", "Action text"), ("completed", "Completed")],
        &mut extras,
        "data.action_items",
        has_summary == Some(false),
    )?;
    out.push_str("## Transcript\n\n");
    match data.remove("transcript") {
        None if has_transcript == Some(false) => {
            out.push_str("Excluded by content scope; not an empty included transcript.\n\n")
        }
        Some(serde_json::Value::Object(mut transcript)) => {
            field(&mut out, "Transcript text", transcript.remove("text"))?;
            out.push_str("### Turns\n\nOffsets are seconds within the recording, not wall-clock times. Speaker IDs are references, not verified identities.\n\n");
            collection(
                &mut out,
                transcript.remove("turns"),
                &[
                    ("speaker_id", "Speaker reference"),
                    ("start_seconds", "Start offset"),
                    ("end_seconds", "End offset"),
                    ("text", "Turn text"),
                ],
                &mut extras,
                "data.transcript.turns",
                false,
            )?;
            if !transcript.is_empty() {
                extras.insert(
                    "data.transcript".into(),
                    serde_json::Value::Object(transcript),
                );
            }
        }
        value => fields(&mut out, value, &[], &mut extras, "data.transcript")?,
    }
    out.push_str(
        "## Speakers\n\nLabels and names are source-provided, not verified identities.\n\n",
    );
    collection(
        &mut out,
        data.remove("speakers"),
        &[("id", "Speaker ID"), ("label", "Label"), ("name", "Name")],
        &mut extras,
        "data.speakers",
        false,
    )?;
    out.push_str("## Source metadata\n\n");
    fields(
        &mut out,
        data.remove("source"),
        &[
            ("app", "Application"),
            ("app_version", "Application version"),
            ("platform", "Platform"),
        ],
        &mut extras,
        "data.source",
    )?;
    if !data.is_empty() {
        extras.insert("data".into(), serde_json::Value::Object(data));
    }
    if !event.is_empty() {
        extras.insert("event".into(), serde_json::Value::Object(event));
    }
    if !extras.is_empty() {
        out.push_str("## Extra source fields\n\nAdditive fields below retain their source location; no meaning is inferred.\n\n");
        literal(&mut out, &serde_json::to_string(&extras)?, "json");
    }
    Ok(out.into_bytes())
}

fn trusted_directory(fd: &OwnedFd, private: bool) -> Result<()> {
    let stat = fs::fstat(fd)?;
    let uid = unsafe { libc::geteuid() };
    ensure!(
        stat.st_uid == 0 || stat.st_uid == uid,
        "directory has untrusted owner"
    );
    // Root-owned sticky ancestors (notably /private/tmp) protect other users' children.
    ensure!(
        stat.st_mode & 0o022 == 0 || (stat.st_uid == 0 && stat.st_mode & 0o1000 != 0),
        "directory has untrusted writers"
    );
    if private {
        ensure!(
            stat.st_uid == uid && stat.st_mode & 0o077 == 0,
            "export directory must be private"
        );
    }
    Ok(())
}

/// Open every path component relative to an already held descriptor; never follow symlinks.
pub(crate) fn open_root(path: &Path, create: bool) -> Result<OwnedFd> {
    let clean = crate::security::absolute_clean(path)?;
    let mut dir = fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    trusted_directory(&dir, false)?;
    let parts: Vec<_> = clean
        .components()
        .filter_map(|p| {
            if let Component::Normal(v) = p {
                Some(v)
            } else {
                None
            }
        })
        .collect();
    for (i, part) in parts.iter().enumerate() {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        dir = match fs::openat(&dir, *part, flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(e) if e == rustix::io::Errno::NOENT && create => {
                match fs::mkdirat(&dir, *part, Mode::from_raw_mode(0o700)) {
                    Ok(()) => fs::fsync(&dir)?,
                    Err(e) if e == rustix::io::Errno::EXIST => {}
                    Err(e) => return Err(e.into()),
                }
                fs::openat(&dir, *part, flags, Mode::empty())?
            }
            Err(e) => return Err(e.into()),
        };
        trusted_directory(&dir, i + 1 == parts.len())?;
    }
    Ok(dir)
}

struct Fence {
    cancelled: Arc<AtomicBool>,
    expires_at: i64,
    deadline: Instant,
}
impl Fence {
    fn check(&self) -> Result<()> {
        ensure!(
            !self.cancelled.load(Ordering::Acquire)
                && now() < self.expires_at
                && Instant::now() < self.deadline,
            "export deadline reached"
        );
        Ok(())
    }
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn verify_file(dir: &OwnedFd, name: &str, expected: &[u8]) -> Result<bool> {
    let fd = match fs::openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(v) => v,
        Err(e) if e == rustix::io::Errno::NOENT => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let stat = fs::fstat(&fd)?;
    ensure!(
        fs::FileType::from_raw_mode(stat.st_mode).is_file()
            && stat.st_mode & 0o077 == 0
            && stat.st_nlink == 1
            && stat.st_uid == unsafe { libc::geteuid() },
        "export artifact is not a private regular file"
    );
    ensure!(
        stat.st_size >= 0 && stat.st_size as usize == expected.len(),
        "export artifact conflict"
    );
    let mut file = File::from(fd);
    let mut offset = 0;
    let mut buffer = [0u8; 16384];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        ensure!(
            expected.get(offset..offset + n) == Some(&buffer[..n]),
            "export artifact conflict"
        );
        offset += n;
    }
    ensure!(offset == expected.len(), "export artifact conflict");
    Ok(true)
}

fn publish(dir: &OwnedFd, name: &str, bytes: &[u8], fence: &Fence) -> Result<()> {
    fence.check()?;
    if verify_file(dir, name, bytes)? {
        return Ok(());
    }
    let temporary = format!(".aurora-{}", uuid::Uuid::new_v4());
    let fd = fs::openat(
        dir,
        temporary.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?;
    let mut published = false;
    let result = (|| -> Result<()> {
        let mut file = File::from(fd);
        for chunk in bytes.chunks(65536) {
            fence.check()?;
            file.write_all(chunk)?;
        }
        file.sync_all()?;
        fence.check()?;
        // Linux renameat2 / Apple renameatx_np: a whole-file no-replace publication.
        // Unlike link+unlink this leaves no crash window with two links to a committed file.
        match fs::renameat_with(
            dir,
            temporary.as_str(),
            dir,
            name,
            fs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => {
                published = true;
            }
            Err(e) if e == rustix::io::Errno::EXIST => {
                verify_file(dir, name, bytes)?;
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    })();
    let cleanup = match fs::unlinkat(dir, temporary.as_str(), AtFlags::empty()) {
        Ok(()) => Ok(()),
        Err(e) if e == rustix::io::Errno::NOENT => Ok(()),
        Err(e) => Err(e),
    };
    result?;
    cleanup?;
    fs::fsync(dir)?;
    if let Err(error) = fence.check() {
        // A slow filesystem syscall can cross the deadline. Retract only this attempt's newly
        // published name; pre-existing matching artifacts are never removed.
        if published {
            fs::unlinkat(dir, name, AtFlags::empty())?;
            fs::fsync(dir)?;
        }
        return Err(error);
    }
    Ok(())
}

#[derive(Serialize)]
struct Receipt<'a> {
    format_version: u32,
    event_id: &'a str,
    destination_id: &'a str,
    destination_revision: String,
    body_sha256: String,
    expires_at: i64,
    files: BTreeMap<&'static str, String>,
}

fn bundle(
    root: &Path,
    formats: &[FolderFormat],
    event_id: &str,
    destination_id: &str,
    revision: String,
    raw: &[u8],
    fence: &Fence,
) -> Result<()> {
    fence.check()?;
    ensure!(raw.len() <= MAX_BODY, "event exceeds export limit");
    #[derive(serde::Deserialize)]
    struct TestFlag {
        #[serde(default)]
        test: bool,
    }
    let value: TestFlag = serde_json::from_slice(raw)?;
    ensure!(!value.test, "test events cannot be exported");
    let root = open_root(root, true)?;
    let name = digest(event_id.as_bytes());
    fence.check()?;
    match fs::mkdirat(&root, name.as_str(), Mode::from_raw_mode(0o700)) {
        Ok(()) => fs::fsync(&root)?,
        Err(e) if e == rustix::io::Errno::EXIST => {}
        Err(e) => return Err(e.into()),
    }
    let dir = fs::openat(
        &root,
        name.as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    trusted_directory(&dir, true)?;
    let mut entries = fs::Dir::read_from(&dir)?;
    let mut count = 0;
    while let Some(entry) = entries.read() {
        fence.check()?;
        let entry = entry?;
        let name = entry.file_name();
        let bytes = name.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        count += 1;
        ensure!(count <= 128, "unexpected export directory contents");
        let expected = bytes == b"receipt.json"
            || (bytes == b"event.json" && formats.contains(&FolderFormat::OriginalJson))
            || (bytes == b"meeting.md" && formats.contains(&FolderFormat::Markdown));
        let temporary = std::str::from_utf8(bytes)
            .ok()
            .and_then(|s| s.strip_prefix(".aurora-"))
            .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok());
        ensure!(expected || temporary, "unexpected export artifact");
        let stat = fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW)?;
        ensure!(
            fs::FileType::from_raw_mode(stat.st_mode).is_file()
                && stat.st_mode & 0o077 == 0
                && stat.st_nlink == 1
                && stat.st_uid == unsafe { libc::geteuid() },
            "unsafe export artifact"
        );
    }
    let md = if formats.contains(&FolderFormat::Markdown) {
        Some(markdown(raw)?)
    } else {
        None
    };
    let body_sha256 = digest(raw);
    let mut files = BTreeMap::new();
    if formats.contains(&FolderFormat::OriginalJson) {
        files.insert("event.json", body_sha256.clone());
    }
    if let Some(md) = &md {
        files.insert("meeting.md", digest(md));
    }
    let receipt = serde_json::to_vec_pretty(&Receipt {
        format_version: 1,
        event_id,
        destination_id,
        destination_revision: revision,
        body_sha256,
        expires_at: fence.expires_at,
        files,
    })?;
    // A receipt from a different identity/revision is always a conflict, including a partial bundle.
    let had_receipt = verify_file(&dir, "receipt.json", &receipt)?;
    let has_json = if formats.contains(&FolderFormat::OriginalJson) {
        verify_file(&dir, "event.json", raw)?
    } else {
        true
    };
    let has_md = if let Some(md) = &md {
        verify_file(&dir, "meeting.md", md)?
    } else {
        true
    };
    ensure!(
        !had_receipt || (has_json && has_md),
        "committed export was edited"
    );
    if had_receipt {
        fence.check()?;
        fs::fsync(&dir)?;
        return Ok(());
    }
    if formats.contains(&FolderFormat::OriginalJson) {
        publish(&dir, "event.json", raw, fence)?;
    }
    if let Some(md) = &md {
        publish(&dir, "meeting.md", md, fence)?;
    }
    fence.check()?;
    publish(&dir, "receipt.json", &receipt, fence)?;
    Ok(())
}

pub(crate) async fn export_one(job: &DeliveryJob) -> Result<()> {
    let DestinationKind::Folder { root, formats } = &job.destination.kind else {
        anyhow::bail!("not a folder destination")
    };
    let root = root.clone();
    let formats = formats.clone();
    let raw = job.body.clone();
    let event_id = job.event_id.clone();
    let destination_id = job.destination.id.clone();
    let revision = job.destination.revision()?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel = CancelOnDrop(cancelled.clone());
    let remaining = (job.expires_at - now()).max(0) as u64;
    let fence = Fence {
        cancelled,
        expires_at: job.expires_at,
        deadline: Instant::now() + Duration::from_secs(60.min(remaining)),
    };
    tokio::task::spawn_blocking(move || {
        bundle(
            &root,
            &formats,
            &event_id,
            &destination_id,
            revision,
            &raw,
            &fence,
        )
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn fence() -> Fence {
        Fence {
            cancelled: Arc::new(AtomicBool::new(false)),
            expires_at: now() + 60,
            deadline: Instant::now() + Duration::from_secs(60),
        }
    }
    fn partition_literals(md: &str) -> (String, String) {
        let (mut source, mut markup, mut active) = (String::new(), String::new(), None::<String>);
        for line in md.lines() {
            if let Some(fence) = &active {
                if line == fence {
                    active = None;
                } else {
                    source.push_str(line);
                    source.push('\n');
                }
            } else {
                let first = line.as_bytes().first().copied().unwrap_or(0);
                let count = line.bytes().take_while(|b| *b == first).count();
                if matches!(first, b'`' | b'~') && count >= 3 {
                    active = Some(line[..count].into());
                } else {
                    markup.push_str(line);
                    markup.push('\n');
                }
            }
        }
        assert!(active.is_none(), "literal fence must close");
        (source, markup)
    }
    #[test]
    fn markdown_is_readable_literal_and_preserves_additive_fields() {
        let summary = "<script>canary</script>\n```\n~~~\n# injected heading";
        let raw = serde_json::to_vec(&serde_json::json!({
            "id":"event","additive_event":{"owner":null},"data":{
                "content_scope":"transcript_and_summary",
                "meeting":{"id":"meeting","title":"Fictional garden","started_at":null,"extra_meeting":{"flag":true}},
                "summary":{"text":summary,"extra_summary":"kept"},
                "action_items":[{"text":"Draft a watering proposal","completed":false,"extra_action":7}],
                "transcript":{"text":"Readable speech\nwithout JSON escapes","turns":[{"speaker_id":null,"start_seconds":0,"end_seconds":1,"text":"A literal turn","extra_turn":true}]},
                "speakers":[{"id":"speaker_0","label":"Speaker 1","name":null,"extra_speaker":"kept"}],
                "source":{"app":"Aurora","app_version":"synthetic","platform":"macos","extra_source":1}
            }
        })).unwrap();
        let md = String::from_utf8(markdown(&raw).unwrap()).unwrap();
        assert_eq!(markdown(&raw).unwrap(), md.as_bytes());
        let (literal, markup) = partition_literals(&md);
        for text in [
            summary,
            "Readable speech\nwithout JSON escapes",
            "A literal turn",
            "Draft a watering proposal",
            "Fictional garden",
        ] {
            assert!(
                literal.contains(text),
                "source text must remain decoded and literal: {text}"
            );
        }
        assert!(!markup.contains("<script>") && !markup.contains("# injected heading"));
        assert_eq!(
            literal.matches("Readable speech").count(),
            1,
            "known transcript must not be duplicated as raw JSON"
        );
        for key in [
            "additive_event",
            "extra_meeting",
            "extra_summary",
            "extra_action",
            "extra_turn",
            "extra_speaker",
            "extra_source",
        ] {
            assert!(literal.contains(key), "additive field must survive: {key}");
        }
        let extra_section = md.split("## Extra source fields").nth(1).unwrap();
        let extra: serde_json::Value =
            serde_json::from_str(partition_literals(extra_section).0.trim()).unwrap();
        assert_eq!(
            extra.pointer("/event/additive_event/owner"),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(
            extra.pointer("/data.action_items[0]/extra_action"),
            Some(&serde_json::json!(7))
        );
    }
    #[test]
    fn export_reconciles_matching_partial_and_rejects_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let raw = br#"{"id":"event","test":false}"#;
        let formats = [FolderFormat::OriginalJson, FolderFormat::Markdown];
        bundle(
            &root,
            &formats,
            "event",
            "folder",
            "revision".into(),
            raw,
            &fence(),
        )
        .unwrap();
        let event = root.join(digest(b"event"));
        std::fs::remove_file(event.join("receipt.json")).unwrap();
        bundle(
            &root,
            &formats,
            "event",
            "folder",
            "revision".into(),
            raw,
            &fence(),
        )
        .unwrap();
        std::fs::write(event.join("event.json"), b"edited").unwrap();
        assert!(
            bundle(
                &root,
                &formats,
                "event",
                "folder",
                "revision".into(),
                raw,
                &fence()
            )
            .is_err()
        );
    }
    #[test]
    fn export_rejects_symlinks_expiry_and_tests() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::os::unix::fs::symlink("/", root.join(digest(b"event"))).unwrap();
        assert!(
            bundle(
                &root,
                &[FolderFormat::OriginalJson],
                "event",
                "folder",
                "r".into(),
                b"{}",
                &fence()
            )
            .is_err()
        );
        let expired = Fence {
            expires_at: now(),
            ..fence()
        };
        assert!(
            bundle(
                &root,
                &[FolderFormat::OriginalJson],
                "expired",
                "folder",
                "r".into(),
                b"{}",
                &expired
            )
            .is_err()
        );
        assert!(
            bundle(
                &root,
                &[FolderFormat::OriginalJson],
                "test",
                "folder",
                "r".into(),
                br#"{"test":true}"#,
                &fence()
            )
            .is_err()
        );
        assert!(!root.join(digest(b"test")).exists());
    }
}
