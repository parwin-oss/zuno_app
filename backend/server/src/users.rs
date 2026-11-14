use axum::{
    body::Bytes,
    extract::State,
    http::{header::CONTENT_TYPE, HeaderMap},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::{authenticate, User},
    error::ApiError,
    state::AppState,
};

const MAX_AVATAR_BYTES: usize = 5 * 1024 * 1024;

#[derive(Deserialize)]
pub struct UpdateProfile {
    display_name: Option<String>,
    bio: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Serialize, FromRow)]
pub struct UserSummary {
    id: Uuid,
    username: String,
    display_name: Option<String>,
    avatar_url: Option<String>,
}

pub async fn get_me(
    State(state): State<AppState>,
    headers: HeaderMap,
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

pub async fn update_me(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateProfile>,
) -> Result<Json<User>, ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    if request
        .display_name
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.trim().len() > 100)
        || request.bio.as_ref().is_some_and(|value| value.len() > 500)
        || request.avatar_url.as_ref().is_some_and(|value| {
            value.len() > 2048 || !(value.starts_with("https://") || value.starts_with("/uploads/"))
        })
    {
        return Err(ApiError::BadRequest("Profile fields are invalid"));
    }

    let user = sqlx::query_as::<_, User>(
        "UPDATE users SET \
            display_name = COALESCE($2, display_name), \
            bio = COALESCE($3, bio), \
            avatar_url = COALESCE($4, avatar_url), \
            updated_at = NOW() \
         WHERE id = $1 AND is_active = TRUE \
         RETURNING id, username, email, display_name, avatar_url, bio, created_at",
    )
    .bind(user_id)
    .bind(request.display_name.map(|value| value.trim().to_owned()))
    .bind(request.bio)
    .bind(request.avatar_url)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    Ok(Json(user))
}

pub async fn upload_avatar(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<User>, ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    if body.is_empty() || body.len() > MAX_AVATAR_BYTES {
        return Err(ApiError::BadRequest(
            "Image must be between 1 byte and 5 MB",
        ));
    }

    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or_default();
    let (expected_type, extension) = image_format(&body).ok_or(ApiError::BadRequest(
        "Only PNG, JPEG, GIF, and WebP images are supported",
    ))?;
    if content_type != expected_type {
        return Err(ApiError::BadRequest(
            "Image content type does not match the file",
        ));
    }

    let old_avatar = sqlx::query_scalar::<_, Option<String>>(
        "SELECT avatar_url FROM users WHERE id = $1 AND is_active = TRUE",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::Unauthorized)?;

    let filename = format!("{user_id}-{}.{}", Uuid::new_v4(), extension);
    if let Err(error) = tokio::fs::create_dir_all(&state.uploads_dir).await {
        tracing::error!(%error, path = %state.uploads_dir.display(), "could not create avatar directory");
        return Err(ApiError::Internal);
    }
    let image_path = state.uploads_dir.join(&filename);
    if let Err(error) = tokio::fs::write(&image_path, &body).await {
        tracing::error!(%error, path = %image_path.display(), "could not store avatar");
        return Err(ApiError::Internal);
    }

    let avatar_url = format!("/uploads/{filename}");
    let updated = sqlx::query_as::<_, User>(
        "UPDATE users SET avatar_url = $2, updated_at = NOW() \
         WHERE id = $1 AND is_active = TRUE \
         RETURNING id, username, email, display_name, avatar_url, bio, created_at",
    )
    .bind(user_id)
    .bind(&avatar_url)
    .fetch_optional(&state.db)
    .await;
    let user = match updated {
        Ok(Some(user)) => user,
        Ok(None) => {
            remove_new_avatar(&image_path).await;
            return Err(ApiError::Unauthorized);
        }
        Err(error) => {
            remove_new_avatar(&image_path).await;
            return Err(error.into());
        }
    };

    if let Some(old_path) = managed_avatar_path(old_avatar.as_deref(), user_id, &state.uploads_dir)
    {
        if let Err(error) = tokio::fs::remove_file(&old_path).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(%error, path = %old_path.display(), "could not remove replaced avatar");
            }
        }
    }

    Ok(Json(user))
}

fn image_format(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "png"))
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(("image/jpeg", "jpg"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("image/gif", "gif"))
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

fn managed_avatar_path(
    url: Option<&str>,
    user_id: Uuid,
    uploads_dir: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let filename = url?.strip_prefix("/uploads/")?;
    if filename.contains('/') || !filename.starts_with(&format!("{user_id}-")) {
        return None;
    }
    Some(uploads_dir.join(filename))
}

async fn remove_new_avatar(path: &std::path::Path) {
    if let Err(error) = tokio::fs::remove_file(path).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(%error, path = %path.display(), "could not clean up avatar after failed profile update");
        }
    }
}

pub async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<SearchQuery>,
) -> Result<Json<Vec<UserSummary>>, ApiError> {
    let current_user = authenticate(&state.jwt_secret, &headers)?;
    let term = query.q.trim();
    if term.len() < 2 || term.len() > 64 {
        return Err(ApiError::BadRequest("Search query must be 2–64 characters"));
    }
    let users = sqlx::query_as::<_, UserSummary>(
        "SELECT id, username, display_name, avatar_url FROM users \
         WHERE is_active = TRUE AND id <> $1 \
           AND (username ILIKE $2 OR COALESCE(display_name, '') ILIKE $2) \
           AND NOT EXISTS (SELECT 1 FROM user_blocks b \
               WHERE (b.blocker_id = $1 AND b.blocked_id = users.id) \
                  OR (b.blocker_id = users.id AND b.blocked_id = $1)) \
         ORDER BY username LIMIT 20",
    )
    .bind(current_user)
    .bind(format!("%{term}%"))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(users))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: String,
}
