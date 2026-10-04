//! Unified command error handling: the user gets a short, kind message;
//! the operator gets the full error chain in the logs. A raw `Debug` dump
//! must never reach chat.

use poise::serenity_prelude as serenity;

use crate::{Data, Error};

/// The user-facing text for an internal command failure.
pub const USER_ERROR_MESSAGE: &str =
    "🍂 Something went wrong on leaf's side, so that didn't work. It's been logged.";

/// The text for a failure that is known to pass: Discord answered with a
/// server error or a rate limit, or did not answer at all.
pub const TRANSIENT_ERROR_MESSAGE: &str =
    "🍂 Discord didn't answer leaf just now, so that didn't work. Try again in a moment.";

/// What someone is told when the command they used has changed since their
/// Discord client (or Discord itself) last saw leaf's command list.
pub const COMMAND_CHANGED_MESSAGE: &str = "🍂 This command changed in an update and Discord \
     hasn't caught up yet. Restart Discord, then try again. If it keeps happening, the server \
     owner can check leaf's logs.";

/// What someone is told when a command's own precondition turned them away.
pub const CHECK_FAILED_MESSAGE: &str = "🍂 You can't use that command here.";

/// What someone is told when they use a server command in a DM.
pub const GUILD_ONLY_MESSAGE: &str = "🍂 This only works inside a server.";

/// What Discord is told when someone uses a command leaf no longer has (a
/// stale entry in their client's command list).
pub const UNKNOWN_COMMAND_MESSAGE: &str =
    "🍂 That command is no longer part of leaf. Restart Discord to refresh the command list.";

/// The HTTP failure under `error`, when a request to Discord is what failed.
fn http_error(error: &Error) -> Option<&serenity::HttpError> {
    error
        .chain()
        .find_map(|cause| match cause.downcast_ref::<serenity::Error>() {
            Some(serenity::Error::Http(http)) => Some(http),
            _ => None,
        })
}

/// What a failed request to Discord says about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// Discord refused the request with this JSON error code.
    Refused(isize),
    /// A server error, a rate limit, or no answer: likely to pass.
    Transient,
    /// Anything else, including errors that are not Discord's.
    Other,
}

fn fault(error: &Error) -> Fault {
    match http_error(error) {
        Some(serenity::HttpError::UnsuccessfulRequest(response)) => {
            fault_of_response(response.status_code.as_u16(), response.error.code)
        }
        Some(serenity::HttpError::Request(_)) => Fault::Transient,
        _ => Fault::Other,
    }
}

const fn fault_of_response(status: u16, code: isize) -> Fault {
    if status == 429 || status >= 500 {
        Fault::Transient
    } else {
        Fault::Refused(code)
    }
}

const fn fault_text(fault: Fault) -> &'static str {
    match fault {
        Fault::Refused(50_001) => {
            "🍂 leaf can't see that channel. A server admin can allow its role to View Channel \
             there."
        }
        Fault::Refused(50_013) => {
            "🍂 leaf is missing a permission it needs for that. A server admin can check its \
             role's permissions in this channel."
        }
        Fault::Transient => TRANSIENT_ERROR_MESSAGE,
        Fault::Refused(_) | Fault::Other => USER_ERROR_MESSAGE,
    }
}

/// The reply for a failed command. A permission leaf is missing is named,
/// because someone in the server can fix it; a failure that will pass says
/// to try again; everything else is the one generic line.
#[must_use]
pub fn user_text(error: &Error) -> &'static str {
    fault_text(fault(error))
}

/// The reply for a failure that is in the logs: `text`, and for the generic
/// line the reference the log entry carries, so whoever runs leaf can find
/// it.
fn with_reference(text: &'static str, reference: u64) -> String {
    if text == USER_ERROR_MESSAGE {
        format!("{text} Reference for the server owner: `{reference}`.")
    } else {
        text.to_owned()
    }
}

/// Discord's name for each permission in `missing`, as its settings show
/// them.
fn permission_names(missing: serenity::Permissions) -> String {
    if missing == serenity::Permissions::MANAGE_GUILD {
        // serenity calls it "Manage Guilds"; Discord's settings do not.
        return "Manage Server".to_owned();
    }
    missing.get_permission_names().join(", ")
}

/// What someone is told when they lack a permission the command needs.
fn missing_user_permissions_text(missing: Option<serenity::Permissions>) -> String {
    missing.filter(|missing| !missing.is_empty()).map_or_else(
        || {
            "🍂 leaf couldn't check your permissions in this server, so it didn't run that \
             command. Try again in a moment."
                .to_owned()
        },
        |missing| {
            format!(
                "🍂 You need the {} permission in this server to use that command.",
                permission_names(missing)
            )
        },
    )
}

/// What someone is told when leaf lacks a permission the command needs.
fn missing_bot_permissions_text(missing: serenity::Permissions) -> String {
    format!(
        "🍂 leaf is missing a permission it needs for that command: {}. A server admin can \
         allow it for leaf's role.",
        permission_names(missing)
    )
}

/// Sends `text` as an ephemeral answer to the failed command.
async fn tell(ctx: crate::Context<'_>, text: impl Into<String>) {
    let reply = poise::CreateReply::default().content(text).ephemeral(true);
    if let Err(e) = ctx.send(reply).await {
        // Usually the interaction is past its deadline: the person already
        // sees Discord's own failure line.
        tracing::warn!(error = %e, "failed to deliver error message to user");
    }
}

/// Central `on_error` hook for the poise framework.
pub async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
    match error {
        poise::FrameworkError::Command { error, ctx, .. } => {
            let reference = ctx.id();
            tracing::error!(
                command = %ctx.command().qualified_name,
                reference,
                error = format!("{error:#}"),
                "command failed"
            );
            tell(ctx, with_reference(user_text(&error), reference)).await;
        }
        poise::FrameworkError::CommandPanic { ctx, payload, .. } => {
            let reference = ctx.id();
            tracing::error!(
                command = %ctx.command().qualified_name,
                reference,
                payload = payload.as_deref().unwrap_or("(none)"),
                "command panicked"
            );
            tell(ctx, with_reference(USER_ERROR_MESSAGE, reference)).await;
        }
        poise::FrameworkError::CommandStructureMismatch {
            ctx, description, ..
        } => {
            // Discord sent the options of an older (or newer) shape of the
            // command: its command list is not the one this build registers.
            tracing::error!(
                command = %ctx.command.qualified_name,
                description,
                "command options did not match; the registered command list is out of date"
            );
            tell(poise::Context::Application(ctx), COMMAND_CHANGED_MESSAGE).await;
        }
        poise::FrameworkError::CommandCheckFailed { ctx, error, .. } => match error {
            Some(error) => {
                let reference = ctx.id();
                tracing::error!(
                    command = %ctx.command().qualified_name,
                    reference,
                    error = format!("{error:#}"),
                    "command check failed with an error"
                );
                tell(ctx, with_reference(user_text(&error), reference)).await;
            }
            None => tell(ctx, CHECK_FAILED_MESSAGE).await,
        },
        poise::FrameworkError::MissingUserPermissions {
            ctx,
            missing_permissions,
            ..
        } => tell(ctx, missing_user_permissions_text(missing_permissions)).await,
        poise::FrameworkError::MissingBotPermissions {
            ctx,
            missing_permissions,
            ..
        } => tell(ctx, missing_bot_permissions_text(missing_permissions)).await,
        poise::FrameworkError::GuildOnly { ctx, .. } => tell(ctx, GUILD_ONLY_MESSAGE).await,
        poise::FrameworkError::UnknownInteraction {
            ctx, interaction, ..
        } => {
            tracing::warn!(command = %interaction.data.name, "received an unknown command");
            let reply = serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .content(UNKNOWN_COMMAND_MESSAGE)
                    .ephemeral(true),
            );
            if let Err(e) = interaction.create_response(ctx, reply).await {
                tracing::warn!(error = %e, "failed to answer an unknown command");
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
    fn user_messages_reveal_no_internals() {
        // The messages are static: nothing interpolated, nothing leakable.
        for text in [
            USER_ERROR_MESSAGE,
            TRANSIENT_ERROR_MESSAGE,
            UNKNOWN_COMMAND_MESSAGE,
            COMMAND_CHANGED_MESSAGE,
            CHECK_FAILED_MESSAGE,
            GUILD_ONLY_MESSAGE,
        ] {
            assert!(!text.contains('{'));
            assert!(text.chars().count() < 200, "{text}");
        }
    }

    #[test]
    fn only_a_failure_that_passes_says_to_try_again() {
        // A permanent fault must not send someone round in circles.
        assert!(!USER_ERROR_MESSAGE.contains("Try again"));
        assert_eq!(fault_of_response(503, 0), Fault::Transient);
        assert_eq!(fault_of_response(429, 0), Fault::Transient);
        assert!(fault_text(Fault::Transient).contains("Try again in a moment"));
        // A refusal is not transient, and the two that an admin can fix are
        // named.
        assert_eq!(fault_of_response(403, 50_013), Fault::Refused(50_013));
        assert!(fault_text(Fault::Refused(50_013)).contains("permission"));
        assert!(fault_text(Fault::Refused(50_001)).contains("View Channel"));
        assert_eq!(fault_text(Fault::Refused(50_035)), USER_ERROR_MESSAGE);
        assert_eq!(fault_text(Fault::Other), USER_ERROR_MESSAGE);
    }

    #[test]
    fn the_generic_line_carries_the_log_reference() {
        let text = with_reference(USER_ERROR_MESSAGE, 1234);
        assert!(text.starts_with(USER_ERROR_MESSAGE));
        assert!(text.contains("`1234`"));
        // A message that already says what to do needs none.
        assert_eq!(
            with_reference(TRANSIENT_ERROR_MESSAGE, 1234),
            TRANSIENT_ERROR_MESSAGE
        );
    }

    #[test]
    fn missing_permissions_are_named_as_discord_names_them() {
        let text = missing_user_permissions_text(Some(serenity::Permissions::MANAGE_GUILD));
        assert!(text.contains("Manage Server"), "{text}");
        assert!(!text.contains("Guild"), "{text}");
        // Discord could not say which: no empty list in the sentence.
        for unknown in [None, Some(serenity::Permissions::empty())] {
            let text = missing_user_permissions_text(unknown);
            assert!(text.contains("couldn't check your permissions"), "{text}");
        }
        let bot = missing_bot_permissions_text(serenity::Permissions::ATTACH_FILES);
        assert!(bot.contains("Attach Files"), "{bot}");
    }

    #[test]
    fn an_error_that_is_not_discord_s_gets_the_generic_line() {
        let e = anyhow::anyhow!("database is locked").context("saving");
        assert_eq!(user_text(&e), USER_ERROR_MESSAGE);
    }
}
