//! Series lookup for the chat commands, and the surface they answer on.
//!
//! Every lookup works over one [`Scope`]: the series the invoker may view
//! (`/search`, `/random`, `/status`, `/wrapped`), the ones they may delete
//! days from (`/delete`), or every series (the Manage Server commands). A
//! name outside the scope is never suggested, listed, or told apart from a
//! typo, so a hidden series stays hidden. Inside it, a typed name is matched
//! the forgiving way a phone keyboard needs (`match_series_name`: exact,
//! any case, prefix, substring), a left-out name means "the only one", and
//! several candidates become a select instead of an error.
//!
//! Once the series is settled, a command answers through [`Via`]: on the
//! command itself, or on the select press that picked the series.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use leaf_core::domain::{Series, SeriesState};
use leaf_core::policy::{self, Viewer};
use leaf_core::series_ops::{self, NameMatch};
use poise::serenity_prelude as serenity;
use serenity::futures::StreamExt as _;

pub(crate) use crate::components::open_gallery_id;
use crate::components::{self, NONCE};
use crate::{Context, Data, Error, checks};

/// The command context the lookups and answers run on.
pub(crate) type App<'a> = poise::ApplicationContext<'a, Data, Error>;

/// How long a select or confirm prompt waits for a press. Well inside the
/// interaction token's 15 minutes, so the prompt can still be edited to say
/// it expired.
pub(crate) const PROMPT_TIMEOUT: Duration = Duration::from_mins(5);

/// Most series named in a "couldn't find it" reply.
const NAMES_SHOWN_MAX: usize = 10;
/// Discord's cap on autocomplete choices and on select options.
pub(crate) const CHOICES_MAX: usize = 25;
/// Discord's cap on a select option's label.
pub(crate) const OPTION_LABEL_MAX_CHARS: usize = 100;
/// How much of a typed name is echoed back.
const ECHO_MAX_CHARS: usize = 40;

/// First segment of this module's collector-scoped component ids. Not
/// `leaf:`, which belongs to the stateless buttons a global router answers.
const ID_PREFIX: &str = "qry";

/// Shown on a picker whose series is no longer available.
const PICK_GONE: &str = "🍂 That series isn't available any more, so nothing happened.";

// ---------------------------------------------------------------------------
// Who is asking
// ---------------------------------------------------------------------------

/// The invoker, as the visibility rules see them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Asker {
    /// User snowflake.
    pub(crate) user_id: String,
    /// Role snowflakes in this server.
    pub(crate) role_ids: Vec<String>,
    /// Holds Manage Server.
    pub(crate) is_admin: bool,
}

impl Asker {
    /// Reads the invoker from the interaction (no extra request: the member
    /// and their permissions come with it, autocomplete included).
    pub(crate) async fn of(ctx: &Context<'_>) -> Self {
        let role_ids = match ctx.author_member().await {
            Some(m) => m.roles.iter().map(ToString::to_string).collect(),
            None => Vec::new(),
        };
        Self {
            user_id: ctx.author().id.to_string(),
            role_ids,
            is_admin: checks::is_admin(ctx),
        }
    }

    /// Whether `policy::can_view` lets them see `series`.
    pub(crate) fn can_view(&self, series: &Series) -> bool {
        let viewer = Viewer {
            user_id: &self.user_id,
            role_ids: &self.role_ids,
            is_admin: self.is_admin,
        };
        policy::can_view(series, &viewer)
    }

    /// Whether the gallery lists `series` for them (no admin override).
    pub(crate) fn gallery_shows(&self, series: &Series) -> bool {
        components::gallery_shows(series, &self.user_id, &self.role_ids)
    }

    /// Whether they created `series`.
    pub(crate) fn owns(&self, series: &Series) -> bool {
        series.creator_id == self.user_id
    }

    /// Whether they may delete days from `series`: an admin always, the
    /// creator unless an admin took the series down (it is read-only then).
    pub(crate) fn can_delete(&self, series: &Series) -> bool {
        self.is_admin || (self.owns(series) && series.state != SeriesState::Revoked)
    }

    /// Their own series that an admin took down. Hidden from them like from
    /// everyone else, but they are told what happened rather than told it
    /// does not exist: nothing is being kept from them.
    pub(crate) fn taken_down_own(&self, series: &Series) -> bool {
        !self.is_admin && self.owns(series) && series.state == SeriesState::Revoked
    }
}

// ---------------------------------------------------------------------------
// Scope and the pure lookup
// ---------------------------------------------------------------------------

/// Which series a command may act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Series the invoker may view.
    Viewable,
    /// Series the gallery shows the invoker: what they may view as a
    /// member. Manage Server does not widen it, because the gallery treats
    /// an admin like anyone else.
    Gallery,
    /// Series the invoker may delete days from.
    Deletable,
    /// Every series in the server, for an admin (Manage Server commands).
    /// Anyone else gets the viewable ones, in case a server lets members
    /// use those commands.
    Any,
}

impl Scope {
    /// Whether `series` is in this scope for `asker`.
    pub(crate) fn admits(self, asker: &Asker, series: &Series) -> bool {
        match self {
            Self::Viewable => asker.can_view(series),
            Self::Gallery => asker.gallery_shows(series),
            Self::Deletable => asker.can_delete(series),
            Self::Any => asker.is_admin || asker.can_view(series),
        }
    }
}

/// How a typed (or left out) series name resolves for one asker.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Lookup<'a> {
    /// One series: go ahead.
    Found(&'a Series),
    /// Several fit, or none was named and there are several: ask.
    Choose(Vec<&'a Series>),
    /// The asker's own series, taken down by an admin.
    TakenDown(&'a Series),
    /// A series the asker can see but may not change (deleting only).
    NotTheirs(&'a Series),
    /// Nothing fits. Holds every series in scope, for the reply.
    Missing(Vec<&'a Series>),
}

/// Resolves `input` against `all` (every series in the server) for `asker`.
/// Only series in `scope` can be found, chosen or listed; the two
/// exceptions name a series the asker already knows about (their own taken
/// down one, or one they can see but not change), so nothing leaks.
///
/// A whole name (exact, or the same in another case) is taken at its word
/// before any part of a name is tried: `/delete series:Daily` answers "only
/// the creator" when "Daily" is someone else's, rather than moving on to
/// the asker's own "Daily Johan". Part of a name prefers what is in scope.
pub(crate) fn lookup<'a>(
    all: &'a [Series],
    asker: &Asker,
    scope: Scope,
    input: Option<&str>,
) -> Lookup<'a> {
    let in_scope: Vec<&Series> = all.iter().filter(|s| scope.admits(asker, s)).collect();
    let taken_down = || all.iter().filter(|s| asker.taken_down_own(s));
    // Visible but not the asker's to change: only deleting tells these apart.
    let not_theirs =
        |s: &Series| scope == Scope::Deletable && asker.can_view(s) && !asker.can_delete(s);

    let Some(input) = input.map(str::trim).filter(|i| !i.is_empty()) else {
        return match in_scope.as_slice() {
            [one] => Lookup::Found(one),
            [] => taken_down()
                .next()
                .map_or(Lookup::Missing(in_scope), Lookup::TakenDown),
            _ => Lookup::Choose(in_scope),
        };
    };

    // Everything the asker knows about, in or out of scope.
    let known = all
        .iter()
        .filter(|s| scope.admits(asker, s) || asker.taken_down_own(s) || not_theirs(s));
    let key = series_ops::fold_name(input);
    let whole: Vec<&Series> = match series_ops::match_series_name(known, input) {
        NameMatch::Found(series) => vec![series],
        NameMatch::Ambiguous(candidates) => candidates,
        NameMatch::NotFound => Vec::new(),
    }
    .into_iter()
    .filter(|s| series_ops::fold_name(&s.name) == key)
    .collect();
    if let Some(named) = whole_name(whole, asker, scope) {
        return named;
    }

    match series_ops::match_series_name(in_scope.iter().copied(), input) {
        NameMatch::Found(series) => return Lookup::Found(series),
        NameMatch::Ambiguous(candidates) => return Lookup::Choose(candidates),
        NameMatch::NotFound => {}
    }
    if let Some(series) = series_ops::match_series_name(taken_down(), input).found() {
        return Lookup::TakenDown(series);
    }
    let visible = all.iter().filter(|s| not_theirs(s));
    if let Some(series) = series_ops::match_series_name(visible, input).found() {
        return Lookup::NotTheirs(series);
    }
    Lookup::Missing(in_scope)
}

/// What a typed whole name resolves to. `whole` holds the series the asker
/// knows about that carry that name (several only when names differ by case
/// alone); `None` when there is none. Those in scope win; otherwise the
/// name is the asker's own taken-down series, or one that is not theirs.
fn whole_name<'a>(whole: Vec<&'a Series>, asker: &Asker, scope: Scope) -> Option<Lookup<'a>> {
    let (admitted, rest): (Vec<&Series>, Vec<&Series>) =
        whole.into_iter().partition(|s| scope.admits(asker, s));
    match admitted.as_slice() {
        [] => {}
        [one] => return Some(Lookup::Found(one)),
        _ => return Some(Lookup::Choose(admitted)),
    }
    rest.iter()
        .copied()
        .find(|s| asker.taken_down_own(s))
        .map(Lookup::TakenDown)
        .or_else(|| rest.first().copied().map(Lookup::NotTheirs))
}

/// Orders series for a list or a select: the asker's own first, then the
/// most recently posted (`recency` maps series id to its latest
/// `posted_at`; series with no posts go last), then by name.
pub(crate) fn rank<'a>(
    mut series: Vec<&'a Series>,
    asker: &Asker,
    recency: &HashMap<i64, i64>,
) -> Vec<&'a Series> {
    series.sort_by_cached_key(|s| {
        (
            !asker.owns(s),
            Reverse(recency.get(&s.id).copied()),
            series_ops::fold_name(&s.name),
        )
    });
    series
}

/// What autocomplete may offer: the series in scope whose name contains
/// what was typed (any case, any script).
pub(crate) fn matching<'a>(
    all: &'a [Series],
    asker: &Asker,
    scope: Scope,
    partial: &str,
) -> Vec<&'a Series> {
    let needle = series_ops::fold_name(partial);
    all.iter()
        .filter(|s| scope.admits(asker, s))
        .filter(|s| series_ops::fold_name(&s.name).contains(&needle))
        .collect()
}

/// Autocomplete choices from the [`matching`] series: best first, capped at
/// Discord's 25.
pub(crate) fn choices(
    matching: Vec<&Series>,
    asker: &Asker,
    recency: &HashMap<i64, i64>,
) -> Vec<String> {
    rank(matching, asker, recency)
        .into_iter()
        .take(CHOICES_MAX)
        .map(|s| s.name.clone())
        .collect()
}

/// The latest `posted_at` of each series, for [`rank`]. One small query
/// per series; a series whose query fails just ranks as if empty.
async fn recency(data: &Data, series: &[&Series]) -> HashMap<i64, i64> {
    let mut latest = HashMap::with_capacity(series.len());
    for s in series {
        match data.posts.latest_posted_at(s.id).await {
            Ok(Some(at)) => {
                latest.insert(s.id, at);
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(series = s.id, error = %e, "could not read a series' latest post");
            }
        }
    }
    latest
}

// ---------------------------------------------------------------------------
// Autocomplete
// ---------------------------------------------------------------------------

/// Autocomplete for commands that read a series: only the series the
/// invoker may view, their own first.
pub async fn autocomplete_viewable_series(ctx: Context<'_>, partial: &str) -> Vec<String> {
    suggestions(ctx, partial, Scope::Viewable).await
}

/// Autocomplete for `/gallery`: only the series the gallery will show the
/// invoker, so it never suggests one it then cannot open.
pub async fn autocomplete_gallery_series(ctx: Context<'_>, partial: &str) -> Vec<String> {
    suggestions(ctx, partial, Scope::Gallery).await
}

/// Autocomplete for `/delete`: the invoker's own series (not ones an admin
/// took down), or every series for an admin.
pub async fn autocomplete_owned_series(ctx: Context<'_>, partial: &str) -> Vec<String> {
    suggestions(ctx, partial, Scope::Deletable).await
}

/// Autocomplete for the Manage Server commands: every series in the server
/// for an admin, the viewable ones for anyone else.
pub async fn autocomplete_any_series(ctx: Context<'_>, partial: &str) -> Vec<String> {
    suggestions(ctx, partial, Scope::Any).await
}

async fn suggestions(ctx: Context<'_>, partial: &str, scope: Scope) -> Vec<String> {
    let Some(guild_id) = ctx.guild_id() else {
        return Vec::new();
    };
    let all = match ctx.data().series.list_by_guild(&guild_id.to_string()).await {
        Ok(all) => all,
        Err(e) => {
            tracing::warn!(error = %e, "series autocomplete could not list series");
            return Vec::new();
        }
    };
    let asker = Asker::of(&ctx).await;
    // Recency only for what can be suggested: one query per series.
    let matching = matching(&all, &asker, scope, partial);
    let latest = recency(ctx.data(), &matching).await;
    choices(matching, &asker, &latest)
}

// ---------------------------------------------------------------------------
// Settling the series for a command
// ---------------------------------------------------------------------------

/// Resolves the command's series option within `scope`, asking with a
/// select when several fit. `None` means the invoker has been answered
/// already (nothing fits, not theirs, taken down, or the prompt expired).
///
/// Must run before anything else is sent: the select, when there is one,
/// is the command's first response.
pub(crate) async fn settle<'a>(
    app: App<'a>,
    guild_id: &str,
    input: Option<&str>,
    scope: Scope,
    asker: &Asker,
) -> Result<Option<(Series, Via<'a>)>, Error> {
    let data = app.data;
    let all = data.series.list_by_guild(guild_id).await?;
    let mut via = Via::command(app);
    let candidates = match lookup(&all, asker, scope, input) {
        Lookup::Found(series) => return Ok(Some((series.clone(), via))),
        Lookup::Choose(candidates) => candidates,
        Lookup::TakenDown(series) => {
            via.send(Answer::private(taken_down_text(series))).await?;
            return Ok(None);
        }
        Lookup::NotTheirs(series) => {
            via.send(Answer::private(not_theirs_text(series))).await?;
            return Ok(None);
        }
        Lookup::Missing(known) => {
            let known = rank(known.clone(), asker, &recency(data, &known).await);
            let mut answer = Answer::private(missing_text(input, &known, scope, asker.is_admin));
            if known.is_empty() && matches!(scope, Scope::Viewable | Scope::Gallery) {
                answer =
                    answer.button(open_gallery_button(None).style(serenity::ButtonStyle::Primary));
            }
            via.send(answer).await?;
            return Ok(None);
        }
    };

    let ranked = rank(candidates.clone(), asker, &recency(data, &candidates).await);
    let options = ranked
        .iter()
        .take(CHOICES_MAX)
        .map(|s| {
            let option = serenity::CreateSelectMenuOption::new(
                clip(&s.name, OPTION_LABEL_MAX_CHARS),
                s.id.to_string(),
            );
            if asker.owns(s) {
                option.description("Your series")
            } else {
                option
            }
        })
        .collect();
    let prompt = picker_text(input, ranked.len());
    let Some((value, mut via)) =
        select_one(via, "pick", prompt, options, "Choose a series").await?
    else {
        return Ok(None);
    };

    // Re-read it: the list is a few minutes old by now.
    let chosen = match value.parse::<i64>() {
        Ok(id) if ranked.iter().any(|s| s.id == id) => data.series.get(id).await?,
        _ => None,
    };
    match chosen {
        Some(series) if scope.admits(asker, &series) => Ok(Some((series, via))),
        _ => {
            via.send(Answer::private(PICK_GONE)).await?;
            Ok(None)
        }
    }
}

/// Resolves a typed series name within `scope` without asking: a miss or
/// an ambiguity is answered (privately) with the names that fit, and gives
/// `None`. For commands whose flow has no room for a select.
pub async fn resolve_series(
    ctx: &Context<'_>,
    guild_id: &str,
    name: &str,
    scope: Scope,
) -> Result<Option<Series>, Error> {
    let asker = Asker::of(ctx).await;
    let all = ctx.data().series.list_by_guild(guild_id).await?;
    let text = match lookup(&all, &asker, scope, Some(name)) {
        Lookup::Found(series) => return Ok(Some(series.clone())),
        Lookup::Choose(candidates) => format!(
            "🍃 More than one series matches {}: {}. Type more of the name.",
            echo(name),
            name_list(&candidates, NAMES_SHOWN_MAX)
        ),
        Lookup::TakenDown(series) => taken_down_text(series),
        Lookup::NotTheirs(series) => not_theirs_text(series),
        Lookup::Missing(known) => {
            let known = rank(known.clone(), &asker, &recency(ctx.data(), &known).await);
            missing_text(Some(name), &known, scope, asker.is_admin)
        }
    };
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true))
        .await?;
    Ok(None)
}

/// Fetches a series by name iff the invoker may delete its days.
///
/// That is: they made it (and an admin has not taken it down), or they are
/// an admin. The name is matched forgivingly; a miss is answered with the
/// same reply whether the series is hidden or absent.
pub async fn owned_series(
    ctx: &Context<'_>,
    guild_id: &str,
    name: &str,
) -> Result<Option<Series>, Error> {
    resolve_series(ctx, guild_id, name, Scope::Deletable).await
}

/// Shows a one-choice select and waits up to [`PROMPT_TIMEOUT`] for the
/// invoker's pick. Gives the chosen value and a [`Via`] on that press, or
/// `None` when the prompt expired (it then says so). `kind` names the
/// prompt in its component id.
pub(crate) async fn select_one<'a>(
    mut via: Via<'a>,
    kind: &str,
    content: String,
    options: Vec<serenity::CreateSelectMenuOption>,
    placeholder: &str,
) -> Result<Option<(String, Via<'a>)>, Error> {
    let app = via.app;
    let id = scoped_id(kind, &[&app.interaction.id.to_string()]);
    // Listening starts before the prompt is shown, so no press is missed.
    let _listening = components::SESSIONS.listen(&id);
    let presses = serenity::ComponentInteractionCollector::new(&app.serenity_context.shard)
        .author_id(app.interaction.user.id)
        .custom_ids(vec![id.clone()])
        .timeout(PROMPT_TIMEOUT)
        .stream();
    let mut presses = std::pin::pin!(presses);
    let menu =
        serenity::CreateSelectMenu::new(id, serenity::CreateSelectMenuKind::String { options })
            .placeholder(placeholder);
    via.send(Answer {
        content,
        components: vec![serenity::CreateActionRow::SelectMenu(menu)],
        ephemeral: true,
        ..Answer::default()
    })
    .await?;

    let Some(press) = presses.next().await else {
        via.expire(expired_text(app)).await;
        return Ok(None);
    };
    let value = match &press.data.kind {
        serenity::ComponentInteractionDataKind::StringSelect { values } => {
            values.first().cloned().unwrap_or_default()
        }
        _ => String::new(),
    };
    Ok(Some((value, Via::pressed(app, press))))
}

// ---------------------------------------------------------------------------
// Where a command answers
// ---------------------------------------------------------------------------

/// One reply: text, an optional embed with its image, and controls.
#[derive(Debug, Default)]
pub(crate) struct Answer {
    /// Message text; may be empty when there is an embed.
    pub(crate) content: String,
    /// The card, if any.
    pub(crate) embed: Option<serenity::CreateEmbed>,
    /// A file the embed shows through `attachment://`.
    pub(crate) image: Option<serenity::CreateAttachment>,
    /// Buttons and selects.
    pub(crate) components: Vec<serenity::CreateActionRow>,
    /// Only the invoker sees it.
    pub(crate) ephemeral: bool,
}

impl Answer {
    /// Private text, no controls.
    pub(crate) fn private(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            ephemeral: true,
            ..Self::default()
        }
    }

    /// Adds `button` on a row of its own.
    pub(crate) fn button(mut self, button: serenity::CreateButton) -> Self {
        self.components
            .push(serenity::CreateActionRow::Buttons(vec![button]));
        self
    }

    /// As an interaction response message. `update` replaces the message a
    /// component sits on, so its text is always written (even empty) and
    /// its ephemerality is left alone. Every new message carries no
    /// mentions, and replaces any earlier attachments.
    fn message(self, update: bool) -> serenity::CreateInteractionResponseMessage {
        let mut message = serenity::CreateInteractionResponseMessage::new()
            .allowed_mentions(serenity::CreateAllowedMentions::new())
            .embeds(self.embed.into_iter().collect())
            .components(self.components);
        if update || !self.content.is_empty() {
            message = message.content(self.content);
        }
        if !update {
            message = message.ephemeral(self.ephemeral);
        }
        if let Some(image) = self.image {
            message = message.add_file(image);
        }
        message
    }

    /// As an edit of a response already sent: everything is replaced.
    fn edit(self) -> serenity::EditInteractionResponse {
        let edit = serenity::EditInteractionResponse::new()
            .content(self.content)
            .embeds(self.embed.into_iter().collect())
            .components(self.components)
            .allowed_mentions(serenity::CreateAllowedMentions::new());
        match self.image {
            Some(image) => edit.new_attachment(image),
            None => edit.clear_attachments(),
        }
    }

    /// As a follow-up message of the command.
    fn followup(self) -> serenity::CreateInteractionResponseFollowup {
        let mut followup = serenity::CreateInteractionResponseFollowup::new()
            .allowed_mentions(serenity::CreateAllowedMentions::new())
            .embeds(self.embed.into_iter().collect())
            .components(self.components)
            .ephemeral(self.ephemeral);
        if !self.content.is_empty() {
            followup = followup.content(self.content);
        }
        if let Some(image) = self.image {
            followup = followup.add_file(image);
        }
        followup
    }
}

/// How far a press-based [`Via`] has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PressState {
    /// Not answered yet.
    Fresh,
    /// The picker message has been replaced (by a loading line or the
    /// answer); later answers edit it.
    Updated,
    /// Showing a loading line on the picker until a public answer is ready.
    PublicPending,
    /// The public answer went out as this follow-up of the command.
    Public(serenity::MessageId),
}

/// The interaction a command answers on: the command itself, or the select
/// press that settled its series. Tracks what has been sent, so a caller
/// can defer, answer and edit without caring which it is.
///
/// A public answer after a press goes out as a follow-up of the command
/// (so the channel shows it under the command, not under a private picker),
/// and the private picker is then removed.
pub(crate) struct Via<'a> {
    app: App<'a>,
    press: Option<(Box<serenity::ComponentInteraction>, PressState)>,
}

impl<'a> Via<'a> {
    /// Answers on the command itself.
    pub(crate) const fn command(app: App<'a>) -> Self {
        Self { app, press: None }
    }

    /// Answers on a press of a component the command showed.
    pub(crate) fn pressed(app: App<'a>, press: serenity::ComponentInteraction) -> Self {
        // Boxed: the interaction is large and `Via` is moved around a lot.
        Self {
            app,
            press: Some((Box::new(press), PressState::Fresh)),
        }
    }

    /// The command context.
    pub(crate) const fn app(&self) -> App<'a> {
        self.app
    }

    /// The bot's shared state.
    pub(crate) const fn data(&self) -> &'a Data {
        self.app.data
    }

    fn http(&self) -> &'a serenity::Http {
        &self.app.serenity_context.http
    }

    /// Acknowledges now with a loading state, for work that may take more
    /// than Discord's three seconds (storage). The next [`Self::send`]
    /// fills it in. On the command, `ephemeral` is fixed here for good.
    pub(crate) async fn defer(&mut self, ephemeral: bool) -> Result<(), serenity::Error> {
        let http = self.http();
        let Some((press, state)) = &mut self.press else {
            return self.app.defer_response(ephemeral).await;
        };
        if *state != PressState::Fresh {
            return Ok(());
        }
        let loading = serenity::CreateInteractionResponseMessage::new()
            .content("⏳ One moment…")
            .components(Vec::new());
        press
            .create_response(
                http,
                serenity::CreateInteractionResponse::UpdateMessage(loading),
            )
            .await?;
        *state = if ephemeral {
            PressState::Updated
        } else {
            PressState::PublicPending
        };
        Ok(())
    }

    /// Replaces a prompt nobody answered with `text`. Best effort: the
    /// invoker may have dismissed the message, and a failure here must not
    /// turn into an error reply minutes later.
    pub(crate) async fn expire(&mut self, text: String) {
        if let Err(e) = self.send(Answer::private(text)).await {
            tracing::debug!(error = %e, "could not mark a prompt as expired");
        }
    }

    /// Sends `answer`: as the first response, or as the edit of what this
    /// surface showed last (a loading state, a prompt).
    pub(crate) async fn send(&mut self, answer: Answer) -> Result<(), serenity::Error> {
        let http = self.http();
        let app = self.app;
        let Some((press, state)) = &mut self.press else {
            if app.has_sent_initial_response.load(Ordering::SeqCst) {
                app.interaction.edit_response(http, answer.edit()).await?;
            } else {
                let response = serenity::CreateInteractionResponse::Message(answer.message(false));
                app.interaction.create_response(http, response).await?;
                app.has_sent_initial_response.store(true, Ordering::SeqCst);
            }
            return Ok(());
        };
        match *state {
            PressState::Fresh if answer.ephemeral => {
                let response =
                    serenity::CreateInteractionResponse::UpdateMessage(answer.message(true));
                press.create_response(http, response).await?;
                *state = PressState::Updated;
            }
            PressState::Fresh | PressState::PublicPending if !answer.ephemeral => {
                if *state == PressState::Fresh {
                    press
                        .create_response(http, serenity::CreateInteractionResponse::Acknowledge)
                        .await?;
                }
                let sent = app
                    .interaction
                    .create_followup(http, answer.followup())
                    .await?;
                *state = PressState::Public(sent.id);
                drop_picker(app).await;
            }
            PressState::Public(id) => {
                let followup = answer.followup();
                app.interaction.edit_followup(http, id, followup).await?;
            }
            PressState::Fresh | PressState::Updated | PressState::PublicPending => {
                press.edit_response(http, answer.edit()).await?;
                *state = PressState::Updated;
            }
        }
        Ok(())
    }
}

/// Removes the command's private picker once a public answer replaced it.
/// Best effort: if it stays, it only shows a loading line.
async fn drop_picker(app: App<'_>) {
    if let Err(e) = app
        .interaction
        .delete_response(&app.serenity_context.http)
        .await
    {
        tracing::debug!(error = %e, "could not remove the series picker");
    }
}

// ---------------------------------------------------------------------------
// Component ids and buttons
// ---------------------------------------------------------------------------

/// A component id that belongs to this process: `qry:<nonce>:<kind>:<parts>`
/// (the nonce is the one the component router compares against).
pub(crate) fn scoped_id(kind: &str, parts: &[&str]) -> String {
    let mut id = format!("{ID_PREFIX}:{}:{kind}", *NONCE);
    for part in parts {
        id.push(':');
        id.push_str(part);
    }
    id
}

/// An Open gallery button for `target` (series, and day within it).
pub(crate) fn open_gallery_button(target: Option<(i64, Option<i64>)>) -> serenity::CreateButton {
    serenity::CreateButton::new(open_gallery_id(target))
        .label("Open gallery")
        .style(serenity::ButtonStyle::Secondary)
}

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

/// A series name in bold, escaped so it shows exactly as typed.
pub(crate) fn bold(name: &str) -> String {
    format!("**{}**", series_ops::display_name(name))
}

/// `text` cut to `max` characters, with an ellipsis when cut.
pub(crate) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// What the invoker typed, echoed safely.
fn echo(input: &str) -> String {
    bold(&clip(input.trim(), ECHO_MAX_CHARS))
}

/// "A", "A and B", "A, B and C".
pub(crate) fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Up to `max` series names in bold, then "and N more".
fn name_list(series: &[&Series], max: usize) -> String {
    let mut names: Vec<String> = series.iter().take(max).map(|s| bold(&s.name)).collect();
    let rest = series.len().saturating_sub(max);
    if rest > 0 {
        names.push(format!("{rest} more"));
    }
    and_list(&names)
}

/// The reply when nothing in scope fits. `known` is everything in scope,
/// best first: the names the invoker could have meant.
fn missing_text(input: Option<&str>, known: &[&Series], scope: Scope, is_admin: bool) -> String {
    let typed = input.map(str::trim).filter(|i| !i.is_empty());
    let (Some(typed), false) = (typed, known.is_empty()) else {
        return empty_scope_text(scope, is_admin);
    };
    let names = name_list(known, NAMES_SHOWN_MAX);
    if scope == Scope::Deletable && !is_admin {
        format!(
            "🍂 I couldn't find a series of yours called {}. Yours: {names}.",
            echo(typed)
        )
    } else {
        format!(
            "🍂 I couldn't find a series called {}. Series here: {names}.",
            echo(typed)
        )
    }
}

/// The reply when the scope is empty.
fn empty_scope_text(scope: Scope, is_admin: bool) -> String {
    match scope {
        Scope::Viewable | Scope::Gallery => {
            "🌱 There's no series here you can view yet. Open the gallery to start one.".to_owned()
        }
        Scope::Deletable if !is_admin => "🍂 You don't have a series here, so there's nothing \
             of yours to delete. If one of your posts was archived into someone else's series, \
             long-press the post (right-click on desktop), then Apps, then Remove Archive Entry."
            .to_owned(),
        Scope::Deletable | Scope::Any => "🍂 There are no series in this server yet.".to_owned(),
    }
}

/// The creator's own series, taken down by an admin.
pub(crate) fn taken_down_text(series: &Series) -> String {
    format!(
        "🔒 {} was revoked by a server admin, so it's hidden and read-only. Ask an admin if \
         you'd like it restored.",
        bold(&series.name)
    )
}

/// A series the invoker can see but may not delete from.
pub(crate) fn not_theirs_text(series: &Series) -> String {
    format!(
        "🍂 Only the creator of {} or a server admin can delete its days.",
        bold(&series.name)
    )
}

/// The question above the series select.
fn picker_text(input: Option<&str>, total: usize) -> String {
    let question = input.map(str::trim).filter(|i| !i.is_empty()).map_or_else(
        || "🍃 Which series?".to_owned(),
        |typed| {
            format!(
                "🍃 More than one series matches {}. Which one?",
                echo(typed)
            )
        },
    );
    if total > CHOICES_MAX {
        format!(
            "{question}\nShowing {CHOICES_MAX} of {total}. Type more of the name in the series \
             option to find the others."
        )
    } else {
        question
    }
}

/// How to try again after a prompt: the command's own name.
pub(crate) fn rerun_hint(app: App<'_>) -> String {
    app.command.context_menu_name.as_deref().map_or_else(
        || format!("Run `/{}` again.", app.command.qualified_name),
        |menu| format!("Use {menu} on the message again."),
    )
}

/// A prompt nobody answered in time.
fn expired_text(app: App<'_>) -> String {
    format!(
        "⏳ This prompt expired, so nothing happened. {}",
        rerun_hint(app)
    )
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "tests may panic"
    )]

    use leaf_core::domain::{Cadence, DetectionMode, Privacy};

    use super::*;

    fn series(id: i64, name: &str, creator: &str) -> Series {
        Series {
            id,
            guild_id: "g".into(),
            creator_id: creator.into(),
            name: name.into(),
            description: String::new(),
            channels: vec![],
            cadence: Cadence::Daily,
            detection_mode: DetectionMode::ContextMenu,
            privacy: Privacy::Public,
            privacy_role_id: None,
            start_day: 1,
            reminder_enabled: false,
            reminder_time: None,
            reminder_timezone: None,
            reminder_dm: true,
            milestone_template: None,
            emoji: "🍃".into(),
            state: SeriesState::Active,
            created_at: 0,
        }
    }

    fn with(mut s: Series, privacy: Privacy, state: SeriesState) -> Series {
        s.privacy = privacy;
        s.state = state;
        s
    }

    fn member(id: &str) -> Asker {
        Asker {
            user_id: id.into(),
            role_ids: vec![],
            is_admin: false,
        }
    }

    fn admin(id: &str) -> Asker {
        Asker {
            is_admin: true,
            ..member(id)
        }
    }

    /// One server: a public series, a private one, a sprout and a revoked
    /// one by "alice", and a public one by "bob".
    fn server() -> Vec<Series> {
        vec![
            series(1, "Daily Johan", "alice"),
            with(
                series(2, "Secret Diary", "alice"),
                Privacy::CreatorOnly,
                SeriesState::Active,
            ),
            with(
                series(3, "Seedling", "alice"),
                Privacy::Public,
                SeriesState::Sprout,
            ),
            with(
                series(4, "Gone Daily", "alice"),
                Privacy::Public,
                SeriesState::Revoked,
            ),
            series(5, "Bob Draws", "bob"),
        ]
    }

    fn ids(found: &[&Series]) -> Vec<i64> {
        found.iter().map(|s| s.id).collect()
    }

    /// The autocomplete choices for `partial`, as the callbacks build them.
    fn suggest(
        all: &[Series],
        asker: &Asker,
        scope: Scope,
        partial: &str,
        recency: &HashMap<i64, i64>,
    ) -> Vec<String> {
        choices(matching(all, asker, scope, partial), asker, recency)
    }

    #[test]
    fn viewable_scope_never_offers_what_a_member_cannot_see() {
        let all = server();
        let carol = member("carol");
        let none = HashMap::new();
        assert_eq!(
            suggest(&all, &carol, Scope::Viewable, "", &none),
            vec!["Bob Draws", "Daily Johan"]
        );
        // A hidden name typed in full is a plain miss listing only what
        // carol may see, the same reply as a typo.
        let Lookup::Missing(known) = lookup(&all, &carol, Scope::Viewable, Some("Secret Diary"))
        else {
            panic!("a hidden series must not be found");
        };
        assert_eq!(ids(&known), vec![1, 5]);
        let Lookup::Missing(typo) = lookup(&all, &carol, Scope::Viewable, Some("Secrte")) else {
            panic!("a typo is a miss");
        };
        assert_eq!(known, typo);
    }

    #[test]
    fn gallery_scope_offers_an_admin_only_what_the_gallery_shows_them() {
        let all = server();
        let none = HashMap::new();
        // In chat an admin reaches every series...
        let boss = admin("boss");
        assert_eq!(suggest(&all, &boss, Scope::Viewable, "", &none).len(), 5);
        // ...but the gallery treats them as a member, so `/gallery` only
        // suggests what it can open for them.
        assert_eq!(
            suggest(&all, &boss, Scope::Gallery, "", &none),
            vec!["Bob Draws", "Daily Johan"]
        );
        for hidden in [&all[1], &all[2], &all[3]] {
            assert!(boss.can_view(hidden));
            assert!(!boss.gallery_shows(hidden), "{}", hidden.name);
        }
        // A creator still gets their own private and sprout series.
        assert_eq!(
            suggest(&all, &member("alice"), Scope::Gallery, "", &none),
            vec!["Daily Johan", "Secret Diary", "Seedling", "Bob Draws"]
        );
    }

    #[test]
    fn the_creator_sees_their_private_and_sprout_series_first() {
        let all = server();
        let alice = member("alice");
        let none = HashMap::new();
        assert_eq!(
            suggest(&all, &alice, Scope::Viewable, "", &none),
            vec!["Daily Johan", "Secret Diary", "Seedling", "Bob Draws"]
        );
    }

    #[test]
    fn names_match_forgivingly_within_scope() {
        let all = server();
        let carol = member("carol");
        for typed in ["daily johan", "  Daily Johan ", "DAILY", "johan"] {
            assert_eq!(
                lookup(&all, &carol, Scope::Viewable, Some(typed)),
                Lookup::Found(&all[0]),
                "{typed:?}"
            );
        }
        // "daily" would also fit alice's revoked "Gone Daily", but carol
        // cannot see it, so it does not make the match ambiguous.
        let alice = member("alice");
        assert_eq!(
            lookup(&all, &alice, Scope::Viewable, Some("dai")),
            Lookup::Found(&all[0])
        );
    }

    #[test]
    fn non_ascii_names_match_in_any_case() {
        let all = [series(1, "Été", "a"), series(2, "Ежедневник", "a")];
        let viewer = member("z");
        assert_eq!(
            lookup(&all, &viewer, Scope::Viewable, Some("été")),
            Lookup::Found(&all[0])
        );
        assert_eq!(
            lookup(&all, &viewer, Scope::Viewable, Some("ЕЖЕ")),
            Lookup::Found(&all[1])
        );
        assert_eq!(
            suggest(&all, &viewer, Scope::Viewable, "ЕЖЕД", &HashMap::new()),
            vec!["Ежедневник"]
        );
    }

    #[test]
    fn several_fits_are_offered_as_a_choice() {
        let all = [series(1, "Sketch A", "a"), series(2, "Sketch B", "b")];
        let Lookup::Choose(candidates) = lookup(&all, &member("z"), Scope::Viewable, Some("sk"))
        else {
            panic!("two prefixes are ambiguous");
        };
        assert_eq!(ids(&candidates), vec![1, 2]);
    }

    #[test]
    fn a_left_out_name_means_the_only_series_or_a_choice() {
        let all = server();
        // bob may delete from one series only: his own.
        assert_eq!(
            lookup(&all, &member("bob"), Scope::Deletable, None),
            Lookup::Found(&all[4])
        );
        let Lookup::Choose(candidates) = lookup(&all, &member("carol"), Scope::Viewable, None)
        else {
            panic!("two viewable series need a choice");
        };
        assert_eq!(ids(&candidates), vec![1, 5]);
        // Nothing at all to choose from.
        assert_eq!(
            lookup(&all, &member("carol"), Scope::Deletable, Some("   ")),
            Lookup::Missing(vec![])
        );
    }

    #[test]
    fn a_creator_is_told_their_series_was_taken_down() {
        let all = server();
        let alice = member("alice");
        assert_eq!(
            lookup(&all, &alice, Scope::Viewable, Some("gone daily")),
            Lookup::TakenDown(&all[3])
        );
        assert_eq!(
            lookup(&all, &alice, Scope::Deletable, Some("Gone Daily")),
            Lookup::TakenDown(&all[3])
        );
        // Anyone else gets a plain miss.
        assert!(matches!(
            lookup(&all, &member("bob"), Scope::Viewable, Some("Gone Daily")),
            Lookup::Missing(_)
        ));
        // A creator whose only series was taken down, naming none.
        let only = vec![all[3].clone()];
        assert_eq!(
            lookup(&only, &alice, Scope::Viewable, None),
            Lookup::TakenDown(&only[0])
        );
    }

    #[test]
    fn deleting_is_for_the_creator_of_a_live_series_or_an_admin() {
        let all = server();
        let alice = member("alice");
        let none = HashMap::new();
        assert_eq!(
            suggest(&all, &alice, Scope::Deletable, "", &none),
            vec!["Daily Johan", "Secret Diary", "Seedling"]
        );
        // Bob's series is visible to alice but not hers to change.
        assert_eq!(
            lookup(&all, &alice, Scope::Deletable, Some("bob draws")),
            Lookup::NotTheirs(&all[4])
        );
        // carol cannot see alice's private series: a plain miss, not
        // "only the creator can".
        assert_eq!(
            lookup(
                &all,
                &member("carol"),
                Scope::Deletable,
                Some("Secret Diary")
            ),
            Lookup::Missing(vec![])
        );
        // An admin may delete from any series, revoked ones included.
        let boss = admin("dave");
        assert_eq!(
            lookup(&all, &boss, Scope::Deletable, Some("gone daily")),
            Lookup::Found(&all[3])
        );
        assert_eq!(suggest(&all, &boss, Scope::Deletable, "", &none).len(), 5);
    }

    #[test]
    fn a_whole_name_is_taken_at_its_word_before_part_of_one() {
        let all = [
            series(1, "Daily Johan", "alice"),
            series(2, "Daily", "bob"),
            with(
                series(3, "Gone", "alice"),
                Privacy::Public,
                SeriesState::Revoked,
            ),
            series(4, "Gone Fishing", "bob"),
        ];
        let (alice, bob) = (member("alice"), member("bob"));
        // "Daily" is bob's: deleting must not move on to alice's own
        // "Daily Johan", which merely starts with it.
        for typed in ["Daily", "daily", " DAILY "] {
            assert_eq!(
                lookup(&all, &alice, Scope::Deletable, Some(typed)),
                Lookup::NotTheirs(&all[1]),
                "{typed:?}"
            );
        }
        // Part of a name still prefers what she may delete from.
        assert_eq!(
            lookup(&all, &alice, Scope::Deletable, Some("dai")),
            Lookup::Found(&all[0])
        );
        assert_eq!(
            lookup(&all, &bob, Scope::Deletable, Some("daily")),
            Lookup::Found(&all[1])
        );
        // Reading: her taken-down series by its whole name, not the visible
        // one that starts with it.
        assert_eq!(
            lookup(&all, &alice, Scope::Viewable, Some("gone")),
            Lookup::TakenDown(&all[2])
        );
        assert_eq!(
            lookup(&all, &alice, Scope::Viewable, Some("gone f")),
            Lookup::Found(&all[3])
        );
        // To bob the taken-down series does not exist.
        assert_eq!(
            lookup(&all, &bob, Scope::Viewable, Some("gone")),
            Lookup::Found(&all[3])
        );
    }

    #[test]
    fn names_that_differ_by_case_resolve_to_the_one_in_scope() {
        let all = [series(1, "art", "alice"), series(2, "Art", "bob")];
        let alice = member("alice");
        assert_eq!(
            lookup(&all, &alice, Scope::Deletable, Some("ART")),
            Lookup::Found(&all[0])
        );
        // Typed exactly, the name is bob's.
        assert_eq!(
            lookup(&all, &alice, Scope::Deletable, Some("Art")),
            Lookup::NotTheirs(&all[1])
        );
        // Both are viewable, so reading has to ask.
        let Lookup::Choose(candidates) = lookup(&all, &alice, Scope::Viewable, Some("ART")) else {
            panic!("two names that differ by case are ambiguous");
        };
        assert_eq!(ids(&candidates), vec![1, 2]);
    }

    #[test]
    fn the_any_scope_is_everything_only_for_admins() {
        let all = server();
        let none = HashMap::new();
        assert_eq!(
            suggest(&all, &admin("dave"), Scope::Any, "", &none).len(),
            5
        );
        assert_eq!(
            suggest(&all, &member("carol"), Scope::Any, "", &none),
            suggest(&all, &member("carol"), Scope::Viewable, "", &none)
        );
    }

    #[test]
    fn ranking_puts_own_then_recent_then_by_name() {
        let all = [
            series(1, "b-old", "x"),
            series(2, "a-empty", "x"),
            series(3, "c-new", "x"),
            series(4, "z-mine", "me"),
        ];
        let recency = HashMap::from([(1, 100), (3, 300), (4, 1)]);
        let ranked = rank(all.iter().collect(), &member("me"), &recency);
        assert_eq!(ids(&ranked), vec![4, 3, 1, 2]);
    }

    #[test]
    fn suggestions_stop_at_discords_limit() {
        let all: Vec<Series> = (0..40)
            .map(|i| series(i, &format!("Series {i:02}"), "x"))
            .collect();
        let got = suggest(
            &all,
            &member("z"),
            Scope::Viewable,
            "series",
            &HashMap::new(),
        );
        assert_eq!(got.len(), CHOICES_MAX);
        assert_eq!(got.first().map(String::as_str), Some("Series 00"));
    }

    #[test]
    fn a_miss_lists_what_could_have_been_meant() {
        let all = server();
        let known: Vec<&Series> = vec![&all[0], &all[4]];
        assert_eq!(
            missing_text(Some(" daly johan "), &known, Scope::Viewable, false),
            "🍂 I couldn't find a series called **daly johan**. Series here: **Daily Johan** \
             and **Bob Draws**."
        );
        assert_eq!(
            missing_text(Some("x"), &known, Scope::Deletable, false),
            "🍂 I couldn't find a series of yours called **x**. Yours: **Daily Johan** and \
             **Bob Draws**."
        );
        let many: Vec<Series> = (0..13).map(|i| series(i, &format!("S{i}"), "x")).collect();
        let many: Vec<&Series> = many.iter().collect();
        let text = missing_text(Some("x"), &many, Scope::Viewable, false);
        assert!(text.ends_with("**S8**, **S9** and 3 more."), "{text}");
        // Typed markdown is shown literally, and long input is cut.
        let text = missing_text(Some("**x**"), &known, Scope::Viewable, false);
        assert!(text.contains(r"**\*\*x\*\***"), "{text}");
        let long = "y".repeat(80);
        let text = missing_text(Some(&long), &known, Scope::Viewable, false);
        assert!(text.contains(&format!("**{}…**", "y".repeat(ECHO_MAX_CHARS - 1))));
    }

    #[test]
    fn an_empty_scope_says_what_to_do_next() {
        assert!(missing_text(Some("x"), &[], Scope::Viewable, false).contains("Open the gallery"));
        assert!(missing_text(None, &[], Scope::Deletable, false).contains("Remove Archive Entry"));
        assert_eq!(
            missing_text(None, &[], Scope::Deletable, true),
            "🍂 There are no series in this server yet."
        );
    }

    #[test]
    fn lists_read_naturally() {
        let list =
            |items: &[&str]| and_list(&items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        assert_eq!(list(&[]), "");
        assert_eq!(list(&["A"]), "A");
        assert_eq!(list(&["A", "B"]), "A and B");
        assert_eq!(list(&["A", "B", "C"]), "A, B and C");
    }

    #[test]
    fn picker_text_explains_a_long_list() {
        assert_eq!(picker_text(None, 3), "🍃 Which series?");
        assert_eq!(
            picker_text(Some("sk"), 2),
            "🍃 More than one series matches **sk**. Which one?"
        );
        assert!(picker_text(None, 30).contains("Showing 25 of 30"));
    }

    #[test]
    fn component_ids_fit_discords_limit() {
        let scoped = scoped_id("del", &["1234567890123456789", "yes"]);
        assert!(scoped.starts_with("qry:"));
        assert!(scoped.ends_with(":del:1234567890123456789:yes"));
        assert!(scoped.len() <= 100);
        assert_eq!(open_gallery_id(None), "leaf:open");
        assert_eq!(open_gallery_id(Some((7, None))), "leaf:open:7");
        assert_eq!(open_gallery_id(Some((7, Some(42)))), "leaf:open:7:42");
    }

    #[test]
    fn clipping_respects_characters() {
        assert_eq!(clip("Été", 3), "Été");
        assert_eq!(clip("Ежедневник", 4), "Еже…");
    }
}
