//! Ostatnie przebiegi zadań w tle („cronów”) — czas trwania (wall-clock), status i krótki opis.
//! Używane w panelu superadmin (www) jako przybliżenie „kosztu” workerów (CPU nie mierzymy osobno).

use std::sync::{Arc, Mutex};

use serde::Serialize;

const CAP: usize = 80;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerCronRunDto {
    pub worker_id: String,
    pub finished_at: String,
    pub duration_ms: u64,
    pub ok: bool,
    pub summary: Option<String>,
}

#[derive(Clone, Default)]
pub struct WorkerMetrics {
    runs: Arc<Mutex<Vec<WorkerCronRunDto>>>,
}

impl WorkerMetrics {
    pub fn new() -> Self {
        Self {
            runs: Arc::new(Mutex::new(Vec::with_capacity(CAP))),
        }
    }

    pub fn record(
        &self,
        worker_id: impl Into<String>,
        duration_ms: u64,
        ok: bool,
        summary: Option<String>,
    ) {
        let row = WorkerCronRunDto {
            worker_id: worker_id.into(),
            finished_at: chrono::Utc::now().to_rfc3339(),
            duration_ms,
            ok,
            summary,
        };
        let Ok(mut g) = self.runs.lock() else {
            return;
        };
        if g.len() >= CAP {
            g.remove(0);
        }
        g.push(row);
    }

    /// Najnowsze na początku.
    pub fn snapshot(&self) -> Vec<WorkerCronRunDto> {
        let Ok(g) = self.runs.lock() else {
            return Vec::new();
        };
        g.iter().rev().cloned().collect()
    }
}
