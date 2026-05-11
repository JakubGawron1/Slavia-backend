use axum::{
    extract::{FromRequestParts, OptionalFromRequestParts},
    http::{StatusCode, request::Parts},
};
use jsonwebtoken::{DecodingKey, Validation, decode};
use serde::{Deserialize, Deserializer, Serialize};

use crate::api_error::{ApiError, api_error};
use crate::models::Role;
use crate::state::AppState;

/// JWT zawiera `roles` jako tablicę nazw albo (starsze tokeny) jak serde serializuje enum.
/// `TrainerAdmin` ze starych tokenów mapujemy na `Admin` + `Trainer`.
fn deserialize_claim_roles<'de, D>(deserializer: D) -> Result<Vec<Role>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Vec::<serde_json::Value>::deserialize(deserializer)?;
    let mut out: Vec<Role> = Vec::new();
    let mut add = |r: Role| {
        if !out.contains(&r) {
            out.push(r);
        }
    };
    for item in raw {
        match item {
            serde_json::Value::String(s) => match s.as_str() {
                "TrainerAdmin" => {
                    add(Role::Admin);
                    add(Role::Trainer);
                }
                other => {
                    if let Ok(r) = other.parse::<Role>() {
                        add(r);
                    }
                }
            },
            serde_json::Value::Object(map) => {
                for key in map.keys() {
                    match key.as_str() {
                        "SuperAdmin" => add(Role::SuperAdmin),
                        "Admin" => add(Role::Admin),
                        "Trainer" => add(Role::Trainer),
                        "Athlete" => add(Role::Athlete),
                        "TrainerAdmin" => {
                            add(Role::Admin);
                            add(Role::Trainer);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    #[serde(deserialize_with = "deserialize_claim_roles")]
    pub roles: Vec<Role>,
    pub exp: usize,
}

/// Kadra (trener i wyżej) — dostęp jak przy starym pojedynczym polu `role` dla tych ról.
pub(crate) fn claims_has_staff_access(claims: &Claims) -> bool {
    claims
        .roles
        .iter()
        .any(|r| matches!(r, Role::Trainer | Role::Admin | Role::SuperAdmin))
}

/// Zawodnik bez uprawnień kadrowych — np. własne zgłoszenia wyniku jako Pending.
pub(crate) fn claims_is_pure_athlete(claims: &Claims) -> bool {
    claims.roles.contains(&Role::Athlete) && !claims_has_staff_access(claims)
}

/// Konta z rolą `SuperAdmin` mogą być modyfikowane (rekord `users`, powiązania, usuwanie konta)
/// wyłącznie przez użytkownika z rolą `SuperAdmin` w JWT.
pub(crate) fn forbid_mutating_superadmin_user_record(
    claims: &Claims,
    target_roles: &[Role],
) -> Result<(), ApiError> {
    if target_roles.contains(&Role::SuperAdmin) && !claims.roles.contains(&Role::SuperAdmin) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Only SuperAdmin can modify SuperAdmin accounts",
        ));
    }
    Ok(())
}

impl FromRequestParts<AppState> for Claims {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());

        let auth_header = match auth_header {
            Some(header) => header,
            None => {
                return Err(api_error(
                    StatusCode::UNAUTHORIZED,
                    "Missing Authorization header",
                ));
            }
        };

        if !auth_header.starts_with("Bearer ") {
            return Err(api_error(
                StatusCode::UNAUTHORIZED,
                "Invalid Authorization header",
            ));
        }

        let token = &auth_header["Bearer ".len()..];

        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(state.jwt_secret.as_ref()),
            &Validation::default(),
        )
        .map_err(|_| api_error(StatusCode::UNAUTHORIZED, "Invalid Token"))?;

        // Banowanie kont jest egzekwowane w backendzie (DB) — działa natychmiast, niezależnie od wieku tokena.
        // Nie blokujemy SuperAdminów, nawet jeśli ktoś ustawiłby `is_banned=1` (dodatkowe zabezpieczenie).
        let mut rows = state
            .db
            .query(
                "SELECT is_banned FROM users WHERE id = ?1 LIMIT 1",
                [token_data.claims.sub.clone()],
            )
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let row = rows
            .next()
            .await
            .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if let Some(r) = row {
            let is_banned: i64 = r.get(0).unwrap_or(0);
            if is_banned != 0 && !token_data.claims.roles.contains(&Role::SuperAdmin) {
                return Err(api_error(StatusCode::FORBIDDEN, "Account is banned"));
            }
        }

        Ok(token_data.claims)
    }
}

/// Pozwala użyć `claims: Option<Claims>` w handlerze, gdy endpoint jest publiczny,
/// ale powinien rozszerzyć zwracane dane dla zalogowanych użytkowników.
/// Brak/niepoprawny token → `None`, bez błędu.
impl OptionalFromRequestParts<AppState> for Claims {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Option<Self>, Self::Rejection> {
        let has_auth = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .is_some();
        if !has_auth {
            return Ok(None);
        }
        match <Self as FromRequestParts<AppState>>::from_request_parts(parts, state).await {
            Ok(c) => Ok(Some(c)),
            Err(_) => Ok(None),
        }
    }
}

pub struct RequireSuperAdmin(pub Claims);

impl FromRequestParts<AppState> for RequireSuperAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let claims =
            <Claims as FromRequestParts<AppState>>::from_request_parts(parts, state).await?;
        if !claims.roles.contains(&Role::SuperAdmin) {
            return Err(api_error(StatusCode::FORBIDDEN, "Requires SuperAdmin role"));
        }
        Ok(RequireSuperAdmin(claims))
    }
}

pub struct RequireAdminOrSuperAdmin(pub Claims);

impl FromRequestParts<AppState> for RequireAdminOrSuperAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let claims =
            <Claims as FromRequestParts<AppState>>::from_request_parts(parts, state).await?;
        if !claims.roles.contains(&Role::Admin) && !claims.roles.contains(&Role::SuperAdmin) {
            return Err(api_error(
                StatusCode::FORBIDDEN,
                "Requires Admin or SuperAdmin role",
            ));
        }
        Ok(RequireAdminOrSuperAdmin(claims))
    }
}

pub struct RequireTrainerOrHigher(pub Claims);

impl FromRequestParts<AppState> for RequireTrainerOrHigher {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let claims =
            <Claims as FromRequestParts<AppState>>::from_request_parts(parts, state).await?;
        if !claims
            .roles
            .iter()
            .any(|r| matches!(r, Role::Trainer | Role::SuperAdmin))
        {
            return Err(api_error(
                StatusCode::FORBIDDEN,
                "Requires Trainer or higher role",
            ));
        }
        Ok(RequireTrainerOrHigher(claims))
    }
}

#[cfg(test)]
mod jwt_claims_tests {
    use super::*;
    use crate::models::Role;
    use chrono::{Duration, Utc};
    use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};

    #[test]
    fn claims_serde_json_roundtrip() {
        let c = Claims {
            sub: "user-1".into(),
            roles: vec![Role::SuperAdmin],
            exp: 2_147_483_647,
        };
        let json = serde_json::to_string(&c).expect("serialize claims");
        let c2: Claims = serde_json::from_str(&json).expect("deserialize claims");
        assert_eq!(c.sub, c2.sub);
        assert_eq!(c.roles, c2.roles);
        assert_eq!(c.exp, c2.exp);
    }

    #[test]
    fn claims_deserialize_legacy_trainer_admin_string() {
        let json = r#"{"sub":"u1","roles":["TrainerAdmin","Athlete"],"exp":99}"#;
        let c: Claims = serde_json::from_str(json).expect("deserialize legacy roles");
        assert!(c.roles.contains(&Role::Admin));
        assert!(c.roles.contains(&Role::Trainer));
        assert!(c.roles.contains(&Role::Athlete));
    }

    #[test]
    fn jwt_encode_decode_roundtrip() {
        let secret = b"test-secret-at-least-32-bytes-long!!";
        let exp = (Utc::now() + Duration::hours(24)).timestamp() as usize;
        let claims = Claims {
            sub: "uid".into(),
            roles: vec![Role::Admin],
            exp,
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_slice()),
        )
        .expect("encode jwt");
        let decoded = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(secret.as_slice()),
            &Validation::default(),
        )
        .expect("decode jwt");
        assert_eq!(decoded.claims.roles, vec![Role::Admin]);
    }
}
