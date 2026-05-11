//! Błędy API jako `application/json` — `(StatusCode, String)` w Axum mapuje się na `text/plain`,
//! przez co frontend (`ofetch` / `getApiErrorMessage`) nie widzi pola `message`.

use axum::{Json, http::StatusCode};
use serde::Serialize;

#[derive(Serialize)]
pub struct ErrorBody {
    pub message: String,
}

pub type ApiError = (StatusCode, Json<ErrorBody>);

#[must_use]
pub fn api_error(status: StatusCode, msg: impl Into<String>) -> ApiError {
    (
        status,
        Json(ErrorBody {
            message: msg.into(),
        }),
    )
}
