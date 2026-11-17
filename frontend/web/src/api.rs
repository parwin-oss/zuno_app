
use gloo_net::http::Request;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use web_sys::window;

const API_BASE: &str = match option_env!("ZUNO_API_URL") {
    Some(url) => url,
    None => "",
};

pub fn websocket_url(token: &str) -> String {
    let (protocol, host) = if !API_BASE.is_empty() {
        if let Some(host) = API_BASE.strip_prefix("https://") {
            ("wss://", host)
        } else if let Some(host) = API_BASE.strip_prefix("http://") {
            ("ws://", host)
        } else {
            ("ws://", API_BASE)
        }
    } else {
        let Some(browser_window) = window() else {
            return String::new();
        };

        let location = browser_window.location();

        let is_secure = location.protocol().unwrap_or_default() == "https:";

        let protocol = if is_secure {
            "wss://"
        } else {
            "ws://"
        };

        let host = location.host().unwrap_or_default();

        return format!(
            "{protocol}{host}/ws?access_token={}",
            encode_query(token)
        );
    };

    let host = host.trim_end_matches('/');

    format!(
        "{protocol}{host}/ws?access_token={}",
        encode_query(token)
    )
}

#[derive(Clone, Debug, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub user: User,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub kind: String,
    pub title: Option<String>,
    pub avatar_url: Option<String>,
    pub last_message: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ConversationMember {
    pub user_id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: String,
    pub joined_at: String,
    pub online: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ConversationDetails {
    pub conversation: Conversation,
    pub members: Vec<ConversationMember>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Message {
    pub id: String,
    pub sender_id: String,
    pub sender_username: String,
    pub sender_display_name: Option<String>,
    pub body: String,
    pub created_at: String,
    pub edited_at: Option<String>,
    pub read_at: Option<String>,
    pub read_count: i64,
    pub recipient_count: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UserSummary {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Serialize)]
pub struct UpdateProfile {
    pub display_name: String,
    pub bio: String,
}

#[derive(Serialize)]
pub struct AddMember {
    pub user_id: String,
}

#[derive(Serialize)]
pub struct UpdateConversation {
    pub title: String,
}

#[derive(Serialize)]
pub struct CreateConversation {
    pub kind: String,
    pub title: Option<String>,
    pub member_ids: Vec<String>,
}

#[derive(Serialize)]
pub struct SendMessage {
    pub body: String,
}

#[derive(Serialize)]
pub struct EditMessage {
    pub body: String,
}

async fn decode<T: DeserializeOwned>(
    response: gloo_net::http::Response,
) -> Result<T, String> {
    if !response.ok() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| {
                value["error"]["message"]
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| format!("Request failed ({status})"));

        return Err(message);
    }

    response
        .json::<T>()
        .await
        .map_err(|error| {
            format!("Could not read the server response: {error}")
        })
}

pub async fn login(
    email: &str,
    password: &str,
) -> Result<AuthResponse, String> {
    let response = Request::post(&format!("{API_BASE}/api/auth/login"))
        .json(&serde_json::json!({
            "email": email,
            "password": password
        }))
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;

    decode(response).await
}

pub async fn logout(refresh_token: &str) -> Result<(), String> {
    let response = Request::post(&format!("{API_BASE}/api/auth/logout"))
        .json(&serde_json::json!({
            "refresh_token": refresh_token
        }))
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        Err(format!(
            "Sign-out could not be completed ({})",
            response.status()
        ))
    }
}

pub async fn refresh(
    refresh_token: &str,
) -> Result<AuthResponse, String> {
    let response = Request::post(&format!("{API_BASE}/api/auth/refresh"))
        .json(&serde_json::json!({
            "refresh_token": refresh_token
        }))
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;

    decode(response).await
}

pub async fn register(
    username: &str,
    email: &str,
    password: &str,
    display_name: &str,
) -> Result<AuthResponse, String> {
    let response = Request::post(&format!("{API_BASE}/api/auth/register"))
        .json(&serde_json::json!({
            "username": username,
            "email": email,
            "password": password,
            "display_name": display_name,
        }))
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;

    decode(response).await
}

pub async fn me(token: &str) -> Result<User, String> {
    let response = Request::get(&format!("{API_BASE}/api/auth/me"))
        .header("Authorization", &format!("Bearer {token}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;

    decode(response).await
}

pub async fn update_profile(
    token: &str,
    profile: &UpdateProfile,
) -> Result<User, String> {
    let response = Request::patch(&format!("{API_BASE}/api/users/me"))
        .header("Authorization", &format!("Bearer {token}"))
        .json(profile)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;

    decode(response).await
}

pub async fn upload_avatar(
    token: &str,
    file: web_sys::File,
) -> Result<User, String> {
    use wasm_bindgen::JsCast;

    let content_type = file.unchecked_ref::<web_sys::Blob>().type_();

    let response = Request::post(&format!(
        "{API_BASE}/api/users/me/avatar"
    ))
    .header("Authorization", &format!("Bearer {token}"))
    .header("Content-Type", &content_type)
    .body(file)
    .map_err(|error| error.to_string())?
    .send()
    .await
    .map_err(|error| error.to_string())?;

    decode(response).await
}

pub fn asset_url(path: &str) -> String {
    if path.starts_with('/') {
        format!("{API_BASE}{path}")
    } else {
        path.to_owned()
    }
}

pub async fn conversations(
    token: &str,
) -> Result<Vec<Conversation>, String> {
    let response = Request::get(&format!(
        "{API_BASE}/api/conversations"
    ))
    .header("Authorization", &format!("Bearer {token}"))
    .send()
    .await
    .map_err(|error| error.to_string())?;

    decode(response).await
}

pub async fn messages(
    token: &str,
    conversation_id: &str,
    offset: usize,
) -> Result<Vec<Message>, String> {
    let response = Request::get(&format!(
        "{API_BASE}/api/conversations/{conversation_id}/messages?limit=50&offset={offset}"
    ))
    .header("Authorization", &format!("Bearer {token}"))
    .send()
    .await
    .map_err(|error| error.to_string())?;

    let mut messages: Vec<Message> = decode(response).await?;

    messages.reverse();

    Ok(messages)
}

pub async fn search_users(
    token: &str,
    query: &str,
) -> Result<Vec<UserSummary>, String> {
    let query = encode_query(query);

    let response = Request::get(&format!(
        "{API_BASE}/api/users/search?q={query}"
    ))
    .header("Authorization", &format!("Bearer {token}"))
    .send()
    .await
    .map_err(|error| error.to_string())?;

    decode(response).await
}

fn encode_query(query: &str) -> String {
    let mut encoded = String::with_capacity(query.len());

    for byte in query.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
        {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }

    encoded
}

pub async fn create_conversation(
    token: &str,
    request: &CreateConversation,
) -> Result<Conversation, String> {
    let response = Request::post(&format!(
        "{API_BASE}/api/conversations"
    ))
    .header("Authorization", &format!("Bearer {token}"))
    .json(request)
    .map_err(|error| error.to_string())?
    .send()
    .await
    .map_err(|error| error.to_string())?;

    #[derive(Deserialize)]
    struct Details {
        conversation: Conversation,
    }

    Ok(decode::<Details>(response).await?.conversation)
}

pub async fn conversation_details(
    token: &str,
    conversation_id: &str,
) -> Result<ConversationDetails, String> {
    let response = Request::get(&format!(
        "{API_BASE}/api/conversations/{conversation_id}"
    ))
    .header("Authorization", &format!("Bearer {token}"))
    .send()
    .await
    .map_err(|error| error.to_string())?;

    decode(response).await
}

pub async fn add_conversation_member(
    token: &str,
    conversation_id: &str,
    user_id: &str,
) -> Result<(), String> {
    let response = Request::post(&format!(
        "{API_BASE}/api/conversations/{conversation_id}/members"
    ))
    .header("Authorization", &format!("Bearer {token}"))
    .json(&AddMember {
        user_id: user_id.to_owned(),
    })
    .map_err(|error| error.to_string())?
    .send()
    .await
    .map_err(|error| error.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        decode::<serde_json::Value>(response)
            .await
            .map(|_| ())
    }
}

pub async fn remove_conversation_member(
    token: &str,
    conversation_id: &str,
    user_id: &str,
) -> Result<(), String> {
    let response = Request::delete(&format!(
        "{API_BASE}/api/conversations/{conversation_id}/members/{user_id}"
    ))
    .header("Authorization", &format!("Bearer {token}"))
    .send()
    .await
    .map_err(|error| error.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        decode::<serde_json::Value>(response)
            .await
            .map(|_| ())
    }
}

pub async fn update_conversation(
    token: &str,
    conversation_id: &str,
    title: &str,
) -> Result<(), String> {
    let response = Request::patch(&format!(
        "{API_BASE}/api/conversations/{conversation_id}"
    ))
    .header("Authorization", &format!("Bearer {token}"))
    .json(&UpdateConversation {
        title: title.to_owned(),
    })
    .map_err(|error| error.to_string())?
    .send()
    .await
    .map_err(|error| error.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        decode::<serde_json::Value>(response)
            .await
            .map(|_| ())
    }
}

pub async fn send_message(
    token: &str,
    conversation_id: &str,
    body: &str,
) -> Result<Message, String> {
    let response = Request::post(&format!(
        "{API_BASE}/api/conversations/{conversation_id}/messages"
    ))
    .header("Authorization", &format!("Bearer {token}"))
    .json(&SendMessage {
        body: body.to_owned(),
    })
    .map_err(|error| error.to_string())?
    .send()
    .await
    .map_err(|error| error.to_string())?;

    decode(response).await
}

pub async fn edit_message(
    token: &str,
    message_id: &str,
    body: &str,
) -> Result<Message, String> {
    let response = Request::patch(&format!(
        "{API_BASE}/api/messages/{message_id}"
    ))
    .header("Authorization", &format!("Bearer {token}"))
    .json(&EditMessage {
        body: body.to_owned(),
    })
    .map_err(|error| error.to_string())?
    .send()
    .await
    .map_err(|error| error.to_string())?;

    decode(response).await
}

pub async fn delete_message(
    token: &str,
    message_id: &str,
) -> Result<(), String> {
    let response = Request::delete(&format!(
        "{API_BASE}/api/messages/{message_id}"
    ))
    .header("Authorization", &format!("Bearer {token}"))
    .send()
    .await
    .map_err(|error| error.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        decode::<serde_json::Value>(response)
            .await
            .map(|_| ())
    }
}

pub async fn mark_conversation_read(
    token: &str,
    conversation_id: &str,
) -> Result<(), String> {
    let response = Request::post(&format!(
        "{API_BASE}/api/conversations/{conversation_id}/read"
    ))
    .header("Authorization", &format!("Bearer {token}"))
    .send()
    .await
    .map_err(|error| error.to_string())?;

    if response.ok() {
        Ok(())
    } else {
        Err(format!(
            "Could not mark conversation as read ({})",
            response.status()
        ))
    }
}
