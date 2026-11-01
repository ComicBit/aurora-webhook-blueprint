use anyhow::{Context, Result, ensure};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs::{File, OpenOptions},
    path::{Component, Path, PathBuf},
};

pub fn absolute_clean(path: &Path) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "path must be absolute");
    let mut out = PathBuf::from("/");
    for part in path.components() {
        match part {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(v) => out.push(v),
            _ => anyhow::bail!("parent traversal is not allowed in managed paths"),
        }
    }
    ensure!(
        out.parent().is_some(),
        "filesystem root is not a managed directory"
    );
    Ok(out)
}

pub fn private_directory(path: &Path) -> Result<()> {
    let clean = absolute_clean(path)?;
    let mut current = PathBuf::from("/");
    for part in clean.components() {
        if let Component::Normal(v) = part {
            current.push(v);
            match std::fs::symlink_metadata(&current) {
                Ok(m) => {
                    ensure!(
                        m.is_dir() && !m.file_type().is_symlink(),
                        "managed path contains a symlink or non-directory"
                    );
                    let owner = m.uid();
                    ensure!(
                        owner == 0 || owner == rustix::process::geteuid().as_raw(),
                        "managed directory has an untrusted owner"
                    );
                    ensure!(
                        m.mode() & 0o022 == 0 || (owner == 0 && m.mode() & 0o1000 != 0),
                        "managed directory has untrusted writers"
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::DirBuilder::new()
                        .mode(0o700)
                        .create(&current)
                        .context("cannot create private directory")?;
                    File::open(
                        current
                            .parent()
                            .context("private directory has no parent")?,
                    )?
                    .sync_all()?;
                }
                Err(_) => anyhow::bail!("cannot inspect managed directory"),
            }
        }
    }
    let m = std::fs::symlink_metadata(&clean)?;
    ensure!(
        m.permissions().mode() & 0o077 == 0,
        "managed directory must be private (0700)"
    );
    Ok(())
}
use std::os::unix::fs::DirBuilderExt;

pub fn private_file(path: &Path, create: bool) -> Result<File> {
    if let Ok(m) = std::fs::symlink_metadata(path) {
        ensure!(
            m.is_file() && !m.file_type().is_symlink() && m.permissions().mode() & 0o077 == 0,
            "managed file must be a private regular file (0600)"
        );
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .context("cannot open private file")
}

pub fn owner_lock(data_dir: &Path) -> Result<File> {
    use fs2::FileExt;
    private_directory(data_dir)?;
    let file = private_file(&data_dir.join("receiver.lock"), true)?;
    file.try_lock_exclusive()
        .context("receiver owns this inbox; stop it before mutating or using offline commands")?;
    Ok(file)
}

pub fn free_bytes(path: &Path) -> Result<u64> {
    let stats = rustix::fs::statvfs(path).context("cannot inspect free storage")?;
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

pub fn write_private_new(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .context("output must be a new private file")?;
    f.write_all(bytes)?;
    f.sync_all()?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_replaceable_ancestors_before_creating_an_inbox() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(private_directory(&root.join("inbox")).is_err());
        assert!(!root.join("inbox").exists());
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        private_directory(&root.join("inbox")).unwrap();
    }
}
