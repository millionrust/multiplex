use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Durability {
    Full,
    RenameOnly,
}

pub trait AtomicWriter: Send + Sync {
    fn write(&self, target: &Path, bytes: &[u8]) -> io::Result<Durability>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemAtomicWriter;

impl AtomicWriter for SystemAtomicWriter {
    fn write(&self, target: &Path, bytes: &[u8]) -> io::Result<Durability> {
        atomic_write(target, bytes)
    }
}

fn atomic_write(target: &Path, bytes: &[u8]) -> io::Result<Durability> {
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "metadata target has no parent")
    })?;
    reject_unsafe_existing_target(target)?;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
    let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let file_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("metadata");
    let temporary = parent.join(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));

    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);

        #[cfg(unix)]
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
        fs::rename(&temporary, target)?;

        #[cfg(unix)]
        fs::set_permissions(target, fs::Permissions::from_mode(0o600))?;

        match File::open(parent).and_then(|directory| directory.sync_all()) {
            Ok(()) => Ok(Durability::Full),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Unsupported
                        | io::ErrorKind::InvalidInput
                        | io::ErrorKind::PermissionDenied
                ) =>
            {
                Ok(Durability::RenameOnly)
            }
            Err(error) => Err(error),
        }
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn reject_unsafe_existing_target(target: &Path) -> io::Result<()> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "metadata target must not be a symlink",
        )),
        Ok(metadata) if !metadata.is_file() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "metadata target must be a regular file",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_writer_replaces_complete_bytes_and_leaves_no_temp_file() {
        let fixture = tempfile::tempdir().unwrap();
        let target = fixture.path().join("projects.json");
        fs::write(&target, b"old").unwrap();
        SystemAtomicWriter.write(&target, b"new-complete").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new-complete");
        assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_writer_rejects_symlink_targets() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let outside = fixture.path().join("outside");
        fs::write(&outside, b"sentinel").unwrap();
        let target = fixture.path().join("projects.json");
        symlink(&outside, &target).unwrap();
        assert_eq!(
            SystemAtomicWriter
                .write(&target, b"replacement")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(fs::read(&outside).unwrap(), b"sentinel");
    }
}
