//! Opcjonalne TOTP (np. Google Authenticator) — konfiguracja po zalogowaniu: `/api/auth/totp/*`.

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use totp_lite::{Sha1, totp_custom};
use uuid::Uuid;

use crate::api_error::{ApiError, api_error};
use crate::middleware::auth::Claims;
use crate::state::AppState;

const B32: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn base32_encode(data: &[u8]) -> String {
    let mut bits: u64 = 0;
    let mut bit_count = 0u32;
    let mut out = String::new();
    for &b in data {
        bits = (bits << 8) | (b as u64);
        bit_count += 8;
        while bit_count >= 5 {
            bit_count -= 5;
            let idx = ((bits >> bit_count) & 0x1f) as usize;
            if idx < B32.len() {
                out.push(B32[idx] as char);
            }
        }
    }
    if bit_count > 0 {
        let idx = (((bits << (5 - bit_count)) & 0x1f) as usize) % B32.len();
        out.push(B32[idx] as char);
    }
    out
}

pub fn decode_totp_secret_b32(s: &str) -> Option<Vec<u8>> {
    decode_base32(s)
}

fn decode_base32(s: &str) -> Option<Vec<u8>> {
    let s = s.trim().to_ascii_uppercase();
    let mut bits: u64 = 0;
    let mut bit_count = 0u32;
    let mut out: Vec<u8> = Vec::with_capacity(s.len() * 5 / 8 + 1);
    for ch in s.chars().filter(|c| *c != '=') {
        let v = B32.iter().position(|&b| b as char == ch)? as u64;
        bits = (bits << 5) | v;
        bit_count += 5;
        while bit_count >= 8 {
            bit_count -= 8;
            out.push(((bits >> bit_count) & 0xff) as u8);
        }
    }
    Some(out)
}

fn random_secret_20() -> [u8; 20] {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let mut out = [0u8; 20];
    out[..16].copy_from_slice(a.as_bytes());
    out[16..].copy_from_slice(&b.as_bytes()[..4]);
    out
}

pub fn totp_verify(secret: &[u8], code: &str) -> bool {
    let code = code.trim();
    if code.len() < 6 || code.len() > 8 || !code.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for step_off in [-1i64, 0, 1] {
        let t = ((now as i64) + step_off * 30).max(0) as u64;
        let tok = totp_custom::<Sha1>(30, 6, secret, t);
        if tok == code {
            return true;
        }
    }
    false
}

#[derive(Serialize)]
pub struct TotpSetupResponse {
    pub secret_base32: String,
    pub otpauth_uri: String,
}

pub async fn totp_setup_handler(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<TotpSetupResponse>, ApiError> {
    if let Err(()) = crate::post_throttle::record_user_post_attempt(&claims.sub, "totp_mutations") {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Zbyt wiele operacji 2FA w krótkim czasie. Spróbuj ponownie później.",
        ));
    }

    let conn = state.db.raw().await;
    let mut rows = conn
        .query(
            "SELECT totp_enabled FROM users WHERE id = ?1",
            [claims.sub.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Użytkownik nie istnieje"))?;
    let enabled: i64 = row.get(0).unwrap_or(0);
    if enabled != 0 {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Wyłącz najpierw 2FA, aby wygenerować nowy sekret.",
        ));
    }

    let raw = random_secret_20();
    let secret_base32 = base32_encode(&raw);
    conn.execute(
        "UPDATE users SET totp_secret = ?1, totp_enabled = 0 WHERE id = ?2",
        (secret_base32.clone(), claims.sub.clone()),
    )
    .await
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut rows2 = conn
        .query(
            "SELECT username FROM users WHERE id = ?1",
            [claims.sub.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let uname: String = rows2
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .and_then(|r| r.get(0).ok())
        .unwrap_or_else(|| "user".into());

    let label_raw = format!("Slavia:{uname}");
    let label = urlencoding::encode(&label_raw);
    let issuer = urlencoding::encode("Slavia");
    let sec = urlencoding::encode(&secret_base32);
    let otpauth_uri =
        format!("otpauth://totp/{label}?secret={sec}&issuer={issuer}&period=30&digits=6");

    Ok(Json(TotpSetupResponse {
        secret_base32,
        otpauth_uri,
    }))
}

#[derive(Deserialize)]
pub struct TotpEnableRequest {
    pub code: String,
}

pub async fn totp_enable_handler(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<TotpEnableRequest>,
) -> Result<StatusCode, ApiError> {
    if let Err(()) = crate::post_throttle::record_user_post_attempt(&claims.sub, "totp_mutations") {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Zbyt wiele operacji 2FA w krótkim czasie. Spróbuj ponownie później.",
        ));
    }

    let conn = state.db.raw().await;
    let mut rows = conn
        .query(
            "SELECT totp_secret, totp_enabled FROM users WHERE id = ?1",
            [claims.sub.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Użytkownik nie istnieje"))?;
    let secret_b32: Option<String> = row.get(0).ok();
    let enabled: i64 = row.get(1).unwrap_or(0);
    if enabled != 0 {
        return Err(api_error(StatusCode::BAD_REQUEST, "2FA jest już włączone."));
    }
    let secret_b32 = secret_b32.filter(|s| !s.trim().is_empty()).ok_or_else(|| {
        api_error(
            StatusCode::BAD_REQUEST,
            "Najpierw wywołaj POST /api/auth/totp/setup.",
        )
    })?;

    let raw = decode_base32(&secret_b32).ok_or_else(|| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Nieprawidłowy format sekretu w bazie.",
        )
    })?;

    if !totp_verify(&raw, &body.code) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "Nieprawidłowy kod TOTP.",
        ));
    }

    conn.execute(
        "UPDATE users SET totp_enabled = 1 WHERE id = ?1",
        [claims.sub.clone()],
    )
    .await
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct TotpDisableRequest {
    pub password: String,
}

pub async fn totp_disable_handler(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<TotpDisableRequest>,
) -> Result<StatusCode, ApiError> {
    if let Err(()) = crate::post_throttle::record_user_post_attempt(&claims.sub, "totp_mutations") {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Zbyt wiele operacji 2FA w krótkim czasie. Spróbuj ponownie później.",
        ));
    }

    let conn = state.db.raw().await;
    let mut rows = conn
        .query(
            "SELECT password_hash, totp_enabled FROM users WHERE id = ?1",
            [claims.sub.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "Użytkownik nie istnieje"))?;
    let hash: String = row
        .get(0)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let enabled: i64 = row.get(1).unwrap_or(0);
    if enabled == 0 {
        conn.execute(
            "UPDATE users SET totp_secret = NULL WHERE id = ?1",
            [claims.sub.clone()],
        )
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(StatusCode::NO_CONTENT);
    }

    let parsed = PasswordHash::new(&hash)
        .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Błąd parsowania hasła"))?;
    if Argon2::default()
        .verify_password(body.password.as_bytes(), &parsed)
        .is_err()
    {
        return Err(api_error(StatusCode::UNAUTHORIZED, "Nieprawidłowe hasło."));
    }

    conn.execute(
        "UPDATE users SET totp_secret = NULL, totp_enabled = 0 WHERE id = ?1",
        [claims.sub.clone()],
    )
    .await
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}
