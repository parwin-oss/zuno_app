use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::{auth::authenticate_token, error::ApiError, state::AppState};

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RealtimeEvent {
    Message {
        conversation_id: Uuid,
        message_id: Uuid,
        sender_id: Uuid,
    },
    MessageUpdated {
        conversation_id: Uuid,
        message_id: Uuid,
        sender_id: Uuid,
    },
    MessageDeleted {
        conversation_id: Uuid,
        message_id: Uuid,
        sender_id: Uuid,
    },
    Typing {
        conversation_id: Uuid,
        user_id: Uuid,
        active: bool,
    },
    ReadReceipt {
        conversation_id: Uuid,
        message_id: Uuid,
        user_id: Uuid,
    },
    Presence {
        user_id: Uuid,
        online: bool,
    },
}

struct UserChannel {
    sender: broadcast::Sender<RealtimeEvent>,
    connections: usize,
}

#[derive(Clone, Default)]
pub struct RealtimeHub {
    channels: Arc<Mutex<HashMap<Uuid, UserChannel>>>,
}

impl RealtimeHub {
    fn connect(&self, user_id: Uuid) -> (broadcast::Receiver<RealtimeEvent>, bool) {
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let channel = channels.entry(user_id).or_insert_with(|| {
            let (sender, _) = broadcast::channel(64);
            UserChannel {
                sender,
                connections: 0,
            }
        });
        channel.connections += 1;
        (channel.sender.subscribe(), channel.connections == 1)
    }

    fn disconnect(&self, user_id: Uuid) -> bool {
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(channel) = channels.get_mut(&user_id) else {
            return false;
        };
        channel.connections = channel.connections.saturating_sub(1);
        if channel.connections == 0 {
            channels.remove(&user_id);
            true
        } else {
            false
        }
    }

    pub fn publish(&self, recipients: impl IntoIterator<Item = Uuid>, event: RealtimeEvent) {
        let channels = self
            .channels
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for recipient in recipients {
            if let Some(channel) = channels.get(&recipient) {
                let _ = channel.sender.send(event.clone());
            }
        }
    }

    pub fn is_online(&self, user_id: Uuid) -> bool {
        self.channels
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(&user_id)
    }
}

#[derive(Deserialize)]
pub struct ConnectQuery {
    access_token: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientEvent {
    Typing { conversation_id: Uuid, active: bool },
}

pub async fn websocket(
    State(state): State<AppState>,
    Query(query): Query<ConnectQuery>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let user_id = authenticate_token(&state.jwt_secret, &query.access_token)?;
    let active: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND is_active = TRUE)")
            .bind(user_id)
            .fetch_one(&state.db)
            .await?;
    if !active {
        return Err(ApiError::Unauthorized);
    }
    Ok(upgrade.on_upgrade(move |socket| handle_socket(socket, state, user_id)))
}

async fn handle_socket(socket: WebSocket, state: AppState, user_id: Uuid) {
    let (mut outgoing, mut incoming) = socket.split();
    let (mut events, first_connection) = state.realtime.connect(user_id);
    if first_connection {
        publish_presence(&state, user_id, true).await;
    }

    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
    heartbeat.tick().await;
    let mut last_pong = tokio::time::Instant::now();

    loop {
        tokio::select! {
            incoming_message = incoming.next() => {
                match incoming_message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientEvent>(&text) {
                            Ok(ClientEvent::Typing { conversation_id, active }) => {
                                match conversation_members(&state, conversation_id).await {
                                    Ok(members) if members.contains(&user_id) => {
                                        state.realtime.publish(
                                            members.into_iter().filter(|member| *member != user_id),
                                            RealtimeEvent::Typing { conversation_id, user_id, active },
                                        );
                                    }
                                    Ok(_) => {
                                        let _ = outgoing.send(Message::Text(
                                            r#"{"type":"error","message":"Conversation not found"}"#.into()
                                        )).await;
                                    }
                                    Err(error) => {
                                        tracing::error!(%error, "could not validate WebSocket event");
                                        break;
                                    }
                                }
                            }
                            Err(error) => {
                                tracing::debug!(%error, "invalid WebSocket client event");
                                let _ = outgoing.send(Message::Text(
                                    r#"{"type":"error","message":"Invalid event"}"#.into()
                                )).await;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if outgoing.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => last_pong = tokio::time::Instant::now(),
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(Message::Binary(_))) => {
                        let _ = outgoing.send(Message::Text(
                            r#"{"type":"error","message":"Binary events are not supported"}"#.into()
                        )).await;
                    }
                }
            }
            event = events.recv() => {
                match event {
                    Ok(event) => match serde_json::to_string(&event) {
                        Ok(payload) => {
                            if outgoing.send(Message::Text(payload.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            tracing::error!(%error, "could not serialize WebSocket event");
                        }
                    },
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(user_id = %user_id, skipped, "WebSocket receiver missed events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = heartbeat.tick() => {
                if tokio::time::Instant::now().duration_since(last_pong) > Duration::from_secs(60)
                    || outgoing.send(Message::Ping(Bytes::new())).await.is_err()
                {
                    break;
                }
            }
        }
    }

    if state.realtime.disconnect(user_id) {
        publish_presence(&state, user_id, false).await;
    }
}

async fn conversation_members(
    state: &AppState,
    conversation_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT user_id FROM conversation_members WHERE conversation_id = $1")
        .bind(conversation_id)
        .fetch_all(&state.db)
        .await
}

async fn publish_presence(state: &AppState, user_id: Uuid, online: bool) {
    let peers = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT peer.user_id FROM conversation_members own \
         JOIN conversation_members peer ON peer.conversation_id = own.conversation_id \
           AND peer.user_id <> own.user_id \
         WHERE own.user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await;
    match peers {
        Ok(peers) => state
            .realtime
            .publish(peers, RealtimeEvent::Presence { user_id, online }),
        Err(error) => tracing::error!(%error, "could not notify conversation peers of presence"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hub_delivers_only_to_subscribed_recipients() {
        let hub = RealtimeHub::default();
        let recipient = Uuid::new_v4();
        let unrelated = Uuid::new_v4();
        let (mut receiver, first_connection) = hub.connect(recipient);
        let (_unrelated_receiver, _) = hub.connect(unrelated);
        assert!(first_connection);

        hub.publish(
            [recipient],
            RealtimeEvent::Presence {
                user_id: Uuid::new_v4(),
                online: true,
            },
        );
        assert!(matches!(
            receiver.try_recv(),
            Ok(RealtimeEvent::Presence { online: true, .. })
        ));
        assert!(hub.disconnect(recipient));
        assert!(hub.disconnect(unrelated));
        assert!(!hub.is_online(unrelated));
    }

    #[tokio::test]
    async fn hub_tracks_multiple_connections_for_presence() {
        let hub = RealtimeHub::default();
        let user_id = Uuid::new_v4();
        let (_first, first_connection) = hub.connect(user_id);
        let (_second, second_connection) = hub.connect(user_id);
        assert!(first_connection);
        assert!(!second_connection);
        assert!(!hub.disconnect(user_id));
        assert!(hub.disconnect(user_id));
    }
}
