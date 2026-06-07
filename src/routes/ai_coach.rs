//! Trener AI (Gemini) — czat, plany treningowe, suplementacja, regeneracja.

use axum::{
    Json,
    extract::State,
    http::StatusCode,
};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::audit::write_audit_log;
use crate::middleware::auth::{Claims, claims_has_staff_access};
use crate::models::Role;
use crate::state::AppState;

const SYSTEM_INSTRUCTION: &str = r#"Jesteś Slavia AI Trener — wirtualny trener dwuboju olimpijskiego (weightlifting) w ekosystemie klubu CKS Slavia Ruda Śląska.

Twoja ekspertyza obejmuje:

1. **Dwubój olimpijski — faza eksplozyjna**
   - Rwanie (snatch): start, first pull, transition, second pull, turnover, złapanie overhead, stabilizacja.
   - Podrzut (clean & jerk): clean (pull, turnover, front rack), jerk (dip-drive, split/power/squat jerk), lockout.
   - Timing triple extension, pozycje stóp, praca barków/łokci, mobilność kostek/bioder/klatki pod start olimpijski.
   - Akcesoria: hang/block pulls, muscle snatch, overhead squat, push press, front/back squat, Romanian deadlift, core anti-rotation.

2. **Plany treningowe**
   - Mikrocykle 1–4 tygodnie, objętość/intensywność, % CM lub PR, RPE, progresja, deload, taper przed zawodami.
   - Struktura tygodnia (np. 3–6 dni), priorytety rwanie vs podrzut, dni ciężkie/lekkie/techniczne.
   - Format planu: dzień → blok główny → akcesoria → uwagi techniczne i czas trwania.

3. **Suplementacja sportowa (siłownia + dwubój)**
   - Kreatyna, kofeina, beta-alanina, białko, omega-3, witamina D, elektrolity, kolagen (ostrożnie przy kontuzjach ścięgien).
   - Dawki orientacyjne, timing, interakcje, co ma sens dla podnoszenia ciężarów — zawsze jako edukacja, nie recepta.
   - Przy chorobach przewlekłych, ciąży, lekach — odsyłaj do lekarza/dietetyka.

4. **Kontuzje i plany regeneracyjne**
   - Przeciążenia: kolano (patellar/quadriceps tendinopathy), łokieć (typowe w C&J), nadgarstek, bark, dół pleców.
   - Return-to-training: regresja obciążeń, ćwiczenia izometryczne/eccentric, mobilność, sen, deload, kiedy do fizjoterapeuty/ortopedy.
   - Nie stawiaj diagnoz medycznych — przy ostrym bólu, obrzęku, drętwieniu, utracie siły: natychmiast specjalista.

Zasady odpowiedzi:
- Pisz po polsku, konkretnie, z nagłówkami i listami; przy planach używaj tabel lub sekcji per dzień (pon–nd).
- Używaj terminologii PL + EN w nawiasie przy pierwszym użyciu (np. rwanie / snatch).
- Jesteś trenerem pomocniczym — przy ważnych decyzjach zachęcaj do konsultacji z trenerem klubu.
- Nie wymyślaj danych zawodnika — jeśli brak kontekstu, zapytaj lub podaj plan szablonowy z placeholderami."#;

#[derive(Debug, Deserialize)]
pub struct AiCoachHistoryMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct AiCoachPlanContext {
    pub training_days_per_week: Option<u8>,
    pub experience: Option<String>,
    pub snatch_max_kg: Option<f64>,
    pub clean_jerk_max_kg: Option<f64>,
    pub squat_max_kg: Option<f64>,
    pub goal: Option<String>,
    pub injuries: Option<String>,
    pub week_start: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AiCoachChatRequest {
    pub message: String,
    /// chat | plan | supplements | recovery
    pub mode: Option<String>,
    pub history: Option<Vec<AiCoachHistoryMessage>>,
    /// Kadra: kontekst zawodnika (PB, regeneracja).
    pub athlete_id: Option<String>,
    pub plan_context: Option<AiCoachPlanContext>,
}

#[derive(Debug, Serialize)]
pub struct AiCoachChatResponse {
    pub reply: String,
    pub model: String,
}

#[derive(Debug, Serialize)]
pub struct AiCoachStatusResponse {
    pub configured: bool,
    pub model: String,
}

#[derive(Serialize)]
struct GeminiPart {
    text: String,
}

#[derive(Serialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize)]
struct GeminiSystemInstruction {
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize)]
pub struct AiCoachImportPlanResponse {
    pub plan_id: String,
    pub title: String,
    pub items_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct AiCoachImportPlanRequest {
    pub plan_text: String,
    pub athlete_id: String,
    pub title: Option<String>,
    pub week_start: Option<String>,
    pub goal: Option<String>,
    pub status: Option<String>,
    pub coach_note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ParsedPlanImport {
    title: Option<String>,
    goal: Option<String>,
    items: Vec<ParsedPlanItem>,
}

#[derive(Debug, Deserialize)]
struct ParsedPlanItem {
    day_of_week: i32,
    custom_exercise_name: String,
    sets: Option<i32>,
    reps: Option<i32>,
    intensity_percent: Option<f64>,
    weight_kg: Option<f64>,
    notes: Option<String>,
    sort_order: Option<i32>,
}

const IMPORT_SYSTEM_INSTRUCTION: &str = r#"Jesteś parserem planów treningowych dwuboju olimpijskiego. Na podstawie tekstu planu zwróć WYŁĄCZNIE poprawny JSON (bez markdown, bez komentarzy) w schemacie:
{
  "title": "krótki tytuł tygodnia",
  "goal": "cel tygodnia",
  "items": [
    {
      "day_of_week": 1,
      "custom_exercise_name": "nazwa ćwiczenia",
      "sets": 5,
      "reps": 2,
      "intensity_percent": 75.0,
      "weight_kg": null,
      "notes": "uwagi techniczne",
      "sort_order": 0
    }
  ]
}
Zasady:
- day_of_week: 1=poniedziałek, 2=wtorek, …, 7=niedziela
- Każde ćwiczenie = osobny element items
- sort_order rośnie w obrębie dnia od 0
- intensity_percent i weight_kg — użyj tego, co wynika z tekstu; brak = null
- Nie pomijaj ćwiczeń głównych ani akcesoriów wymienionych w planie"#;

#[derive(Serialize)]
struct GeminiGenerationConfig {
    temperature: f32,
    max_output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_mime_type: Option<String>,
}

#[derive(Serialize)]
struct GeminiRequest {
    system_instruction: GeminiSystemInstruction,
    contents: Vec<GeminiContent>,
    generation_config: GeminiGenerationConfig,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiResponseContent>,
}

#[derive(Deserialize)]
struct GeminiResponseContent {
    parts: Option<Vec<GeminiResponsePart>>,
}

#[derive(Deserialize)]
struct GeminiResponsePart {
    text: Option<String>,
}

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    error: Option<GeminiErrorBody>,
}

#[derive(Deserialize)]
struct GeminiErrorBody {
    message: Option<String>,
}

fn coach_role_allowed(claims: &Claims) -> bool {
    claims.roles.iter().any(|r| {
        matches!(
            r,
            crate::models::Role::Athlete
                | crate::models::Role::Trainer
                | crate::models::Role::Admin
                | crate::models::Role::SuperAdmin
        )
    })
}

fn normalize_mode(raw: Option<&str>) -> &'static str {
    match raw.unwrap_or("chat").trim().to_lowercase().as_str() {
        "plan" | "training_plan" | "plan_treningowy" => "plan",
        "supplements" | "suplementacja" | "suplementy" => "supplements",
        "recovery" | "regeneracja" | "kontuzja" | "kontuzje" => "recovery",
        _ => "chat",
    }
}

fn mode_prefix(mode: &str) -> &'static str {
    match mode {
        "plan" => "[Tryb: generator planu treningowego]\nWygeneruj szczegółowy plan tygodniowy dwuboju olimpijskiego (faza eksplozywna). Użyj dni tygodnia, % CM/RPE, serie×powt., akcesoria.\n\n",
        "supplements" => "[Tryb: suplementacja sportowa]\nSkup się na suplementacji pod siłownię i dwubój olimpijski — dawki orientacyjne, timing, bezpieczeństwo, disclaimer medyczny.\n\n",
        "recovery" => "[Tryb: kontuzje i regeneracja]\nSkup się na bezpiecznym powrocie do treningu, regresji obciążeń, mobility i kiedy iść do specjalisty. Nie diagnozuj.\n\n",
        _ => "",
    }
}

fn format_plan_context(ctx: &AiCoachPlanContext) -> String {
    let mut lines = vec!["Kontekst planu:".to_string()];
    if let Some(d) = ctx.training_days_per_week {
        lines.push(format!("- Dni treningowe w tygodniu: {d}"));
    }
    if let Some(ref e) = ctx.experience {
        if !e.trim().is_empty() {
            lines.push(format!("- Doświadczenie: {}", e.trim()));
        }
    }
    if let Some(v) = ctx.snatch_max_kg {
        lines.push(format!("- CM rwanie: {v} kg"));
    }
    if let Some(v) = ctx.clean_jerk_max_kg {
        lines.push(format!("- CM podrzut: {v} kg"));
    }
    if let Some(v) = ctx.squat_max_kg {
        lines.push(format!("- CM przysiad: {v} kg"));
    }
    if let Some(ref g) = ctx.goal {
        if !g.trim().is_empty() {
            lines.push(format!("- Cel: {}", g.trim()));
        }
    }
    if let Some(ref i) = ctx.injuries {
        if !i.trim().is_empty() {
            lines.push(format!("- Kontuzje / ograniczenia: {}", i.trim()));
        }
    }
    if let Some(ref w) = ctx.week_start {
        if !w.trim().is_empty() {
            lines.push(format!("- Tydzień od: {}", w.trim()));
        }
    }
    if let Some(ref n) = ctx.notes {
        if !n.trim().is_empty() {
            lines.push(format!("- Notatki: {}", n.trim()));
        }
    }
    if lines.len() == 1 {
        String::new()
    } else {
        lines.join("\n") + "\n\n"
    }
}

async fn fetch_athlete_context(state: &AppState, athlete_id: &str) -> Option<String> {
    let mut rows = state
        .db
        .query(
            "SELECT full_name, weight_category, best_snatch_kg, best_clean_jerk_kg, total_kg, bodyweight \
             FROM athletes WHERE id = ?1 LIMIT 1",
            [athlete_id.to_string()],
        )
        .await
        .ok()?;
    let row = rows.next().await.ok()??;
    let name: String = row.get(0).unwrap_or_default();
    let cat: Option<String> = row.get(1).ok();
    let snatch: Option<f64> = row.get(2).ok();
    let cj: Option<f64> = row.get(3).ok();
    let total: Option<f64> = row.get(4).ok();
    let bw: Option<f64> = row.get(5).ok();

    let mut parts = vec![format!("Profil zawodnika: {name}")];
    if let Some(c) = cat.filter(|s| !s.trim().is_empty()) {
        parts.push(format!("Kategoria: {c}"));
    }
    if let Some(v) = bw {
        parts.push(format!("Masa ciała: {v} kg"));
    }
    if let Some(v) = snatch {
        parts.push(format!("PB rwanie: {v} kg"));
    }
    if let Some(v) = cj {
        parts.push(format!("PB podrzut: {v} kg"));
    }
    if let Some(v) = total {
        parts.push(format!("PB dwubój: {v} kg"));
    }

    if let Ok(mut rec) = state
        .db
        .query(
            "SELECT date, sleep_hours, fatigue_level, soreness_level, readiness_level, note \
             FROM recovery_logs WHERE athlete_id = ?1 ORDER BY date DESC LIMIT 3",
            [athlete_id.to_string()],
        )
        .await
    {
        let mut rec_lines = Vec::new();
        while let Ok(Some(r)) = rec.next().await {
            let date: String = r.get(0).unwrap_or_default();
            let sleep: Option<f64> = r.get(1).ok();
            let fatigue: Option<i64> = r.get(2).ok();
            let soreness: Option<i64> = r.get(3).ok();
            let readiness: Option<i64> = r.get(4).ok();
            let notes: Option<String> = r.get(5).ok();
            let mut line = format!("{date}:");
            if let Some(s) = sleep {
                line.push_str(&format!(" sen {s}h,"));
            }
            if let Some(f) = fatigue {
                line.push_str(&format!(" zmęczenie {f}/10,"));
            }
            if let Some(s) = soreness {
                line.push_str(&format!(" DOMS {s}/10,"));
            }
            if let Some(r) = readiness {
                line.push_str(&format!(" gotowość {r}/10,"));
            }
            if let Some(n) = notes.filter(|s| !s.trim().is_empty()) {
                line.push_str(&format!(" notatka: {n}"));
            }
            rec_lines.push(line.trim_end_matches(',').to_string());
        }
        if !rec_lines.is_empty() {
            parts.push("Ostatnie check-iny regeneracji:\n".to_string() + &rec_lines.join("\n"));
        }
    }

    Some(parts.join("\n"))
}

async fn fetch_athlete_context_for_user(state: &AppState, user_id: &str) -> Option<String> {
    let mut rows = state
        .db
        .query(
            "SELECT id FROM athletes WHERE user_id = ?1 LIMIT 1",
            [user_id.to_string()],
        )
        .await
        .ok()?;
    let row = rows.next().await.ok()??;
    let athlete_id: String = row.get(0).ok()?;
    fetch_athlete_context(state, &athlete_id).await
}

async fn call_gemini(
    state: &AppState,
    user_text: String,
    history: &[AiCoachHistoryMessage],
    system_instruction: &str,
    temperature: f32,
    json_response: bool,
) -> Result<String, ApiError> {
    let api_key = state.gemini_api_key.trim();
    if api_key.is_empty() {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Trener AI nie jest skonfigurowany (brak GEMINI_API_KEY na backendzie)",
        ));
    }

    let model = if state.gemini_model.trim().is_empty() {
        "gemini-2.0-flash".to_string()
    } else {
        state.gemini_model.trim().to_string()
    };

    let mut contents: Vec<GeminiContent> = Vec::new();
    for h in history {
        let role = if h.role == "assistant" || h.role == "model" {
            "model"
        } else {
            "user"
        };
        let text = h.content.trim();
        if text.is_empty() {
            continue;
        }
        contents.push(GeminiContent {
            role: role.to_string(),
            parts: vec![GeminiPart {
                text: text.to_string(),
            }],
        });
    }
    contents.push(GeminiContent {
        role: "user".to_string(),
        parts: vec![GeminiPart { text: user_text }],
    });

    let body = GeminiRequest {
        system_instruction: GeminiSystemInstruction {
            parts: vec![GeminiPart {
                text: system_instruction.to_string(),
            }],
        },
        contents,
        generation_config: GeminiGenerationConfig {
            temperature,
            max_output_tokens: 8192,
            response_mime_type: if json_response {
                Some("application/json".to_string())
            } else {
                None
            },
        },
    };

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}"
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let res = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Błąd połączenia z Gemini: {e}"),
            )
        })?;

    let status = res.status();
    let parsed: GeminiResponse = res.json().await.map_err(|e| {
        api_error(
            StatusCode::BAD_GATEWAY,
            format!("Nieprawidłowa odpowiedź Gemini: {e}"),
        )
    })?;

    if !status.is_success() {
        let msg = parsed
            .error
            .and_then(|e| e.message)
            .unwrap_or_else(|| format!("Gemini HTTP {status}"));
        return Err(api_error(StatusCode::BAD_GATEWAY, msg));
    }

    let text = parsed
        .candidates
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.content)
        .and_then(|c| c.parts)
        .and_then(|p| p.into_iter().next())
        .and_then(|p| p.text)
        .unwrap_or_default()
        .trim()
        .to_string();

    if text.is_empty() {
        return Err(api_error(
            StatusCode::BAD_GATEWAY,
            "Gemini zwróciło pustą odpowiedź",
        ));
    }

    Ok(text)
}

fn extract_json_payload(raw: &str) -> String {
    let mut t = raw.trim();
    if t.starts_with("```") {
        t = t
            .strip_prefix("```json")
            .or_else(|| t.strip_prefix("```"))
            .unwrap_or(t);
        if let Some(end) = t.rfind("```") {
            t = t[..end].trim();
        }
    }
    t.to_string()
}

fn normalize_plan_status(s: Option<&str>) -> String {
    let raw = s.unwrap_or("planned").trim().to_lowercase();
    if matches!(raw.as_str(), "planned" | "active" | "completed" | "paused") {
        raw
    } else {
        "planned".to_string()
    }
}

fn clamp_day_of_week(d: i32) -> i32 {
    d.clamp(1, 7)
}

fn staff_actor_label(claims: &Claims) -> &'static str {
    if claims.roles.contains(&Role::SuperAdmin) {
        "SuperAdmin"
    } else if claims.roles.contains(&Role::Admin) {
        "Admin"
    } else {
        "Trainer"
    }
}

pub async fn coach_import_plan(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<AiCoachImportPlanRequest>,
) -> Result<Json<AiCoachImportPlanResponse>, ApiError> {
    if !claims_has_staff_access(&claims) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Import planu wymaga uprawnień kadry",
        ));
    }

    let plan_text = payload.plan_text.trim();
    if plan_text.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Tekst planu nie może być pusty",
        ));
    }
    if payload.athlete_id.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Wybierz zawodnika do przypisania planu",
        ));
    }

    let parse_prompt = format!(
        "Przekształć poniższy plan treningowy na JSON według schematu z instrukcji systemowej.\n\n---\n{plan_text}\n---"
    );
    let json_raw = call_gemini(
        &state,
        parse_prompt,
        &[],
        IMPORT_SYSTEM_INSTRUCTION,
        0.2,
        true,
    )
    .await?;
    let json_clean = extract_json_payload(&json_raw);
    let parsed: ParsedPlanImport = serde_json::from_str(&json_clean).map_err(|e| {
        api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("Nie udało się sparsować planu AI: {e}"),
        )
    })?;

    if parsed.items.is_empty() {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Plan AI nie zawiera ćwiczeń do importu",
        ));
    }

    let week_start = payload
        .week_start
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());

    let title = payload
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            parsed
                .title
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("Plan AI — {week_start}"));

    let goal = payload
        .goal
        .as_ref()
        .or(parsed.goal.as_ref())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let coach_note = payload
        .coach_note
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let status = normalize_plan_status(payload.status.as_deref());
    let plan_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let athlete_id = payload.athlete_id.trim().to_string();

    state
        .db
        .execute(
            "INSERT INTO training_plans (id, athlete_id, title, goal, week_start, status, coach_note, progress_percent, created_by, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, ?9)",
            (
                plan_id.clone(),
                athlete_id.clone(),
                title.clone(),
                goal.clone(),
                week_start.clone(),
                status.clone(),
                coach_note.clone(),
                claims.sub.clone(),
                now.clone(),
            ),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut sort_by_day: std::collections::HashMap<i32, i32> = std::collections::HashMap::new();
    let items_count = parsed.items.len();

    for item in parsed.items {
        let name = item.custom_exercise_name.trim();
        if name.is_empty() {
            continue;
        }
        let day = clamp_day_of_week(item.day_of_week);
        let order = item.sort_order.unwrap_or_else(|| {
            let n = sort_by_day.entry(day).or_insert(0);
            let v = *n;
            *n += 1;
            v
        });
        let item_id = Uuid::new_v4().to_string();
        let notes = item
            .notes
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        state
            .db
            .execute(
                "INSERT INTO training_plan_items (id, plan_id, day_of_week, exercise_id, custom_exercise_name, sets, reps, intensity_percent, weight_kg, notes, sort_order, created_at) \
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                (
                    item_id,
                    plan_id.clone(),
                    day,
                    name.to_string(),
                    item.sets,
                    item.reps,
                    item.intensity_percent,
                    item.weight_kg,
                    notes,
                    order,
                    now.clone(),
                ),
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    crate::notifications::notify_training_plan_assigned(
        &state,
        &athlete_id,
        &title,
        &week_start,
    );

    let details = serde_json::json!({
        "title": title,
        "week_start": week_start,
        "status": status,
        "items_count": items_count,
        "source": "ai_coach_import"
    })
    .to_string();
    let conn_arc = state.db.raw().await;
    let _ = write_audit_log(
        conn_arc.as_ref(),
        Some(&claims.sub),
        Some(staff_actor_label(&claims)),
        "training_plan",
        "create",
        Some("athlete"),
        Some(&athlete_id),
        Some(&details),
    )
    .await;

    Ok(Json(AiCoachImportPlanResponse {
        plan_id,
        title,
        items_count,
    }))
}

pub async fn coach_status(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<AiCoachStatusResponse>, ApiError> {
    if !coach_role_allowed(&claims) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Brak uprawnień do trenera AI",
        ));
    }
    let model = if state.gemini_model.trim().is_empty() {
        "gemini-2.0-flash".to_string()
    } else {
        state.gemini_model.trim().to_string()
    };
    Ok(Json(AiCoachStatusResponse {
        configured: !state.gemini_api_key.trim().is_empty(),
        model,
    }))
}

pub async fn coach_chat(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<AiCoachChatRequest>,
) -> Result<Json<AiCoachChatResponse>, ApiError> {
    if !coach_role_allowed(&claims) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Brak uprawnień do trenera AI",
        ));
    }

    let message = payload.message.trim();
    if message.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Treść wiadomości nie może być pusta",
        ));
    }

    let mode = normalize_mode(payload.mode.as_deref());
    let history = payload.history.unwrap_or_default();

    let mut augmented = String::new();
    augmented.push_str(mode_prefix(mode));

    if let Some(ref ctx) = payload.plan_context {
        augmented.push_str(&format_plan_context(ctx));
    }

    if let Some(ref athlete_id) = payload.athlete_id.filter(|_| claims_has_staff_access(&claims)) {
        if let Some(ctx) = fetch_athlete_context(&state, athlete_id).await {
            augmented.push_str(&ctx);
            augmented.push_str("\n\n");
        }
    } else if claims.roles.contains(&Role::Athlete) {
        if let Some(ctx) = fetch_athlete_context_for_user(&state, &claims.sub).await {
            augmented.push_str(&ctx);
            augmented.push_str("\n\n");
        }
    }

    augmented.push_str(message);

    let model = if state.gemini_model.trim().is_empty() {
        "gemini-2.0-flash".to_string()
    } else {
        state.gemini_model.trim().to_string()
    };

    let reply = call_gemini(
        &state,
        augmented,
        &history,
        SYSTEM_INSTRUCTION,
        0.65,
        false,
    )
    .await?;

    Ok(Json(AiCoachChatResponse { reply, model }))
}
