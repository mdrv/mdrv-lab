// Copyright (c) 2024-present, fjall-rs
// This source code is licensed under both the Apache 2.0 and MIT License
// (found in the LICENSE-* files in the repository)

use std::{
    fs::{File, OpenOptions},
    path::Path,
    sync::Arc,
};

struct LockedFileGuardInner(File);

/// std's `File::try_lock`/`unlock` are unimplemented on `target_os = "android"`
/// (they return `io::ErrorKind::Unsupported, "try_lock() not supported"`).
/// flock(2) itself works on Android, so call libc directly there.
fn try_lock_impl(file: &File) -> std::result::Result<(), std::fs::TryLockError> {
    #[cfg(target_os = "android")]
    {
        use std::os::fd::AsRawFd;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::WouldBlock {
            Err(std::fs::TryLockError::WouldBlock)
        } else {
            Err(std::fs::TryLockError::Error(err))
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        file.try_lock()
    }
}

fn unlock_impl(file: &File) -> std::io::Result<()> {
    #[cfg(target_os = "android")]
    {
        use std::os::fd::AsRawFd;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        file.unlock()
    }
}

impl Drop for LockedFileGuardInner {
    fn drop(&mut self) {
        log::debug!("Unlocking database lock");

        unlock_impl(&self.0)
            .inspect_err(|e| {
                log::warn!("Failed to unlock database lock: {e:?}");
            })
            .ok();
    }
}

#[derive(Clone)]
#[expect(unused)]
pub struct LockedFileGuard(Arc<LockedFileGuardInner>);

impl LockedFileGuard {
    pub fn create_new(path: &Path) -> crate::Result<Self> {
        log::debug!("Acquiring database lock at {}", path.display());

        let file = match File::create_new(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                OpenOptions::new().read(true).write(true).open(path)?
            }
            e => e?,
        };

        try_lock_impl(&file).map_err(|e| match e {
            std::fs::TryLockError::Error(e) => {
                log::error!("Failed to acquire database lock - if this is expected, you can try opening again (maybe wait a little)");
                crate::Error::Io(e)
            }
            std::fs::TryLockError::WouldBlock => crate::Error::Locked,
        })?;

        Ok(Self(Arc::new(LockedFileGuardInner(file))))
    }

    pub fn try_acquire(path: &Path) -> crate::Result<Self> {
        const RETRIES: usize = 3;

        log::debug!("Acquiring database lock at {}", path.display());

        let file = OpenOptions::new().read(true).write(true).open(path)?;

        for i in 1..=RETRIES {
            if let Err(e) = try_lock_impl(&file) {
                match e {
                    std::fs::TryLockError::Error(e) => {
                        log::error!("Failed to acquire database lock - if this is expected, you can try opening again (maybe wait a little)");
                        return Err(crate::Error::Io(e));
                    }
                    std::fs::TryLockError::WouldBlock => {
                        if i == RETRIES {
                            return Err(crate::Error::Locked);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            } else {
                // Success
                break;
            }
        }

        Ok(Self(Arc::new(LockedFileGuardInner(file))))
    }
}

#[cfg(test)]
mod tests {
    use super::LockedFileGuard;
    use std::fs::File;

    #[test]
    fn create_new_acquires_lock_when_file_already_exists() -> crate::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("lock");

        File::create(&path)?;

        let _guard = LockedFileGuard::create_new(&path)?;

        Ok(())
    }

    #[test]
    fn try_acquire_acquires_lock_on_existing_file() -> crate::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("lock");

        File::create(&path)?;

        let _guard = LockedFileGuard::try_acquire(&path)?;

        Ok(())
    }
}
