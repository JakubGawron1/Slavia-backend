use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

use crate::api_error::{api_error, ApiError};
use crate::models::Role;
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub role: Role,
    pub exp: usize,
}

impl FromRequestParts<AppState> for Claims {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());

        let auth_header = match auth_header {
            Some(header) => header,
            None => return Err(api_error(StatusCode::UNAUTHORIZED, "Missing Authorization header")),
        };

        if !auth_header.starts_with("Bearer ") {
            return Err(api_error(StatusCode::UNAUTHORIZED, "Invalid Authorization header"));
        }

        let token = &auth_header["Bearer ".len()..];

        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(state.jwt_secret.as_ref()),
            &Validation::default(),
        )
        .map_err(|_| api_error(StatusCode::UNAUTHORIZED, "Invalid Token"))?;

        Ok(token_data.claims)
    }
}

pub struct RequireSuperAdmin(pub Claims);

impl FromRequestParts<AppState> for RequireSuperAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let claims = Claims::from_request_parts(parts, state).await?;
        if claims.role != Role::SuperAdmin {
            return Err(api_error(StatusCode::FORBIDDEN, "Requires SuperAdmin role"));
        }
        Ok(RequireSuperAdmin(claims))
    }
}

pub struct RequireAdminOrSuperAdmin(pub Claims);

impl FromRequestParts<AppState> for RequireAdminOrSuperAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let claims = Claims::from_request_parts(parts, state).await?;
        if claims.role != Role::Admin && claims.role != Role::SuperAdmin {
            return Err(api_error(StatusCode::FORBIDDEN, "Requires Admin or SuperAdmin role"));
        }
        Ok(RequireAdminOrSuperAdmin(claims))
    }
}

#[cfg(test)]
mod jwt_claims_tests {
    use super::*;
    use crate::models::Role;
    use chrono::{Duration, Utc};
    use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};

    #[test]
    fn claims_serde_json_roundtrip() {
        let c = Claims {
            sub: "user-1".into(),
            role: Role::SuperAdmin,
            exp: 2_147_483_647,
        };
        let json = serde_json::to_string(&c).expect("serialize claims");
        let c2: Claims = serde_json::from_str(&json).expect("deserialize claims");
        assert_eq!(c.sub, c2.sub);
        assert_eq!(c.role, c2.role);
        assert_eq!(c.exp, c2.exp);
    }

    #[test]
    fn jwt_encode_decode_roundtrip() {
        let secret = b"test-secret-at-least-32-bytes-long!!";
        let exp = (Utc::now() + Duration::hours(24)).timestamp() as usize;
        let claims = Claims {
            sub: "uid".into(),
            role: Role::Admin,
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
        assert_eq!(decoded.claims.role, Role::Admin);
    }
}
