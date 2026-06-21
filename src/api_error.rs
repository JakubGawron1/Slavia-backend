//! Błędy API jako `application/json` — `(StatusCode, String)` w Axum mapuje się na `text/plain`,
//! przez co frontend (`ofetch` / `getApiErrorMessage`) nie widzi pola `message`.

use axum::{Json, http::StatusCode};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct ErrorBody {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Korelacja z logami serwera (`x-request-id`); uzupełniane middleware przy 5xx.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

pub type ApiError = (StatusCode, Json<ErrorBody>);

pub fn api_error(status: StatusCode, msg: impl Into<String>) -> ApiError {
    api_error_with_code(status, msg, None::<&str>)
}

/// Błąd z kodem maszynowym (`code`) dla frontendu + opcjonalny `detail` (np. walidacja).
pub fn api_error_with_code(
    status: StatusCode,
    msg: impl Into<String>,
    code: Option<impl Into<String>>,
) -> ApiError {
    let message = msg.into();
    let code = code.map(Into::into);
    (
        status,
        Json(ErrorBody {
            message: polish_api_message(status, &message),
            code,
            detail: None,
            request_id: None,
        }),
    )
}

#[allow(dead_code)]
pub fn api_validation_error(msg: impl Into<String>, detail: impl Into<String>) -> ApiError {
    let message = msg.into();
    let detail = detail.into();
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorBody {
            message: polish_api_message(StatusCode::BAD_REQUEST, &message),
            code: Some("validation_error".to_string()),
            detail: Some(detail),
            request_id: None,
        }),
    )
}

/// Krótkie, czytelne komunikaty po polsku dla typowych kodów HTTP (gdy handler poda angielski skrót).
fn polish_api_message(status: StatusCode, msg: &str) -> String {
    let trimmed = msg.trim();
    if trimmed.is_empty() {
        return match status {
            StatusCode::UNAUTHORIZED => "Brak autoryzacji — zaloguj się ponownie.".to_string(),
            StatusCode::FORBIDDEN => "Brak uprawnień do tej operacji.".to_string(),
            StatusCode::NOT_FOUND => "Nie znaleziono zasobu.".to_string(),
            StatusCode::CONFLICT => "Konflikt danych — odśwież widok.".to_string(),
            StatusCode::BAD_REQUEST => "Nieprawidłowe żądanie.".to_string(),
            StatusCode::INTERNAL_SERVER_ERROR => "Błąd serwera — spróbuj ponownie.".to_string(),
            _ => format!("Błąd HTTP {}.", status.as_u16()),
        };
    }
    // Zostaw polskie i szczegółowe komunikaty z handlerów bez zmian.
    if trimmed.chars().any(|c| "ąćęłńóśźżĄĆĘŁŃÓŚŹŻ".contains(c)) {
        return trimmed.to_string();
    }
    match (status, trimmed) {
        (StatusCode::UNAUTHORIZED, "Missing Authorization header") => {
            "Brak nagłówka Authorization — zaloguj się.".to_string()
        }
        (StatusCode::UNAUTHORIZED, "Invalid Authorization header") => {
            "Nieprawidłowy nagłówek Authorization.".to_string()
        }
        (StatusCode::UNAUTHORIZED, "Invalid Token") => {
            "Sesja wygasła lub token jest nieprawidłowy — zaloguj się ponownie.".to_string()
        }
        (StatusCode::FORBIDDEN, "Requires Admin or SuperAdmin role") => {
            "Wymagana rola Administrator lub SuperAdmin.".to_string()
        }
        (StatusCode::FORBIDDEN, "Requires Trainer or higher role") => {
            "Wymagana rola Trener lub wyższa.".to_string()
        }
        (StatusCode::FORBIDDEN, "Brak uprawnień") => trimmed.to_string(),
        (StatusCode::NOT_FOUND, "Post not found") => "Nie znaleziono wpisu.".to_string(),
        (StatusCode::NOT_FOUND, "Announcement not found") => {
            "Nie znaleziono ogłoszenia.".to_string()
        }
        (StatusCode::NOT_FOUND, "Photo not found") => "Nie znaleziono zdjęcia.".to_string(),
        (StatusCode::BAD_REQUEST, "Name is required") => "Nazwa ćwiczenia jest wymagana.".to_string(),
        (StatusCode::BAD_REQUEST, "exercise_id is required") => {
            "Parametr exercise_id jest wymagany.".to_string()
        }
        (StatusCode::BAD_REQUEST, "Invalid ui_theme_preset") => {
            "Nieprawidłowy motyw interfejsu.".to_string()
        }
        (StatusCode::NOT_FOUND, "Exercise not found") => {
            "Nie znaleziono ćwiczenia.".to_string()
        }
        _ => trimmed.to_string(),
    }
}
