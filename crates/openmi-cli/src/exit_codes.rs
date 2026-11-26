use std::process::ExitCode;

use openmi_core::CoreError;
use thiserror::Error;

use crate::auth::AuthError;

pub const EXIT_OK: u8 = 0;
pub const EXIT_ERROR: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_AUTH: u8 = 3;
pub const EXIT_OUT_OF_CREDITS: u8 = 4;
pub const EXIT_LOOP_CAP: u8 = 5;
pub const EXIT_CANCELLED: u8 = 130;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("{0}")]
    Usage(String),

    #[error("{0}")]
    Auth(#[from] AuthError),

    #[error("insufficient credits (HTTP 402) — top up your Openmi balance to continue")]
    OutOfCredits,

    #[error("loop cap exceeded after {0} steps")]
    LoopCap(u32),

    #[error("cancelled")]
    Cancelled,

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl CliError {
    pub fn exit_code(&self) -> ExitCode {
        let code = match self {
            CliError::Usage(_) => EXIT_USAGE,
            CliError::Auth(_) => EXIT_AUTH,
            CliError::OutOfCredits => EXIT_OUT_OF_CREDITS,
            CliError::LoopCap(_) => EXIT_LOOP_CAP,
            CliError::Cancelled => EXIT_CANCELLED,
            CliError::Other(_) => EXIT_ERROR,
        };
        ExitCode::from(code)
    }
}

impl From<CoreError> for CliError {
    fn from(err: CoreError) -> Self {
        match err {
            CoreError::Cancelled => CliError::Cancelled,
            CoreError::Unauthorized => CliError::Auth(AuthError::Revoked),
            CoreError::OutOfCredits => CliError::OutOfCredits,
            CoreError::LoopCapExceeded(n) => CliError::LoopCap(n),
            other => CliError::Other(other.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_core_and_auth_errors_to_spec_exit_codes() {
        assert_eq!(CliError::Usage("bad".into()).exit_code(), ExitCode::from(2));
        assert_eq!(
            CliError::from(AuthError::NotLoggedIn).exit_code(),
            ExitCode::from(3)
        );
        assert_eq!(
            CliError::from(CoreError::Unauthorized).exit_code(),
            ExitCode::from(3)
        );
        assert_eq!(
            CliError::from(CoreError::OutOfCredits).exit_code(),
            ExitCode::from(4)
        );
        assert_eq!(
            CliError::from(CoreError::LoopCapExceeded(8)).exit_code(),
            ExitCode::from(5)
        );
        assert_eq!(
            CliError::from(CoreError::Cancelled).exit_code(),
            ExitCode::from(130)
        );
    }
}
