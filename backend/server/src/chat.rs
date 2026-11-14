use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{auth::authenticate, error::ApiError, realtime::RealtimeEvent, state::AppState};

#[derive(Serialize, FromRow)]
pub struct Conversation {
    id: Uuid,
    kind: String,
    title: Option<String>,
    avatar_url: Option<String>,
    created_by: Uuid,
    created_at: DateTime<Utc>,
    last_message: Option<String>,
    last_message_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, FromRow)]
pub struct ConversationMember {
    user_id: Uuid,
    username: String,
    display_name: Option<String>,
    avatar_url: Option<String>,
    role: String,
    joined_at: DateTime<Utc>,
    online: bool,
}

#[derive(Serialize)]
pub struct ConversationDetails {
    conversation: Conversation,
    members: Vec<ConversationMember>,
}

#[derive(Deserialize)]
pub struct CreateConversation {
    kind: String,
    title: Option<String>,
    member_ids: Vec<Uuid>,
}

#[derive(Serialize, FromRow)]
pub struct Message {
    id: Uuid,
    conversation_id: Uuid,
    sender_id: Uuid,
    sender_username: String,
    sender_display_name: Option<String>,
    body: String,
    message_type: String,
    created_at: DateTime<Utc>,
    edited_at: Option<DateTime<Utc>>,
    read_at: Option<DateTime<Utc>>,
    read_count: i64,
    recipient_count: i64,
}

#[derive(Deserialize)]
pub struct SendMessage {
    body: String,
}

#[derive(Deserialize)]
pub struct EditMessage {
    body: String,
}

#[derive(Deserialize)]
pub struct MessagePage {
    limit: Option<i64>,
    offset: Option<i64>,
}

pub async fn list_conversations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Conversation>>, ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    let rows = sqlx::query_as::<_, Conversation>(
        "SELECT c.id, c.type AS kind, \
                COALESCE(c.title, peer.display_name, peer.username) AS title, \
                peer.avatar_url, c.created_by, c.created_at, \
                latest.body AS last_message, latest.created_at AS last_message_at \
         FROM conversations c \
         JOIN conversation_members mine ON mine.conversation_id = c.id AND mine.user_id = $1 \
         LEFT JOIN LATERAL ( \
             SELECT u.username, u.display_name, u.avatar_url FROM conversation_members cm \
             JOIN users u ON u.id = cm.user_id \
             WHERE cm.conversation_id = c.id AND cm.user_id <> $1 \
             ORDER BY cm.joined_at LIMIT 1 \
         ) peer ON c.type = 'direct' \
         LEFT JOIN LATERAL ( \
             SELECT body, created_at FROM messages \
             WHERE conversation_id = c.id AND deleted_at IS NULL \
             ORDER BY created_at DESC, id DESC LIMIT 1 \
         ) latest ON TRUE \
         ORDER BY COALESCE(latest.created_at, c.created_at) DESC \
         LIMIT 100",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

pub async fn create_conversation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateConversation>,
) -> Result<(StatusCode, Json<ConversationDetails>), ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    if !matches!(request.kind.as_str(), "direct" | "group") {
        return Err(ApiError::BadRequest(
            "Conversation kind must be direct or group",
        ));
    }
    let mut members = request.member_ids;
    members.push(user_id);
    members.sort_unstable();
    members.dedup();
    if members.len() > 100 || (request.kind == "direct" && members.len() != 2) {
        return Err(ApiError::BadRequest(
            "A direct conversation requires two users",
        ));
    }
    if request.kind == "group" && members.len() < 2 {
        return Err(ApiError::BadRequest("A group requires at least two users"));
    }
    let title = request.title.map(|value| value.trim().to_owned());
    if request.kind == "group"
        && title
            .as_ref()
            .is_none_or(|value| value.is_empty() || value.len() > 100)
    {
        return Err(ApiError::BadRequest(
            "Group title must be between 1 and 100 characters",
        ));
    }
    if request.kind == "direct" && title.is_some() {
        return Err(ApiError::BadRequest(
            "Direct conversations cannot have a title",
        ));
    }

    let mut transaction = state.db.begin().await?;
    if request.kind == "direct" {
        let pair_key = format!("{}:{}", members[0], members[1]);
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(pair_key)
            .execute(&mut *transaction)
            .await?;
        if let Some(existing_id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT c.id FROM conversations c \
             JOIN conversation_members cm ON cm.conversation_id = c.id \
             WHERE c.type = 'direct' AND cm.user_id = ANY($1) \
             GROUP BY c.id HAVING count(*) = 2 \
             ORDER BY c.created_at LIMIT 1",
        )
        .bind(&members)
        .fetch_optional(&mut *transaction)
        .await?
        {
            transaction.commit().await?;
            return Ok((
                StatusCode::OK,
                Json(load_details(&state, existing_id, user_id).await?),
            ));
        }
    }

    let existing_users: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE id = ANY($1) AND is_active = TRUE")
            .bind(&members)
            .fetch_one(&mut *transaction)
            .await?;
    if existing_users != members.len() as i64 {
        return Err(ApiError::BadRequest(
            "One or more conversation members do not exist",
        ));
    }
    let conversation_id: Uuid = sqlx::query_scalar(
        "INSERT INTO conversations (type, title, created_by) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(&request.kind)
    .bind(title)
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await?;
    for member_id in members {
        sqlx::query(
            "INSERT INTO conversation_members (conversation_id, user_id, role) \
             VALUES ($1, $2, $3)",
        )
        .bind(conversation_id)
        .bind(member_id)
        .bind(if member_id == user_id {
            "owner"
        } else {
            "member"
        })
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load_details(&state, conversation_id, user_id).await?),
    ))
}

pub async fn get_conversation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<ConversationDetails>, ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    Ok(Json(load_details(&state, conversation_id, user_id).await?))
}

pub async fn add_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<AddMember>,
) -> Result<StatusCode, ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    ensure_member(&state, conversation_id, user_id).await?;
    let mut transaction = state.db.begin().await?;
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM conversation_members WHERE conversation_id = $1 AND user_id = $2",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if !matches!(role.as_deref(), Some("owner" | "admin")) {
        return Err(ApiError::Forbidden);
    }
    let kind: String = sqlx::query_scalar("SELECT type FROM conversations WHERE id = $1")
        .bind(conversation_id)
        .fetch_one(&mut *transaction)
        .await?;
    if kind != "group" {
        return Err(ApiError::BadRequest("Members can only be added to groups"));
    }
    let member_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM conversation_members WHERE conversation_id = $1")
            .bind(conversation_id)
            .fetch_one(&mut *transaction)
            .await?;
    if member_count >= 100 {
        return Err(ApiError::BadRequest(
            "Groups cannot have more than 100 members",
        ));
    }
    let inserted = sqlx::query(
        "INSERT INTO conversation_members (conversation_id, user_id) \
         SELECT $1, id FROM users WHERE id = $2 AND is_active = TRUE \
         ON CONFLICT DO NOTHING",
    )
    .bind(conversation_id)
    .bind(request.user_id)
    .execute(&mut *transaction)
    .await?;
    if inserted.rows_affected() == 0 {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND is_active = TRUE)",
        )
        .bind(request.user_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !exists {
            return Err(ApiError::NotFound);
        }
    }
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((conversation_id, target_user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    ensure_member(&state, conversation_id, user_id).await?;
    let role: String = sqlx::query_scalar(
        "SELECT role FROM conversation_members WHERE conversation_id = $1 AND user_id = $2",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;
    if user_id != target_user_id && !matches!(role.as_str(), "owner" | "admin") {
        return Err(ApiError::Forbidden);
    }
    let result = sqlx::query(
        "DELETE FROM conversation_members WHERE conversation_id = $1 AND user_id = $2 \
         AND role <> 'owner'",
    )
    .bind(conversation_id)
    .bind(target_user_id)
    .execute(&state.db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<Uuid>,
    Query(page): Query<MessagePage>,
) -> Result<Json<Vec<Message>>, ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    ensure_member(&state, conversation_id, user_id).await?;
    let limit = page.limit.unwrap_or(50);
    let offset = page.offset.unwrap_or(0);
    if !(1..=100).contains(&limit) || !(0..=100_000).contains(&offset) {
        return Err(ApiError::BadRequest("Pagination values are out of range"));
    }
    let rows = sqlx::query_as::<_, Message>(
        "SELECT m.id, m.conversation_id, m.sender_id, u.username AS sender_username, \
                u.display_name AS sender_display_name, \
                CASE WHEN m.deleted_at IS NULL THEN m.body ELSE '[message deleted]' END AS body, \
                m.message_type, m.created_at, m.edited_at, \
                (SELECT MAX(r.read_at) FROM message_receipts r WHERE r.message_id = m.id) AS read_at, \
                (SELECT COUNT(*) FROM message_receipts r WHERE r.message_id = m.id) AS read_count, \
                (SELECT COUNT(*) FROM conversation_members cm \
                    WHERE cm.conversation_id = m.conversation_id AND cm.user_id <> m.sender_id) AS recipient_count \
         FROM messages m JOIN users u ON u.id = m.sender_id \
         WHERE m.conversation_id = $1 \
         ORDER BY m.created_at DESC, m.id DESC LIMIT $2 OFFSET $3",
    )
    .bind(conversation_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

pub async fn send_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<SendMessage>,
) -> Result<(StatusCode, Json<Message>), ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    let body = request.body.trim();
    if body.is_empty() || body.len() > 10_000 {
        return Err(ApiError::BadRequest(
            "Message must be between 1 and 10000 bytes",
        ));
    }
    ensure_member(&state, conversation_id, user_id).await?;
    let mut transaction = state.db.begin().await?;
    let blocked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_blocks \
         WHERE (blocker_id = $1 AND blocked_id = ANY( \
             SELECT user_id FROM conversation_members WHERE conversation_id = $2 AND user_id <> $1)) \
            OR (blocked_id = $1 AND blocker_id = ANY( \
             SELECT user_id FROM conversation_members WHERE conversation_id = $2 AND user_id <> $1)))",
    )
    .bind(user_id)
    .bind(conversation_id)
    .fetch_one(&mut *transaction)
    .await?;
    if blocked {
        return Err(ApiError::Forbidden);
    }
    let message_id: Uuid = sqlx::query_scalar(
        "INSERT INTO messages (conversation_id, sender_id, body) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(conversation_id)
    .bind(user_id)
    .bind(body)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query("UPDATE conversations SET updated_at = NOW() WHERE id = $1")
        .bind(conversation_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    let message = load_message(&state, message_id).await?;
    let recipients = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM conversation_members WHERE conversation_id = $1",
    )
    .bind(conversation_id)
    .fetch_all(&state.db)
    .await?;
    state.realtime.publish(
        recipients,
        RealtimeEvent::Message {
            conversation_id,
            message_id,
            sender_id: user_id,
        },
    );
    Ok((StatusCode::CREATED, Json(message)))
}

pub async fn edit_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(message_id): Path<Uuid>,
    Json(request): Json<EditMessage>,
) -> Result<Json<Message>, ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    let body = request.body.trim();
    if body.is_empty() || body.len() > 10_000 {
        return Err(ApiError::BadRequest(
            "Message must be between 1 and 10000 bytes",
        ));
    }
    let updated = sqlx::query_scalar::<_, Uuid>(
        "UPDATE messages SET body = $1, edited_at = NOW() \
         WHERE id = $2 AND sender_id = $3 AND deleted_at IS NULL \
           AND EXISTS (SELECT 1 FROM conversation_members cm \
               WHERE cm.conversation_id = messages.conversation_id AND cm.user_id = $3) \
           AND created_at > NOW() - INTERVAL '15 minutes' RETURNING id",
    )
    .bind(body)
    .bind(message_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    if updated.is_none() {
        return Err(ApiError::NotFound);
    }
    let message = load_message(&state, message_id).await?;
    let recipients = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM conversation_members WHERE conversation_id = $1",
    )
    .bind(message.conversation_id)
    .fetch_all(&state.db)
    .await?;
    state.realtime.publish(
        recipients,
        RealtimeEvent::MessageUpdated {
            conversation_id: message.conversation_id,
            message_id,
            sender_id: user_id,
        },
    );
    Ok(Json(message))
}

pub async fn delete_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(message_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    let deleted = sqlx::query_as::<_, (Uuid, Uuid)>(
        "UPDATE messages SET body = '', deleted_at = NOW() \
         WHERE id = $1 AND sender_id = $2 AND deleted_at IS NULL \
           AND EXISTS (SELECT 1 FROM conversation_members cm \
               WHERE cm.conversation_id = messages.conversation_id AND cm.user_id = $2) \
         RETURNING id, conversation_id",
    )
    .bind(message_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    let (_, conversation_id) = deleted.ok_or(ApiError::NotFound)?;
    let recipients = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM conversation_members WHERE conversation_id = $1",
    )
    .bind(conversation_id)
    .fetch_all(&state.db)
    .await?;
    state.realtime.publish(
        recipients,
        RealtimeEvent::MessageDeleted {
            conversation_id,
            message_id,
            sender_id: user_id,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

pub async fn mark_read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(message_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    let message: Option<(Uuid, Uuid)> =
        sqlx::query_as("SELECT conversation_id, sender_id FROM messages WHERE id = $1")
            .bind(message_id)
            .fetch_optional(&state.db)
            .await?;
    let (conversation_id, sender_id) = message.ok_or(ApiError::NotFound)?;
    ensure_member(&state, conversation_id, user_id).await?;
    if sender_id == user_id {
        return Ok(StatusCode::NO_CONTENT);
    }
    let inserted = sqlx::query(
        "INSERT INTO message_receipts (message_id, user_id) VALUES ($1, $2) \
         ON CONFLICT (message_id, user_id) DO NOTHING",
    )
    .bind(message_id)
    .bind(user_id)
    .execute(&state.db)
    .await?;
    if inserted.rows_affected() > 0 {
        let members = sqlx::query_scalar::<_, Uuid>(
            "SELECT user_id FROM conversation_members WHERE conversation_id = $1",
        )
        .bind(conversation_id)
        .fetch_all(&state.db)
        .await?;
        state.realtime.publish(
            members.into_iter().filter(|member| *member != user_id),
            RealtimeEvent::ReadReceipt {
                conversation_id,
                message_id,
                user_id,
            },
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn mark_conversation_read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    ensure_member(&state, conversation_id, user_id).await?;
    let mut transaction = state.db.begin().await?;
    let message_ids = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO message_receipts (message_id, user_id) \
         SELECT id, $2 FROM messages \
         WHERE conversation_id = $1 AND sender_id <> $2 AND deleted_at IS NULL \
         ON CONFLICT (message_id, user_id) DO NOTHING \
         RETURNING message_id",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_all(&mut *transaction)
    .await?;
    let members = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM conversation_members WHERE conversation_id = $1",
    )
    .bind(conversation_id)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    for message_id in message_ids {
        state.realtime.publish(
            members.iter().copied().filter(|member| *member != user_id),
            RealtimeEvent::ReadReceipt {
                conversation_id,
                message_id,
                user_id,
            },
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct AddMember {
    user_id: Uuid,
}

#[derive(Deserialize)]
pub struct UpdateConversation {
    title: String,
}

pub async fn update_conversation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<Uuid>,
    Json(request): Json<UpdateConversation>,
) -> Result<StatusCode, ApiError> {
    let user_id = authenticate(&state.jwt_secret, &headers)?;
    let title = request.title.trim();
    if title.is_empty() || title.len() > 100 {
        return Err(ApiError::BadRequest(
            "Group title must be between 1 and 100 characters",
        ));
    }

    let mut transaction = state.db.begin().await?;
    let role: Option<String> = sqlx::query_scalar(
        "SELECT cm.role FROM conversation_members cm \
         JOIN conversations c ON c.id = cm.conversation_id \
         WHERE cm.conversation_id = $1 AND cm.user_id = $2 AND c.type = 'group'",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if !matches!(role.as_deref(), Some("owner" | "admin")) {
        return Err(if role.is_some() {
            ApiError::Forbidden
        } else {
            ApiError::NotFound
        });
    }
    sqlx::query("UPDATE conversations SET title = $1, updated_at = NOW() WHERE id = $2")
        .bind(title)
        .bind(conversation_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_member(
    state: &AppState,
    conversation_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let is_member: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM conversation_members WHERE conversation_id = $1 AND user_id = $2)",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;
    if !is_member {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

async fn load_details(
    state: &AppState,
    conversation_id: Uuid,
    user_id: Uuid,
) -> Result<ConversationDetails, ApiError> {
    ensure_member(state, conversation_id, user_id).await?;
    let conversation = sqlx::query_as::<_, Conversation>(
        "SELECT c.id, c.type AS kind, \
                COALESCE(c.title, peer.display_name, peer.username) AS title, \
                peer.avatar_url, c.created_by, c.created_at, \
                latest.body AS last_message, latest.created_at AS last_message_at \
         FROM conversations c LEFT JOIN LATERAL ( \
             SELECT u.username, u.display_name, u.avatar_url FROM conversation_members cm \
             JOIN users u ON u.id = cm.user_id \
             WHERE cm.conversation_id = c.id AND cm.user_id <> $2 \
             ORDER BY cm.joined_at LIMIT 1 \
         ) peer ON c.type = 'direct' LEFT JOIN LATERAL ( \
             SELECT body, created_at FROM messages \
             WHERE conversation_id = c.id AND deleted_at IS NULL ORDER BY created_at DESC, id DESC LIMIT 1 \
         ) latest ON TRUE WHERE c.id = $1",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let mut members = sqlx::query_as::<_, ConversationMember>(
        "SELECT u.id AS user_id, u.username, u.display_name, u.avatar_url, \
                cm.role, cm.joined_at, FALSE AS online \
         FROM conversation_members cm JOIN users u ON u.id = cm.user_id \
         WHERE cm.conversation_id = $1 ORDER BY cm.joined_at",
    )
    .bind(conversation_id)
    .fetch_all(&state.db)
    .await?;
    for member in &mut members {
        member.online = state.realtime.is_online(member.user_id);
    }
    Ok(ConversationDetails {
        conversation,
        members,
    })
}

async fn load_message(state: &AppState, message_id: Uuid) -> Result<Message, ApiError> {
    sqlx::query_as::<_, Message>(
        "SELECT m.id, m.conversation_id, m.sender_id, u.username AS sender_username, \
                u.display_name AS sender_display_name, m.body, m.message_type, \
                m.created_at, m.edited_at, \
                (SELECT MAX(r.read_at) FROM message_receipts r WHERE r.message_id = m.id) AS read_at, \
                (SELECT COUNT(*) FROM message_receipts r WHERE r.message_id = m.id) AS read_count, \
                (SELECT COUNT(*) FROM conversation_members cm \
                    WHERE cm.conversation_id = m.conversation_id AND cm.user_id <> m.sender_id) AS recipient_count \
         FROM messages m JOIN users u ON u.id = m.sender_id WHERE m.id = $1",
    )
    .bind(message_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)
}
