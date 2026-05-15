//! Automatyczne usuwanie nieaktywnych wątków czatu.
//!
//! Wątek (`chat_threads`) jest „żywy" tak długo, jak długo padają w nim nowe wiadomości
//! (`chat_messages.created_at`). Każda nowa wiadomość automatycznie przedłuża życie
//! wątku — nie potrzebujemy do tego żadnego dodatkowego pola, bo bierzemy
//! `MAX(chat_messages.created_at)`. Dla wątków bez wiadomości fallbackujemy do
//! `chat_threads.created_at`.
//!
//! Zadanie wykonuje się okresowo w tle (`spawn_chat_pruner_task`) oraz raz przy
//! starcie aplikacji (przyszły scheduler nie potrzebuje persistent state).

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::task::JoinHandle;

use crate::audit::write_audit_log;
use crate::state::Db;
use crate::worker_metrics::WorkerMetrics;

/// Po ilu dniach bezczynności (od ostatniej wiadomości) wątek jest usuwany.
/// Każda nowa wiadomość resetuje liczenie — to ona aktualizuje `MAX(created_at)`.
pub const CHAT_INACTIVITY_DAYS: i64 = 30;

/// Co ile godzin uruchamia się przegląd. 6 h to kompromis między reaktywnością
/// (wątek bez wiadomości znika maks ~6 h po przekroczeniu progu) a obciążeniem DB.
const PRUNE_INTERVAL_HOURS: u64 = 6;

/// Wątek do usunięcia — pobierany przed `DELETE`, żeby zapisać go w audit logu.
struct StaleThread {
    id: String,
    last_activity: String,
}

/// Pobiera wątki, które nie miały aktywności od `cutoff` (RFC3339 UTC).
async fn select_stale_threads(conn: &Db, cutoff: &str) -> Result<Vec<StaleThread>, libsql::Error> {
    // COALESCE: wątki bez żadnych wiadomości fallbackują do `created_at`.
    // To pozwala usuwać też puste wątki, które ktoś otworzył i zostawił bez napisania słowa.
    let mut rows = conn
        .query(
            "SELECT t.id,
                    COALESCE(
                        (SELECT MAX(m.created_at) FROM chat_messages m WHERE m.thread_id = t.id),
                        t.created_at
                    ) AS last_activity
             FROM chat_threads t
             WHERE COALESCE(
                       (SELECT MAX(m.created_at) FROM chat_messages m WHERE m.thread_id = t.id),
                       t.created_at
                   ) < ?1",
            [cutoff.to_string()],
        )
        .await?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0).unwrap_or_default();
        let last_activity: String = row.get(1).unwrap_or_default();
        if !id.is_empty() {
            out.push(StaleThread { id, last_activity });
        }
    }
    Ok(out)
}

/// Wykonuje jeden przebieg czyszczący. Zwraca liczbę usuniętych wątków.
///
/// Bezpieczne do wywołania ręcznie (np. z testów lub komendy diagnostycznej).
/// FK z `ON DELETE CASCADE` zadba o wyczyszczenie `chat_messages` i `chat_reads`.
pub async fn prune_inactive_chat_threads(
    conn: &Db,
    inactivity_days: i64,
) -> Result<usize, libsql::Error> {
    let cutoff_dt: DateTime<Utc> = Utc::now() - chrono::Duration::days(inactivity_days);
    let cutoff = cutoff_dt.to_rfc3339();

    let stale = select_stale_threads(conn, &cutoff).await?;
    if stale.is_empty() {
        return Ok(0);
    }

    let mut deleted = 0usize;
    for thread in &stale {
        let n = conn
            .execute(
                "DELETE FROM chat_threads WHERE id = ?1",
                [thread.id.clone()],
            )
            .await?;
        if n > 0 {
            deleted += n as usize;
            let details = serde_json::json!({
                "thread_id": thread.id,
                "last_activity": thread.last_activity,
                "inactivity_days": inactivity_days,
                "reason": "inactivity_auto_prune",
            })
            .to_string();
            // Audit zapisujemy „best-effort"; brak audytu nie powinien blokować czyszczenia.
            let audit_conn = conn.raw().await;
            let _ = write_audit_log(
                audit_conn.as_ref(),
                None,
                Some("system"),
                "chat",
                "thread_auto_pruned",
                Some("thread"),
                Some(&thread.id),
                Some(&details),
            )
            .await;
        }
    }

    Ok(deleted)
}

/// Uruchamia wieczne zadanie w tle czyszczące nieaktywne wątki.
///
/// Pierwszy przebieg następuje krótko po starcie (po 60 s — żeby nie obciążać uruchomienia),
/// kolejne co `PRUNE_INTERVAL_HOURS` godzin. Niepowodzenia są logowane na stderr,
/// ale pętla biegnie dalej (chwilowy błąd DB nie powinien zatrzymywać pruner-a).
pub fn spawn_chat_pruner_task(db: Db, metrics: Arc<WorkerMetrics>) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Drobny delay startowy — aplikacja zdąży wstać, migracje już są skończone.
        tokio::time::sleep(Duration::from_secs(60)).await;
        let interval = Duration::from_secs(PRUNE_INTERVAL_HOURS * 3600);
        loop {
            let t0 = std::time::Instant::now();
            match prune_inactive_chat_threads(&db, CHAT_INACTIVITY_DAYS).await {
                Ok(n) => {
                    metrics.record(
                        "chat_pruner_scheduler",
                        t0.elapsed().as_millis() as u64,
                        true,
                        Some(format!("deleted_threads={n}")),
                    );
                    if n > 0 {
                        eprintln!(
                            "[chat-pruner] usunięto {n} nieaktywnych wątków czatu (>{} dni bez wiadomości)",
                            CHAT_INACTIVITY_DAYS
                        );
                    }
                }
                Err(e) => {
                    metrics.record(
                        "chat_pruner_scheduler",
                        t0.elapsed().as_millis() as u64,
                        false,
                        Some(e.to_string()),
                    );
                    eprintln!("[chat-pruner] błąd przebiegu: {e}");
                }
            }
            tokio::time::sleep(interval).await;
        }
    })
}
