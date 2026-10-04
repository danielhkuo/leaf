//! Process-wide runtime status that one half of leaf reports and the other
//! half shows.
//!
//! The bot and the HTTP server run in the same process but do not share
//! state, and a dead gateway leaves the server (and `/healthz`) looking
//! healthy. The bot records its gateway state here; the server reads it for
//! `GET /api/status`, the admin panel and the setup page.

use std::sync::{PoisonError, RwLock};

/// Where the Discord gateway connection stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GatewayState {
    /// Not connected yet: process start, or waiting to retry.
    #[default]
    Starting,
    /// Connected; commands are being answered.
    Online,
    /// The last attempt failed. The detail is shown to the server owner, so
    /// it must be a plain sentence with no secrets in it.
    Error(String),
}

impl GatewayState {
    /// Stable machine name: `starting`, `online` or `error`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Online => "online",
            Self::Error(_) => "error",
        }
    }

    /// What went wrong, when the state is [`GatewayState::Error`].
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Error(detail) => Some(detail),
            Self::Starting | Self::Online => None,
        }
    }
}

static GATEWAY: RwLock<GatewayState> = RwLock::new(GatewayState::Starting);

/// Records the gateway state. Called by the bot as it connects, fails and
/// retries.
pub fn set_gateway(state: GatewayState) {
    // A poisoned lock only means a writer panicked mid-assignment of a plain
    // value; the stored state is still usable, so recover it.
    *GATEWAY.write().unwrap_or_else(PoisonError::into_inner) = state;
}

/// The most recently recorded gateway state ([`GatewayState::Starting`]
/// until the bot reports otherwise).
#[must_use]
pub fn gateway() -> GatewayState {
    GATEWAY
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

static NOTICE: RwLock<Option<String>> = RwLock::new(None);

/// Records (or with `None` clears) a problem that leaves the bot connected.
///
/// For something that needs the server owner's attention without the
/// connection being down, such as a command list Discord has not accepted.
/// Shown beside the gateway state, so it must be a plain sentence with no
/// secrets in it.
pub fn set_notice(notice: Option<String>) {
    *NOTICE.write().unwrap_or_else(PoisonError::into_inner) = notice;
}

/// The problem recorded by [`set_notice`], if one stands.
#[must_use]
pub fn notice() -> Option<String> {
    NOTICE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The state is process-global, so every transition is exercised in this
    // one test rather than in several that would race each other.
    #[test]
    fn gateway_state_round_trips() {
        assert_eq!(gateway(), GatewayState::Starting);
        assert_eq!(gateway().as_str(), "starting");
        assert_eq!(gateway().detail(), None);

        set_gateway(GatewayState::Online);
        assert_eq!(gateway(), GatewayState::Online);
        assert_eq!(gateway().as_str(), "online");

        set_gateway(GatewayState::Error(
            "Discord rejected the bot token.".to_owned(),
        ));
        let state = gateway();
        assert_eq!(state.as_str(), "error");
        assert_eq!(state.detail(), Some("Discord rejected the bot token."));

        set_gateway(GatewayState::Starting);
        assert_eq!(gateway(), GatewayState::default());

        // The notice stands beside the state and is cleared on its own.
        assert_eq!(notice(), None);
        set_notice(Some(
            "Discord has not accepted the command list.".to_owned(),
        ));
        assert_eq!(
            notice().as_deref(),
            Some("Discord has not accepted the command list.")
        );
        assert_eq!(gateway(), GatewayState::Starting);
        set_notice(None);
        assert_eq!(notice(), None);
    }
}
