//! Series name resolution shared by `query`, `transfer`, and `wrapped`.
//!
//! The series lifecycle itself now lives in the leaf Activity; these helpers
//! only look series up by name for the remaining admin/owner tools.

use leaf_core::domain::Series;

use crate::{Context, Error, checks};

/// Fetches a series by name iff the invoker owns it (or is an admin).
pub async fn owned_series(
    ctx: &Context<'_>,
    guild_id: &str,
    name: &str,
) -> Result<Option<Series>, Error> {
    let found = ctx.data().series.get_by_name(guild_id, name).await?;
    let Some(s) = found else {
        ctx.send(
            poise::CreateReply::default()
                .content(format!("No series named **{name}** here."))
                .ephemeral(true),
        )
        .await?;
        return Ok(None);
    };
    if s.creator_id != ctx.author().id.to_string() && !checks::is_admin(ctx) {
        ctx.send(
            poise::CreateReply::default()
                .content("🍂 Only the series creator (or an admin) can do that.")
                .ephemeral(true),
        )
        .await?;
        return Ok(None);
    }
    Ok(Some(s))
}

/// Autocomplete: every series name in the guild (admin commands).
pub async fn autocomplete_any_series(ctx: Context<'_>, partial: &str) -> Vec<String> {
    let Some(gid) = ctx.guild_id().map(|g| g.to_string()) else {
        return Vec::new();
    };
    let needle = partial.to_ascii_lowercase();
    ctx.data().series.list_by_guild(&gid).await.map_or_else(
        |_| Vec::new(),
        |list| {
            list.into_iter()
                .map(|s| s.name)
                .filter(|n| n.to_ascii_lowercase().contains(&needle))
                .take(25)
                .collect()
        },
    )
}
