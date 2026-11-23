use rusqlite::{Connection, params};
use rustseis_core::{
    FirstBreakTable, MuteTable, Result, SeismicError, StaticsTable, SurfaceConsistentAmpTable,
    VelocityGrid2D, VelocityTable,
};
use serde::{Serialize, de::DeserializeOwned};
use std::path::Path;

/// SQLite-backed Ordered Parameter File (OPF) database — clean-room replacement for ProMAX
/// binary OPFs (`VEL`, `MUTE`, `FBPICK`, `STAT`, `SCAMP`, `FLOW_RUNS`).
pub struct OpfDatabase {
    conn: Connection,
}

fn map_sql(e: rusqlite::Error) -> SeismicError {
    SeismicError::InvalidFormat(format!("SQLite OPF error: {e}"))
}

impl OpfDatabase {
    /// Open or create a persistent SQLite OPF database file (e.g., `project/tables/project.opf.sqlite`).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path).map_err(map_sql)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// Create an in-memory SQLite OPF database (for testing or ephemeral sessions).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(map_sql)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "
                PRAGMA journal_mode = WAL;
                CREATE TABLE IF NOT EXISTS opf_tables (
                    category TEXT NOT NULL,
                    name     TEXT NOT NULL,
                    payload  TEXT NOT NULL,
                    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                    PRIMARY KEY (category, name)
                );
                CREATE TABLE IF NOT EXISTS flow_history (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    flow_name TEXT NOT NULL,
                    total_elapsed_ms INTEGER NOT NULL,
                    final_trace_count INTEGER NOT NULL,
                    report_json TEXT NOT NULL,
                    executed_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                ",
            )
            .map_err(map_sql)?;
        Ok(())
    }

    fn upsert_typed<T: Serialize>(&self, category: &str, name: &str, val: &T) -> Result<()> {
        let json = serde_json::to_string(val).map_err(|e| SeismicError::Serialization(e.to_string()))?;
        self.conn
            .execute(
                "INSERT INTO opf_tables (category, name, payload, updated_at)
                 VALUES (?1, ?2, ?3, datetime('now'))
                 ON CONFLICT(category, name) DO UPDATE SET payload = excluded.payload, updated_at = datetime('now')",
                params![category, name, json],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    fn load_typed<T: DeserializeOwned>(&self, category: &str, name: &str) -> Result<Option<T>> {
        let mut stmt = self
            .conn
            .prepare("SELECT payload FROM opf_tables WHERE category = ?1 AND name = ?2")
            .map_err(map_sql)?;
        let mut rows = stmt.query(params![category, name]).map_err(map_sql)?;
        if let Some(row) = rows.next().map_err(map_sql)? {
            let payload: String = row.get(0).map_err(map_sql)?;
            let val: T = serde_json::from_str(&payload)
                .map_err(|e| SeismicError::Serialization(e.to_string()))?;
            Ok(Some(val))
        } else {
            Ok(None)
        }
    }

    fn load_all_typed<T: DeserializeOwned>(&self, category: &str) -> Result<Vec<(String, T)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, payload FROM opf_tables WHERE category = ?1 ORDER BY name")
            .map_err(map_sql)?;
        let rows = stmt
            .query_map(params![category], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            let (name, payload) = r.map_err(map_sql)?;
            let val: T = serde_json::from_str(&payload)
                .map_err(|e| SeismicError::Serialization(e.to_string()))?;
            out.push((name, val));
        }
        Ok(out)
    }

    pub fn list_entries(&self) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT category, name, updated_at FROM opf_tables ORDER BY category, name")
            .map_err(map_sql)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql)?);
        }
        Ok(out)
    }

    pub fn save_velocity(&self, vt: &VelocityTable) -> Result<()> {
        self.upsert_typed("VEL", &vt.name, vt)
    }
    pub fn load_velocity(&self, name: &str) -> Result<Option<VelocityTable>> {
        self.load_typed("VEL", name)
    }
    pub fn load_all_velocities(&self) -> Result<Vec<(String, VelocityTable)>> {
        self.load_all_typed("VEL")
    }

    pub fn save_mute(&self, mt: &MuteTable) -> Result<()> {
        self.upsert_typed("MUTE", &mt.name, mt)
    }
    pub fn load_mute(&self, name: &str) -> Result<Option<MuteTable>> {
        self.load_typed("MUTE", name)
    }
    pub fn load_all_mutes(&self) -> Result<Vec<(String, MuteTable)>> {
        self.load_all_typed("MUTE")
    }

    pub fn save_first_breaks(&self, fb: &FirstBreakTable) -> Result<()> {
        self.upsert_typed("FBPICK", &fb.name, fb)
    }
    pub fn load_first_breaks(&self, name: &str) -> Result<Option<FirstBreakTable>> {
        self.load_typed("FBPICK", name)
    }
    pub fn load_all_first_breaks(&self) -> Result<Vec<(String, FirstBreakTable)>> {
        self.load_all_typed("FBPICK")
    }

    pub fn save_statics(&self, st: &StaticsTable) -> Result<()> {
        self.upsert_typed("STAT", &st.name, st)
    }
    pub fn load_statics(&self, name: &str) -> Result<Option<StaticsTable>> {
        self.load_typed("STAT", name)
    }
    pub fn load_all_statics(&self) -> Result<Vec<(String, StaticsTable)>> {
        self.load_all_typed("STAT")
    }

    pub fn save_sc_amp(&self, sc: &SurfaceConsistentAmpTable) -> Result<()> {
        self.upsert_typed("SCAMP", &sc.name, sc)
    }
    pub fn load_sc_amp(&self, name: &str) -> Result<Option<SurfaceConsistentAmpTable>> {
        self.load_typed("SCAMP", name)
    }
    pub fn load_all_sc_amps(&self) -> Result<Vec<(String, SurfaceConsistentAmpTable)>> {
        self.load_all_typed("SCAMP")
    }

    pub fn save_velocity_grid(&self, grid: &VelocityGrid2D) -> Result<()> {
        self.upsert_typed("VELGRID", &grid.name, grid)
    }
    pub fn load_velocity_grid(&self, name: &str) -> Result<Option<VelocityGrid2D>> {
        self.load_typed("VELGRID", name)
    }
    pub fn load_all_velocity_grids(&self) -> Result<Vec<(String, VelocityGrid2D)>> {
        self.load_all_typed("VELGRID")
    }

    pub fn record_flow_run(
        &self,
        flow_name: &str,
        total_elapsed_ms: u128,
        final_trace_count: usize,
        report_json: &str,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO flow_history (flow_name, total_elapsed_ms, final_trace_count, report_json)
                 VALUES (?1, ?2, ?3, ?4)",
                params![flow_name, total_elapsed_ms as i64, final_trace_count as i64, report_json],
            )
            .map_err(map_sql)?;
        Ok(())
    }
}
