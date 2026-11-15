//! Local state in SQLite.
//!
//! The schema version is in `PRAGMA user_version`. Each migration runs once,
//! in order, inside one transaction. Later milestones add more tables.

use std::path::Path;

use std::collections::HashMap;

use anyhow::{Context as _, Result, bail};
use rusqlite::Connection;

use crate::source::download::FileFormat;
use crate::source::model::{ChannelRef, Chapter, Thumbnail, Video, VideoKind};
use crate::source::streams::FormatInfo;

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
    // 2: watch history, one row for each video. Positions are seconds.
    "CREATE TABLE history (
        video_id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        channel_id TEXT,
        channel_name TEXT,
        duration_secs INTEGER,
        position_secs INTEGER NOT NULL,
        finished INTEGER NOT NULL,
        watched_at INTEGER NOT NULL
    );
    CREATE INDEX history_by_time ON history (watched_at DESC);",
    // 3: the subscriptions feed. Each channel's newest videos, and when the
    // channel was loaded. Thumbnails are a JSON list of [url, width, height].
    "CREATE TABLE feed_channels (
        channel_id TEXT PRIMARY KEY,
        fetched_at INTEGER NOT NULL
    );
    CREATE TABLE feed_videos (
        channel_id TEXT NOT NULL,
        video_id TEXT NOT NULL,
        title TEXT NOT NULL,
        kind TEXT NOT NULL,
        channel_name TEXT,
        channel_handle TEXT,
        duration_secs INTEGER,
        duration_text TEXT,
        views_text TEXT,
        published_text TEXT,
        published_at INTEGER NOT NULL,
        members_only INTEGER NOT NULL,
        thumbnails TEXT NOT NULL,
        PRIMARY KEY (channel_id, video_id)
    );",
    // 4: downloaded videos, and videos waiting to download. Files are names
    // in the downloads folder. Chapters, captions, and thumbnails are JSON.
    // `segments` is the SponsorBlock answer as JSON, or NULL before reeltty
    // asked for it.
    "CREATE TABLE downloads (
        video_id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        channel_id TEXT,
        channel_name TEXT,
        channel_handle TEXT,
        duration_secs INTEGER,
        thumbnails TEXT NOT NULL DEFAULT '[]',
        state TEXT NOT NULL,
        error TEXT,
        max_height INTEGER NOT NULL,
        file_format TEXT NOT NULL,
        video_file TEXT,
        audio_file TEXT,
        size_bytes INTEGER,
        format_id TEXT,
        width INTEGER,
        height INTEGER,
        fps INTEGER,
        vcodec TEXT,
        acodec TEXT,
        chapters TEXT NOT NULL DEFAULT '[]',
        captions TEXT NOT NULL DEFAULT '[]',
        segments TEXT,
        segments_at INTEGER,
        thumbnail_file TEXT,
        added_at INTEGER NOT NULL,
        finished_at INTEGER
    );",
];

/// The most history entries that the app loads.
pub const HISTORY_LIMIT: usize = 1000;

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

/// A video the user watched. Only the last watch of each video is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub video_id: String,
    pub title: String,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    pub duration_secs: Option<u32>,
    /// Where the user stopped, for resume.
    pub position_secs: u32,
    /// The video played to the end.
    pub finished: bool,
    pub watched_at: i64,
}

/// A video in the feed, with its channel and approximate time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedVideo {
    pub channel_id: String,
    /// Unix seconds, from the relative date.
    pub published_at: i64,
    pub video: Video,
}

/// Where a download is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    /// Waiting to download, or downloading. A download that runs when
    /// reeltty stops goes on at the next start.
    Queued,
    /// The last try failed. `Download::error` says why.
    Failed,
    /// The files are on disk.
    Done,
}

impl DownloadState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Failed => "failed",
            Self::Done => "done",
        }
    }

    fn from_name(name: &str) -> Self {
        match name {
            "done" => Self::Done,
            "failed" => Self::Failed,
            _ => Self::Queued,
        }
    }
}

/// A caption track on disk.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedCaption {
    /// A language code such as `en` or `en-orig`.
    pub lang: String,
    /// The language name, such as "English".
    pub name: String,
    /// Made by speech recognition, not by a person.
    pub auto: bool,
    /// A `WebVTT` file in the downloads folder.
    pub file: String,
}

/// A video that is downloaded, or waits to download.
#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    pub video: Video,
    pub state: DownloadState,
    /// Why the last try failed.
    pub error: Option<String>,
    /// At most this many pixels on the short side.
    pub max_height: u32,
    pub file_format: FileFormat,
    /// The video file, a name in the downloads folder. In the `Mkv` format
    /// it has the audio too.
    pub video_file: Option<String>,
    /// The audio file, in the `Separate` format.
    pub audio_file: Option<String>,
    pub size_bytes: Option<u64>,
    /// The format that was downloaded. Empty before it is done.
    pub format: FormatInfo,
    pub chapters: Vec<Chapter>,
    pub captions: Vec<SavedCaption>,
    /// The SponsorBlock segments as JSON, and when reeltty got them.
    pub segments: Option<(String, i64)>,
    /// The thumbnail, a name in the downloads folder.
    pub thumbnail_file: Option<String>,
    pub added_at: i64,
    pub finished_at: Option<i64>,
}

impl Download {
    /// A new download that waits in the queue.
    pub fn queued(video: Video, max_height: u32, file_format: FileFormat, now: i64) -> Self {
        Self {
            video,
            state: DownloadState::Queued,
            error: None,
            max_height,
            file_format,
            video_file: None,
            audio_file: None,
            size_bytes: None,
            format: FormatInfo::default(),
            chapters: Vec::new(),
            captions: Vec::new(),
            segments: None,
            thumbnail_file: None,
            added_at: now,
            finished_at: None,
        }
    }
}

fn kind_name(kind: VideoKind) -> &'static str {
    match kind {
        VideoKind::Normal => "normal",
        VideoKind::Short => "short",
        VideoKind::Live => "live",
        VideoKind::PastStream => "past_stream",
    }
}

fn kind_of(name: &str) -> VideoKind {
    match name {
        "short" => VideoKind::Short,
        "live" => VideoKind::Live,
        "past_stream" => VideoKind::PastStream,
        _ => VideoKind::Normal,
    }
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

    /// The watch history, newest first, at most `HISTORY_LIMIT` entries.
    pub fn history(&self) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT video_id, title, channel_id, channel_name, duration_secs, position_secs, finished, watched_at
             FROM history ORDER BY watched_at DESC, rowid DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([i64::try_from(HISTORY_LIMIT)?], |r| {
            Ok(HistoryEntry {
                video_id: r.get(0)?,
                title: r.get(1)?,
                channel_id: r.get(2)?,
                channel_name: r.get(3)?,
                duration_secs: r.get(4)?,
                position_secs: r.get(5)?,
                finished: r.get(6)?,
                watched_at: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Adds a watch, or replaces the last watch of the same video.
    pub fn record_watch(&self, e: &HistoryEntry) -> Result<()> {
        self.conn.execute(
            "INSERT INTO history
                 (video_id, title, channel_id, channel_name, duration_secs, position_secs, finished, watched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (video_id) DO UPDATE SET title = excluded.title, channel_id = excluded.channel_id,
                 channel_name = excluded.channel_name, duration_secs = excluded.duration_secs,
                 position_secs = excluded.position_secs, finished = excluded.finished,
                 watched_at = excluded.watched_at",
            (
                &e.video_id,
                &e.title,
                &e.channel_id,
                &e.channel_name,
                e.duration_secs,
                e.position_secs,
                e.finished,
                e.watched_at,
            ),
        )?;
        Ok(())
    }

    /// The feed, and when each channel was loaded.
    pub fn feed(&self) -> Result<(Vec<FeedVideo>, HashMap<String, i64>)> {
        let mut stmt = self.conn.prepare("SELECT channel_id, fetched_at FROM feed_channels")?;
        let fetched = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut stmt = self.conn.prepare(
            "SELECT channel_id, video_id, title, kind, channel_name, channel_handle, duration_secs, duration_text,
                 views_text, published_text, published_at, members_only, thumbnails
             FROM feed_videos ORDER BY published_at DESC, rowid",
        )?;
        let rows = stmt.query_map([], |r| {
            let channel_id: String = r.get(0)?;
            let channel_name: Option<String> = r.get(4)?;
            let thumbnails: String = r.get(12)?;
            let thumbnails: Vec<(String, u32, u32)> = serde_json::from_str(&thumbnails).unwrap_or_default();
            Ok(FeedVideo {
                video: Video {
                    id: r.get(1)?,
                    title: r.get(2)?,
                    kind: kind_of(&r.get::<_, String>(3)?),
                    channels: channel_name
                        .map(|name| ChannelRef { id: Some(channel_id.clone()), name, handle: r.get(5).ok().flatten() })
                        .into_iter()
                        .collect(),
                    duration_secs: r.get(6)?,
                    duration_text: r.get(7)?,
                    views_text: r.get(8)?,
                    published_text: r.get(9)?,
                    thumbnails: thumbnails
                        .into_iter()
                        .map(|(url, width, height)| Thumbnail { url, width, height })
                        .collect(),
                    members_only: r.get(11)?,
                    badges: Vec::new(),
                    description_snippet: None,
                },
                published_at: r.get(10)?,
                channel_id,
            })
        })?;
        Ok((rows.collect::<rusqlite::Result<_>>()?, fetched))
    }

    /// Replaces the feed videos of one channel.
    pub fn save_channel_feed(&self, channel_id: &str, fetched_at: i64, videos: &[FeedVideo]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO feed_channels (channel_id, fetched_at) VALUES (?1, ?2)
             ON CONFLICT (channel_id) DO UPDATE SET fetched_at = excluded.fetched_at",
            (channel_id, fetched_at),
        )?;
        tx.execute("DELETE FROM feed_videos WHERE channel_id = ?1", [channel_id])?;
        for f in videos {
            let v = &f.video;
            let channel = v.channels.first();
            let thumbnails: Vec<(&str, u32, u32)> =
                v.thumbnails.iter().map(|t| (t.url.as_str(), t.width, t.height)).collect();
            tx.execute(
                "INSERT OR REPLACE INTO feed_videos (channel_id, video_id, title, kind, channel_name, channel_handle,
                     duration_secs, duration_text, views_text, published_text, published_at, members_only, thumbnails)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                rusqlite::params![
                    channel_id,
                    v.id,
                    v.title,
                    kind_name(v.kind),
                    channel.map(|c| c.name.as_str()),
                    channel.and_then(|c| c.handle.as_deref()),
                    v.duration_secs,
                    v.duration_text,
                    v.views_text,
                    v.published_text,
                    f.published_at,
                    v.members_only,
                    serde_json::to_string(&thumbnails)?,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn forget_channel_feed(&self, channel_id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM feed_videos WHERE channel_id = ?1", [channel_id])?;
        tx.execute("DELETE FROM feed_channels WHERE channel_id = ?1", [channel_id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn forget_watch(&self, video_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM history WHERE video_id = ?1", [video_id])?;
        Ok(())
    }

    /// All downloads, in the order they were added.
    pub fn downloads(&self) -> Result<Vec<Download>> {
        let mut stmt = self.conn.prepare(
            "SELECT video_id, title, channel_id, channel_name, channel_handle, duration_secs, thumbnails, state,
                 error, max_height, file_format, video_file, audio_file, size_bytes, format_id, width, height, fps,
                 vcodec, acodec, chapters, captions, segments, segments_at, thumbnail_file, added_at, finished_at
             FROM downloads ORDER BY added_at, rowid",
        )?;
        let rows = stmt.query_map([], |r| {
            let json = |i: usize| -> rusqlite::Result<String> { r.get(i) };
            let thumbnails: Vec<(String, u32, u32)> = serde_json::from_str(&json(6)?).unwrap_or_default();
            let channel_name: Option<String> = r.get(3)?;
            let segments: Option<String> = r.get(22)?;
            let segments_at: Option<i64> = r.get(23)?;
            Ok(Download {
                video: Video {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    kind: VideoKind::Normal,
                    channels: channel_name
                        .map(|name| ChannelRef { id: r.get(2).ok().flatten(), name, handle: r.get(4).ok().flatten() })
                        .into_iter()
                        .collect(),
                    duration_secs: r.get(5)?,
                    duration_text: None,
                    views_text: None,
                    published_text: None,
                    thumbnails: thumbnails
                        .into_iter()
                        .map(|(url, width, height)| Thumbnail { url, width, height })
                        .collect(),
                    members_only: false,
                    badges: Vec::new(),
                    description_snippet: None,
                },
                state: DownloadState::from_name(&r.get::<_, String>(7)?),
                error: r.get(8)?,
                max_height: r.get(9)?,
                file_format: FileFormat::from_name(&r.get::<_, String>(10)?),
                video_file: r.get(11)?,
                audio_file: r.get(12)?,
                size_bytes: r.get::<_, Option<i64>>(13)?.and_then(|n| u64::try_from(n).ok()),
                format: FormatInfo {
                    id: r.get::<_, Option<String>>(14)?.unwrap_or_default(),
                    width: r.get(15)?,
                    height: r.get(16)?,
                    fps: r.get(17)?,
                    vcodec: r.get::<_, Option<String>>(18)?.unwrap_or_default(),
                    acodec: r.get::<_, Option<String>>(19)?.unwrap_or_default(),
                },
                chapters: serde_json::from_str(&json(20)?).unwrap_or_default(),
                captions: serde_json::from_str(&json(21)?).unwrap_or_default(),
                segments: segments.zip(segments_at),
                thumbnail_file: r.get(24)?,
                added_at: r.get(25)?,
                finished_at: r.get(26)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Adds a download, or replaces all that is known about it but the time
    /// it was added.
    pub fn save_download(&self, d: &Download) -> Result<()> {
        let v = &d.video;
        let channel = v.channels.first();
        let thumbnails: Vec<(&str, u32, u32)> =
            v.thumbnails.iter().map(|t| (t.url.as_str(), t.width, t.height)).collect();
        let empty = |s: &str| (!s.is_empty()).then(|| s.to_string());
        self.conn.execute(
            "INSERT INTO downloads (video_id, title, channel_id, channel_name, channel_handle, duration_secs,
                 thumbnails, state, error, max_height, file_format, video_file, audio_file, size_bytes, format_id,
                 width, height, fps, vcodec, acodec, chapters, captions, segments, segments_at, thumbnail_file,
                 added_at, finished_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                 ?21, ?22, ?23, ?24, ?25, ?26, ?27)
             ON CONFLICT (video_id) DO UPDATE SET title = excluded.title, channel_id = excluded.channel_id,
                 channel_name = excluded.channel_name, channel_handle = excluded.channel_handle,
                 duration_secs = excluded.duration_secs, thumbnails = excluded.thumbnails, state = excluded.state,
                 error = excluded.error, max_height = excluded.max_height, file_format = excluded.file_format,
                 video_file = excluded.video_file, audio_file = excluded.audio_file,
                 size_bytes = excluded.size_bytes, format_id = excluded.format_id, width = excluded.width,
                 height = excluded.height, fps = excluded.fps, vcodec = excluded.vcodec, acodec = excluded.acodec,
                 chapters = excluded.chapters, captions = excluded.captions, segments = excluded.segments,
                 segments_at = excluded.segments_at, thumbnail_file = excluded.thumbnail_file,
                 finished_at = excluded.finished_at",
            rusqlite::params![
                v.id,
                v.title,
                channel.and_then(|c| c.id.as_deref()),
                channel.map(|c| c.name.as_str()),
                channel.and_then(|c| c.handle.as_deref()),
                v.duration_secs,
                serde_json::to_string(&thumbnails)?,
                d.state.as_str(),
                d.error,
                d.max_height,
                d.file_format.as_str(),
                d.video_file,
                d.audio_file,
                d.size_bytes.and_then(|n| i64::try_from(n).ok()),
                empty(&d.format.id),
                d.format.width,
                d.format.height,
                d.format.fps,
                empty(&d.format.vcodec),
                empty(&d.format.acodec),
                serde_json::to_string(&d.chapters)?,
                serde_json::to_string(&d.captions)?,
                d.segments.as_ref().map(|(json, _)| json),
                d.segments.as_ref().map(|(_, at)| at),
                d.thumbnail_file,
                d.added_at,
                d.finished_at,
            ],
        )?;
        Ok(())
    }

    pub fn forget_download(&self, video_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM downloads WHERE video_id = ?1", [video_id])?;
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

    fn watch(id: &str, at: i64) -> HistoryEntry {
        HistoryEntry {
            video_id: id.into(),
            title: format!("T{id}"),
            channel_id: Some("UC1".into()),
            channel_name: Some("C".into()),
            duration_secs: Some(600),
            position_secs: 42,
            finished: false,
            watched_at: at,
        }
    }

    #[test]
    fn history_round_trip() {
        let s = Store::open_in_memory().unwrap();
        s.record_watch(&watch("a", 1)).unwrap();
        s.record_watch(&watch("b", 2)).unwrap();
        // Watching a again moves it to the top, with the new position.
        s.record_watch(&HistoryEntry { position_secs: 0, finished: true, ..watch("a", 3) }).unwrap();
        let all = s.history().unwrap();
        assert_eq!(all.iter().map(|e| e.video_id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(all[0], HistoryEntry { position_secs: 0, finished: true, ..watch("a", 3) });
        s.forget_watch("a").unwrap();
        assert_eq!(s.history().unwrap(), [watch("b", 2)]);
    }

    #[test]
    fn feed_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let video = |id: &str, kind| Video {
            id: id.into(),
            title: format!("T{id}"),
            kind,
            channels: vec![ChannelRef { id: Some("UC1".into()), name: "C".into(), handle: Some("@c".into()) }],
            duration_secs: Some(60),
            duration_text: Some("1:00".into()),
            views_text: Some("5 views".into()),
            published_text: Some("1 day ago".into()),
            thumbnails: vec![Thumbnail { url: "https://i.ytimg.com/vi/x/a.jpg".into(), width: 336, height: 188 }],
            members_only: false,
            badges: Vec::new(),
            description_snippet: None,
        };
        let feed =
            |id: &str, at, kind| FeedVideo { channel_id: "UC1".into(), published_at: at, video: video(id, kind) };
        s.save_channel_feed("UC1", 100, &[feed("a", 90, VideoKind::Normal), feed("b", 95, VideoKind::Short)]).unwrap();
        let (videos, fetched) = s.feed().unwrap();
        assert_eq!(videos, [feed("b", 95, VideoKind::Short), feed("a", 90, VideoKind::Normal)]);
        assert_eq!(fetched.get("UC1"), Some(&100));
        // A new load replaces the channel's videos.
        s.save_channel_feed("UC1", 200, &[feed("c", 190, VideoKind::PastStream)]).unwrap();
        let (videos, fetched) = s.feed().unwrap();
        assert_eq!(videos.iter().map(|v| v.video.id.as_str()).collect::<Vec<_>>(), ["c"]);
        assert_eq!(fetched.get("UC1"), Some(&200));
        s.forget_channel_feed("UC1").unwrap();
        let (videos, fetched) = s.feed().unwrap();
        assert!(videos.is_empty() && fetched.is_empty());
    }

    fn video(id: &str) -> Video {
        Video {
            id: id.into(),
            title: format!("T{id}"),
            kind: VideoKind::Normal,
            channels: vec![ChannelRef { id: Some("UC1".into()), name: "C".into(), handle: Some("@c".into()) }],
            duration_secs: Some(60),
            duration_text: None,
            views_text: None,
            published_text: None,
            thumbnails: vec![Thumbnail { url: "https://i.ytimg.com/vi/x/a.jpg".into(), width: 336, height: 188 }],
            members_only: false,
            badges: Vec::new(),
            description_snippet: None,
        }
    }

    #[test]
    fn downloads_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let queued = Download::queued(video("b"), 1080, FileFormat::Separate, 1);
        s.save_download(&queued).unwrap();
        s.save_download(&Download::queued(video("a"), 720, FileFormat::Mkv, 2)).unwrap();
        assert_eq!(s.downloads().unwrap(), [queued.clone(), Download::queued(video("a"), 720, FileFormat::Mkv, 2)]);

        // Done, with all the details. The time it was added stays.
        let done = Download {
            state: DownloadState::Done,
            video_file: Some("b.f136.mp4".into()),
            audio_file: Some("b.f251.webm".into()),
            size_bytes: Some(412_000_000),
            format: FormatInfo {
                id: "136+251".into(),
                width: Some(1280),
                height: Some(720),
                fps: Some(30),
                vcodec: "avc1.4d401f".into(),
                acodec: "opus".into(),
            },
            chapters: vec![Chapter { title: "Start".into(), start_secs: 0 }],
            captions: vec![SavedCaption {
                lang: "en".into(),
                name: "English".into(),
                auto: true,
                file: "b.en.vtt".into(),
            }],
            segments: Some(("[]".into(), 50)),
            thumbnail_file: Some("b.jpg".into()),
            finished_at: Some(60),
            ..Download { added_at: 99, ..queued.clone() }
        };
        s.save_download(&done).unwrap();
        let all = s.downloads().unwrap();
        assert_eq!(all[0], Download { added_at: 1, ..done });

        // Failed, then forgotten.
        let failed = Download { state: DownloadState::Failed, error: Some("HTTP 403".into()), ..queued };
        s.save_download(&failed).unwrap();
        assert_eq!(s.downloads().unwrap()[0], failed);
        s.forget_download("b").unwrap();
        s.forget_download("missing").unwrap();
        assert_eq!(s.downloads().unwrap().len(), 1);
    }

    #[test]
    fn opens_with_the_real_migrations() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), MIGRATIONS.len());
    }
}
