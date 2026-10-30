//! Local state in SQLite.
//!
//! The schema version is in `PRAGMA user_version`. Each migration runs once,
//! in order, inside one transaction. Later milestones add the tables.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use rusqlite::Connection;

/// Migration `i` takes the schema from version `i` to version `i + 1`.
const MIGRATIONS: &[&str] = &[];

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

    #[test]
    fn opens_with_the_real_migrations() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), MIGRATIONS.len());
    }
}
