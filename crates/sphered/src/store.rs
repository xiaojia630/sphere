//! The event store: SQLite in WAL mode. One `events` table with a global
//! sequence number, plus the read offsets into the host's output files, saved
//! in the same transaction as the events they produced.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use sphere_proto::{now_rfc3339, Event, EventBody};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub struct Store {
    conn: Connection,
}

/// Where sphered has read up to in a child's output file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetUpdate {
    pub child: String,
    pub stream: &'static str,
    pub offset: u64,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        Self::init(conn, Some(path))
    }

    pub fn open_in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?, None)
    }

    fn init(conn: Connection, path: Option<&Path>) -> Result<Store> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                 seq  INTEGER PRIMARY KEY AUTOINCREMENT,
                 task TEXT NOT NULL,
                 at   TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 body TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS events_by_task ON events(task, seq);
             CREATE TABLE IF NOT EXISTS offsets (
                 child  TEXT NOT NULL,
                 stream TEXT NOT NULL,
                 off    INTEGER NOT NULL,
                 PRIMARY KEY (child, stream)
             );",
        )?;
        if let Some(path) = path {
            // The store is user-only.
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(Store { conn })
    }

    pub fn journal_mode(&self) -> Result<String> {
        Ok(self
            .conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))?)
    }

    /// Append events (and move an offset) atomically. Returns the stored events.
    pub fn append(
        &mut self,
        task: &str,
        bodies: &[EventBody],
        offset: Option<&OffsetUpdate>,
    ) -> Result<Vec<Event>> {
        let tx = self.conn.transaction()?;
        let at = now_rfc3339();
        let mut out = Vec::with_capacity(bodies.len());
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO events (task, at, kind, body) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for body in bodies {
                let json = serde_json::to_string(body)?;
                stmt.execute(params![task, at, body.kind_name(), json])?;
                out.push(Event {
                    seq: tx.last_insert_rowid() as u64,
                    task: task.to_string(),
                    at: at.clone(),
                    body: body.clone(),
                });
            }
        }
        if let Some(o) = offset {
            tx.execute(
                "INSERT INTO offsets (child, stream, off) VALUES (?1, ?2, ?3)
                 ON CONFLICT(child, stream) DO UPDATE SET off = excluded.off",
                params![o.child, o.stream, o.offset as i64],
            )?;
        }
        tx.commit()?;
        Ok(out)
    }

    pub fn last_seq(&self) -> Result<u64> {
        let n: Option<i64> = self
            .conn
            .query_row("SELECT MAX(seq) FROM events", [], |r| r.get(0))?;
        Ok(n.unwrap_or(0) as u64)
    }

    fn row_to_event(seq: i64, task: String, at: String, body: String) -> Option<Event> {
        match serde_json::from_str::<EventBody>(&body) {
            Ok(body) => Some(Event {
                seq: seq as u64,
                task,
                at,
                body,
            }),
            Err(e) => {
                tracing::warn!(seq, "skipping unreadable event: {e}");
                None
            }
        }
    }

    /// Events with `since < seq <= upto`, oldest first, at most `limit`.
    pub fn events_after(
        &self,
        since: u64,
        upto: u64,
        task: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT seq, task, at, body FROM events
             WHERE seq > ?1 AND seq <= ?2 AND (?3 IS NULL OR task = ?3)
             ORDER BY seq LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![since as i64, upto as i64, task, limit as i64],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let mut out = vec![];
        for row in rows {
            let (seq, task, at, body) = row?;
            out.extend(Self::row_to_event(seq, task, at, body));
        }
        Ok(out)
    }

    /// The newest `limit` events older than `before`, oldest first.
    pub fn history(
        &self,
        task: Option<&str>,
        before: Option<u64>,
        limit: usize,
    ) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT seq, task, at, body FROM events
             WHERE seq < ?1 AND (?2 IS NULL OR task = ?2)
             ORDER BY seq DESC LIMIT ?3",
        )?;
        let before = before.map(|b| b as i64).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![before, task, limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        let mut out = vec![];
        for row in rows {
            let (seq, task, at, body) = row?;
            out.extend(Self::row_to_event(seq, task, at, body));
        }
        out.reverse();
        Ok(out)
    }

    pub fn offset(&self, child: &str, stream: &str) -> Result<u64> {
        let n: Option<i64> = self
            .conn
            .query_row(
                "SELECT off FROM offsets WHERE child = ?1 AND stream = ?2",
                params![child, stream],
                |r| r.get(0),
            )
            .optional()?;
        Ok(n.unwrap_or(0) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn say(t: &str) -> EventBody {
        EventBody::Say { text: t.into() }
    }

    #[test]
    fn append_assigns_global_increasing_seq() {
        let mut s = Store::open_in_memory().unwrap();
        let a = s.append("t1", &[say("a"), say("b")], None).unwrap();
        let b = s.append("t2", &[say("c")], None).unwrap();
        assert_eq!(a.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(b[0].seq, 3);
        assert_eq!(s.last_seq().unwrap(), 3);
        assert_eq!(a[0].at.len(), 24);
    }

    #[test]
    fn queries_filter_and_page() {
        let mut s = Store::open_in_memory().unwrap();
        for i in 0..10 {
            s.append(
                if i % 2 == 0 { "t1" } else { "t2" },
                &[say(&i.to_string())],
                None,
            )
            .unwrap();
        }
        let all = s.events_after(0, 100, None, 100).unwrap();
        assert_eq!(all.len(), 10);
        let t1 = s.events_after(0, 100, Some("t1"), 100).unwrap();
        assert_eq!(t1.len(), 5);
        let window = s.events_after(3, 6, None, 100).unwrap();
        assert_eq!(
            window.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![4, 5, 6]
        );
        let page = s.events_after(0, 100, None, 4).unwrap();
        assert_eq!(page.len(), 4);

        let hist = s.history(None, None, 3).unwrap();
        assert_eq!(
            hist.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![8, 9, 10]
        );
        let older = s.history(None, Some(8), 3).unwrap();
        assert_eq!(
            older.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![5, 6, 7]
        );
        let h2 = s.history(Some("t2"), None, 2).unwrap();
        assert!(h2.iter().all(|e| e.task == "t2") && h2.len() == 2);
    }

    #[test]
    fn offsets_move_with_the_events_in_one_transaction() {
        let mut s = Store::open_in_memory().unwrap();
        assert_eq!(s.offset("c1", "out").unwrap(), 0);
        let o = OffsetUpdate {
            child: "c1".into(),
            stream: "out",
            offset: 120,
        };
        s.append("t1", &[say("x")], Some(&o)).unwrap();
        assert_eq!(s.offset("c1", "out").unwrap(), 120);
        let o2 = OffsetUpdate { offset: 200, ..o };
        s.append("t1", &[], Some(&o2)).unwrap();
        assert_eq!(s.offset("c1", "out").unwrap(), 200);
        assert_eq!(s.offset("c1", "err").unwrap(), 0);
    }

    #[test]
    fn file_store_uses_wal_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sphere.db");
        {
            let mut s = Store::open(&path).unwrap();
            assert_eq!(s.journal_mode().unwrap(), "wal");
            s.append("t1", &[say("kept")], None).unwrap();
        }
        let s = Store::open(&path).unwrap();
        let evs = s.history(None, None, 10).unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].body, say("kept"));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn unreadable_rows_are_skipped_not_fatal() {
        let mut s = Store::open_in_memory().unwrap();
        s.append("t1", &[say("a")], None).unwrap();
        s.conn
            .execute("INSERT INTO events (task, at, kind, body) VALUES ('t1','x','future','{\"kind\":\"from_the_future\"}')", [])
            .unwrap();
        s.append("t1", &[say("b")], None).unwrap();
        assert_eq!(s.events_after(0, 100, None, 100).unwrap().len(), 2);
    }
}
