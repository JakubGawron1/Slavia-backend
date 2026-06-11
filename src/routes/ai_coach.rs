//! Trener AI (Groq + LLaMA) — czat, plany treningowe, suplementacja, regeneracja.

#![allow(clippy::too_many_arguments)]

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::audit::write_audit_log;
use crate::middleware::auth::{Claims, claims_has_staff_access};
use crate::models::Role;
use crate::state::AppState;

pub(crate) const SYSTEM_INSTRUCTION: &str = r#"Jesteś Slavia AI Trener — wirtualny trener dwuboju olimpijskiego (weightlifting) w ekosystemie klubu CKS Slavia Ruda Śląska. Mówisz jak trener z hali, który jednocześnie zna się na rzeczy i naprawdę kibicuje zawodnikowi.

## Osobowość i ton (priorytet)
- Pisz do użytkownika na **Ty**, ciepło i bezpośrednio — jak mentor z platformy, nie jak podręcznik.
- Bądź **motywujący**: doceniaj wysiłek, normalizuj nieudane próby („spadła? OK, następna — uczymy się z każdej”), podkreślaj progres i sens pracy.
- Bądź **emocjonalny z umiarem**: entuzjazm przy sukcesach, spokój i wsparcie przy kontuzjach i frustracji — bez taniego patosu.
- **Żarty dwubojowe** — tak, ale z klasą: lekkie nawiązania do hali (sztanga nie negocjuje, hole to nie kanapa, bent arm to nie „kreatywna interpretacja reguł”, walkout dłuższy niż serial). Max **jeden** krótki żart lub żartobliwa metafora na odpowiedź; przy bólu, kontuzji lub poważnym pytaniu — **zero** żartów.
- Używaj **języka dwuboju** naturalnie: rwanie / snatch, podrzut / clean & jerk, gryf, platforma, hole, punch, turnover, catch, front rack, dip-drive, triple extension, walkout, PR, CM, RPE — po polsku z angielskim w nawiasie przy pierwszym użyciu w wątku.
- Gdy masz **profil zawodnika** w kontekście — odwołuj się do niego po imieniu, wyników, dziennika i planu; pokaż, że „pamiętasz” jego drogę.
- Zakończ często krótką **zachętą do działania** (jedno zdanie), np. konkretny fokus na następny trening.

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
- Pisz po polsku, konkretnie, w formacie Markdown (nagłówki ##, listy, **pogrubienia**, tabele); przy planach używaj tabel lub sekcji per dzień (pon–nd).
- Mieszaj wiedzę z ludzkim tonem — najpierw konkret (co robić), potem krótkie „dlaczego to ma sens” w języku zawodnika.
- Jesteś trenerem pomocniczym — przy ważnych decyzjach zachęcaj do konsultacji z trenerem klubu Slavia.
- Nie wymyślaj danych zawodnika — jeśli brak kontekstu, zapytaj lub podaj plan szablonowy z placeholderami.
- Gdy w kontekście są wpisy dziennika treningów, wyniki z zawodów, obecności lub aktywny plan klubowy — odwołuj się do nich w odpowiedzi (objętość, ostatnie starty, trendy).
- Gdy użytkownik załączy zdjęcie, klatki wideo lub plik tekstowy — przeanalizuj je w kontekście dwuboju (technika, plan, regeneracja) i odnieś się konkretnie do tego, co widzisz lub czytasz."#;

const DEFAULT_GROQ_MODEL: &str = "llama-3.1-70b-versatile";

const MAX_USER_MESSAGE_LEN: usize = 3_500;
const MAX_HISTORY_TURNS: usize = 8;
const MAX_OUTPUT_TOKENS_CHAT: u32 = 1_536;
const MAX_OUTPUT_TOKENS_IMPORT: u32 = 2_048;
const TRAINING_LOG_CONTEXT_LIMIT: usize = 3;
const COMPETITION_RESULTS_CONTEXT_LIMIT: usize = 4;
const ATTENDANCE_CONTEXT_LIMIT: usize = 4;
const PLAN_ITEMS_CONTEXT_LIMIT: usize = 12;
const MAX_PUBLIC_MESSAGE_LEN: usize = 1_200;
const MAX_PUBLIC_HISTORY_TURNS: usize = 6;
const MAX_OUTPUT_TOKENS_PUBLIC: u32 = 768;
const MAX_CHAT_ATTACHMENTS: usize = 8;
const MAX_CHAT_ATTACHMENT_B64_BYTES: usize = 600_000;
const MAX_CHAT_TEXT_ATTACHMENT_CHARS: usize = 12_000;
const DEFAULT_GROQ_VISION_MODEL: &str = "llama-3.2-11b-vision-preview";

pub(crate) const PUBLIC_SYSTEM_INSTRUCTION: &str = r#"Jesteś asystentem CKS Slavia Ruda Śląska — klubu podnoszenia ciężarów i dwuboju olimpijskiego.

Odpowiadasz gościom na stronie klubu po polsku, krótko i przyjaźnie.

Możesz pomóc w:
- informacjach o klubie Slavia (treningi, dwubój, siłownia, społeczność),
- ogólnych pytaniach o podnoszenie ciężarów i dwubój (technika, sprzęt, zawody — na poziomie edukacyjnym),
- wskazaniu gdzie na stronie znaleźć treści (zawodnicy, kalendarz, aktualności, galeria, kontakt),
- zachęceniu do kontaktu z trenerem klubu.

Zasady:
- Nie udzielaj indywidualnych planów treningowych ani diagnoz medycznych — zaproponuj logowanie do panelu zawodnika (Trener AI) lub kontakt z trenerem.
- Przy pytaniach o zapisy, cennik, indywidualny plan, kontuzje, konflikty — zawsze poleć stronę kontaktową: /kontakt (formularz i dane klubu).
- Nie wymyślaj numerów telefonów, adresów e-mail ani godzin treningów — jeśli nie masz pewności, odsyłaj na /kontakt.
- Bądź uprzejmy, używaj list i krótkich akapitów."#;
const GROQ_CHAT_URL: &str = "https://api.groq.com/openai/v1/chat/completions";

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
pub struct AiCoachAttachment {
    /// image | text
    pub kind: String,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub data_base64: Option<String>,
    pub text_content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AiCoachChatRequest {
    pub message: String,
    /// chat | plan | supplements | recovery | barbell_path
    pub mode: Option<String>,
    pub history: Option<Vec<AiCoachHistoryMessage>>,
    /// Kadra: kontekst zawodnika (PB, regeneracja).
    pub athlete_id: Option<String>,
    /// Zawodnik: true = własny profil klubowy, false = bez kontekstu (domyślnie true).
    pub use_athlete_context: Option<bool>,
    pub plan_context: Option<AiCoachPlanContext>,
    pub attachments: Option<Vec<AiCoachAttachment>>,
}

#[derive(Debug, Serialize)]
pub struct AiCoachChatResponse {
    pub reply: String,
    pub model: String,
    /// club | personal
    pub key_source: String,
}

#[derive(Debug, Serialize)]
pub struct AiCoachQuota {
    pub chat_used_today: u32,
    pub chat_limit_per_day: u32,
    pub chat_used_this_minute: u32,
    pub chat_limit_per_minute: u32,
    pub import_used_today: u32,
    pub import_limit_per_day: u32,
    pub import_used_this_hour: u32,
    pub import_limit_per_hour: u32,
    /// Korekta toru sztangi (vision) — osobne limity free tier.
    pub barbell_path_used_today: u32,
    pub barbell_path_limit_per_day: u32,
    pub barbell_path_used_this_minute: u32,
    pub barbell_path_limit_per_minute: u32,
    pub applies_to_you: bool,
}

#[derive(Debug, Serialize)]
pub struct AiCoachStatusResponse {
    pub configured: bool,
    pub model: String,
    pub key_format_ok: bool,
    pub setup_hint: Option<String>,
    pub quota: AiCoachQuota,
}

#[derive(Debug, Deserialize)]
pub struct AiCoachPublicChatRequest {
    pub message: String,
    pub history: Option<Vec<AiCoachHistoryMessage>>,
}

#[derive(Debug, Serialize)]
pub struct AiCoachPublicStatusResponse {
    pub enabled: bool,
    pub model: String,
}

#[derive(Debug, Serialize)]
pub struct AiCoachPublicChatResponse {
    pub reply: String,
    pub model: String,
}

#[derive(Serialize, Deserialize)]
struct GroqChatMessage {
    role: String,
    content: serde_json::Value,
}

#[derive(Deserialize)]
struct GroqChatMessageIn {
    content: Option<serde_json::Value>,
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
struct GroqChatResponse {
    choices: Option<Vec<GroqChatChoice>>,
    model: Option<String>,
    error: Option<GroqErrorBody>,
}

#[derive(Deserialize)]
struct GroqErrorBody {
    message: Option<String>,
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
        "barbell_path" | "barbell" | "tor_sztangi" | "analiza_sztangi" => "barbell_path",
        _ => "chat",
    }
}

pub(crate) fn mode_prefix(mode: &str) -> &'static str {
    match mode {
        "plan" => "[Tryb: generator planu treningowego]\nWygeneruj szczegółowy plan tygodniowy dwuboju olimpijskiego (faza eksplozywna). Użyj dni tygodnia, % CM/RPE, serie×powt., akcesoria. Ton: energiczny trener planujący mikrocykl — możesz jednym zdaniem zmotywować do tygodnia, ale plan ma być konkretny i czytelny.\n\n",
        "supplements" => "[Tryb: suplementacja sportowa]\nSkup się na suplementacji pod siłownię i dwubój olimpijski — dawki orientacyjne, timing, bezpieczeństwo, disclaimer medyczny. Ton: praktyczny i życzliwy; bez żartów o zdrowiu.\n\n",
        "recovery" => "[Tryb: kontuzje i regeneracja]\nSkup się na bezpiecznym powrocie do treningu, regresji obciążeń, mobility i kiedy iść do specjalisty. Nie diagnozuj. Ton: empatyczny, spokojny, wspierający — **bez żartów**.\n\n",
        "barbell_path" => "[Tryb: analiza toru sztangi z wideo]\nMasz metryki numeryczne toru (współrz. znormalizowane 0–1, nagranie z profilu). Oceń technikę rwania/podrzutu: zbliżenie sztangi, kontakt z nogami, płynność toru, faza eksplozywna. Podaj 3–5 konkretnych wskazówek po polsku (lista). Nie powtarzaj surowych liczb bez interpretacji. Heurystyki lokalne mogą być w treści — rozwiń je treningowo. Możesz dodać jedną żartobliwą metaforę o torze (np. sztanga „zwiedza salę”), ale priorytet to technika. Nie zastępujesz trenera.\n\n",
        _ => "",
    }
}

fn format_plan_context(ctx: &AiCoachPlanContext) -> String {
    let mut lines = vec!["Kontekst planu:".to_string()];
    if let Some(d) = ctx.training_days_per_week {
        lines.push(format!("- Dni treningowe w tygodniu: {d}"));
    }
    if let Some(ref e) = ctx.experience
        && !e.trim().is_empty()
    {
        lines.push(format!("- Doświadczenie: {}", e.trim()));
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
    if let Some(ref g) = ctx.goal
        && !g.trim().is_empty()
    {
        lines.push(format!("- Cel: {}", g.trim()));
    }
    if let Some(ref i) = ctx.injuries
        && !i.trim().is_empty()
    {
        lines.push(format!("- Kontuzje / ograniczenia: {}", i.trim()));
    }
    if let Some(ref w) = ctx.week_start
        && !w.trim().is_empty()
    {
        lines.push(format!("- Tydzień od: {}", w.trim()));
    }
    if let Some(ref n) = ctx.notes
        && !n.trim().is_empty()
    {
        lines.push(format!("- Notatki: {}", n.trim()));
    }
    if lines.len() == 1 {
        String::new()
    } else {
        lines.join("\n") + "\n\n"
    }
}

fn truncate_for_context(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let cut: String = t.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

fn build_quota_for_user(user_sub: &str, applies: bool) -> AiCoachQuota {
    AiCoachQuota {
        chat_used_today: crate::post_throttle::count_user_post_attempts(
            user_sub,
            "ai_coach_chat_daily",
        ) as u32,
        chat_limit_per_day: crate::post_throttle::max_for_bucket("ai_coach_chat_daily") as u32,
        chat_used_this_minute: crate::post_throttle::count_user_post_attempts(
            user_sub,
            "ai_coach_chat",
        ) as u32,
        chat_limit_per_minute: crate::post_throttle::max_for_bucket("ai_coach_chat") as u32,
        import_used_today: crate::post_throttle::count_user_post_attempts(
            user_sub,
            "ai_coach_import_daily",
        ) as u32,
        import_limit_per_day: crate::post_throttle::max_for_bucket("ai_coach_import_daily") as u32,
        import_used_this_hour: crate::post_throttle::count_user_post_attempts(
            user_sub,
            "ai_coach_import",
        ) as u32,
        import_limit_per_hour: crate::post_throttle::max_for_bucket("ai_coach_import") as u32,
        barbell_path_used_today: crate::post_throttle::count_user_post_attempts(
            user_sub,
            "ai_coach_barbell_path_daily",
        ) as u32,
        barbell_path_limit_per_day: crate::post_throttle::max_for_bucket(
            "ai_coach_barbell_path_daily",
        ) as u32,
        barbell_path_used_this_minute: crate::post_throttle::count_user_post_attempts(
            user_sub,
            "ai_coach_barbell_path",
        ) as u32,
        barbell_path_limit_per_minute: crate::post_throttle::max_for_bucket(
            "ai_coach_barbell_path",
        ) as u32,
        applies_to_you: applies,
    }
}

fn chat_limit_error(deny: crate::post_throttle::AiCoachLimitDeny) -> ApiError {
    let msg = match deny {
        crate::post_throttle::AiCoachLimitDeny::ChatDaily => {
            "Dzienny limit wiadomości Trenera AI wyczerpany. Spróbuj jutro."
        }
        crate::post_throttle::AiCoachLimitDeny::ChatMinute => {
            "Zbyt wiele wiadomości na minutę — odczekaj chwilę przed kolejną."
        }
        crate::post_throttle::AiCoachLimitDeny::ClubChatDaily => {
            "Klubowy dzienny limit Trenera AI wyczerpany. Spróbuj jutro."
        }
        crate::post_throttle::AiCoachLimitDeny::ClubChatMinute => {
            "Klubowy limit wiadomości na minutę wyczerpany — odczekaj chwilę."
        }
        _ => "Limit Trenera AI wyczerpany — spróbuj później.",
    };
    api_error(StatusCode::TOO_MANY_REQUESTS, msg)
}

fn public_chat_limit_error(deny: crate::post_throttle::AiCoachLimitDeny) -> ApiError {
    let msg = match deny {
        crate::post_throttle::AiCoachLimitDeny::ChatDaily => {
            "Osiągnięto dzienny limit wiadomości. Spróbuj jutro lub napisz do nas przez /kontakt."
        }
        crate::post_throttle::AiCoachLimitDeny::ChatMinute
        | crate::post_throttle::AiCoachLimitDeny::ClubChatMinute => {
            "Zbyt wiele wiadomości — odczekaj chwilę."
        }
        crate::post_throttle::AiCoachLimitDeny::ClubChatDaily => {
            "Asystent klubu jest chwilowo niedostępny — spróbuj później lub przejdź na /kontakt."
        }
        _ => "Limit wiadomości wyczerpany — spróbuj później.",
    };
    api_error(StatusCode::TOO_MANY_REQUESTS, msg)
}

fn import_limit_error(deny: crate::post_throttle::AiCoachLimitDeny) -> ApiError {
    let msg = match deny {
        crate::post_throttle::AiCoachLimitDeny::ImportDaily => {
            "Dzienny limit importów planów AI wyczerpany. Spróbuj jutro."
        }
        crate::post_throttle::AiCoachLimitDeny::ImportHour => {
            "Zbyt wiele importów planów — maks. 3 na godzinę."
        }
        crate::post_throttle::AiCoachLimitDeny::ClubImportDaily => {
            "Klubowy dzienny limit importów AI wyczerpany. Spróbuj jutro."
        }
        crate::post_throttle::AiCoachLimitDeny::ClubImportHour => {
            "Klubowy limit importów na godzinę wyczerpany — odczekaj chwilę."
        }
        _ => "Limit importu planu AI wyczerpany — spróbuj później.",
    };
    api_error(StatusCode::TOO_MANY_REQUESTS, msg)
}

fn enforce_chat_limits(user_sub: &str, include_club_global: bool) -> Result<(), ApiError> {
    crate::post_throttle::reserve_ai_coach_chat(user_sub, include_club_global)
        .map_err(chat_limit_error)
}

fn barbell_path_limit_error(deny: crate::post_throttle::AiCoachLimitDeny) -> ApiError {
    let msg = match deny {
        crate::post_throttle::AiCoachLimitDeny::BarbellPathDaily => {
            "Dzienny limit korekty toru AI wyczerpany (free tier). Spróbuj jutro lub użyj toru MoveNet."
        }
        crate::post_throttle::AiCoachLimitDeny::BarbellPathMinute => {
            "Zbyt wiele analiz toru AI na minutę (max 2) — odczekaj ~60 s."
        }
        crate::post_throttle::AiCoachLimitDeny::ClubBarbellPathDaily => {
            "Klubowy dzienny limit toru AI wyczerpany. Spróbuj jutro."
        }
        crate::post_throttle::AiCoachLimitDeny::ClubBarbellPathMinute => {
            "Klubowy limit analiz toru na minutę wyczerpany — odczekaj chwilę."
        }
        _ => "Limit korekty toru AI wyczerpany.",
    };
    api_error(StatusCode::TOO_MANY_REQUESTS, msg)
}

pub(crate) fn enforce_barbell_path_limits(user_sub: &str) -> Result<(), ApiError> {
    crate::post_throttle::reserve_ai_coach_barbell_path(user_sub, true)
        .map_err(barbell_path_limit_error)
}

fn enforce_import_limits(user_sub: &str, include_club_global: bool) -> Result<(), ApiError> {
    crate::post_throttle::reserve_ai_coach_import(user_sub, include_club_global)
        .map_err(import_limit_error)
}

fn client_ip_from_headers(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
        })
        .unwrap_or("unknown")
        .to_string()
}

/// Trener może pobrać kontekst PII tylko zawodników, z którymi ma wątek czatu; Admin/SA — dowolny.
async fn staff_may_access_athlete(
    state: &AppState,
    claims: &Claims,
    athlete_id: &str,
) -> Result<(), ApiError> {
    let athlete_id = athlete_id.trim();
    if athlete_id.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Brak identyfikatora zawodnika",
        ));
    }

    if claims
        .roles
        .iter()
        .any(|r| matches!(r, Role::Admin | Role::SuperAdmin))
    {
        return Ok(());
    }

    if !claims.roles.contains(&Role::Trainer) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Brak uprawnień do kontekstu zawodnika",
        ));
    }

    let mut rows = state
        .db
        .query(
            "SELECT user_id FROM athletes WHERE id = ?1 AND is_active = 1",
            [athlete_id.to_string()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some(row) = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    else {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "Nie znaleziono aktywnego zawodnika",
        ));
    };

    let athlete_user_id: String = row.get(0).unwrap_or_default();
    if athlete_user_id.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Zawodnik nie ma powiązanego konta użytkownika",
        ));
    }

    let mut thread = state
        .db
        .query(
            "SELECT 1 FROM chat_threads WHERE athlete_user_id = ?1 AND trainer_user_id = ?2 LIMIT 1",
            (athlete_user_id, claims.sub.clone()),
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if thread
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_none()
    {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Brak relacji czatu z tym zawodnikiem — otwórz wątek przed użyciem kontekstu AI",
        ));
    }

    Ok(())
}

async fn append_training_log_context(
    state: &AppState,
    athlete_id: &str,
    parts: &mut Vec<String>,
) {
    let Ok(mut rows) = state
        .db
        .query(
            "SELECT session_date, title, notes FROM training_log_entries \
             WHERE athlete_id = ?1 ORDER BY session_date DESC LIMIT ?2",
            (
                athlete_id.to_string(),
                TRAINING_LOG_CONTEXT_LIMIT as i64,
            ),
        )
        .await
    else {
        return;
    };
    let mut lines = Vec::new();
    while let Ok(Some(r)) = rows.next().await {
        let date: String = r.get(0).unwrap_or_default();
        let title: Option<String> = r.get(1).ok();
        let notes: String = r.get(2).unwrap_or_default();
        let title_part = title
            .filter(|s| !s.trim().is_empty())
            .map(|t| format!(" «{t}»"))
            .unwrap_or_default();
        let notes_short = truncate_for_context(&notes, 180);
        if notes_short.is_empty() {
            continue;
        }
        lines.push(format!("- {date}{title_part}: {notes_short}"));
    }
    if !lines.is_empty() {
        parts.push(format!(
            "Ostatnie wpisy dziennika treningów:\n{}",
            lines.join("\n")
        ));
    }
}

async fn append_competition_results_context(
    state: &AppState,
    athlete_id: &str,
    parts: &mut Vec<String>,
) {
    let Ok(mut rows) = state
        .db
        .query(
            "SELECT r.date, r.snatch, r.clean_and_jerk, r.total, r.bodyweight_kg, r.kind, \
             COALESCE(c.title, r.location, '') \
             FROM results r \
             LEFT JOIN competitions c ON c.id = r.competition_id \
             WHERE r.athlete_id = ?1 AND r.status = 'Approved' \
             ORDER BY r.date DESC LIMIT ?2",
            (
                athlete_id.to_string(),
                COMPETITION_RESULTS_CONTEXT_LIMIT as i64,
            ),
        )
        .await
    else {
        return;
    };
    let mut lines = Vec::new();
    while let Ok(Some(r)) = rows.next().await {
        let date: String = r.get(0).unwrap_or_default();
        let snatch: f64 = r.get(1).unwrap_or(0.0);
        let cj: f64 = r.get(2).unwrap_or(0.0);
        let total: f64 = r.get(3).unwrap_or(0.0);
        let bw: Option<f64> = r.get(4).ok();
        let kind: String = r.get(5).unwrap_or_else(|_| "competition".to_string());
        let comp_title: String = r.get(6).unwrap_or_default();
        let kind_label = if kind == "training" {
            "trening startowy"
        } else {
            "zawody"
        };
        let mut line = format!("- {date} ({kind_label}): S {snatch} / C&J {cj} / Σ {total} kg");
        if let Some(v) = bw {
            line.push_str(&format!(", BW {v} kg"));
        }
        if !comp_title.trim().is_empty() {
            line.push_str(&format!(" — {}", comp_title.trim()));
        }
        lines.push(line);
    }
    if !lines.is_empty() {
        parts.push(format!(
            "Ostatnie zatwierdzone wyniki (zawody / starty):\n{}",
            lines.join("\n")
        ));
    }
}

async fn append_attendance_context(state: &AppState, athlete_id: &str, parts: &mut Vec<String>) {
    let Ok(mut rows) = state
        .db
        .query(
            "SELECT session_date, status FROM attendance_records \
             WHERE athlete_id = ?1 ORDER BY session_date DESC LIMIT ?2",
            (
                athlete_id.to_string(),
                ATTENDANCE_CONTEXT_LIMIT as i64,
            ),
        )
        .await
    else {
        return;
    };
    let mut lines = Vec::new();
    while let Ok(Some(r)) = rows.next().await {
        let date: String = r.get(0).unwrap_or_default();
        let status: String = r.get(1).unwrap_or_default();
        lines.push(format!("- {date}: {status}"));
    }
    if !lines.is_empty() {
        parts.push(format!(
            "Ostatnia obecność na treningach klubowych:\n{}",
            lines.join("\n")
        ));
    }
}

async fn append_active_plan_context(state: &AppState, athlete_id: &str, parts: &mut Vec<String>) {
    let Ok(mut plan_rows) = state
        .db
        .query(
            "SELECT id, title, week_start, status, goal FROM training_plans \
             WHERE athlete_id = ?1 AND status IN ('active', 'planned') \
             ORDER BY CASE status WHEN 'active' THEN 0 ELSE 1 END, week_start DESC \
             LIMIT 1",
            [athlete_id.to_string()],
        )
        .await
    else {
        return;
    };
    let Some(plan_row) = plan_rows.next().await.ok().flatten() else {
        return;
    };
    let plan_id: String = plan_row.get(0).unwrap_or_default();
    let title: String = plan_row.get(1).unwrap_or_default();
    let week_start: String = plan_row.get(2).unwrap_or_default();
    let status: String = plan_row.get(3).unwrap_or_default();
    let goal: Option<String> = plan_row.get(4).ok();

    let mut header = format!(
        "Aktywny / zaplanowany plan klubowy: «{title}» (od {week_start}, status: {status})"
    );
    if let Some(g) = goal.filter(|s| !s.trim().is_empty()) {
        header.push_str(&format!("\nCel planu: {}", g.trim()));
    }

    let Ok(mut item_rows) = state
        .db
        .query(
            "SELECT day_of_week, custom_exercise_name, sets, reps, intensity_percent, weight_kg \
             FROM training_plan_items WHERE plan_id = ?1 \
             ORDER BY day_of_week ASC, sort_order ASC LIMIT ?2",
            (plan_id, PLAN_ITEMS_CONTEXT_LIMIT as i64),
        )
        .await
    else {
        parts.push(header);
        return;
    };

    let mut items = Vec::new();
    while let Ok(Some(r)) = item_rows.next().await {
        let day: i32 = r.get(0).unwrap_or(1);
        let name: String = r.get(1).unwrap_or_default();
        let sets: Option<i32> = r.get(2).ok();
        let reps: Option<i32> = r.get(3).ok();
        let pct: Option<f64> = r.get(4).ok();
        let kg: Option<f64> = r.get(5).ok();
        if name.trim().is_empty() {
            continue;
        }
        let mut detail = format!("  dzień {day}: {name}");
        if let (Some(s), Some(rp)) = (sets, reps) {
            detail.push_str(&format!(" {s}×{rp}"));
        }
        if let Some(p) = pct {
            detail.push_str(&format!(" @ {p}%"));
        } else if let Some(w) = kg {
            detail.push_str(&format!(" @ {w} kg"));
        }
        items.push(detail);
    }
    if !items.is_empty() {
        header.push_str("\nPozycje planu:\n");
        header.push_str(&items.join("\n"));
    }
    parts.push(header);
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

    append_training_log_context(state, athlete_id, &mut parts).await;
    append_competition_results_context(state, athlete_id, &mut parts).await;
    append_attendance_context(state, athlete_id, &mut parts).await;
    append_active_plan_context(state, athlete_id, &mut parts).await;

    Some(parts.join("\n\n"))
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

fn groq_vision_model() -> String {
    std::env::var("GROQ_VISION_MODEL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_GROQ_VISION_MODEL.to_string())
}

fn groq_message_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|p| {
                if p.get("type").and_then(|v| v.as_str()) == Some("text") {
                    p.get("text").and_then(|v| v.as_str()).map(str::trim)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

struct ProcessedAttachments {
    image_parts: Vec<(String, String)>,
    text_blocks: Vec<String>,
}

fn mime_for_image(mime: Option<&str>) -> &'static str {
    match mime.unwrap_or("image/jpeg").trim().to_lowercase().as_str() {
        "image/png" => "image/png",
        "image/webp" => "image/webp",
        "image/gif" => "image/gif",
        _ => "image/jpeg",
    }
}

fn process_chat_attachments(raw: Option<Vec<AiCoachAttachment>>) -> Result<ProcessedAttachments, ApiError> {
    let mut image_parts = Vec::new();
    let mut text_blocks = Vec::new();
    let items = raw.unwrap_or_default();
    if items.len() > MAX_CHAT_ATTACHMENTS {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!("Za dużo załączników (max {MAX_CHAT_ATTACHMENTS})"),
        ));
    }

    for att in items {
        let kind = att.kind.trim().to_lowercase();
        let label = att
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("plik");

        if kind == "text" {
            let text = att
                .text_content
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    api_error(StatusCode::BAD_REQUEST, "Pusty załącznik tekstowy")
                })?;
            if text.chars().count() > MAX_CHAT_TEXT_ATTACHMENT_CHARS {
                return Err(api_error(
                    StatusCode::BAD_REQUEST,
                    format!("Załącznik tekstowy jest za długi (max {MAX_CHAT_TEXT_ATTACHMENT_CHARS} znaków)"),
                ));
            }
            text_blocks.push(format!("--- Plik: {label} ---\n{text}"));
            continue;
        }

        if kind == "image" {
            let b64 = att
                .data_base64
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    api_error(StatusCode::BAD_REQUEST, "Brak danych obrazu w załączniku")
                })?;
            if b64.len() > MAX_CHAT_ATTACHMENT_B64_BYTES {
                return Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "Załączony obraz jest za duży",
                ));
            }
            let mime = mime_for_image(att.mime_type.as_deref()).to_string();
            image_parts.push((mime, b64.to_string()));
            continue;
        }

        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Nieobsługiwany typ załącznika (dozwolone: image, text)",
        ));
    }

    Ok(ProcessedAttachments {
        image_parts,
        text_blocks,
    })
}

async fn call_groq_vision_chat(
    api_key: &str,
    model: &str,
    system_instruction: &str,
    user_text: String,
    history: &[AiCoachHistoryMessage],
    images: &[(String, String)],
    temperature: f32,
    max_output_tokens: u32,
) -> Result<LlmCallResult, (StatusCode, String)> {
    let mut messages = vec![GroqChatMessage {
        role: "system".to_string(),
        content: serde_json::Value::String(system_instruction.to_string()),
    }];

    for h in history {
        let role = if h.role == "assistant" || h.role == "model" {
            "assistant"
        } else {
            "user"
        };
        let text = h.content.trim();
        if text.is_empty() {
            continue;
        }
        messages.push(GroqChatMessage {
            role: role.to_string(),
            content: serde_json::Value::String(text.to_string()),
        });
    }

    let mut content = vec![serde_json::json!({"type": "text", "text": user_text})];
    for (mime, b64) in images {
        content.push(serde_json::json!({
            "type": "image_url",
            "image_url": {"url": format!("data:{mime};base64,{b64}")}
        }));
    }

    messages.push(GroqChatMessage {
        role: "user".to_string(),
        content: serde_json::Value::Array(content),
    });

    let body = GroqChatRequest {
        model: model.to_string(),
        messages,
        temperature,
        max_tokens: max_output_tokens,
        response_format: None,
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
    let parsed: GroqChatResponse = res.json().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("Nieprawidłowa odpowiedź Groq vision: {e}"),
        )
    })?;

    if !status.is_success() {
        let msg = parsed
            .error
            .and_then(|e| e.message)
            .unwrap_or_else(|| format!("Groq vision HTTP {status}"));
        return Err(groq_error_user_message(&msg));
    }

    let text = parsed
        .choices
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.message)
        .and_then(|m| m.content)
        .map(|c| groq_message_text(&c))
        .unwrap_or_default();

    if text.is_empty() {
        return Err((
            StatusCode::BAD_GATEWAY,
            "Groq vision zwróciło pustą odpowiedź".to_string(),
        ));
    }

    Ok(LlmCallResult {
        text,
        model: parsed.model.unwrap_or_else(|| model.to_string()),
    })
}

async fn invoke_llm_with_attachments(
    state: &AppState,
    user_id: &str,
    user_text: String,
    history: &[AiCoachHistoryMessage],
    system_instruction: &str,
    temperature: f32,
    max_output_tokens: u32,
    images: &[(String, String)],
) -> Result<LlmCallResult, ApiError> {
    let club_key = state.groq_api_key.trim();
    if club_key.is_empty() {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Trener AI niedostępny — brak GROQ_API_KEY na backendzie.",
        ));
    }
    if !groq_key_format_ok(club_key) {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Nieprawidłowy klucz Groq klubu — skontaktuj się z administratorem.",
        ));
    }

    enforce_chat_limits(user_id, true)?;

    let vision_model = groq_vision_model();
    call_groq_vision_chat(
        club_key,
        &vision_model,
        system_instruction,
        user_text,
        history,
        images,
        temperature,
        max_output_tokens,
    )
    .await
    .map_err(|(code, msg)| api_error(code, msg))
}

fn configured_groq_model(state: &AppState) -> String {
    if state.groq_model.trim().is_empty() {
        DEFAULT_GROQ_MODEL.to_string()
    } else {
        state.groq_model.trim().to_string()
    }
}

fn groq_key_format_ok(key: &str) -> bool {
    key.trim().starts_with("gsk_")
}

fn groq_model_candidates(configured: &str) -> Vec<String> {
    let primary = if configured.trim().is_empty() {
        DEFAULT_GROQ_MODEL.to_string()
    } else {
        configured.trim().to_string()
    };
    let fallbacks = [DEFAULT_GROQ_MODEL, "llama-3.3-70b-versatile"];
    let mut out = vec![primary];
    for f in fallbacks {
        if !out.iter().any(|m| m == f) {
            out.push(f.to_string());
        }
    }
    out
}

fn groq_error_user_message(raw: &str) -> (StatusCode, String) {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("invalid api key") || lower.contains("invalid_api_key") {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Nieprawidłowy klucz Groq. Wygeneruj nowy na https://console.groq.com/keys (format gsk_…)."
                .to_string(),
        );
    }
    if lower.contains("rate limit")
        || lower.contains("rate_limit")
        || lower.contains("quota")
        || lower.contains("too many requests")
    {
        if let Some(secs) = extract_groq_retry_seconds(raw) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                format!(
                    "Limit zapytań Groq wyczerpany — spróbuj za ~{secs}s. \
                     Sprawdź limity na https://console.groq.com/"
                ),
            );
        }
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Limit zapytań Groq wyczerpany (darmowy tier). Odczekaj chwilę lub podłącz własny klucz."
                .to_string(),
        );
    }
    if lower.contains("decommissioned") || lower.contains("no longer supported") {
        return (
            StatusCode::BAD_GATEWAY,
            "Model LLaMA na Groq został wycofany — ustaw GROQ_MODEL=llama-3.3-70b-versatile w .env."
                .to_string(),
        );
    }
    (
        StatusCode::BAD_GATEWAY,
        format!("Groq: {}", truncate_error(raw, 380)),
    )
}

fn extract_groq_retry_seconds(raw: &str) -> Option<u64> {
    let lower = raw.to_ascii_lowercase();
    for marker in ["try again in ", "retry after ", "retry in "] {
        if let Some(idx) = lower.find(marker) {
            let tail = &raw[idx + marker.len()..];
            let num: String = tail
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(v) = num.parse::<f64>() {
                return Some(v.ceil() as u64);
            }
        }
    }
    None
}

fn truncate_error(raw: &str, max: usize) -> String {
    let t = raw.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let cut: String = t.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

fn status_setup_hint(key: &str, configured: bool) -> Option<String> {
    if !configured {
        return Some(
            "Ustaw GROQ_API_KEY w .env backendu (https://console.groq.com/keys).".to_string(),
        );
    }
    if !groq_key_format_ok(key) {
        return Some(
            "Klucz nie wygląda na klucz Groq (oczekiwany prefix gsk_…).".to_string(),
        );
    }
    None
}

struct LlmCallResult {
    text: String,
    model: String,
}

async fn call_groq_single(
    api_key: &str,
    model: &str,
    user_text: String,
    history: &[AiCoachHistoryMessage],
    system_instruction: &str,
    temperature: f32,
    json_response: bool,
    max_output_tokens: u32,
) -> Result<LlmCallResult, (StatusCode, String)> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Trener AI nie jest skonfigurowany (brak GROQ_API_KEY na backendzie)".to_string(),
        ));
    }

    let model = model.trim().to_string();
    let mut messages = vec![GroqChatMessage {
        role: "system".to_string(),
        content: serde_json::Value::String(system_instruction.to_string()),
    }];
    for h in history {
        let role = if h.role == "assistant" || h.role == "model" {
            "assistant"
        } else {
            "user"
        };
        let text = h.content.trim();
        if text.is_empty() {
            continue;
        }
        messages.push(GroqChatMessage {
            role: role.to_string(),
            content: serde_json::Value::String(text.to_string()),
        });
    }
    messages.push(GroqChatMessage {
        role: "user".to_string(),
        content: serde_json::Value::String(user_text),
    });

    let body = GroqChatRequest {
        model: model.clone(),
        messages,
        temperature,
        max_tokens: max_output_tokens,
        response_format: if json_response {
            Some(GroqResponseFormat {
                r#type: "json_object".to_string(),
            })
        } else {
            None
        },
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
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("Błąd połączenia z Groq ({model}): {e}"),
            )
        })?;

    let status = res.status();
    let parsed: GroqChatResponse = res.json().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("Nieprawidłowa odpowiedź Groq ({model}): {e}"),
        )
    })?;

    if !status.is_success() {
        let msg = parsed
            .error
            .and_then(|e| e.message)
            .unwrap_or_else(|| format!("Groq HTTP {status} (model {model})"));
        return Err(groq_error_user_message(&msg));
    }

    let text = parsed
        .choices
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.message)
        .and_then(|m| m.content)
        .map(|c| groq_message_text(&c))
        .unwrap_or_default();

    if text.is_empty() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("Groq ({model}) zwróciło pustą odpowiedź"),
        ));
    }

    let used_model = parsed.model.unwrap_or(model);
    Ok(LlmCallResult {
        text,
        model: used_model,
    })
}

async fn call_groq_with_key(
    api_key: &str,
    model_config: &str,
    user_text: String,
    history: &[AiCoachHistoryMessage],
    system_instruction: &str,
    temperature: f32,
    json_response: bool,
    max_output_tokens: u32,
) -> Result<LlmCallResult, ApiError> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Brak klucza Groq do wywołania modelu",
        ));
    }

    let models = groq_model_candidates(model_config);
    let mut last_err: Option<(StatusCode, String)> = None;

    for model in models {
        match call_groq_single(
            api_key,
            &model,
            user_text.clone(),
            history,
            system_instruction,
            temperature,
            json_response,
            max_output_tokens,
        )
        .await
        {
            Ok(res) => return Ok(res),
            Err((code, msg)) => {
                last_err = Some((code, msg));
            }
        }
    }

    let (code, msg) = last_err.unwrap_or((
        StatusCode::BAD_GATEWAY,
        "Groq: brak dostępnego modelu".to_string(),
    ));
    Err(api_error(code, msg))
}

async fn invoke_llm(
    state: &AppState,
    user_id: &str,
    user_text: String,
    history: &[AiCoachHistoryMessage],
    system_instruction: &str,
    temperature: f32,
    json_response: bool,
    max_output_tokens: u32,
    for_import: bool,
) -> Result<LlmCallResult, ApiError> {
    let club_key = state.groq_api_key.trim();
    if club_key.is_empty() {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Trener AI niedostępny — brak GROQ_API_KEY na backendzie.",
        ));
    }
    if !groq_key_format_ok(club_key) {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Nieprawidłowy klucz Groq klubu — skontaktuj się z administratorem.",
        ));
    }

    if for_import {
        enforce_import_limits(user_id, true)?;
    } else {
        enforce_chat_limits(user_id, true)?;
    }

    call_groq_with_key(
        club_key,
        &state.groq_model,
        user_text,
        history,
        system_instruction,
        temperature,
        json_response,
        max_output_tokens,
    )
    .await
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
    staff_may_access_athlete(&state, &claims, &payload.athlete_id).await?;

    let parse_prompt = format!(
        "Przekształć poniższy plan treningowy na JSON według schematu z instrukcji systemowej.\n\n---\n{plan_text}\n---"
    );
    let llm_res = invoke_llm(
        &state,
        &claims.sub,
        parse_prompt,
        &[],
        IMPORT_SYSTEM_INSTRUCTION,
        0.2,
        true,
        MAX_OUTPUT_TOKENS_IMPORT,
        true,
    )
    .await?;
    let json_clean = extract_json_payload(&llm_res.text);
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
            "INSERT INTO training_plans (id, athlete_id, title, goal, week_start, duration_weeks, status, coach_note, progress_percent, created_by, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, 0, ?8, ?9, ?9)",
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
                "INSERT INTO training_plan_items (id, plan_id, week_number, day_of_week, exercise_id, custom_exercise_name, sets, reps, intensity_percent, weight_kg, notes, sort_order, created_at) \
                 VALUES (?1, ?2, 1, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
    let key = state.groq_api_key.trim();
    let club_available = !key.is_empty();
    let configured = club_available && groq_key_format_ok(key);

    Ok(Json(AiCoachStatusResponse {
        configured,
        model: configured_groq_model(&state),
        key_format_ok: !club_available || groq_key_format_ok(key),
        setup_hint: status_setup_hint(key, club_available),
        quota: build_quota_for_user(&claims.sub, configured),
    }))
}

pub async fn coach_public_status(
    State(state): State<AppState>,
) -> Result<Json<AiCoachPublicStatusResponse>, ApiError> {
    let key = state.groq_api_key.trim();
    let enabled = !key.is_empty() && groq_key_format_ok(key);
    Ok(Json(AiCoachPublicStatusResponse {
        enabled,
        model: configured_groq_model(&state),
    }))
}

pub async fn coach_public_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AiCoachPublicChatRequest>,
) -> Result<Json<AiCoachPublicChatResponse>, ApiError> {
    let key = state.groq_api_key.trim();
    if key.is_empty() || !groq_key_format_ok(key) {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Asystent klubu jest chwilowo niedostępny.",
        ));
    }

    let message = payload.message.trim();
    if message.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Treść wiadomości nie może być pusta",
        ));
    }
    if message.chars().count() > MAX_PUBLIC_MESSAGE_LEN {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!("Wiadomość jest za długa (max {MAX_PUBLIC_MESSAGE_LEN} znaków)."),
        ));
    }

    let client_ip = client_ip_from_headers(&headers);
    crate::post_throttle::reserve_ai_coach_public_chat(&client_ip).map_err(public_chat_limit_error)?;

    let history = payload
        .history
        .unwrap_or_default()
        .into_iter()
        .rev()
        .take(MAX_PUBLIC_HISTORY_TURNS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();

    let ai_settings = crate::routes::ai_coach_settings::load_ai_coach_settings(&state).await?;
    let public_instruction =
        crate::routes::ai_coach_settings::resolve_public_system_instruction(&ai_settings);
    let public_temp =
        crate::routes::ai_coach_settings::resolve_public_chat_temperature(&ai_settings);

    let llm_res = call_groq_with_key(
        key,
        &state.groq_model,
        message.to_string(),
        &history,
        &public_instruction,
        public_temp,
        false,
        MAX_OUTPUT_TOKENS_PUBLIC,
    )
    .await?;

    Ok(Json(AiCoachPublicChatResponse {
        reply: llm_res.text,
        model: llm_res.model,
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

    let attachments = process_chat_attachments(payload.attachments)?;
    let has_images = !attachments.image_parts.is_empty();
    let has_text_files = !attachments.text_blocks.is_empty();

    let message = payload.message.trim();
    if message.is_empty() && !has_images && !has_text_files {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Treść wiadomości nie może być pusta",
        ));
    }
    if message.chars().count() > MAX_USER_MESSAGE_LEN {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!(
                "Wiadomość jest za długa (max {MAX_USER_MESSAGE_LEN} znaków — oszczędzamy limit Groq)"
            ),
        ));
    }

    let mode = normalize_mode(payload.mode.as_deref());
    let history = payload
        .history
        .unwrap_or_default()
        .into_iter()
        .rev()
        .take(MAX_HISTORY_TURNS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();

    let ai_settings = crate::routes::ai_coach_settings::load_ai_coach_settings(&state).await?;
    let coach_instruction =
        crate::routes::ai_coach_settings::resolve_coach_system_instruction(&ai_settings);
    let chat_temp = crate::routes::ai_coach_settings::resolve_chat_temperature(&ai_settings);
    let vision_temp =
        crate::routes::ai_coach_settings::resolve_vision_chat_temperature(&ai_settings);

    let mut augmented = String::new();
    augmented.push_str(
        &crate::routes::ai_coach_settings::resolve_mode_prefix(mode, &ai_settings),
    );

    if let Some(ref ctx) = payload.plan_context {
        augmented.push_str(&format_plan_context(ctx));
    }

    if let Some(ref athlete_id) = payload.athlete_id.filter(|_| claims_has_staff_access(&claims)) {
        staff_may_access_athlete(&state, &claims, athlete_id).await?;
        if let Some(ctx) = fetch_athlete_context(&state, athlete_id).await {
            augmented.push_str(&ctx);
            augmented.push_str("\n\n");
        }
    } else if claims.roles.contains(&Role::Athlete) {
        let use_own_context = payload.use_athlete_context.unwrap_or(true);
        if use_own_context
            && let Some(ctx) = fetch_athlete_context_for_user(&state, &claims.sub).await
        {
            augmented.push_str(&ctx);
            augmented.push_str("\n\n");
        }
    }

    if has_text_files {
        augmented.push_str("Załączone pliki tekstowe:\n");
        augmented.push_str(&attachments.text_blocks.join("\n\n"));
        augmented.push_str("\n\n");
    }

    if has_images {
        augmented.push_str(&format!(
            "Użytkownik dołączył {} obraz(ów)/klatki wideo — przeanalizuj je wizualnie.\n\n",
            attachments.image_parts.len()
        ));
    }

    augmented.push_str(if message.is_empty() {
        "Przeanalizuj załączone materiały."
    } else {
        message
    });

    let llm_res = if has_images {
        invoke_llm_with_attachments(
            &state,
            &claims.sub,
            augmented,
            &history,
            &coach_instruction,
            vision_temp,
            MAX_OUTPUT_TOKENS_CHAT,
            &attachments.image_parts,
        )
        .await?
    } else {
        invoke_llm(
            &state,
            &claims.sub,
            augmented,
            &history,
            &coach_instruction,
            chat_temp,
            false,
            MAX_OUTPUT_TOKENS_CHAT,
            false,
        )
        .await?
    };

    Ok(Json(AiCoachChatResponse {
        reply: llm_res.text,
        model: llm_res.model,
        key_source: "club".to_string(),
    }))
}
