//! Unified command error handling: the user gets a short, kind message;
//! the operator gets the full error chain in the logs. A raw `Debug` dump
//! must never reach chat.

use crate::{Data, Error};

/// The user-facing text for an internal command failure.
pub const USER_ERROR_MESSAGE: &str =
    "🍂 Something went wrong on my end. It's been logged — try again in a moment.";

/// Central `on_error` hook for the poise framework.
pub async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
    match error {
        poise::FrameworkError::Command { error, ctx, .. } => {
            tracing::error!(
                command = %ctx.command().qualified_name,
                error = format!("{error:#}"),
                "command failed"
            );
            let reply = poise::CreateReply::default()
                .content(USER_ERROR_MESSAGE)
                .ephemeral(true);
            if let Err(e) = ctx.send(reply).await {
                tracing::error!(error = %e, "failed to deliver error message to user");
            }
        }
        other => {
            // Setup/registration/permission errors and the rest: log via
            // poise's default handling, which never exposes internals.
            if let Err(e) = poise::builtins::on_error(other).await {
                tracing::error!(error = %e, "error while handling framework error");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_reveals_no_internals() {
        // The message is static: nothing interpolated, nothing leakable.
        assert!(!USER_ERROR_MESSAGE.contains('{'));
        assert!(USER_ERROR_MESSAGE.len() < 120);
    }
}
