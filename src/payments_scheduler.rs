//! Auto-składki dla zawodników z aktywnym przelewem stałym.
//!
//! Logika: dla każdego aktywnego zawodnika z `has_standing_order = 1`, jeśli w bieżącym
//! miesiącu nie istnieje jeszcze Approved-wpis w `membership_payments`, scheduler tworzy
//! taki wpis (kwota = `MONTHLY_FEE_PLN`, status = `Approved`, autor = `system`).
//!
//! Operacja jest **idempotentna** — sprawdzamy obecność Approved przed insertem, więc
//! kolejne uruchomienia nie generują duplikatów. Bezpiecznie więc uruchamiać scheduler
//! kilka razy dziennie.

use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Utc};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::audit::write_audit_log;
use crate::state::Db;
use crate::worker_metrics::WorkerMetrics;

/// Składka miesięczna w PLN (musi być spójna z `routes::payments::MONTHLY_FEE_PLN`).
const MONTHLY_FEE_PLN: f64 = 50.0;

/// Co ile godzin scheduler robi przegląd. 12 h = 2× dziennie — wystarczy, żeby na pewno
/// łapać start nowego miesiąca i nie obciążać DB.
const RUN_INTERVAL_HOURS: u64 = 12;

fn current_month_yyyy_mm() -> String {
    let now = Utc::now().date_naive();
    format!("{:04}-{:02}", now.year(), now.month())
}

struct PendingAthlete {
    id: String,
    full_name: String,
}

/// Pobiera zawodników z włączonym przelewem stałym, którzy nie mają jeszcze
/// Approved-wpisu w `membership_payments` dla wskazanego miesiąca.
async fn select_athletes_needing_auto_payment(
    conn: &Db,
    month: &str,
) -> Result<Vec<PendingAthlete>, libsql::Error> {
    let mut rows = conn
        .query(
            "SELECT a.id, a.full_name \
             FROM athletes a \
             WHERE COALESCE(a.has_standing_order, 0) = 1 \
               AND (a.is_active IS NULL OR a.is_active = 1) \
               AND NOT EXISTS ( \
                   SELECT 1 FROM membership_payments p \
                   WHERE p.athlete_id = a.id AND p.month = ?1 AND p.status = 'Approved' \
               )",
            [month.to_string()],
        )
        .await?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0).unwrap_or_default();
        let full_name: String = row.get(1).unwrap_or_else(|_| "?".to_string());
        if !id.is_empty() {
            out.push(PendingAthlete { id, full_name });
        }
    }
    Ok(out)
}

async fn insert_auto_approved_payment(
    conn: &Db,
    athlete_id: &str,
    month: &str,
) -> Result<(), libsql::Error> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO membership_payments (id, athlete_id, month, amount_pln, note, status, created_by_user_id, created_at, approved_by_user_id, approved_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'Approved', NULL, ?6, NULL, ?7)",
        (
            id,
            athlete_id.to_string(),
            month.to_string(),
            Some(MONTHLY_FEE_PLN),
            Some("Przelew stały (auto)".to_string()),
            now.clone(),
            now,
        ),
    )
    .await?;
    Ok(())
}

/// Wykonuje jeden przebieg auto-składki dla bieżącego miesiąca.
///
/// Zwraca liczbę utworzonych Approved-wpisów. Bezpieczne do wywołania ręcznego
/// (np. ze skryptu admin / endpointu diagnostycznego).
pub async fn run_standing_orders_for_current_month(conn: &Db) -> Result<usize, libsql::Error> {
    let month = current_month_yyyy_mm();
    let pending = select_athletes_needing_auto_payment(conn, &month).await?;
    if pending.is_empty() {
        return Ok(0);
    }

    let mut created = 0usize;
    for ath in &pending {
        if let Err(e) = insert_auto_approved_payment(conn, &ath.id, &month).await {
            tracing::error!(
                athlete_id = %ath.id,
                athlete_name = %ath.full_name,
                error = %e,
                "standing-order: błąd insertu auto-składki"
            );
            continue;
        }
        created += 1;
        let details = serde_json::json!({
            "athlete_id": ath.id,
            "athlete_name": ath.full_name,
            "month": month,
            "amount_pln": MONTHLY_FEE_PLN,
            "reason": "standing_order_auto",
        })
        .to_string();
        let audit_conn = conn.raw().await;
        let _ = write_audit_log(
            audit_conn.as_ref(),
            None,
            Some("system"),
            "payments",
            "standing_order_auto_created",
            Some("athlete"),
            Some(&ath.id),
            Some(&details),
        )
        .await;
    }

    Ok(created)
}

/// Uruchamia wieczne zadanie w tle: co kilka godzin tworzy auto-składki dla bieżącego
/// miesiąca dla zawodników z włączonym przelewem stałym.
///
/// Pierwszy przebieg jest opóźniony o 30 sekund (żeby aplikacja zdążyła wstać i
/// migracje były na pewno dokończone), a kolejne — co `RUN_INTERVAL_HOURS` godzin.
pub fn spawn_standing_order_task(db: Db, metrics: Arc<WorkerMetrics>) -> JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let interval = Duration::from_secs(RUN_INTERVAL_HOURS * 3600);
        loop {
            let t0 = std::time::Instant::now();
            match run_standing_orders_for_current_month(&db).await {
                Ok(n) => {
                    metrics.record(
                        "standing_order_scheduler",
                        t0.elapsed().as_millis() as u64,
                        true,
                        Some(format!("created_auto_payments={n}")),
                    );
                    if n > 0 {
                        tracing::info!(
                            created = n,
                            "standing-order scheduler: utworzono auto-składki"
                        );
                    }
                }
                Err(e) => {
                    metrics.record(
                        "standing_order_scheduler",
                        t0.elapsed().as_millis() as u64,
                        false,
                        Some(e.to_string()),
                    );
                    tracing::error!(error = %e, "standing-order scheduler: błąd przebiegu");
                }
            }
            tokio::time::sleep(interval).await;
        }
    })
}
