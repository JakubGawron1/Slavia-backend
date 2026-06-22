//! AI-assisted barbell path refinement — vision + numeric correction (Groq).

use axum::{Json, extract::State, http::StatusCode};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};

use crate::api_error::{ApiError, api_error};
use crate::middleware::auth::Claims;
use crate::models::Role;
use crate::state::AppState;

fn coach_role_allowed(claims: &Claims) -> bool {
    claims.roles.iter().any(|r| {
        matches!(
            r,
            Role::Athlete | Role::Trainer | Role::Admin | Role::SuperAdmin
        )
    })
}

const GROQ_CHAT_URL: &str = "https://api.groq.com/openai/v1/chat/completions";
const DEFAULT_GROQ_VISION_MODEL: &str = "llama-3.2-11b-vision-preview";
const MAX_SAMPLES: usize = 120;
const MAX_FRAMES: usize = 10;
const MAX_FRAME_B64_BYTES: usize = 450_000;

const NUMERIC_SYSTEM: &str = r#"Jesteś asystentem biomechaniki dwuboju olimpijskiego. Poprawiasz szumny tor sztangi z detekcji pozy (nadgarstki jako proxy gryfu).

Wejście: tablica punktów {t, barX, barY, hipMidX, shoulderMidX} — współrzędne znormalizowane 0–1, nagranie z profilu bocznego.

Zwróć WYŁĄCZNIE JSON (bez markdown):
{"samples":[{"t":0.0,"barX":0.0,"barY":0.0,"hipMidX":0.0,"shoulderMidX":0.0}, ...], "notes":"krótko po polsku"}

Zasady korekty:
- Wygładź jitter detekcji, usuń skoki >0.08 między sąsiednimi klatkami (outliery).
- Zachowaj kolejność czasową i liczbę punktów (lub ±1).
- W pociągu sztanga zbliża się do ciała; tor pionowy w fazie podnoszenia.
- barX, barY, hipMidX, shoulderMidX w [0,1].
- Nie wymyślaj nowych timestampów poza zakresem wejścia."#;

const VISION_SYSTEM: &str = r#"Jesteś ekspertem od śledzenia sztangi w dwuboju olimpijskim. Na klatkach wideo z profilu bocznego wyznacz środek gryfu / obciążenia.

Zwróć WYŁĄCZNIE JSON:
{"samples":[{"t":0.0,"barX":0.0,"barY":0.0}, ...], "notes":"krótko po polsku"}

barX, barY — środek sztangi w układzie 0–1 (lewy górny róg = 0,0; prawy dolny = 1,1).
Jedna para barX,barY na każdy podany timestamp t. Kolejność rosnąca wg t."#;

#[derive(Debug, Deserialize, Serialize)]
pub struct BarbellPathSampleDto {
    pub t: f64,
    #[serde(alias = "barX")]
    pub bar_x: f64,
    #[serde(alias = "barY")]
    pub bar_y: f64,
    #[serde(alias = "hipMidX")]
    pub hip_mid_x: f64,
    #[serde(alias = "shoulderMidX")]
    pub shoulder_mid_x: f64,
}

#[derive(Debug, Deserialize)]
pub struct BarbellPathFrameDto {
    pub t: f64,
    #[serde(alias = "jpegBase64")]
    pub jpeg_base64: String,
}

#[derive(Debug, Deserialize)]
pub struct BarbellPathRefineRequest {
    #[serde(alias = "rawSamples")]
    pub raw_samples: Vec<BarbellPathSampleDto>,
    pub frames: Option<Vec<BarbellPathFrameDto>>,
    #[serde(alias = "liftType")]
    pub lift_type: Option<String>,
    /// auto | groq_numeric | groq_vision
    pub provider: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BarbellPathSampleOut {
    pub t: f64,
    #[serde(rename = "barX")]
    pub bar_x: f64,
    #[serde(rename = "barY")]
    pub bar_y: f64,
    #[serde(rename = "hipMidX")]
    pub hip_mid_x: f64,
    #[serde(rename = "shoulderMidX")]
    pub shoulder_mid_x: f64,
}

#[derive(Debug, Serialize)]
pub struct BarbellPathRefineResponse {
    pub samples: Vec<BarbellPathSampleOut>,
    pub model: String,
    pub provider: String,
    pub method: String,
    pub notes: Option<String>,
}

#[derive(Serialize)]
struct GroqChatMessage {
    role: String,
    content: serde_json::Value,
}

#[derive(Serialize)]
struct GroqResponseFormat {
    r#type: String,
}

#[derive(Serialize)]
struct GroqChatRequest {
    model: String,
    messages: Vec<GroqChatMessage>,
    temperature: f32,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<GroqResponseFormat>,
}

#[derive(Deserialize)]
struct GroqChatChoice {
    message: Option<GroqChatMessageIn>,
}

#[derive(Deserialize)]
struct GroqChatMessageIn {
    content: Option<String>,
}

#[derive(Deserialize)]
struct GroqChatResponse {
    choices: Option<Vec<GroqChatChoice>>,
    model: Option<String>,
    error: Option<GroqErrorBody>,
}

#[derive(Deserialize)]
struct GroqErrorBody {
    message: Option<String>,
}

#[derive(Deserialize)]
struct LlmSamplesPayload {
    samples: Option<Vec<LlmSamplePoint>>,
    notes: Option<String>,
}

#[derive(Deserialize)]
struct LlmSamplePoint {
    t: Option<f64>,
    bar_x: Option<f64>,
    bar_y: Option<f64>,
    hip_mid_x: Option<f64>,
    shoulder_mid_x: Option<f64>,
    #[serde(rename = "barX")]
    bar_x_camel: Option<f64>,
    #[serde(rename = "barY")]
    bar_y_camel: Option<f64>,
    #[serde(rename = "hipMidX")]
    hip_mid_x_camel: Option<f64>,
    #[serde(rename = "shoulderMidX")]
    shoulder_mid_x_camel: Option<f64>,
}

fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

fn groq_key(state: &AppState) -> Result<&str, ApiError> {
    let key = state.groq_api_key.trim();
    if key.is_empty() {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Brak GROQ_API_KEY — AI toru niedostępne.",
        ));
    }
    Ok(key)
}

fn groq_vision_model() -> String {
    std::env::var("GROQ_VISION_MODEL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_GROQ_VISION_MODEL.to_string())
}

fn normalize_provider(raw: Option<&str>, has_frames: bool) -> &'static str {
    match raw.unwrap_or("auto").trim().to_lowercase().as_str() {
        "groq_numeric" | "numeric" => "groq_numeric",
        "groq_vision" | "vision" => "groq_vision",
        _ => {
            if has_frames {
                "groq_vision"
            } else {
                "groq_numeric"
            }
        }
    }
}

fn lift_label(lift_type: Option<&str>) -> &'static str {
    match lift_type.unwrap_or("unknown").trim().to_lowercase().as_str() {
        "snatch" | "rwanie" => "rwanie (snatch)",
        "clean_jerk" | "clean" | "podrzut" => "podrzut (clean & jerk)",
        _ => "dwubój — typ nie podany",
    }
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

fn merge_refined(
    raw: &[BarbellPathSampleDto],
    llm: &[LlmSamplePoint],
) -> Vec<BarbellPathSampleOut> {
    let n = raw.len().max(llm.len());
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let src = &raw[i.min(raw.len().saturating_sub(1))];
        let p = llm.get(i).or_else(|| llm.last());
        let (bar_x, bar_y) = if let Some(pt) = p {
            let bx = pt.bar_x.or(pt.bar_x_camel).unwrap_or(src.bar_x);
            let by = pt.bar_y.or(pt.bar_y_camel).unwrap_or(src.bar_y);
            (clamp01(bx), clamp01(by))
        } else {
            (clamp01(src.bar_x), clamp01(src.bar_y))
        };
        let t = p.and_then(|x| x.t).unwrap_or(src.t);
        let hip = p
            .and_then(|x| x.hip_mid_x.or(x.hip_mid_x_camel))
            .unwrap_or(src.hip_mid_x);
        let sh = p
            .and_then(|x| x.shoulder_mid_x.or(x.shoulder_mid_x_camel))
            .unwrap_or(src.shoulder_mid_x);
        out.push(BarbellPathSampleOut {
            t,
            bar_x,
            bar_y,
            hip_mid_x: clamp01(hip),
            shoulder_mid_x: clamp01(sh),
        });
    }
    out
}

fn parse_llm_samples(json_text: &str, raw: &[BarbellPathSampleDto]) -> Option<Vec<BarbellPathSampleOut>> {
    let payload: LlmSamplesPayload = serde_json::from_str(&extract_json_payload(json_text)).ok()?;
    let pts = payload.samples?;
    if pts.is_empty() {
        return None;
    }
    Some(merge_refined(raw, &pts))
}

async fn call_groq_text_json(
    api_key: &str,
    model: &str,
    system: &str,
    user_text: String,
) -> Result<(String, String), (StatusCode, String)> {
    let body = GroqChatRequest {
        model: model.to_string(),
        messages: vec![
            GroqChatMessage {
                role: "system".to_string(),
                content: serde_json::Value::String(system.to_string()),
            },
            GroqChatMessage {
                role: "user".to_string(),
                content: serde_json::Value::String(user_text),
            },
        ],
        temperature: 0.25,
        max_tokens: 2048,
        response_format: Some(GroqResponseFormat {
            r#type: "json_object".to_string(),
        }),
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let res = client
        .post(GROQ_CHAT_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Groq: {e}")))?;

    let status = res.status();
    let parsed: GroqChatResponse = res
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Groq JSON: {e}")))?;

    if !status.is_success() {
        let msg = parsed
            .error
            .and_then(|e| e.message)
            .unwrap_or_else(|| format!("Groq HTTP {status}"));
        return Err((StatusCode::BAD_GATEWAY, msg));
    }

    let text = parsed
        .choices
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.message)
        .and_then(|m| m.content)
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err((StatusCode::BAD_GATEWAY, "Groq zwróciło pustą odpowiedź".to_string()));
    }
    Ok((text, parsed.model.unwrap_or_else(|| model.to_string())))
}

async fn call_groq_vision_json(
    api_key: &str,
    model: &str,
    system: &str,
    user_text: String,
    frames: &[(f64, String)],
) -> Result<(String, String), (StatusCode, String)> {
    let mut content = vec![serde_json::json!({"type": "text", "text": user_text})];
    for (_t, b64) in frames {
        content.push(serde_json::json!({
            "type": "image_url",
            "image_url": {"url": format!("data:image/jpeg;base64,{b64}")}
        }));
    }

    let body = GroqChatRequest {
        model: model.to_string(),
        messages: vec![
            GroqChatMessage {
                role: "system".to_string(),
                content: serde_json::Value::String(system.to_string()),
            },
            GroqChatMessage {
                role: "user".to_string(),
                content: serde_json::Value::Array(content),
            },
        ],
        temperature: 0.2,
        max_tokens: 2048,
        response_format: Some(GroqResponseFormat {
            r#type: "json_object".to_string(),
        }),
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let res = client
        .post(GROQ_CHAT_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Groq vision: {e}")))?;

    let status = res.status();
    let parsed: GroqChatResponse = res
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Groq vision JSON: {e}")))?;

    if !status.is_success() {
        let msg = parsed
            .error
            .and_then(|e| e.message)
            .unwrap_or_else(|| format!("Groq vision HTTP {status}"));
        return Err((StatusCode::BAD_GATEWAY, msg));
    }

    let text = parsed
        .choices
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.message)
        .and_then(|m| m.content)
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err((
            StatusCode::BAD_GATEWAY,
            "Groq vision zwróciło pustą odpowiedź".to_string(),
        ));
    }
    Ok((text, parsed.model.unwrap_or_else(|| model.to_string())))
}

fn enforce_barbell_path_limits(user_sub: &str) -> Result<(), ApiError> {
    crate::routes::ai_coach::enforce_barbell_path_limits(user_sub)
}

pub async fn refine_barbell_path(
    State(state): State<AppState>,
    claims: Claims,
    Json(payload): Json<BarbellPathRefineRequest>,
) -> Result<Json<BarbellPathRefineResponse>, ApiError> {
    if !coach_role_allowed(&claims) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Brak uprawnień do AI toru sztangi",
        ));
    }

    let raw = payload.raw_samples;
    if raw.len() < 4 {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Za mało punktów toru (min. 4).",
        ));
    }
    if raw.len() > MAX_SAMPLES {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!("Za dużo punktów (max {MAX_SAMPLES})."),
        ));
    }

    let frames_in = payload.frames.unwrap_or_default();
    if frames_in.len() > MAX_FRAMES {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!("Za dużo klatek (max {MAX_FRAMES})."),
        ));
    }

    let mut frame_pairs: Vec<(f64, String)> = Vec::new();
    for f in &frames_in {
        let b64 = f.jpeg_base64.trim();
        if b64.is_empty() {
            continue;
        }
        if b64.len() > MAX_FRAME_B64_BYTES {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "Klatka JPEG jest zbyt duża.",
            ));
        }
        if B64.decode(b64).is_err() {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "Nieprawidłowe kodowanie base64 klatki.",
            ));
        }
        frame_pairs.push((f.t, b64.to_string()));
    }

    crate::routes::ai_coach::enforce_authenticated_ai_monthly(&state).await?;
    enforce_barbell_path_limits(&claims.sub)?;

    let provider = normalize_provider(payload.provider.as_deref(), !frame_pairs.is_empty());
    let lift = lift_label(payload.lift_type.as_deref());

    let raw_json = serde_json::to_string(&raw).unwrap_or_else(|_| "[]".to_string());

    let (llm_text, model, method) = match provider {
        "groq_vision" => {
            let key = groq_key(&state)?;
            let model = groq_vision_model();
            let mut lines = vec![format!("Typ ruchu: {lift}.")];
            for (i, (t, _)) in frame_pairs.iter().enumerate() {
                lines.push(format!("Obraz {}: t={t:.3}s", i + 1));
            }
            lines.push(format!(
                "Surowy tor z detekcji pozy (szumny — skoryguj wizualnie): {raw_json}"
            ));
            let (text, used) = call_groq_vision_json(
                key,
                &model,
                VISION_SYSTEM,
                lines.join("\n"),
                &frame_pairs,
            )
            .await
            .map_err(|(c, m)| api_error(c, m))?;
            (text, used, "groq_vision")
        }
        _ => {
            let key = groq_key(&state)?;
            let model = if state.groq_model.trim().is_empty() {
                "llama-3.3-70b-versatile".to_string()
            } else {
                state.groq_model.trim().to_string()
            };
            let user = format!(
                "Typ ruchu: {lift}.\nSurowe punkty toru (JSON):\n{raw_json}"
            );
            let (text, used) = call_groq_text_json(key, &model, NUMERIC_SYSTEM, user)
                .await
                .map_err(|(c, m)| api_error(c, m))?;
            (text, used, "groq_numeric")
        }
    };

    let notes = serde_json::from_str::<LlmSamplesPayload>(&extract_json_payload(&llm_text))
        .ok()
        .and_then(|p| p.notes);

    let samples = parse_llm_samples(&llm_text, &raw).ok_or_else(|| {
        api_error(
            StatusCode::BAD_GATEWAY,
            "AI zwróciło nieprawidłowy format toru — spróbuj ponownie.",
        )
    })?;

    Ok(Json(BarbellPathRefineResponse {
        samples,
        model,
        provider: provider.to_string(),
        method: method.to_string(),
        notes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_refined_keeps_count() {
        let raw = vec![
            BarbellPathSampleDto {
                t: 0.0,
                bar_x: 0.5,
                bar_y: 0.8,
                hip_mid_x: 0.48,
                shoulder_mid_x: 0.49,
            },
            BarbellPathSampleDto {
                t: 0.1,
                bar_x: 0.52,
                bar_y: 0.7,
                hip_mid_x: 0.48,
                shoulder_mid_x: 0.49,
            },
        ];
        let llm = vec![
            LlmSamplePoint {
                t: Some(0.0),
                bar_x: None,
                bar_y: None,
                hip_mid_x: None,
                shoulder_mid_x: None,
                bar_x_camel: Some(0.51),
                bar_y_camel: Some(0.79),
                hip_mid_x_camel: None,
                shoulder_mid_x_camel: None,
            },
            LlmSamplePoint {
                t: Some(0.1),
                bar_x: None,
                bar_y: None,
                hip_mid_x: None,
                shoulder_mid_x: None,
                bar_x_camel: Some(0.51),
                bar_y_camel: Some(0.69),
                hip_mid_x_camel: None,
                shoulder_mid_x_camel: None,
            },
        ];
        let out = merge_refined(&raw, &llm);
        assert_eq!(out.len(), 2);
        assert!((out[0].bar_x - 0.51).abs() < 0.001);
    }
}
