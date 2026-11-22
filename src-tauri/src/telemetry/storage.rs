use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension};

use super::types::{SampleRecord, WindowBounds};

const RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;

pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open_default() -> Result<Self, String> {
        let base = dirs::data_local_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let dir = base.join("dev.wattlog.app");
        std::fs::create_dir_all(&dir).map_err(|e| format!("create storage dir: {e}"))?;
        let path = dir.join("wattlog.db");
        Self::open_path(path)
    }

    pub fn open_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|e| format!("sqlite memory: {e}"))?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    pub fn open_path(path: PathBuf) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("sqlite open: {e}"))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("sqlite WAL: {e}"))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| format!("sqlite sync: {e}"))?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS samples (
                    timestamp_ms INTEGER PRIMARY KEY,
                    system_watts REAL NOT NULL,
                    battery_pct INTEGER,
                    is_charging INTEGER NOT NULL,
                    external_connected INTEGER NOT NULL,
                    cpu_watts REAL,
                    gpu_watts REAL,
                    ane_watts REAL,
                    state TEXT NOT NULL,
                    memory_pressure TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_samples_ts ON samples(timestamp_ms);

                CREATE TABLE IF NOT EXISTS window_bounds (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    x INTEGER NOT NULL,
                    y INTEGER NOT NULL,
                    width INTEGER NOT NULL,
                    height INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS prefs (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                ",
            )
            .map_err(|e| format!("sqlite schema: {e}"))?;
        Ok(())
    }

    pub fn insert_sample(&self, sample: &SampleRecord) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO samples (
                    timestamp_ms, system_watts, battery_pct, is_charging,
                    external_connected, cpu_watts, gpu_watts, ane_watts,
                    state, memory_pressure
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    sample.timestamp_ms,
                    sample.system_watts,
                    sample.battery_pct.map(|v| v as i64),
                    if sample.is_charging { 1 } else { 0 },
                    if sample.external_connected { 1 } else { 0 },
                    sample.cpu_watts,
                    sample.gpu_watts,
                    sample.ane_watts,
                    sample.state,
                    sample.memory_pressure,
                ],
            )
            .map_err(|e| format!("insert sample: {e}"))?;

        let cutoff = sample.timestamp_ms - RETENTION_MS;
        let _ = self
            .conn
            .execute("DELETE FROM samples WHERE timestamp_ms < ?1", params![cutoff]);
        Ok(())
    }

    pub fn recent_samples(&self, hours: i64, limit: usize) -> Result<Vec<SampleRecord>, String> {
        let now = chrono::Utc::now().timestamp_millis();
        let cutoff = now - hours * 3600 * 1000;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT timestamp_ms, system_watts, battery_pct, is_charging,
                        external_connected, cpu_watts, gpu_watts, ane_watts,
                        state, memory_pressure
                 FROM samples
                 WHERE timestamp_ms >= ?1
                 ORDER BY timestamp_ms ASC
                 LIMIT ?2",
            )
            .map_err(|e| format!("prepare recent_samples: {e}"))?;

        let rows = stmt
            .query_map(params![cutoff, limit as i64], |row| {
                let pct: Option<i64> = row.get(2)?;
                Ok(SampleRecord {
                    timestamp_ms: row.get(0)?,
                    system_watts: row.get(1)?,
                    battery_pct: pct.map(|v| v.clamp(0, 100) as u8),
                    is_charging: row.get::<_, i64>(3)? != 0,
                    external_connected: row.get::<_, i64>(4)? != 0,
                    cpu_watts: row.get(5)?,
                    gpu_watts: row.get(6)?,
                    ane_watts: row.get(7)?,
                    state: row.get(8)?,
                    memory_pressure: row.get(9)?,
                })
            })
            .map_err(|e| format!("query recent_samples: {e}"))?;

        let mut out = Vec::new();
        for r in rows.flatten() {
            out.push(r);
        }
        Ok(out)
    }

    pub fn save_window_bounds(&self, bounds: &WindowBounds) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO window_bounds (id, x, y, width, height)
                 VALUES (1, ?1, ?2, ?3, ?4)",
                params![bounds.x, bounds.y, bounds.width as i64, bounds.height as i64],
            )
            .map_err(|e| format!("save window bounds: {e}"))?;
        Ok(())
    }

    pub fn load_window_bounds(&self) -> Result<Option<WindowBounds>, String> {
        self.conn
            .query_row(
                "SELECT x, y, width, height FROM window_bounds WHERE id = 1",
                [],
                |row| {
                    Ok(WindowBounds {
                        x: row.get(0)?,
                        y: row.get(1)?,
                        width: row.get::<_, i64>(2)?.max(640) as u32,
                        height: row.get::<_, i64>(3)?.max(420) as u32,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("load window bounds: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_wal_roundtrip_and_pruning() {
        let store = Storage::open_in_memory().expect("in-memory sqlite");
        let now = chrono::Utc::now().timestamp_millis();
        let old = SampleRecord {
            timestamp_ms: now - (8 * 24 * 3600 * 1000),
            system_watts: 9.5,
            battery_pct: Some(50),
            is_charging: false,
            external_connected: false,
            cpu_watts: Some(2.1),
            gpu_watts: Some(0.4),
            ane_watts: Some(0.0),
            state: "discharging".into(),
            memory_pressure: "green".into(),
        };
        let recent = SampleRecord {
            timestamp_ms: now - 10_000,
            system_watts: 6.2,
            battery_pct: Some(74),
            is_charging: false,
            external_connected: false,
            cpu_watts: Some(1.3),
            gpu_watts: Some(0.2),
            ane_watts: Some(0.0),
            state: "discharging".into(),
            memory_pressure: "green".into(),
        };
        store.insert_sample(&old).unwrap();
        store.insert_sample(&recent).unwrap();

        let loaded = store.recent_samples(24, 100).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].battery_pct, Some(74));

        let bounds = WindowBounds {
            x: 120,
            y: 80,
            width: 780,
            height: 520,
        };
        store.save_window_bounds(&bounds).unwrap();
        let got = store.load_window_bounds().unwrap().unwrap();
        assert_eq!(got.width, 780);
        assert_eq!(got.x, 120);
    }
}
