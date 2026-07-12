//! Ustawienia Trenera AI — override promptów i temperatury (system_settings, SuperAdmin).

use axum::{Json, extract::State, http::StatusCode};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::api_error::{ApiError, api_error};
use crate::audit::write_audit_log;
use crate::middleware::auth::RequireSuperAdmin;
use crate::routes::ai_coach::{PUBLIC_SYSTEM_INSTRUCTION, SYSTEM_INSTRUCTION, mode_prefix};
use crate::state::AppState;

pub const SETTINGS_KEY: &str = "ai_coach_settings";
pub const DEFAULT_CHAT_TEMPERATURE: f32 = 0.72;
pub const DEFAULT_PUBLIC_CHAT_TEMPERATURE: f32 = 0.55;
pub const DEFAULT_VISION_CHAT_TEMPERATURE: f32 = 0.72;
pub const DEFAULT_MONTHLY_LIMIT: u32 = 300;
pub const MAX_MONTHLY_LIMIT: u32 = 50_000;

const MAX_INSTRUCTION_OVERRIDE_CHARS: usize = 14_000;
const MAX_INSTRUCTION_APPEND_CHARS: usize = 6_000;
const MAX_MODE_HINT_CHARS: usize = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AiCoachSettingsStored {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coach_instruction_append: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coach_instruction_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_instruction_append: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_instruction_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_chat_temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vision_chat_temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_plan_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_supplements_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_recovery_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_barbell_path_hint: Option<String>,
    /// Wspólna miesięczna pula zapytań panelowego AI (wszystkie role). `None` = domyślne 300.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monthly_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AiCoachSettingsDefaultsDto {
    pub chat_temperature: f32,
    pub public_chat_temperature: f32,
    pub vision_chat_temperature: f32,
    pub monthly_limit: u32,
    pub coach_instruction_preview: String,
    pub public_instruction_preview: String,
}

#[derive(Debug, Serialize)]
pub struct AiCoachSettingsResponse {
    pub settings: AiCoachSettingsStored,
    pub defaults: AiCoachSettingsDefaultsDto,
    pub has_customizations: bool,
    pub effective_coach_instruction_chars: usize,
    pub effective_public_instruction_chars: usize,
    /// Bieżące zużycie miesięcznej puli klubu (panelowe AI).
    pub club_used_this_month: u32,
    pub club_monthly_resets_label: String,
}

#[derive(Debug, Deserialize)]
pub struct AiCoachSettingsUpdateRequest {
    pub coach_instruction_append: Option<String>,
    pub coach_instruction_override: Option<String>,
    pub public_instruction_append: Option<String>,
    pub public_instruction_override: Option<String>,
    pub chat_temperature: Option<f32>,
    pub public_chat_temperature: Option<f32>,
    pub vision_chat_temperature: Option<f32>,
    pub mode_plan_hint: Option<String>,
    pub mode_supplements_hint: Option<String>,
    pub mode_recovery_hint: Option<String>,
    pub mode_barbell_path_hint: Option<String>,
    pub monthly_limit: Option<u32>,
    pub reset_to_defaults: Option<bool>,
}

fn trim_opt(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn clamp_temperature(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn validate_optional_text(
    label: &str,
    value: &Option<String>,
    max: usize,
) -> Result<Option<String>, ApiError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let t = raw.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if t.chars().count() > max {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!("{label} jest za długi (max {max} znaków)"),
        ));
    }
    Ok(Some(t.to_string()))
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max_chars {
        return t.to_string();
    }
    let cut: String = t.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{cut}…")
}

pub fn has_customizations(settings: &AiCoachSettingsStored) -> bool {
    settings.coach_instruction_append.is_some()
        || settings.coach_instruction_override.is_some()
        || settings.public_instruction_append.is_some()
        || settings.public_instruction_override.is_some()
        || settings.chat_temperature.is_some()
        || settings.public_chat_temperature.is_some()
        || settings.vision_chat_temperature.is_some()
        || settings.mode_plan_hint.is_some()
        || settings.mode_supplements_hint.is_some()
        || settings.mode_recovery_hint.is_some()
        || settings.mode_barbell_path_hint.is_some()
        || settings.monthly_limit.is_some()
}

pub fn resolve_monthly_limit(settings: &AiCoachSettingsStored) -> u32 {
    settings
        .monthly_limit
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MONTHLY_LIMIT)
}

fn validate_monthly_limit(value: Option<u32>) -> Result<Option<u32>, ApiError> {
    let Some(n) = value else {
        return Ok(None);
    };
    if n == 0 || n > MAX_MONTHLY_LIMIT {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!(
                "Miesięczny limit AI musi być między 1 a {MAX_MONTHLY_LIMIT}"
            ),
        ));
    }
    Ok(Some(n))
}

pub fn resolve_coach_system_instruction(settings: &AiCoachSettingsStored) -> String {
    if let Some(ref ov) = settings.coach_instruction_override {
        return ov.clone();
    }
    let mut out = SYSTEM_INSTRUCTION.to_string();
    if let Some(ref append) = settings.coach_instruction_append {
        out.push_str("\n\n## Wytyczne dodatkowe (SuperAdmin)\n");
        out.push_str(append);
    }
    out
}

pub fn resolve_public_system_instruction(settings: &AiCoachSettingsStored) -> String {
    if let Some(ref ov) = settings.public_instruction_override {
        return ov.clone();
    }
    let mut out = PUBLIC_SYSTEM_INSTRUCTION.to_string();
    if let Some(ref append) = settings.public_instruction_append {
        out.push_str("\n\n## Wytyczne dodatkowe (SuperAdmin)\n");
        out.push_str(append);
    }
    out
}

pub fn resolve_chat_temperature(settings: &AiCoachSettingsStored) -> f32 {
    settings
        .chat_temperature
        .map(clamp_temperature)
        .unwrap_or(DEFAULT_CHAT_TEMPERATURE)
}

pub fn resolve_public_chat_temperature(settings: &AiCoachSettingsStored) -> f32 {
    settings
        .public_chat_temperature
        .map(clamp_temperature)
        .unwrap_or(DEFAULT_PUBLIC_CHAT_TEMPERATURE)
}

pub fn resolve_vision_chat_temperature(settings: &AiCoachSettingsStored) -> f32 {
    settings
        .vision_chat_temperature
        .map(clamp_temperature)
        .unwrap_or(DEFAULT_VISION_CHAT_TEMPERATURE)
}

pub fn resolve_mode_prefix(mode: &str, settings: &AiCoachSettingsStored) -> String {
    let custom = match mode {
        "plan" => settings.mode_plan_hint.as_deref(),
        "supplements" => settings.mode_supplements_hint.as_deref(),
        "recovery" => settings.mode_recovery_hint.as_deref(),
        "barbell_path" => settings.mode_barbell_path_hint.as_deref(),
        _ => None,
    };
    if let Some(hint) = custom.filter(|s| !s.trim().is_empty()) {
        return format!("[Tryb: {mode}]\n{hint}\n\n");
    }
    mode_prefix(mode).to_string()
}

pub async fn load_ai_coach_settings(state: &AppState) -> Result<AiCoachSettingsStored, ApiError> {
    let mut rows = state
        .db
        .query(
            "SELECT value FROM system_settings WHERE key = ?1 LIMIT 1",
            [SETTINGS_KEY.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    else {
        return Ok(AiCoachSettingsStored::default());
    };

    let raw: String = row
        .get(0)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    serde_json::from_str(&raw).map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Nieprawidłowe ai_coach_settings w bazie: {e}"),
        )
    })
}

async fn save_ai_coach_settings(
    state: &AppState,
    settings: &AiCoachSettingsStored,
) -> Result<(), ApiError> {
    let json = serde_json::to_string(settings).map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Serializacja ustawień AI: {e}"),
        )
    })?;
    state
        .db
        .execute(
            "INSERT INTO system_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (SETTINGS_KEY.to_string(), json),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(())
}

async fn build_response(state: &AppState, settings: AiCoachSettingsStored) -> AiCoachSettingsResponse {
    let effective_coach = resolve_coach_system_instruction(&settings);
    let effective_public = resolve_public_system_instruction(&settings);
    let club_used = crate::ai_coach_monthly::count_club_monthly_usage(state).await;
    AiCoachSettingsResponse {
        has_customizations: has_customizations(&settings),
        effective_coach_instruction_chars: effective_coach.chars().count(),
        effective_public_instruction_chars: effective_public.chars().count(),
        club_used_this_month: club_used,
        club_monthly_resets_label: crate::ai_coach_monthly::next_month_reset_label_pl(),
        defaults: AiCoachSettingsDefaultsDto {
            chat_temperature: DEFAULT_CHAT_TEMPERATURE,
            public_chat_temperature: DEFAULT_PUBLIC_CHAT_TEMPERATURE,
            vision_chat_temperature: DEFAULT_VISION_CHAT_TEMPERATURE,
            monthly_limit: DEFAULT_MONTHLY_LIMIT,
            coach_instruction_preview: preview_text(SYSTEM_INSTRUCTION, 480),
            public_instruction_preview: preview_text(PUBLIC_SYSTEM_INSTRUCTION, 320),
        },
        settings,
    }
}

fn normalize_update(payload: AiCoachSettingsUpdateRequest) -> Result<AiCoachSettingsStored, ApiError> {
    if payload.reset_to_defaults == Some(true) {
        return Ok(AiCoachSettingsStored::default());
    }

    Ok(AiCoachSettingsStored {
        coach_instruction_append: validate_optional_text(
            "Dodatek instrukcji trenera",
            &trim_opt(payload.coach_instruction_append),
            MAX_INSTRUCTION_APPEND_CHARS,
        )?,
        coach_instruction_override: validate_optional_text(
            "Nadpisanie instrukcji trenera",
            &trim_opt(payload.coach_instruction_override),
            MAX_INSTRUCTION_OVERRIDE_CHARS,
        )?,
        public_instruction_append: validate_optional_text(
            "Dodatek instrukcji publicznej",
            &trim_opt(payload.public_instruction_append),
            MAX_INSTRUCTION_APPEND_CHARS,
        )?,
        public_instruction_override: validate_optional_text(
            "Nadpisanie instrukcji publicznej",
            &trim_opt(payload.public_instruction_override),
            MAX_INSTRUCTION_OVERRIDE_CHARS,
        )?,
        chat_temperature: payload.chat_temperature.map(clamp_temperature),
        public_chat_temperature: payload.public_chat_temperature.map(clamp_temperature),
        vision_chat_temperature: payload.vision_chat_temperature.map(clamp_temperature),
        mode_plan_hint: validate_optional_text(
            "Hint trybu plan",
            &trim_opt(payload.mode_plan_hint),
            MAX_MODE_HINT_CHARS,
        )?,
        mode_supplements_hint: validate_optional_text(
            "Hint trybu suplementacja",
            &trim_opt(payload.mode_supplements_hint),
            MAX_MODE_HINT_CHARS,
        )?,
        mode_recovery_hint: validate_optional_text(
            "Hint trybu regeneracja",
            &trim_opt(payload.mode_recovery_hint),
            MAX_MODE_HINT_CHARS,
        )?,
        mode_barbell_path_hint: validate_optional_text(
            "Hint trybu tor sztangi",
            &trim_opt(payload.mode_barbell_path_hint),
            MAX_MODE_HINT_CHARS,
        )?,
        monthly_limit: validate_monthly_limit(payload.monthly_limit)?,
        updated_at: None,
        updated_by: None,
    })
}

pub async fn coach_get_settings(
    State(state): State<AppState>,
    RequireSuperAdmin(_claims): RequireSuperAdmin,
) -> Result<Json<AiCoachSettingsResponse>, ApiError> {
    let settings = load_ai_coach_settings(&state).await?;
    Ok(Json(build_response(&state, settings).await))
}

pub async fn coach_update_settings(
    State(state): State<AppState>,
    RequireSuperAdmin(claims): RequireSuperAdmin,
    Json(payload): Json<AiCoachSettingsUpdateRequest>,
) -> Result<Json<AiCoachSettingsResponse>, ApiError> {
    let is_reset = payload.reset_to_defaults == Some(true);
    let mut settings = normalize_update(payload)?;
    settings.updated_at = Some(Utc::now().to_rfc3339());
    settings.updated_by = Some(claims.sub.clone());

    save_ai_coach_settings(&state, &settings).await?;

    let conn_arc = state.db_conn().await?;
    let details = serde_json::json!({
        "has_customizations": has_customizations(&settings),
        "reset": is_reset,
    })
    .to_string();
    let _ = write_audit_log(
        conn_arc.as_ref(),
        Some(&claims.sub),
        Some("SuperAdmin"),
        "ai_coach_settings",
        if is_reset { "reset" } else { "update" },
        Some("system_settings"),
        Some(SETTINGS_KEY),
        Some(&details),
    )
    .await;

    Ok(Json(build_response(&state, settings).await))
}
