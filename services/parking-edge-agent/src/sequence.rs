use std::path::Path;

use parking_domain::CameraId;
use rusqlite::{Connection, OptionalExtension, params};

pub struct SequenceStore {
    connection: Connection,
}

impl SequenceStore {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS camera_sequence (
                camera_id TEXT PRIMARY KEY NOT NULL,
                last_sequence INTEGER NOT NULL CHECK(last_sequence >= 0)
            );",
        )?;
        Ok(Self { connection })
    }

    pub fn allocate(&mut self, camera_id: CameraId) -> rusqlite::Result<u64> {
        let transaction = self.connection.transaction()?;
        let camera_id = camera_id.into_uuid().to_string();
        let previous: Option<i64> = transaction
            .query_row(
                "SELECT last_sequence FROM camera_sequence WHERE camera_id = ?1",
                [&camera_id],
                |row| row.get(0),
            )
            .optional()?;
        let next = previous
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| rusqlite::Error::IntegralValueOutOfRange(0, i64::MAX))?;
        transaction.execute(
            "INSERT INTO camera_sequence(camera_id, last_sequence) VALUES (?1, ?2)
             ON CONFLICT(camera_id) DO UPDATE SET last_sequence = excluded.last_sequence",
            params![camera_id, next],
        )?;
        transaction.commit()?;
        Ok(next as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_survives_edge_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("edge-state.db");
        let camera_id = CameraId::new();
        {
            let mut first = SequenceStore::open(&path).unwrap();
            assert_eq!(first.allocate(camera_id).unwrap(), 1);
            assert_eq!(first.allocate(camera_id).unwrap(), 2);
        }
        let mut second = SequenceStore::open(&path).unwrap();
        assert_eq!(second.allocate(camera_id).unwrap(), 3);
    }
}
