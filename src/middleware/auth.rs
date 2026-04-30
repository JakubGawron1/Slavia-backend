use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

use crate::models::Role;
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub role: Role,
    pub exp: usize,
}

impl FromRequestParts<AppState> for Claims {
    type Rejection = (StatusCode, String);

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());

        let auth_header = match auth_header {
            Some(header) => header,
            None => return Err((StatusCode::UNAUTHORIZED, "Missing Authorization header".to_string())),
        };

        if !auth_header.starts_with("Bearer ") {
            return Err((StatusCode::UNAUTHORIZED, "Invalid Authorization header".to_string()));
        }

        let token = &auth_header["Bearer ".len()..];

        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(state.jwt_secret.as_ref()),
            &Validation::default(),
        )
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid Token".to_string()))?;

        Ok(token_data.claims)
    }
}

pub struct RequireSuperAdmin(pub Claims);

impl FromRequestParts<AppState> for RequireSuperAdmin {
    type Rejection = (StatusCode, String);

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let claims = Claims::from_request_parts(parts, state).await?;
        if claims.role != Role::SuperAdmin {
            return Err((StatusCode::FORBIDDEN, "Requires SuperAdmin role".to_string()));
        }
        Ok(RequireSuperAdmin(claims))
    }
}

pub struct RequireAdminOrSuperAdmin(pub Claims);

impl FromRequestParts<AppState> for RequireAdminOrSuperAdmin {
    type Rejection = (StatusCode, String);

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let claims = Claims::from_request_parts(parts, state).await?;
        if claims.role != Role::Admin && claims.role != Role::SuperAdmin {
            return Err((StatusCode::FORBIDDEN, "Requires Admin or SuperAdmin role".to_string()));
        }
        Ok(RequireAdminOrSuperAdmin(claims))
    }
}
