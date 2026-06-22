//! Wspólny miesięczny limit zapytań AI klubu (wszystkie role, panelowe endpointy).
//! Publiczny asystent (`/api/ai/coach/public/*`) nie zużywa tego puli.
//! Limit konfigurowalny w devtools (`ai_coach_settings.monthly_limit`).

use chrono::{Datelike, Utc};
use serde::{Deserialize, Serialize};

use crate::post_throttle::AiCoachLimitDeny;
use crate::routes::ai_coach_settings::{load_ai_coach_settings, resolve_monthly_limit};
use crate::state::AppState;

const SETTINGS_KEY: &str = "ai_coach_monthly_usage";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct MonthlyUsageStore {
    month: String,
    count: u32,
}

pub async fn monthly_limit_max(state: &AppState) -> u32 {
    let settings = load_ai_coach_settings(state).await.unwrap_or_default();
    resolve_monthly_limit(&settings)
}

fn current_month_utc() -> String {
    Utc::now().format("%Y-%m").to_string()
}

/// Etykieta odnowienia limitu (pierwszy dzień następnego miesiąca, PL).
pub fn next_month_reset_label_pl() -> String {
    let now = Utc::now();
    let (year, month) = if now.month() == 12 {
        (now.year() + 1, 1)
    } else {
        (now.year(), now.month() + 1)
    };
    format!("1.{month:02}.{year}")
}

async fn load_usage(state: &AppState) -> MonthlyUsageStore {
    let mut rows = match state
        .db
        .query(
            "SELECT value FROM system_settings WHERE key = ?1 LIMIT 1",
            [SETTINGS_KEY],
        )
        .await
    {
        Ok(rows) => rows,
        Err(_) => return MonthlyUsageStore::default(),
    };
    let row = match rows.next().await {
        Ok(Some(row)) => row,
        _ => return MonthlyUsageStore::default(),
    };
    let raw: String = row.get(0).unwrap_or_default();
    serde_json::from_str(&raw).unwrap_or_default()
}

async fn save_usage(state: &AppState, store: &MonthlyUsageStore) -> Result<(), ()> {
    let json = serde_json::to_string(store).map_err(|_| ())?;
    state
        .db
        .execute(
            "INSERT INTO system_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (SETTINGS_KEY, json),
        )
        .await
        .map_err(|_| ())?;
    Ok(())
}

pub async fn count_club_monthly_usage(state: &AppState) -> u32 {
    let stored = load_usage(state).await;
    if stored.month == current_month_utc() {
        stored.count
    } else {
        0
    }
}

pub async fn club_monthly_exhausted(state: &AppState) -> bool {
    count_club_monthly_usage(state).await >= monthly_limit_max(state).await
}

/// Rezerwuje slot w miesięcznej puli klubu (panelowe AI).
pub async fn reserve_club_monthly_slot(state: &AppState) -> Result<(), AiCoachLimitDeny> {
    let month = current_month_utc();
    let max = monthly_limit_max(state).await;
    let mut stored = load_usage(state).await;
    if stored.month != month {
        stored = MonthlyUsageStore {
            month: month.clone(),
            count: 0,
        };
    }
    if stored.count >= max {
        return Err(AiCoachLimitDeny::ClubMonthly);
    }
    stored.count += 1;
    save_usage(state, &stored)
        .await
        .map_err(|_| AiCoachLimitDeny::ClubMonthly)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::routes::ai_coach_settings::{AiCoachSettingsStored, DEFAULT_MONTHLY_LIMIT, resolve_monthly_limit};

    #[test]
    fn monthly_limit_default_is_positive() {
        assert!(resolve_monthly_limit(&AiCoachSettingsStored::default()) >= 1);
        assert_eq!(
            resolve_monthly_limit(&AiCoachSettingsStored::default()),
            DEFAULT_MONTHLY_LIMIT
        );
    }
}
