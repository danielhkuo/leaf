-- UX audit (docs/ux-audit.md): reminder delivery failures and the
-- chat-to-gallery hand-off. Additive only: two nullable columns and a new
-- table, so it applies to a live database without rewriting rows.

-- Why the last reminder could not be delivered (DMs closed, channel gone,
-- missing permission) and when. NULL while delivery works.
ALTER TABLE series ADD COLUMN reminder_error TEXT;
ALTER TABLE series ADD COLUMN reminder_error_at INTEGER; -- unix seconds

-- One pending "open the gallery here" target per (user, guild). The bot
-- writes it just before answering with LAUNCH_ACTIVITY (which carries no
-- payload); the Activity consumes it on boot. Rows live about two minutes.
-- No foreign key on series_id: the reader re-checks that the series still
-- exists and is viewable, and an expired row is pruned on the next write.
CREATE TABLE launch_intents (
    user_id    TEXT NOT NULL,
    guild_id   TEXT NOT NULL,
    series_id  INTEGER NOT NULL,
    day        INTEGER,          -- NULL opens the series, not a day
    expires_at INTEGER NOT NULL, -- unix seconds
    PRIMARY KEY (user_id, guild_id)
);
