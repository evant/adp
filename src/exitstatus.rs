use std::process::ExitStatus;

use thiserror::Error;

#[derive(Error, Debug, Clone)]
#[error("status code {0}")]
pub(crate) struct ExitStatusError(ExitStatus);

impl ExitStatusError {
    pub fn code(&self) -> Option<i32> {
        self.0.code()
    }
}

pub(crate) trait ExitStatusExt {
    fn exit_ok_(&self) -> std::result::Result<(), ExitStatusError>;
}

impl ExitStatusExt for ExitStatus {
    fn exit_ok_(&self) -> std::result::Result<(), ExitStatusError> {
        if self.success() {
            Ok(())
        } else {
            Err(ExitStatusError(self.clone()))
        }
    }
}
