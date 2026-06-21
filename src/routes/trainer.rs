use axum::{Json, extract::State};
use serde::Serialize;

use crate::api_error::ApiError;
use crate::middleware::auth::RequireTrainerOrHigher;
use crate::models::CompetitionResult;
use crate::routes::payments::{PendingPaymentRow, fetch_all_pending_payments};
use crate::routes::results::fetch_all_pending_results;
use crate::routes::system_logs::{
    TrainerMonitoringSummary, trainer_monitoring_summary_for_state,
};
use crate::state::AppState;

#[derive(Serialize)]
pub struct TrainerDashboardResponse {
    pub pending_results: Vec<CompetitionResult>,
    pub pending_payments: Vec<PendingPaymentRow>,
    pub monitoring_summary: TrainerMonitoringSummary,
}

/// Agregowany payload dashboardu trenera — jeden round-trip zamiast wielu GET (wyniki, składki, KPI).
pub(crate) async fn trainer_dashboard_for_state(
    state: &AppState,
) -> Result<TrainerDashboardResponse, ApiError> {
    let pending_results = fetch_all_pending_results(state).await?;
    let pending_payments = fetch_all_pending_payments(state).await?;
    let monitoring_summary = trainer_monitoring_summary_for_state(state).await?;
    Ok(TrainerDashboardResponse {
        pending_results,
        pending_payments,
        monitoring_summary,
    })
}

pub async fn trainer_dashboard_handler(
    State(state): State<AppState>,
    _auth: RequireTrainerOrHigher,
) -> Result<Json<TrainerDashboardResponse>, ApiError> {
    Ok(Json(trainer_dashboard_for_state(&state).await?))
}
