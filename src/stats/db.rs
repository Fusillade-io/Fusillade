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

    pub fn get_report(&self, id: i64) -> Result<ReportStats> {
        let summary_json: String = self.conn.query_row(
            "SELECT summary_json FROM runs WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;

        let report: ReportStats = serde_json::from_str(&summary_json)?;
        Ok(report)
    }

    /// Open an in-memory database (for testing)
    #[cfg(test)]
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn create_test_report() -> ReportStats {
        ReportStats {
            total_requests: 1000,
            total_duration_ms: 10000,
            avg_latency_ms: 45.5,
            min_latency_ms: 10,
            max_latency_ms: 500,
            p50_latency_ms: 40.0,
            p90_latency_ms: 80.0,
            p95_latency_ms: 120.0,
            p99_latency_ms: 200.0,
            status_codes: HashMap::new(),
            errors: HashMap::new(),
            checks: HashMap::new(),
            grouped_requests: HashMap::new(),
            total_data_sent: 1024,
            total_data_received: 5120,
            histograms: HashMap::new(),
            rates: HashMap::new(),
            counters: HashMap::new(),
            gauges: HashMap::new(),
            pool_hits: 0,
            pool_misses: 0,
        }
    }

    #[test]
    fn test_history_db_save_and_list() {
        let db = HistoryDb::open_memory().unwrap();
        let report = create_test_report();

        // Save a run
        let id = db.save_run("test_scenario.js", &report).unwrap();
        assert!(id > 0);

        // List runs
        let runs = db.list_runs(10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, id);
        assert_eq!(runs[0].2, "test_scenario.js");
    }

    #[test]
    fn test_history_db_get_report() {
        let db = HistoryDb::open_memory().unwrap();
        let report = create_test_report();

        let id = db.save_run("test.js", &report).unwrap();

        // Retrieve the report
        let loaded = db.get_report(id).unwrap();
        assert_eq!(loaded.total_requests, 1000);
        assert_eq!(loaded.avg_latency_ms, 45.5);
    }

    #[test]
    fn test_history_db_list_limit() {
        let db = HistoryDb::open_memory().unwrap();
        let report = create_test_report();

        // Save multiple runs
        for i in 0..5 {
            db.save_run(&format!("scenario_{}.js", i), &report).unwrap();
        }

        // List with limit
        let runs = db.list_runs(3).unwrap();
        assert_eq!(runs.len(), 3);

        // Should be in descending order (newest first)
        assert!(runs[0].0 > runs[1].0);
    }

    #[test]
    fn test_history_db_empty() {
        let db = HistoryDb::open_memory().unwrap();
        let runs = db.list_runs(10).unwrap();
        assert!(runs.is_empty());
    }
}
