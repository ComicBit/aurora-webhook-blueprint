use anyhow::{Result, ensure};
use rustix::fs::{self, Mode, OFlags};
use std::{
    fs::File,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Component, Path},
};

const MAX_BYTES: u64 = 64 * 1024;

/// Parse the complete selected file without changing the process environment.
/// Only the implicit config-adjacent file may be absent. All diagnostics omit
/// paths, source lines and parser/I/O errors, which could contain credentials.
pub fn read(config: &Path, explicit: Option<&Path>) -> Result<Vec<(String, String)>> {
    let selected = explicit.map(Path::to_path_buf).unwrap_or_else(|| {
        config
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".env")
    });
    let bytes = read_secure(&selected, explicit.is_none())
        .map_err(|_| anyhow::anyhow!("cannot securely read environment file (requires trusted ancestors and an owned single-link regular 0600 file, at most 64KiB)"))?;
    let Some(bytes) = bytes else {
        return Ok(Vec::new());
    };
    let entries = dotenvy::from_read_iter(bytes.as_slice())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| anyhow::anyhow!("invalid environment file"))?;
    // set_var rejects NULs and malformed keys. Reject every entry before the
    // caller applies any, rather than panicking with a potentially secret value.
    ensure!(
        entries.iter().all(|(key, value)| !key.is_empty()
            && !key.contains(['\0', '='])
            && !value.contains('\0')),
        "invalid environment file"
    );
    Ok(entries)
}

fn read_secure(path: &Path, optional: bool) -> Result<Option<Vec<u8>>> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let clean = crate::security::absolute_clean(&absolute)?;
    let mut directory = fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    // Walk held descriptors, not inspect-then-open pathnames: a rename cannot
    // redirect a later open through an unchecked ancestor or symlink.
    let mut parts = clean
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .peekable();
    loop {
        let stat = fs::fstat(&directory)?;
        let owner = stat.st_uid;
        ensure!(
            owner == 0 || owner == rustix::process::geteuid().as_raw(),
            "untrusted environment directory owner"
        );
        // Match managed-path policy: root-owned sticky temporary directories
        // protect another user's child even though the ancestor is writable.
        ensure!(
            stat.st_mode & 0o022 == 0 || (owner == 0 && stat.st_mode & 0o1000 != 0),
            "untrusted environment directory writers"
        );
        let name = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing filename"))?;
        let last = parts.peek().is_none();
        let flags = OFlags::RDONLY
            | OFlags::NOFOLLOW
            | OFlags::CLOEXEC
            | if last {
                OFlags::NONBLOCK
            } else {
                OFlags::DIRECTORY
            };
        let descriptor = match fs::openat(&directory, name, flags, Mode::empty()) {
            Ok(descriptor) => descriptor,
            Err(error) if optional && error == rustix::io::Errno::NOENT => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !last {
            directory = descriptor;
            continue;
        }
        let mut file = File::from(descriptor);
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file()
                && metadata.uid() == rustix::process::geteuid().as_raw()
                && metadata.nlink() == 1
                && metadata.mode() & 0o7777 == 0o600,
            "insecure environment file"
        );
        ensure!(
            metadata.len() <= MAX_BYTES,
            "environment file exceeds limit"
        );
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        (&mut file).take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_BYTES,
            "environment file exceeds limit"
        );
        return Ok(Some(bytes));
    }
}
