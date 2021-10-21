use core::result::Result::Ok;
use std::fs::File;
use std::io::Result;
use std::ops::{Deref, DerefMut};

use fs2::FileExt;

pub(crate) struct FileLockGuard(File);

pub(crate) trait FileLockGuardExt {
    fn into_lock_exclusive(self) -> Result<FileLockGuard>;
}

impl FileLockGuardExt for File {
    fn into_lock_exclusive(self) -> Result<FileLockGuard> {
        self.lock_exclusive()?;
        Ok(FileLockGuard(self))
    }
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl Deref for FileLockGuard {
    type Target = File;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for FileLockGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
