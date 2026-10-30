//! Local state in SQLite.
//!
//! The schema version is in `PRAGMA user_version`. Each migration runs once,
//! in order, inside one transaction. Later milestones add more tables.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use rusqlite::Connection;

/// Migration `i` takes the schema from version `i` to version `i + 1`.
const MIGRATIONS: &[&str] = &[
    // 1: subscriptions and saved playlists. Times are Unix seconds.
    "CREATE TABLE subscriptions (
        channel_id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        handle TEXT,
        subscribed_at INTEGER NOT NULL
    );
    CREATE TABLE saved_playlists (
        playlist_id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        channel_id TEXT,
        channel_name TEXT,
        videos_text TEXT,
        saved_at INTEGER NOT NULL
    );",
];

/// A channel the user follows. The list stays on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    /// `UC...`.
    pub channel_id: String,
    pub name: String,
    pub handle: Option<String>,
    pub subscribed_at: i64,
}

/// A playlist the user saved. reeltty keeps a link to it, not its videos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPlaylist {
    /// `PL...`.
    pub playlist_id: String,
    pub title: String,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    /// "13 videos", when it was saved.
    pub videos_text: Option<String>,
    pub saved_at: i64,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        Self::init(conn, MIGRATIONS)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, MIGRATIONS)
    }

    fn init(mut conn: Connection, migrations: &[&str]) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut conn, migrations)?;
        Ok(Self { conn })
    }

    pub fn schema_version(&self) -> Result<usize> {
        schema_version(&self.conn)
    }

    /// All subscriptions, by name.
    pub fn subscriptions(&self) -> Result<Vec<Subscription>> {
        let mut stmt = self.conn.prepare(
            "SELECT channel_id, name, handle, subscribed_at FROM subscriptions ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Subscription { channel_id: r.get(0)?, name: r.get(1)?, handle: r.get(2)?, subscribed_at: r.get(3)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Adds a subscription, or updates its name and handle.
    pub fn subscribe(&self, s: &Subscription) -> Result<()> {
        self.conn.execute(
            "INSERT INTO subscriptions (channel_id, name, handle, subscribed_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (channel_id) DO UPDATE SET name = excluded.name, handle = excluded.handle",
            (&s.channel_id, &s.name, &s.handle, s.subscribed_at),
        )?;
        Ok(())
    }

    pub fn unsubscribe(&self, channel_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM subscriptions WHERE channel_id = ?1", [channel_id])?;
        Ok(())
    }

    /// All saved playlists, newest first.
    pub fn saved_playlists(&self) -> Result<Vec<SavedPlaylist>> {
        let mut stmt = self.conn.prepare(
            "SELECT playlist_id, title, channel_id, channel_name, videos_text, saved_at
             FROM saved_playlists ORDER BY saved_at DESC, title",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SavedPlaylist {
                playlist_id: r.get(0)?,
                title: r.get(1)?,
                channel_id: r.get(2)?,
                channel_name: r.get(3)?,
                videos_text: r.get(4)?,
                saved_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Saves a playlist, or updates what is known about it.
    pub fn save_playlist(&self, p: &SavedPlaylist) -> Result<()> {
        self.conn.execute(
            "INSERT INTO saved_playlists (playlist_id, title, channel_id, channel_name, videos_text, saved_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (playlist_id) DO UPDATE SET title = excluded.title, channel_id = excluded.channel_id,
                 channel_name = excluded.channel_name, videos_text = excluded.videos_text",
            (&p.playlist_id, &p.title, &p.channel_id, &p.channel_name, &p.videos_text, p.saved_at),
        )?;
        Ok(())
    }

    pub fn unsave_playlist(&self, playlist_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM saved_playlists WHERE playlist_id = ?1", [playlist_id])?;
        Ok(())
    }
}

fn schema_version(conn: &Connection) -> Result<usize> {
    let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    Ok(usize::try_from(v)?)
}

fn migrate(conn: &mut Connection, migrations: &[&str]) -> Result<()> {
    let current = schema_version(conn)?;
    if current > migrations.len() {
        bail!(
            "the database has schema version {current}, but this reeltty knows only {}. Use a newer reeltty.",
            migrations.len()
        );
    }
    for (i, sql) in migrations.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql).with_context(|| format!("migration {}", i + 1))?;
        tx.pragma_update(None, "user_version", i64::try_from(i + 1)?)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST: &[&str] = &["CREATE TABLE a (x INTEGER);", "CREATE TABLE b (y INTEGER);"];

    #[test]
    fn runs_migrations_once_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let s = Store::init(Connection::open(&path).unwrap(), &TEST[..1]).unwrap();
        assert_eq!(s.schema_version().unwrap(), 1);
        drop(s);
        let s = Store::init(Connection::open(&path).unwrap(), TEST).unwrap();
        assert_eq!(s.schema_version().unwrap(), 2);
        s.conn.execute("INSERT INTO a VALUES (1)", []).unwrap();
        s.conn.execute("INSERT INTO b VALUES (1)", []).unwrap();
    }

    #[test]
    fn refuses_a_newer_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        drop(Store::init(Connection::open(&path).unwrap(), TEST).unwrap());
        let err = Store::init(Connection::open(&path).unwrap(), &TEST[..1]).err().unwrap();
        assert!(err.to_string().contains("newer reeltty"), "{err}");
    }

    #[test]
    fn failed_migration_rolls_back() {
        let mut conn = Connection::open_in_memory().unwrap();
        let bad = &["CREATE TABLE a (x INTEGER);", "CREATE TABLE a (x INTEGER);"];
        assert!(migrate(&mut conn, bad).is_err());
        assert_eq!(schema_version(&conn).unwrap(), 1);
    }

    fn sub(id: &str, name: &str, at: i64) -> Subscription {
        Subscription { channel_id: id.into(), name: name.into(), handle: None, subscribed_at: at }
    }

    #[test]
    fn subscriptions_round_trip() {
        let s = Store::open_in_memory().unwrap();
        s.subscribe(&sub("UC2", "beta", 2)).unwrap();
        s.subscribe(&sub("UC1", "Alpha", 1)).unwrap();
        // A second subscribe updates the name and keeps the first time.
        s.subscribe(&Subscription { handle: Some("@alpha".into()), ..sub("UC1", "Alpha 2", 9) }).unwrap();
        let all = s.subscriptions().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], Subscription { handle: Some("@alpha".into()), ..sub("UC1", "Alpha 2", 1) });
        assert_eq!(all[1].name, "beta", "sorted by name without case");
        s.unsubscribe("UC1").unwrap();
        s.unsubscribe("UC-missing").unwrap();
        assert_eq!(s.subscriptions().unwrap(), [sub("UC2", "beta", 2)]);
    }

    #[test]
    fn saved_playlists_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let p = |id: &str, at| SavedPlaylist {
            playlist_id: id.into(),
            title: format!("T{id}"),
            channel_id: None,
            channel_name: Some("C".into()),
            videos_text: Some("3 videos".into()),
            saved_at: at,
        };
        s.save_playlist(&p("PL1", 1)).unwrap();
        s.save_playlist(&p("PL2", 2)).unwrap();
        s.save_playlist(&SavedPlaylist { videos_text: Some("4 videos".into()), ..p("PL1", 5) }).unwrap();
        let all = s.saved_playlists().unwrap();
        assert_eq!(all.iter().map(|x| x.playlist_id.as_str()).collect::<Vec<_>>(), ["PL2", "PL1"]);
        assert_eq!(all[1].videos_text.as_deref(), Some("4 videos"));
        assert_eq!(all[1].saved_at, 1);
        s.unsave_playlist("PL2").unwrap();
        assert_eq!(s.saved_playlists().unwrap().len(), 1);
    }

    #[test]
    fn opens_with_the_real_migrations() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), MIGRATIONS.len());
    }
}
