-- Per-series reminder bookkeeping (at-most-once per missing day).
ALTER TABLE series ADD COLUMN last_reminder_day INTEGER;
ALTER TABLE series ADD COLUMN last_reminder_check INTEGER NOT NULL DEFAULT 0;
