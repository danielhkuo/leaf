-- leaf schema v1.
-- Discord snowflakes are stored as TEXT: they exceed i64 comfort and we
-- never do arithmetic on them. Day numbers and timestamps are INTEGER.

CREATE TABLE guild_settings (
    guild_id                TEXT PRIMARY KEY NOT NULL,
    setup_complete          INTEGER NOT NULL DEFAULT 0,
    log_channel_id          TEXT,
    watched_channels        TEXT NOT NULL DEFAULT '[]', -- JSON array of channel ids
    creator_role_id         TEXT,
    timezone                TEXT NOT NULL DEFAULT 'UTC',
    max_series_per_user     INTEGER NOT NULL DEFAULT 3,
    min_account_age_days    INTEGER NOT NULL DEFAULT 0,
    min_membership_age_days INTEGER NOT NULL DEFAULT 0,
    sprout_enabled          INTEGER NOT NULL DEFAULT 0,
    sprout_threshold        INTEGER NOT NULL DEFAULT 3,
    active_persona          TEXT NOT NULL DEFAULT 'default'
);

CREATE TABLE series (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    guild_id          TEXT NOT NULL,
    creator_id        TEXT NOT NULL,
    name              TEXT NOT NULL,
    description       TEXT NOT NULL DEFAULT '',
    channels          TEXT NOT NULL DEFAULT '[]', -- JSON array; subset of guild watched_channels
    cadence           TEXT NOT NULL DEFAULT 'daily'
                      CHECK (cadence IN ('daily', 'weekdays', 'weekly', 'freeform')),
    detection_mode    TEXT NOT NULL DEFAULT 'context_menu'
                      CHECK (detection_mode IN ('context_menu', 'passive')),
    privacy           TEXT NOT NULL DEFAULT 'public'
                      CHECK (privacy IN ('public', 'role_gated', 'creator_only')),
    privacy_role_id   TEXT,
    start_day         INTEGER NOT NULL DEFAULT 1,
    reminder_enabled  INTEGER NOT NULL DEFAULT 0,
    reminder_time     TEXT,            -- 'HH:MM', series-local
    reminder_timezone TEXT,            -- IANA override of the guild timezone
    reminder_dm       INTEGER NOT NULL DEFAULT 1,
    milestone_template TEXT,
    emoji             TEXT NOT NULL DEFAULT '🍃',
    state             TEXT NOT NULL DEFAULT 'active'
                      CHECK (state IN ('sprout', 'active', 'revoked')),
    created_at        INTEGER NOT NULL, -- unix seconds
    UNIQUE (guild_id, name),
    FOREIGN KEY (guild_id) REFERENCES guild_settings (guild_id) ON DELETE CASCADE
);

CREATE INDEX idx_series_guild ON series (guild_id);
CREATE INDEX idx_series_creator ON series (guild_id, creator_id);

CREATE TABLE posts (
    series_id   INTEGER NOT NULL,
    day         INTEGER NOT NULL,
    message_id  TEXT NOT NULL,
    channel_id  TEXT NOT NULL,
    caption     TEXT NOT NULL DEFAULT '',
    posted_at   INTEGER NOT NULL,  -- unix seconds, original message
    archived_at INTEGER NOT NULL,  -- unix seconds
    PRIMARY KEY (series_id, day),
    FOREIGN KEY (series_id) REFERENCES series (id) ON DELETE CASCADE
);

CREATE INDEX idx_posts_message ON posts (message_id);

CREATE TABLE media_attachments (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    series_id     INTEGER NOT NULL,
    day           INTEGER NOT NULL,
    attachment_id TEXT NOT NULL,
    channel_id    TEXT NOT NULL,
    message_id    TEXT NOT NULL,
    content_type  TEXT NOT NULL,
    original_key  TEXT,             -- R2 object key; NULL iff media_missing
    thumb_key     TEXT,
    media_missing INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (series_id, day) REFERENCES posts (series_id, day) ON DELETE CASCADE
);

CREATE INDEX idx_media_post ON media_attachments (series_id, day);
