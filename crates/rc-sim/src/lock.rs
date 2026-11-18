//! `session.lock` lock (`anvil-world.md` § 8.1): "☃" written, forced to disk, then exclusive
//! non-blocking system lock, held until the world is fully closed.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use crate::Error;

pub struct SessionLock {
    file: File,
}

impl SessionLock {
    pub fn take(world: &Path) -> Result<SessionLock, Error> {
        let path = world.join("session.lock");
        let refused = || Error::Refused("world already open (official game, server or another instance)".into());
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|_| refused())?;
        // Locked by another: writing fails (Windows) or the lock is refused.
        file.try_lock().map_err(|_| refused())?;
        file.set_len(0).map_err(|e| Error::io(&path, e))?;
        file.write_all("\u{2603}".as_bytes()).map_err(|e| Error::io(&path, e))?;
        file.sync_all().map_err(|e| Error::io(&path, e))?;
        Ok(SessionLock { file })
    }

    /// World locked by someone else (world list): lock attempted then released.
    pub fn is_locked(world: &Path) -> bool {
        let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(world.join("session.lock")) else {
            return false;
        };
        match f.try_lock() {
            Ok(()) => {
                let _ = f.unlock();
                false
            }
            Err(_) => true,
        }
    }

    pub fn release(self) {
        let _ = self.file.unlock();
    }
}
