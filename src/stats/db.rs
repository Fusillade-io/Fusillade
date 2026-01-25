use super::ReportStats;
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};

pub struct HistoryDb {
    conn: Connection,
}

impl HistoryDb {
    pub fn open_default() -> Result<Self> {
        let path = "fusillade_history.db";
        let conn = Connection::open(path)?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS runs (
                id INTEGER PRIMARY KEY,
                start_time TEXT NOT NULL,
                scenario TEXT,
                summary_json TEXT NOT NULL
            )",
            [],
        )?;

        Ok(Self { conn })
    }

    pub fn save_run(&self, scenario: &str, report: &ReportStats) -> Result<i64> {
        let start_time = Utc::now().to_rfc3339();
        let summary_json = serde_json::to_string(report)?;

        self.conn.execute(
            "INSERT INTO runs (start_time, scenario, summary_json) VALUES (?1, ?2, ?3)",
            params![start_time, scenario, summary_json],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_runs(&self, limit: usize) -> Result<Vec<(i64, String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, start_time, scenario FROM runs ORDER BY id DESC LIMIT ?1")?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    #[allow(dead_code)]
    pub fn get_report(&self, id: i64) -> Result<ReportStats> {
        let summary_json: String = self.conn.query_row(
            "SELECT summary_json FROM runs WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;

        let report: ReportStats = serde_json::from_str(&summary_json)?;
        Ok(report)
    }
}
