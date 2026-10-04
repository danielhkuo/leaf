//! Checks 6 and 7: the database and its migrations.
//!
//! Check 6 opens the configured database read-only and compares the
//! migrations recorded in it with the ones built into this binary. Check 7
//! (`--db-copy`) copies a database file into a temporary directory and
//! applies the migrations to the copy, the way leaf's start would. Neither
//! writes to the file it is given.
//!
//! Check 6 can leave one trace beside the database, and only when there is a
//! `-wal` file (leaf is running, or was stopped uncleanly): `SQLite` cannot
//! read a write-ahead log without its index, so the read-only connection
//! creates the `-shm` file when it is missing and rewrites it otherwise. The
//! database file and the log themselves are not written (see
//! [`open_read_only`]).
//!
//! The SQL here is raw, not `sqlx::query!`: it reads sqlx's own migration
//! table and the pragmas of `SQLite`, and it has to run against a database of any
//! schema version (older, newer, or not leaf's at all), which a statement
//! compile-checked against today's schema cannot promise. Every statement is
//! run by the tests below.

use std::path::{Path, PathBuf};

use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{ConnectOptions as _, Connection as _, Row as _};

use super::report::{Check, Finding, Redactor};

/// The migrations built into this binary: the same directory `leaf-core`
/// embeds for `leaf_core::db::connect`.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// Most migration versions listed in one finding.
const VERSIONS_LISTED_MAX: usize = 8;

const RESTORE_BACKUP: &str = "Restore the database from a backup (guide/01-install.md, “Data, \
    backups, and updates”).";
const COPY_AGAIN: &str = "If leaf was running while the file was copied, the copy may have \
    caught a write half-way: stop leaf and run this again. If it fails on a file nobody was \
    writing, do not upgrade this install; keep a backup and report it.";

/// A migration built into this leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embedded {
    /// Its number (`0003` is 3).
    pub version: i64,
    /// Its name, from the file name.
    pub description: String,
    /// The hash of its SQL, as sqlx records it when applying it.
    pub checksum: Vec<u8>,
}

/// The migrations built into this binary, in order.
pub fn embedded() -> Vec<Embedded> {
    MIGRATOR
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
        .map(|m| Embedded {
            version: m.version,
            description: m.description.to_string(),
            checksum: m.checksum.to_vec(),
        })
        .collect()
}

/// A migration recorded in a database.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Applied {
    version: i64,
    /// False when it was started and did not finish.
    success: bool,
    checksum: Vec<u8>,
}

/// How many rows the tables that hold an archive have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counts {
    series: i64,
    posts: i64,
    media: i64,
}

/// `path` with `suffix` added to its file name: the `-wal` and `-shm` files
/// of a database sit beside it under such names.
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// The file's name, for a sentence.
fn file_name(path: &Path, redactor: &Redactor) -> String {
    redactor.quoted(&path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    ))
}

/// Opens a database without the means to change it.
///
/// A WAL database with nobody connected has no `-wal` file, and `SQLite`
/// would create one (and its `-shm` index) just to read it. `immutable`
/// reads the file as it stands and creates nothing. With a `-wal` file
/// present the newest commits may be in it, so the reader goes through it
/// as an ordinary read-only connection.
///
/// Read-only is what matters there. A connection that may write merges the
/// log into the database and deletes it when it is the last to close, and
/// after an unclean stop the doctor is the only connection there is. A
/// read-only one leaves both files as they are. What it does write is the
/// log's index, the `-shm` file: `SQLite` creates it when it is missing and
/// rewrites it otherwise.
async fn open_read_only(path: &Path) -> Result<SqliteConnection, sqlx::Error> {
    let in_use = sidecar(path, "-wal").exists();
    SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .immutable(!in_use)
        .connect()
        .await
}

/// Closes a connection; a failure to is only worth a log line.
async fn close(conn: SqliteConnection) {
    if let Err(e) = conn.close().await {
        tracing::debug!(error = %e, "doctor: closing a database connection failed");
    }
}

/// Whether the database has a table of this name.
async fn has_table(conn: &mut SqliteConnection, table: &str) -> Result<bool, sqlx::Error> {
    let found: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(table)
            .fetch_one(conn)
            .await?;
    Ok(found > 0)
}

/// The migrations recorded in the database, or `None` when it has no
/// migration record (it is not a database leaf set up).
async fn applied(conn: &mut SqliteConnection) -> Result<Option<Vec<Applied>>, sqlx::Error> {
    if !has_table(conn, "_sqlx_migrations").await? {
        return Ok(None);
    }
    let rows: Vec<(i64, bool, Vec<u8>)> =
        sqlx::query_as("SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(conn)
            .await?;
    Ok(Some(
        rows.into_iter()
            .map(|(version, success, checksum)| Applied {
                version,
                success,
                checksum,
            })
            .collect(),
    ))
}

/// The row counts, or `None` when the tables are not there yet.
async fn counts(conn: &mut SqliteConnection) -> Result<Option<Counts>, sqlx::Error> {
    for table in ["series", "posts", "media_attachments"] {
        if !has_table(conn, table).await? {
            return Ok(None);
        }
    }
    let series = sqlx::query_scalar("SELECT COUNT(*) FROM series")
        .fetch_one(&mut *conn)
        .await?;
    let posts = sqlx::query_scalar("SELECT COUNT(*) FROM posts")
        .fetch_one(&mut *conn)
        .await?;
    let media = sqlx::query_scalar("SELECT COUNT(*) FROM media_attachments")
        .fetch_one(&mut *conn)
        .await?;
    Ok(Some(Counts {
        series,
        posts,
        media,
    }))
}

/// Some migrations by number and name: the first few, then how many more.
/// The names are this binary's own (its migration files'), never a
/// database's.
fn versions(list: &[(i64, Option<&str>)]) -> String {
    let mut names: Vec<String> = list
        .iter()
        .take(VERSIONS_LISTED_MAX)
        .map(|(version, description)| {
            description.map_or_else(
                || version.to_string(),
                |description| format!("{version} ({description})"),
            )
        })
        .collect();
    if list.len() > VERSIONS_LISTED_MAX {
        names.push(format!("and {} more", list.len() - VERSIONS_LISTED_MAX));
    }
    names.join(", ")
}

/// `n migration` or `n migrations`.
fn migrations(n: usize) -> String {
    if n == 1 {
        "1 migration".to_owned()
    } else {
        format!("{n} migrations")
    }
}

/// The counts as a phrase: `1 series, 2 posts and 3 media files`.
fn rows(counts: Counts) -> String {
    let some = |n: i64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    format!(
        "{} series, {} and {}",
        counts.series,
        some(counts.posts, "post", "posts"),
        some(counts.media, "media file", "media files")
    )
}

/// Check 6: the configured database opens read-only and holds exactly the
/// migrations this binary has.
pub async fn check(db_path: &Path, embedded: &[Embedded], redactor: &Redactor) -> Vec<Finding> {
    let name = file_name(db_path, redactor);
    if !db_path.is_file() {
        return vec![Finding::fail(
            Check::Database,
            format!(
                "There is no {name} in {}: leaf has not run here yet, or DATA_DIR points \
                 somewhere else.",
                redactor.quoted(&db_path.parent().unwrap_or(db_path).display().to_string())
            ),
            "Start leaf once (it creates the database on its first start after setup), or point \
             DATA_DIR at the directory that holds it.",
        )];
    }
    let read = async {
        let mut conn = open_read_only(db_path).await?;
        let applied = applied(&mut conn).await;
        close(conn).await;
        applied
    };
    match read.await {
        Ok(Some(applied)) => compare(&name, &applied, embedded),
        Ok(None) => vec![Finding::fail(
            Check::Database,
            format!(
                "{name} is an SQLite database, but not one leaf has set up: it has no record of migrations."
            ),
            "Point DATA_DIR at leaf's data directory, or restore leaf.db from a backup.",
        )],
        Err(e) => vec![Finding::fail(
            Check::Database,
            format!(
                "{name} could not be opened read-only ({}).",
                redactor.quoted(&e.to_string())
            ),
            "Check that the user running the doctor may read the file, and that it is leaf's \
             database.",
        )],
    }
}

/// Compares the migrations in a database with the ones built in.
fn compare(name: &str, applied: &[Applied], embedded: &[Embedded]) -> Vec<Finding> {
    let known = |version: i64| embedded.iter().find(|m| m.version == version);
    let described = |version: i64| (version, known(version).map(|m| m.description.as_str()));

    let unfinished: Vec<_> = applied
        .iter()
        .filter(|m| !m.success)
        .map(|m| described(m.version))
        .collect();
    let unknown: Vec<_> = applied
        .iter()
        .filter(|m| known(m.version).is_none())
        .map(|m| (m.version, None))
        .collect();
    let changed: Vec<_> = applied
        .iter()
        .filter(|m| known(m.version).is_some_and(|k| k.checksum != m.checksum))
        .map(|m| described(m.version))
        .collect();
    let pending: Vec<_> = embedded
        .iter()
        .filter(|m| !applied.iter().any(|a| a.version == m.version))
        .map(|m| (m.version, Some(m.description.as_str())))
        .collect();

    let mut findings = Vec::new();
    if !unfinished.is_empty() {
        findings.push(Finding::fail(
            Check::Database,
            format!(
                "{name} has {} that started and did not finish ({}); leaf refuses to start on \
                 it.",
                migrations(unfinished.len()),
                versions(&unfinished)
            ),
            RESTORE_BACKUP,
        ));
    }
    if !unknown.is_empty() {
        findings.push(Finding::fail(
            Check::Database,
            format!(
                "{name} has {} this leaf does not know ({}): a newer leaf has used it, and this \
                 one refuses to start on it.",
                migrations(unknown.len()),
                versions(&unknown)
            ),
            "Run the newer leaf again (git pull && docker compose up -d --build), or restore a \
             backup from before the upgrade.",
        ));
    }
    if !changed.is_empty() {
        findings.push(Finding::fail(
            Check::Database,
            format!(
                "{name} recorded {} whose content differs from the one built into this leaf \
                 ({}); leaf refuses to start on it.",
                migrations(changed.len()),
                versions(&changed)
            ),
            "Run the leaf build that created this database, or restore a backup made with this \
             one.",
        ));
    }
    if !pending.is_empty() {
        findings.push(Finding::warn(
            Check::Database,
            format!(
                "{name} does not have {} yet ({}); leaf applies them at its next start.",
                migrations(pending.len()),
                versions(&pending)
            ),
            "Try them on a copy first: leaf doctor --db-copy <path to the database or a copy of \
             it>.",
        ));
    }
    if findings.is_empty() {
        findings.push(Finding::ok(
            Check::Database,
            format!(
                "{name} opens read-only, and its {} are exactly the ones this leaf has.",
                migrations(applied.len())
            ),
        ));
    }
    findings
}

/// The attachment id of the newest media file the database says is stored,
/// for the `--url` check to ask a running leaf for. `Err` is a sentence
/// saying why the database could not be asked.
pub async fn stored_attachment(
    db_path: &Path,
    redactor: &Redactor,
) -> Result<Option<String>, String> {
    if !db_path.is_file() {
        return Err(format!("there is no {}", file_name(db_path, redactor)));
    }
    let read = async {
        let mut conn = open_read_only(db_path).await?;
        let found = if has_table(&mut conn, "media_attachments").await? {
            sqlx::query_scalar(
                "SELECT attachment_id FROM media_attachments \
                 WHERE original_key IS NOT NULL ORDER BY id DESC LIMIT 1",
            )
            .fetch_optional(&mut conn)
            .await
        } else {
            Ok(None)
        };
        close(conn).await;
        found
    };
    read.await.map_err(|e: sqlx::Error| {
        format!(
            "{} could not be read ({})",
            file_name(db_path, redactor),
            redactor.quoted(&e.to_string())
        )
    })
}

/// What the copy looked like before the migrations ran.
#[derive(Debug)]
struct Before {
    applied: Vec<i64>,
    counts: Option<Counts>,
}

/// What it looked like after.
#[derive(Debug)]
struct After {
    applied: Vec<i64>,
    counts: Option<Counts>,
    /// Lines of `PRAGMA integrity_check`: a single `ok` when all is well.
    integrity: Vec<String>,
    /// Tables named by `PRAGMA foreign_key_check`, one per broken row.
    orphans: Vec<String>,
}

/// Copies `source` and the `-wal` and `-shm` files beside it into `dir`.
/// Only ever reads the originals.
async fn copy_into(source: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    let name = source.file_name().unwrap_or(source.as_os_str());
    let copy = dir.join(name);
    tokio::fs::copy(source, &copy).await?;
    for suffix in ["-wal", "-shm"] {
        let beside = sidecar(source, suffix);
        if beside.is_file() {
            tokio::fs::copy(&beside, sidecar(&copy, suffix)).await?;
        }
    }
    Ok(copy)
}

/// Reads the copy as it was copied.
async fn before(copy: &Path) -> Result<Before, sqlx::Error> {
    let mut conn = SqliteConnectOptions::new().filename(copy).connect().await?;
    let read = async {
        let applied = applied(&mut conn).await?.unwrap_or_default();
        let counts = counts(&mut conn).await?;
        Ok(Before {
            applied: applied.iter().map(|m| m.version).collect(),
            counts,
        })
    }
    .await;
    close(conn).await;
    read
}

/// Reads the copy once the migrations have run.
async fn after(pool: &leaf_core::db::SqlitePool) -> Result<After, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    let applied = applied(&mut conn).await?.unwrap_or_default();
    let counts = counts(&mut conn).await?;
    let integrity = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&mut *conn)
        .await?;
    let orphans = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(|row| row.try_get::<String, _>(0))
        .collect::<Result<_, _>>()?;
    Ok(After {
        applied: applied.iter().map(|m| m.version).collect(),
        counts,
        integrity,
        orphans,
    })
}

/// Check 7 (`--db-copy <path>`): the migrations apply to a copy of this
/// database, the migrated copy is sound, and no rows went missing.
///
/// The copy lives in a temporary directory that is removed afterwards. The
/// migrations are applied by `leaf_core::db::connect`, the call leaf's start
/// makes, so this is a rehearsal of exactly that.
pub async fn check_copy(source: &Path, redactor: &Redactor) -> Vec<Finding> {
    let name = file_name(source, redactor);
    let fail = |message: String, next: &str| vec![Finding::fail(Check::DbCopy, message, next)];
    if !source.is_file() {
        return fail(
            format!(
                "There is no file at {}.",
                redactor.quoted(&source.display().to_string())
            ),
            "Give --db-copy the path of leaf.db or of a copy of it.",
        );
    }
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(e) => {
            return fail(
                format!(
                    "A temporary directory for the copy could not be made ({}).",
                    e.kind()
                ),
                "Check that the temporary directory (TMPDIR) is writable and has room.",
            );
        }
    };
    let copy = match copy_into(source, dir.path()).await {
        Ok(copy) => copy,
        Err(e) => {
            return fail(
                format!("{name} could not be copied ({}).", e.kind()),
                "Check that the file is readable and that the temporary directory (TMPDIR) has \
                 room for a copy.",
            );
        }
    };
    let before = match before(&copy).await {
        Ok(before) => before,
        Err(e) => {
            return fail(
                format!(
                    "The copy of {name} could not be read as an SQLite database ({}).",
                    redactor.quoted(&e.to_string())
                ),
                "Check that the path is leaf.db or a copy of it. If leaf was writing to it \
                 during the copy, run this again.",
            );
        }
    };
    let pool = match leaf_core::db::connect(&copy).await {
        Ok(pool) => pool,
        Err(e) => {
            return fail(
                format!(
                    "The migrations do not apply to a copy of {name} ({}). The original was not \
                     touched.",
                    redactor.quoted(&e.to_string())
                ),
                COPY_AGAIN,
            );
        }
    };
    let after = after(&pool).await;
    pool.close().await;
    let after = match after {
        Ok(after) => after,
        Err(e) => {
            return fail(
                format!(
                    "The migrated copy of {name} could not be checked ({}).",
                    redactor.quoted(&e.to_string())
                ),
                COPY_AGAIN,
            );
        }
    };
    vec![
        migrated(&name, &before, &after),
        soundness(&after, redactor),
        counted(before.counts, after.counts),
    ]
}

/// Which migrations the rehearsal applied.
fn migrated(name: &str, before: &Before, after: &After) -> Finding {
    let embedded = embedded();
    let applied_now: Vec<_> = after
        .applied
        .iter()
        .filter(|version| !before.applied.contains(version))
        .map(|version| {
            let known = embedded.iter().find(|m| m.version == *version);
            (*version, known.map(|m| m.description.as_str()))
        })
        .collect();
    if applied_now.is_empty() {
        return Finding::ok(
            Check::DbCopy,
            format!(
                "A copy of {name} needs no migration: all {} are already applied.",
                after.applied.len()
            ),
        );
    }
    Finding::ok(
        Check::DbCopy,
        format!(
            "The migrations apply to a copy of {name}: {} applied now ({}), {} already there.",
            applied_now.len(),
            versions(&applied_now),
            before.applied.len()
        ),
    )
}

/// What the two pragmas said about the migrated copy.
fn soundness(after: &After, redactor: &Redactor) -> Finding {
    let sound = matches!(after.integrity.as_slice(), [only] if only == "ok");
    if !sound {
        let first = after.integrity.first().map_or("no answer", String::as_str);
        return Finding::fail(
            Check::DbCopy,
            format!(
                "integrity_check on the migrated copy reports a damaged database: {}.",
                redactor.quoted(first)
            ),
            COPY_AGAIN,
        );
    }
    if let Some(table) = after.orphans.first() {
        let rows = match after.orphans.len() {
            1 => "1 row that points at a row which is not there".to_owned(),
            n => format!("{n} rows that point at rows which are not there"),
        };
        return Finding::fail(
            Check::DbCopy,
            format!(
                "foreign_key_check on the migrated copy found {rows} (the first is in table {}).",
                redactor.quoted(table)
            ),
            COPY_AGAIN,
        );
    }
    Finding::ok(
        Check::DbCopy,
        "The migrated copy passes integrity_check and foreign_key_check.",
    )
}

/// Whether the migrations kept every row.
fn counted(before: Option<Counts>, after: Option<Counts>) -> Finding {
    let Some(after) = after else {
        return Finding::fail(
            Check::DbCopy,
            "The migrated copy has no series, posts or media tables.",
            COPY_AGAIN,
        );
    };
    let now = rows(after);
    match before {
        None => Finding::ok(
            Check::DbCopy,
            format!("The copy had no leaf tables before the migrations; after them it has {now}."),
        ),
        Some(before) if before == after => Finding::ok(
            Check::DbCopy,
            format!("The row counts are the same before and after the migrations: {now}."),
        ),
        Some(before) => Finding::fail(
            Check::DbCopy,
            format!(
                "The migrations changed the row counts: series {} → {}, posts {} → {}, media \
                 files {} → {}.",
                before.series, after.series, before.posts, after.posts, before.media, after.media
            ),
            "Do not upgrade this install. Keep a backup of the database and report this.",
        ),
    }
}

/// Databases for the doctor's tests.
///
/// Each is built over one connection, never a pool: closing the one
/// connection is what leaves a database quiet (a single file, no `-wal`),
/// and a pool's `close` can return while one of its connections is still
/// checkpointing, which would change the file under a test that asserts it
/// did not change.
#[cfg(test)]
pub mod fixtures {
    #![allow(clippy::unwrap_used, reason = "test fixtures may panic")]

    use sqlx::sqlite::SqliteJournalMode;

    use super::*;

    /// Creates a database at `path` as leaf does (WAL, every migration)
    /// and leaves the connection open, as a running leaf would.
    pub async fn migrated(path: &Path) -> SqliteConnection {
        let mut conn = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .connect()
            .await
            .unwrap();
        MIGRATOR.run(&mut conn).await.unwrap();
        conn
    }

    /// One series with two days and three media rows: `a1` and `a2` have a
    /// stored file, the newer `a3` has none.
    pub async fn seed(conn: &mut SqliteConnection) {
        for sql in [
            "INSERT INTO guild_settings (guild_id) VALUES ('g1')",
            "INSERT INTO series (id, guild_id, creator_id, name, created_at) \
             VALUES (1, 'g1', 'u1', 'Daily Sketch', 1)",
            "INSERT INTO posts (series_id, day, message_id, channel_id, posted_at, archived_at) \
             VALUES (1, 1, 'm1', 'c1', 1, 1), (1, 2, 'm2', 'c1', 2, 2)",
            "INSERT INTO media_attachments \
             (series_id, day, attachment_id, channel_id, message_id, content_type, original_key) \
             VALUES (1, 1, 'a1', 'c1', 'm1', 'image/png', 'g/g1/s/1/d/1/a1'), \
                    (1, 2, 'a2', 'c1', 'm2', 'video/mp4', 'g/g1/s/1/d/2/a2'), \
                    (1, 2, 'a3', 'c1', 'm2', 'image/png', NULL)",
        ] {
            sqlx::query(sql).execute(&mut *conn).await.unwrap();
        }
    }

    /// A migrated, seeded `leaf.db` in `dir` that nobody has open.
    pub async fn closed(dir: &Path) -> PathBuf {
        let path = dir.join("leaf.db");
        let mut conn = migrated(&path).await;
        seed(&mut conn).await;
        conn.close().await.unwrap();
        path
    }

    /// Every file in `dir` with its bytes: what "nothing changed" means.
    pub fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read(entry.path()).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "tests may panic"
    )]

    use sqlx::migrate::Migrate as _;
    use sqlx::sqlite::SqliteJournalMode;

    use super::fixtures::{closed as closed_db, migrated, seed, snapshot};
    use super::*;
    use crate::doctor::report::Status;
    use crate::doctor::testing::{redactor, statuses};

    fn names(snapshot: &[(String, Vec<u8>)]) -> Vec<&str> {
        snapshot.iter().map(|(name, _)| name.as_str()).collect()
    }

    // ---- check 6 ----

    #[tokio::test]
    async fn an_up_to_date_database_is_ok_and_is_left_exactly_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let path = closed_db(dir.path()).await;
        let was = snapshot(dir.path());
        // A stopped leaf leaves one file: no -wal, no -shm.
        assert_eq!(names(&was), ["leaf.db"]);

        let findings = check(&path, &embedded(), &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Ok]);
        assert_eq!(
            findings[0].message,
            format!(
                "leaf.db opens read-only, and its {} migrations are exactly the ones this leaf \
                 has.",
                embedded().len()
            )
        );
        // Not a byte changed and no file appeared beside it.
        assert_eq!(snapshot(dir.path()), was);
    }

    #[tokio::test]
    async fn a_database_in_use_is_read_through_its_write_ahead_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leaf.db");
        // Held open, as a running leaf holds it: everything the migrations
        // and the seed wrote is still in the -wal file.
        let mut running = migrated(&path).await;
        seed(&mut running).await;
        let main_file = std::fs::read(&path).unwrap();
        let wal = std::fs::read(sidecar(&path, "-wal")).unwrap();
        assert!(wal.len() > main_file.len());

        let findings = check(&path, &embedded(), &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Ok], "{findings:?}");
        assert_eq!(
            stored_attachment(&path, &redactor()).await,
            Ok(Some("a2".to_owned()))
        );
        // The reader moved nothing into the database file and added
        // nothing to the log. (The -shm file is SQLite's bookkeeping of
        // who is reading; it is not compared.)
        assert_eq!(std::fs::read(&path).unwrap(), main_file);
        assert_eq!(std::fs::read(sidecar(&path, "-wal")).unwrap(), wal);
        let mut files = names(&snapshot(dir.path()))
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        files.sort();
        assert_eq!(files, ["leaf.db", "leaf.db-shm", "leaf.db-wal"]);

        // And the running leaf can still write.
        sqlx::query("INSERT INTO guild_settings (guild_id) VALUES ('g2')")
            .execute(&mut running)
            .await
            .unwrap();
        running.close().await.unwrap();
    }

    #[tokio::test]
    async fn a_log_left_by_an_unclean_stop_is_read_and_not_merged() {
        // A leaf that was killed leaves its -wal file behind with nobody
        // holding it, with or without the -shm index beside it. Made by
        // copying the files from under an open connection, as the kill
        // would have left them: closing the connection instead would merge
        // the log and delete it.
        let live = tempfile::tempdir().unwrap();
        let running_path = live.path().join("leaf.db");
        let mut running = migrated(&running_path).await;
        seed(&mut running).await;
        for left_behind in [&["", "-wal"][..], &["", "-wal", "-shm"]] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("leaf.db");
            for suffix in left_behind {
                std::fs::copy(sidecar(&running_path, suffix), sidecar(&path, suffix)).unwrap();
            }
            // The migrations and the rows are in the log, not in the
            // database.
            let main_file = std::fs::read(&path).unwrap();
            let wal = std::fs::read(sidecar(&path, "-wal")).unwrap();
            assert!(wal.len() > main_file.len());

            // Both readers find them there.
            let findings = check(&path, &embedded(), &redactor()).await;
            assert_eq!(statuses(&findings), [Status::Ok], "{findings:?}");
            assert_eq!(
                stored_attachment(&path, &redactor()).await,
                Ok(Some("a2".to_owned()))
            );

            // And the doctor, the only connection there was, did not merge
            // the log into the database or delete it on its way out.
            assert_eq!(std::fs::read(&path).unwrap(), main_file);
            assert!(sidecar(&path, "-wal").exists(), "{left_behind:?}");
            assert_eq!(std::fs::read(sidecar(&path, "-wal")).unwrap(), wal);
            // The one file that may appear is the log's index, which SQLite
            // needs to read the log at all.
            for (name, _) in snapshot(dir.path()) {
                assert!(
                    ["leaf.db", "leaf.db-wal", "leaf.db-shm"].contains(&name.as_str()),
                    "{name}"
                );
            }
        }
        running.close().await.unwrap();
    }

    #[tokio::test]
    async fn a_missing_database_fails_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let findings = check(&dir.path().join("leaf.db"), &embedded(), &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Fail]);
        assert!(findings[0].message.contains("There is no leaf.db"));
        assert!(snapshot(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn a_file_that_is_not_leafs_database_fails() {
        let dir = tempfile::tempdir().unwrap();
        // Not SQLite at all.
        let text = dir.path().join("notes.db");
        std::fs::write(&text, "this is not a database, it is a note".repeat(40)).unwrap();
        let findings = check(&text, &embedded(), &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Fail]);
        assert!(
            findings[0]
                .message
                .contains("could not be opened read-only")
        );

        // SQLite, but nothing leaf made.
        let other = dir.path().join("other.db");
        let mut conn = SqliteConnectOptions::new()
            .filename(&other)
            .create_if_missing(true)
            .connect()
            .await
            .unwrap();
        sqlx::query("CREATE TABLE notes (body TEXT)")
            .execute(&mut conn)
            .await
            .unwrap();
        conn.close().await.unwrap();
        let findings = check(&other, &embedded(), &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Fail]);
        assert!(findings[0].message.contains("no record of migrations"));
    }

    #[tokio::test]
    async fn pending_unknown_and_changed_migrations_are_told_apart() {
        let dir = tempfile::tempdir().unwrap();
        let path = closed_db(dir.path()).await;
        let built_in = embedded();
        let newest = built_in.last().unwrap().clone();

        // This binary has a migration the database has not: pending.
        let mut newer = built_in.clone();
        newer.push(Embedded {
            version: newest.version + 1,
            description: "add things".to_owned(),
            checksum: vec![1, 2, 3],
        });
        let findings = check(&path, &newer, &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Warn]);
        assert_eq!(
            findings[0].message,
            format!(
                "leaf.db does not have 1 migration yet ({} (add things)); leaf applies them at \
                 its next start.",
                newest.version + 1
            )
        );
        assert!(findings[0].next.as_deref().unwrap().contains("--db-copy"));

        // The database has one this binary does not: a newer leaf used it.
        let older = &built_in[..built_in.len() - 1];
        let findings = check(&path, older, &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Fail]);
        assert!(
            findings[0]
                .message
                .contains(&format!("this leaf does not know ({})", newest.version)),
            "{findings:?}"
        );

        // Same number, other content.
        let mut edited = built_in.clone();
        edited.last_mut().unwrap().checksum = vec![9; 48];
        let findings = check(&path, &edited, &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Fail]);
        assert!(findings[0].message.contains("content differs"));

        // Both at once are both reported.
        let mut both = older.to_vec();
        both.push(Embedded {
            version: newest.version + 1,
            description: "add things".to_owned(),
            checksum: vec![1, 2, 3],
        });
        let findings = check(&path, &both, &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Fail, Status::Warn]);
    }

    #[tokio::test]
    async fn a_migration_that_did_not_finish_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = closed_db(dir.path()).await;
        let mut conn = SqliteConnectOptions::new()
            .filename(&path)
            .connect()
            .await
            .unwrap();
        sqlx::query("UPDATE _sqlx_migrations SET success = 0 WHERE version = 2")
            .execute(&mut conn)
            .await
            .unwrap();
        conn.close().await.unwrap();

        let findings = check(&path, &embedded(), &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Fail]);
        assert!(
            findings[0]
                .message
                .contains("started and did not finish (2 (reminder state))"),
            "{findings:?}"
        );
        assert!(findings[0].next.as_deref().unwrap().contains("backup"));
    }

    #[test]
    fn the_embedded_migrations_are_the_migrations_directory() {
        let built_in = embedded();
        let versions: Vec<i64> = built_in.iter().map(|m| m.version).collect();
        let mut on_disk: Vec<i64> =
            std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations"))
                .unwrap()
                .map(|entry| {
                    let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                    name.split('_').next().unwrap().parse().unwrap()
                })
                .collect();
        on_disk.sort_unstable();
        assert_eq!(versions, on_disk);
        assert_eq!(built_in[0].description, "init");
        // SHA-384, as sqlx stores it.
        assert!(built_in.iter().all(|m| m.checksum.len() == 48));
    }

    #[test]
    fn long_migration_lists_are_cut() {
        let many: Vec<(i64, Option<&str>)> = (1..=11).map(|v| (v, None)).collect();
        assert_eq!(versions(&many), "1, 2, 3, 4, 5, 6, 7, 8, and 3 more");
        assert_eq!(versions(&[(3, Some("ux audit"))]), "3 (ux audit)");
        assert_eq!(migrations(1), "1 migration");
        assert_eq!(migrations(3), "3 migrations");
    }

    // ---- the --url check's question ----

    #[tokio::test]
    async fn the_newest_stored_attachment_is_found_and_missing_files_are_passed_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = closed_db(dir.path()).await;
        let was = snapshot(dir.path());
        // a3 is newer but has no stored file.
        assert_eq!(
            stored_attachment(&path, &redactor()).await,
            Ok(Some("a2".to_owned()))
        );
        assert_eq!(snapshot(dir.path()), was);

        let empty = tempfile::tempdir().unwrap();
        let path = empty.path().join("leaf.db");
        migrated(&path).await.close().await.unwrap();
        assert_eq!(stored_attachment(&path, &redactor()).await, Ok(None));

        let missing = stored_attachment(&empty.path().join("nope.db"), &redactor()).await;
        assert_eq!(missing, Err("there is no nope.db".to_owned()));
    }

    // ---- check 7 ----

    /// A database as an older leaf left it: only the first migration.
    async fn old_db(dir: &Path) -> PathBuf {
        let path = dir.join("leaf.db");
        let mut conn = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .connect()
            .await
            .unwrap();
        conn.ensure_migrations_table().await.unwrap();
        let first = MIGRATOR.iter().next().unwrap();
        conn.apply(first).await.unwrap();
        for sql in [
            "INSERT INTO guild_settings (guild_id) VALUES ('g1')",
            "INSERT INTO series (id, guild_id, creator_id, name, created_at) \
             VALUES (1, 'g1', 'u1', 'Daily Sketch', 1)",
            "INSERT INTO posts (series_id, day, message_id, channel_id, posted_at, archived_at) \
             VALUES (1, 1, 'm1', 'c1', 1, 1)",
        ] {
            sqlx::query(sql).execute(&mut conn).await.unwrap();
        }
        conn.close().await.unwrap();
        path
    }

    #[tokio::test]
    async fn an_old_database_is_migrated_as_a_copy_and_the_original_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = old_db(dir.path()).await;
        let was = snapshot(dir.path());

        let findings = check_copy(&path, &redactor()).await;
        assert_eq!(
            statuses(&findings),
            [Status::Ok, Status::Ok, Status::Ok],
            "{findings:?}"
        );
        let pending = embedded().len() - 1;
        assert!(
            findings[0].message.starts_with(&format!(
                "The migrations apply to a copy of leaf.db: {pending} applied now (2 (reminder \
                 state), "
            )),
            "{findings:?}"
        );
        assert!(findings[0].message.ends_with("1 already there."));
        assert_eq!(
            findings[2].message,
            "The row counts are the same before and after the migrations: 1 series, 1 post and \
             0 media files."
        );

        // The original: same bytes, no new files, still one migration.
        assert_eq!(snapshot(dir.path()), was);
        let still = check(&path, &embedded(), &redactor()).await;
        assert_eq!(statuses(&still), [Status::Warn]);
    }

    #[tokio::test]
    async fn an_up_to_date_copy_needs_no_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = closed_db(dir.path()).await;
        let was = snapshot(dir.path());
        let findings = check_copy(&path, &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Ok, Status::Ok, Status::Ok]);
        assert_eq!(
            findings[0].message,
            format!(
                "A copy of leaf.db needs no migration: all {} are already applied.",
                embedded().len()
            )
        );
        assert_eq!(
            findings[1].message,
            "The migrated copy passes integrity_check and foreign_key_check."
        );
        assert!(
            findings[2]
                .message
                .ends_with("1 series, 2 posts and 3 media files.")
        );
        assert_eq!(snapshot(dir.path()), was);
    }

    #[tokio::test]
    async fn a_database_in_use_is_copied_with_its_write_ahead_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leaf.db");
        let mut running = migrated(&path).await;
        seed(&mut running).await;
        // The tables and the rows are in the -wal file: a copy of leaf.db
        // alone would have none of them.
        let main_file = std::fs::read(&path).unwrap();
        let wal = std::fs::read(sidecar(&path, "-wal")).unwrap();
        assert!(wal.len() > main_file.len());

        let findings = check_copy(&path, &redactor()).await;
        assert_eq!(
            statuses(&findings),
            [Status::Ok, Status::Ok, Status::Ok],
            "{findings:?}"
        );
        assert!(
            findings[2]
                .message
                .ends_with("1 series, 2 posts and 3 media files."),
            "{findings:?}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), main_file);
        assert_eq!(std::fs::read(sidecar(&path, "-wal")).unwrap(), wal);
        running.close().await.unwrap();
    }

    #[tokio::test]
    async fn rows_that_point_at_nothing_fail_the_foreign_key_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = closed_db(dir.path()).await;
        // Written with enforcement off, as a damaged import might have.
        let mut conn = SqliteConnectOptions::new()
            .filename(&path)
            .foreign_keys(false)
            .connect()
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO posts (series_id, day, message_id, channel_id, posted_at, archived_at) \
             VALUES (999, 1, 'm9', 'c1', 1, 1)",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        conn.close().await.unwrap();

        let findings = check_copy(&path, &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Ok, Status::Fail, Status::Ok]);
        assert!(
            findings[1].message.contains(
                "found 1 row that points at a row which is not there (the first is in table posts)"
            ),
            "{findings:?}"
        );
    }

    #[tokio::test]
    async fn an_index_that_disagrees_with_its_table_fails_the_integrity_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = closed_db(dir.path()).await;
        // Damage SQLite itself has to find: the index of posts by message
        // keeps its entries while its definition is pointed at another
        // column, so no row is where the index says it is.
        let mut conn = SqliteConnectOptions::new()
            .filename(&path)
            .connect()
            .await
            .unwrap();
        sqlx::query("PRAGMA writable_schema = ON")
            .execute(&mut conn)
            .await
            .unwrap();
        let redefined = sqlx::query(
            "UPDATE sqlite_master \
             SET sql = 'CREATE INDEX idx_posts_message ON posts (channel_id)' \
             WHERE type = 'index' AND name = 'idx_posts_message'",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        assert_eq!(redefined.rows_affected(), 1);
        conn.close().await.unwrap();

        // The migrations still apply and the rows still count; only the
        // pragma can tell.
        let findings = check_copy(&path, &redactor()).await;
        assert_eq!(
            statuses(&findings),
            [Status::Ok, Status::Fail, Status::Ok],
            "{findings:?}"
        );
        assert!(
            findings[1]
                .message
                .starts_with("integrity_check on the migrated copy reports a damaged database: "),
            "{findings:?}"
        );
        assert!(
            findings[1].message.contains("idx_posts_message"),
            "{findings:?}"
        );
    }

    #[tokio::test]
    async fn a_copy_that_cannot_be_made_or_read_fails() {
        let dir = tempfile::tempdir().unwrap();
        let findings = check_copy(&dir.path().join("nope.db"), &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Fail]);
        assert!(findings[0].message.contains("There is no file at"));

        let text = dir.path().join("notes.db");
        std::fs::write(&text, "this is not a database, it is a note".repeat(40)).unwrap();
        let findings = check_copy(&text, &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Fail]);
        assert!(
            findings[0]
                .message
                .contains("could not be read as an SQLite database"),
            "{findings:?}"
        );
    }

    #[tokio::test]
    async fn a_database_from_a_newer_leaf_does_not_migrate() {
        let dir = tempfile::tempdir().unwrap();
        let path = closed_db(dir.path()).await;
        let mut conn = SqliteConnectOptions::new()
            .filename(&path)
            .connect()
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO _sqlx_migrations \
             (version, description, success, checksum, execution_time) \
             VALUES (9999, 'from the future', 1, x'00', 0)",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        conn.close().await.unwrap();

        let findings = check_copy(&path, &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Fail]);
        assert!(
            findings[0]
                .message
                .starts_with("The migrations do not apply to a copy of leaf.db ("),
            "{findings:?}"
        );
        assert!(findings[0].message.contains("9999"), "{findings:?}");
        assert!(
            findings[0]
                .message
                .ends_with("The original was not touched.")
        );
    }

    #[test]
    fn a_damaged_copy_fails_the_integrity_check() {
        let after = |integrity: &[&str], orphans: &[&str]| After {
            applied: vec![1],
            counts: None,
            integrity: integrity.iter().map(|s| (*s).to_owned()).collect(),
            orphans: orphans.iter().map(|s| (*s).to_owned()).collect(),
        };
        let soundness = |after: &After| soundness(after, &redactor());
        assert_eq!(soundness(&after(&["ok"], &[])).status, Status::Ok);

        let damaged = soundness(&after(&["*** in database main ***\nPage 7: btree"], &[]));
        assert_eq!(damaged.status, Status::Fail);
        assert!(
            damaged
                .message
                .contains("damaged database: *** in database main *** Page 7")
        );

        // No answer is not a pass, and two lines are not "ok".
        assert_eq!(soundness(&after(&[], &[])).status, Status::Fail);
        assert_eq!(soundness(&after(&["ok", "ok"], &[])).status, Status::Fail);

        let orphaned = soundness(&after(&["ok"], &["posts", "posts", "media_attachments"]));
        assert_eq!(orphaned.status, Status::Fail);
        assert!(orphaned.message.contains("found 3 rows"));
    }

    #[test]
    fn row_counts_that_change_fail() {
        let counts = |series, posts, media| {
            Some(Counts {
                series,
                posts,
                media,
            })
        };
        assert_eq!(counted(counts(1, 2, 3), counts(1, 2, 3)).status, Status::Ok);
        let lost = counted(counts(6, 170, 172), counts(6, 168, 172));
        assert_eq!(lost.status, Status::Fail);
        assert!(lost.message.contains("posts 170 → 168"), "{lost:?}");
        // More rows is as wrong as fewer.
        assert_eq!(
            counted(counts(1, 2, 3), counts(1, 2, 4)).status,
            Status::Fail
        );
        // A database that had no tables yet has nothing to lose.
        assert_eq!(counted(None, counts(0, 0, 0)).status, Status::Ok);
        assert_eq!(counted(counts(1, 2, 3), None).status, Status::Fail);
    }
}
