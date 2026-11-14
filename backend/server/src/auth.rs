use std::time::{SystemTime, UNIX_EPOCH};

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{extract::State, http::header::AUTHORIZATION, Json};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{error::ApiError, state::AppState};

const ACCESS_TOKEN_MINUTES: i64 = 15;
const REFRESH_TOKEN_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(FromRow)]
struct LoginRecord {
    id: Uuid,
    username: String,
    email: String,
    display_name: Option<String>,
    avatar_url: Option<String>,
    bio: Option<String>,
    created_at: chrono::DateTime<Utc>,
    password_hash: String,
}

impl LoginRecord {
    fn into_user(self) -> User {
        User {
            id: self.id,
            username: self.username,
            email: self.email,
            display_name: self.display_name,
            avatar_url: self.avatar_url,
            bio: self.bio,
            created_at: self.created_at,
        }
    }
}

#[derive(Deserialize)]
pub struct RegisterRequest {
    username: String,
    email: String,
    password: String,
    display_name: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    refresh_token: String,
}

#[derive(Serialize)]
pub struct TokenResponse {
    access_token: String,
    refresh_token: String,
    token_type: &'static str,
    expires_in: i64,
    user: User,
}

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: Uuid,
    exp: usize,
    iat: usize,
}

pub async fn register(
    State(state): State<AppState>,
    Json(request): Json<RegisterRequest>,
) -> Result<(axum::http::StatusCode, Json<TokenResponse>), ApiError> {
    let username = request.username.trim().to_ascii_lowercase();
    let email = request.email.trim().to_ascii_lowercase();
    let display_name = request.display_name.map(|name| name.trim().to_owned());
    if !(3..=30).contains(&username.len())
        || !username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(ApiError::BadRequest(
            "Username must be 3–30 letters, numbers, _ or -",
        ));
    }
    if email.len() > 255 || !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err(ApiError::BadRequest("A valid email address is required"));
    }
    if !(12..=128).contains(&request.password.len()) {
        return Err(ApiError::BadRequest(
            "Password must be between 12 and 128 characters",
        ));
    }
    if display_name
        .as_ref()
        .is_some_and(|name| name.is_empty() || name.len() > 100)
    {
        return Err(ApiError::BadRequest(
            "Display name must be between 1 and 100 characters",
        ));
    }

    let password_hash = hash_password(&request.password)?;
    let mut transaction = state.db.begin().await?;
    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (username, email, password_hash, display_name) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, username, email, display_name, avatar_url, bio, created_at",
    )
    .bind(username)
    .bind(email)
    .bind(password_hash)
    .bind(display_name)
    .fetch_one(&mut *transaction)
    .await?;
    let (access_token, refresh_token) =
        issue_tokens(&mut transaction, &state.jwt_secret, user.id).await?;
    transaction.commit().await?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(TokenResponse {
            access_token,
            refresh_token,
            token_type: "Bearer",
            expires_in: ACCESS_TOKEN_MINUTES * 60,
            user,
        }),
    ))
}

pub async fn login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    if request.password.is_empty() || request.password.len() > 128 {
        return Err(ApiError::Unauthorized);
    }
    let record = sqlx::query_as::<_, LoginRecord>(
        "SELECT id, username, email, display_name, avatar_url, bio, created_at, password_hash \
         FROM users WHERE lower(email) = lower($1) AND is_active = TRUE",
    )
    .bind(request.email.trim())
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    verify_password(&request.password, &record.password_hash)?;
    let user = record.into_user();
    let mut transaction = state.db.begin().await?;
    let (access_token, refresh_token) =
        issue_tokens(&mut transaction, &state.jwt_secret, user.id).await?;
    transaction.commit().await?;
    Ok(Json(TokenResponse {
        access_token,
        refresh_token,
        token_type: "Bearer",
        expires_in: ACCESS_TOKEN_MINUTES * 60,
        user,
    }))
}

pub async fn me(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<User>, ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    let user = sqlx::query_as::<_, User>(
        "SELECT id, username, email, display_name, avatar_url, bio, created_at \
         FROM users WHERE id = $1 AND is_active = TRUE",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    Ok(Json(user))
}

pub async fn logout(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> Result<axum::http::StatusCode, ApiError> {
    if request.refresh_token.len() > 256 {
        return Err(ApiError::BadRequest("Invalid refresh token"));
    }
    let hash = hash_refresh_token(&request.refresh_token);
    sqlx::query(
        "UPDATE refresh_tokens SET revoked_at = NOW() WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(hash)
    .execute(&state.db)
    .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn refresh(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    if request.refresh_token.len() > 256 {
        return Err(ApiError::Unauthorized);
    }
    let old_hash = hash_refresh_token(&request.refresh_token);
    let mut transaction = state.db.begin().await?;
    let user_id: Uuid = sqlx::query_scalar(
        "UPDATE refresh_tokens SET revoked_at = NOW() \
         WHERE token_hash = $1 AND revoked_at IS NULL AND expires_at > NOW() \
         RETURNING user_id",
    )
    .bind(old_hash)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    let user = sqlx::query_as::<_, User>(
        "SELECT id, username, email, display_name, avatar_url, bio, created_at \
         FROM users WHERE id = $1 AND is_active = TRUE",
    )
    .bind(user_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    let (access_token, refresh_token) =
        issue_tokens(&mut transaction, &state.jwt_secret, user_id).await?;
    transaction.commit().await?;
    Ok(Json(TokenResponse {
        access_token,
        refresh_token,
        token_type: "Bearer",
        expires_in: ACCESS_TOKEN_MINUTES * 60,
        user,
    }))
}

pub fn authenticate(secret: &str, headers: &axum::http::HeaderMap) -> Result<Uuid, ApiError> {
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)?;
    authenticate_token(secret, token)
}

pub fn authenticate_token(secret: &str, token: &str) -> Result<Uuid, ApiError> {
    let claims = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| ApiError::Unauthorized)?
    .claims;
    Ok(claims.sub)
}

fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| {
            tracing::error!(error = %error, "password hashing failed");
            ApiError::Internal
        })
}

fn verify_password(password: &str, password_hash: &str) -> Result<(), ApiError> {
    let parsed = PasswordHash::new(password_hash).map_err(|_| ApiError::Internal)?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| ApiError::Unauthorized)
}

async fn issue_tokens(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    secret: &str,
    user_id: Uuid,
) -> Result<(String, String), ApiError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ApiError::Internal)?
        .as_secs() as usize;
    let claims = Claims {
        sub: user_id,
        exp: (Utc::now() + Duration::minutes(ACCESS_TOKEN_MINUTES)).timestamp() as usize,
        iat: now,
    };
    let access_token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|error| {
        tracing::error!(error = %error, "access token creation failed");
        ApiError::Internal
    })?;
    let refresh_token = Uuid::new_v4().to_string() + &Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO refresh_tokens (user_id, token_hash, expires_at) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(hash_refresh_token(&refresh_token))
        .bind(Utc::now() + Duration::days(REFRESH_TOKEN_DAYS))
        .execute(&mut **transaction)
        .await?;
    Ok((access_token, refresh_token))
}

fn hash_refresh_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}
