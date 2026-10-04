//! How leaf's messages walk someone through Discord's menus.
//!
//! Two paths are taught: from a post to one of leaf's message commands
//! (Archive to Series, Remove Archive Entry), and from the message box to
//! the gallery. On a phone both go through a list of the server's apps,
//! where leaf's is listed under the bot's own name. That is "leaf" only when
//! whoever hosts it called it that, so the name comes from Discord
//! ([`crate::Data::app_name`]) and every message that names a path gets its
//! wording here.
//!
//! The phone path to a message command, as Discord's Android app has it:
//! press and hold the message; **Apps** is below the fold of the menu that
//! opens; Apps lists the server's apps; an app lists its commands. Once
//! someone has used a command, Discord also offers it at the top of Apps;
//! the steps as written still work. On desktop the Apps submenu lists the
//! commands themselves, with no app to choose first.
//!
//! The steps go into reminders and replies that are mostly read on a phone,
//! at about 40 characters to a line, so they are kept short: each path says
//! which device it is for and names every tap, and nothing else.

use leaf_core::series_ops::display_name;

/// The message command that archives a post, as Discord lists it.
pub const ARCHIVE_COMMAND: &str = "Archive to Series";

/// The message command that removes a post's archive entry, as Discord
/// lists it.
pub const REMOVE_COMMAND: &str = "Remove Archive Entry";

/// Stands in for the app's name while Discord has not said what it is. The
/// only messages that can go out that early are reminders, which the bot
/// itself sends: its name is on them.
const UNNAMED_APP: &str = "this bot's name";

/// leaf's app as one step of a path: its name in bold, escaped so it shows
/// exactly as the menu has it. A name that is not known is described
/// instead.
fn app_step(app: &str) -> String {
    let app = app.trim();
    if app.is_empty() {
        UNNAMED_APP.to_owned()
    } else {
        format!("**{}**", display_name(app))
    }
}

/// The way from a post to `command`, for the end of a sentence (it starts
/// in lower case and carries no final full stop): the phone's steps, then
/// the desktop's, each saying which it is. `app` is the name Discord lists
/// leaf's app under.
pub fn command_steps(app: &str, command: &str) -> String {
    format!(
        "on a phone, press and hold the post, tap **Apps** (scroll down to find it), {}, then \
         **{command}**. On desktop: right-click it, **Apps**, **{command}**",
        app_step(app)
    )
}

/// [`command_steps`] for Archive to Series.
pub fn archive_steps(app: &str) -> String {
    command_steps(app, ARCHIVE_COMMAND)
}

/// The way to the gallery through the app launcher, for after "on a phone, ":
/// the rest of that sentence, then the desktop's.
pub fn launcher_steps(app: &str) -> String {
    let app = app_step(app);
    format!(
        "tap **+** next to the message box, then **Apps**, then {app}. On desktop, open the app \
         launcher in the message box and choose {app}."
    )
}

/// Shown when the gallery cannot be opened from a button or a command.
pub fn open_gallery_fallback(app: &str) -> String {
    format!(
        "🍂 I couldn't open the gallery from here. On a phone, {}",
        launcher_steps(app)
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;

    #[test]
    fn the_phone_path_goes_through_apps_and_the_app_s_own_name() {
        assert_eq!(
            archive_steps("leaf-dev"),
            "on a phone, press and hold the post, tap **Apps** (scroll down to find it), \
             **leaf-dev**, then **Archive to Series**. On desktop: right-click it, **Apps**, \
             **Archive to Series**"
        );
        // Each path says which device it is for, the phone's first.
        let steps = archive_steps("leaf-dev");
        let (phone, desktop) = steps.split_once(". On desktop: ").unwrap();
        assert!(phone.starts_with("on a phone, "), "{phone}");
        // The desktop menu lists the commands themselves: no app to choose.
        assert!(phone.contains("**leaf-dev**"), "{phone}");
        assert!(!desktop.contains("leaf-dev"), "{desktop}");

        // The same path leads to the other message command.
        assert_eq!(
            command_steps("leaf-dev", REMOVE_COMMAND),
            archive_steps("leaf-dev").replace(ARCHIVE_COMMAND, REMOVE_COMMAND)
        );
    }

    #[test]
    fn the_app_is_named_as_discord_lists_it_not_as_leaf() {
        for path in [
            archive_steps("Daily Art Bot"),
            launcher_steps("Daily Art Bot"),
            open_gallery_fallback("Daily Art Bot"),
        ] {
            assert!(path.contains("**Daily Art Bot**"), "{path}");
            assert!(!path.contains("leaf"), "{path}");
        }
    }

    #[test]
    fn a_name_shows_as_typed_whatever_it_holds() {
        // Underscores, asterisks and mentions would otherwise be formatting.
        let steps = archive_steps("my_leaf_bot");
        assert!(steps.contains(r"it), **my\_leaf\_bot**, then"), "{steps}");
        let steps = archive_steps("**leaf** <@&9>");
        assert!(
            steps.contains(r"it), **\*\*leaf\*\* \<@&9\>**, then"),
            "{steps}"
        );
        let launcher = launcher_steps(" my_leaf ");
        assert!(launcher.contains(r"then **my\_leaf**. On"), "{launcher}");
        assert!(launcher.ends_with(r"choose **my\_leaf**."), "{launcher}");
    }

    #[test]
    fn an_app_whose_name_is_not_known_is_described_instead() {
        for unknown in ["", "  "] {
            let steps = archive_steps(unknown);
            assert!(
                steps.contains("(scroll down to find it), this bot's name, then **Archive"),
                "{steps}"
            );
            assert!(!steps.contains("****"), "{steps}");
            assert!(
                open_gallery_fallback(unknown).ends_with("choose this bot's name."),
                "{unknown:?}"
            );
        }
    }

    #[test]
    fn the_fallback_says_where_the_gallery_is_on_both_kinds_of_device() {
        assert_eq!(
            open_gallery_fallback("leaf-dev"),
            "🍂 I couldn't open the gallery from here. On a phone, tap **+** next to the \
             message box, then **Apps**, then **leaf-dev**. On desktop, open the app launcher in \
             the message box and choose **leaf-dev**."
        );
    }

    #[test]
    fn the_commands_are_named_as_they_are_registered() {
        let menus: Vec<String> = crate::commands::all()
            .into_iter()
            .filter_map(|command| command.context_menu_name)
            .collect();
        for named in [ARCHIVE_COMMAND, REMOVE_COMMAND] {
            assert!(menus.iter().any(|menu| menu == named), "{named}: {menus:?}");
        }
    }

    #[test]
    fn the_steps_stay_a_few_lines_on_a_phone() {
        /// Characters a phone shows on one line, about.
        const LINE_CHARS: usize = 40;
        /// The longest name Discord lets a bot or an application have.
        const NAME_MAX_CHARS: usize = 32;
        // What is read: the text without the marks that make it bold.
        let shown = |app: &str| archive_steps(app).replace("**", "").chars().count();

        // Both paths together: four lines under a name of ordinary length,
        // five under the longest there can be.
        assert!(shown("leaf-dev") <= 4 * LINE_CHARS, "{}", shown("leaf-dev"));
        let longest = "n".repeat(NAME_MAX_CHARS);
        assert!(shown(&longest) <= 5 * LINE_CHARS, "{}", shown(&longest));
    }
}
