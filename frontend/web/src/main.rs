mod api;

use std::{cell::RefCell, rc::Rc};

use futures_util::{Sink, StreamExt};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

type SocketSink = futures_util::stream::SplitSink<
    gloo_net::websocket::futures::WebSocket,
    gloo_net::websocket::Message,
>;

thread_local! {
    static SOCKET_SENDER: RefCell<Option<SocketSink>> = const { RefCell::new(None) };
}

fn main() {
    mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let (token, set_token) = signal(load_token());
    let (user, set_user) = signal(None::<api::User>);
    let (conversations, set_conversations) = signal(Vec::<api::Conversation>::new());
    let (active, set_active) = signal(None::<String>);
    let (messages, set_messages) = signal(Vec::<api::Message>::new());
    let (has_older_messages, set_has_older_messages) = signal(false);
    let (loading_older_messages, set_loading_older_messages) = signal(false);
    let (sending, set_sending) = signal(false);
    let (typing_notice, set_typing_notice) = signal(false);
    let (typing_sent, set_typing_sent) = signal(false);
    let (typing_generation, set_typing_generation) = signal(0_u64);
    let (socket_connected, set_socket_connected) = signal(false);
    let (query, set_query) = signal(String::new());
    let (results, set_results) = signal(Vec::<api::UserSummary>::new());
    let (searching, set_searching) = signal(false);
    let (selected_members, set_selected_members) = signal(Vec::<String>::new());
    let (group_title, set_group_title) = signal(String::new());
    let (creating_group, set_creating_group) = signal(false);
    let (conversation_details, set_conversation_details) = signal(None::<api::ConversationDetails>);
    let (details_open, set_details_open) = signal(false);
    let (details_loading, set_details_loading) = signal(false);
    let (members_saving, set_members_saving) = signal(false);
    let (group_title_edit, set_group_title_edit) = signal(String::new());
    let (group_title_saving, set_group_title_saving) = signal(false);
    let (draft, set_draft) = signal(String::new());
    let (email, set_email) = signal(String::new());
    let (username, set_username) = signal(String::new());
    let (display_name, set_display_name) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (registering, set_registering) = signal(false);
    let (busy, set_busy) = signal(false);
    let (profile_open, set_profile_open) = signal(false);
    let (profile_display_name, set_profile_display_name) = signal(String::new());
    let (profile_bio, set_profile_bio) = signal(String::new());
    let (profile_saving, set_profile_saving) = signal(false);
    let (avatar_uploading, set_avatar_uploading) = signal(false);
    let (error, set_error) = signal(None::<String>);

    let last_typing_conversation = Rc::new(RefCell::new(None::<String>));
    Effect::new({
        let last_typing_conversation = Rc::clone(&last_typing_conversation);
        move |_| {
            let current = active.get();
            let previous =
                std::mem::replace(&mut *last_typing_conversation.borrow_mut(), current.clone());
            if previous != current {
                if typing_sent.get_untracked() {
                    if let Some(conversation_id) = previous {
                        send_typing_event(conversation_id, false);
                    }
                }
                set_typing_sent.set(false);
                set_typing_notice.set(false);
                set_typing_generation.update(|generation| *generation = generation.wrapping_add(1));
            }
        }
    });

    Effect::new(move |_| {
        if let Some(access_token) = token.get() {
            set_busy.set(true);
            leptos::task::spawn_local(async move {
                let session = match api::me(&access_token).await {
                    Ok(current_user) => Some((access_token, current_user)),
                    Err(first_error) => match load_refresh_token() {
                        Some(refresh_token) => match api::refresh(&refresh_token).await {
                            Ok(auth) => {
                                store_tokens(&auth.access_token, &auth.refresh_token);
                                set_token.set(Some(auth.access_token.clone()));
                                Some((auth.access_token, auth.user))
                            }
                            Err(refresh_error) => {
                                clear_token();
                                set_token.set(None);
                                set_error.set(Some(format!(
                                    "Your session expired ({first_error}; refresh: {refresh_error}). Please sign in again."
                                )));
                                None
                            }
                        },
                        None => {
                            clear_token();
                            set_token.set(None);
                            set_error.set(Some(first_error));
                            None
                        }
                    },
                };
                if let Some((session_token, current_user)) = session {
                    set_user.set(Some(current_user));
                    match api::conversations(&session_token).await {
                        Ok(items) => set_conversations.set(items),
                        Err(message) => set_error.set(Some(message)),
                    }
                }
                set_busy.set(false);
            });
        }
    });

    Effect::new(move |_| {
        if let (Some(access_token), Some(conversation_id)) = (token.get(), active.get()) {
            set_messages.set(Vec::new());
            set_has_older_messages.set(false);
            leptos::task::spawn_local(async move {
                match api::messages(&access_token, &conversation_id, 0).await {
                    Ok(items) => {
                        set_has_older_messages.set(items.len() == 50);
                        set_messages.set(items);
                        if let Err(message) =
                            api::mark_conversation_read(&access_token, &conversation_id).await
                        {
                            set_error.set(Some(message));
                        }
                        match api::conversation_details(&access_token, &conversation_id).await {
                            Ok(details) => {
                                set_conversation_details.set(Some(details));
                            }
                            Err(message) => set_error.set(Some(message)),
                        }
                    }
                    Err(message) => set_error.set(Some(message)),
                }
            });
        }
    });

    Effect::new(move |_| {
        if let Some(access_token) = token.get() {
            let socket_url = api::websocket_url(&access_token);
            leptos::task::spawn_local(async move {
                let mut reconnect_delay = 500;
                loop {
                    if token.get_untracked().as_deref() != Some(access_token.as_str()) {
                        break;
                    }
                    match gloo_net::websocket::futures::WebSocket::open(&socket_url) {
                        Ok(socket) => {
                            let (outgoing, mut socket) = socket.split();
                            SOCKET_SENDER.with(|sender| *sender.borrow_mut() = Some(outgoing));
                            set_socket_connected.set(true);
                            reconnect_delay = 500;
                            match api::conversations(&access_token).await {
                                Ok(items) => set_conversations.set(items),
                                Err(message) => set_error.set(Some(message)),
                            }
                            if let Some(conversation_id) = active.get_untracked() {
                                match api::messages(&access_token, &conversation_id, 0).await {
                                    Ok(items) => {
                                        set_has_older_messages.set(items.len() == 50);
                                        set_messages.set(items);
                                        if let Err(message) = api::mark_conversation_read(
                                            &access_token,
                                            &conversation_id,
                                        )
                                        .await
                                        {
                                            set_error.set(Some(message));
                                        }
                                    }
                                    Err(message) => set_error.set(Some(message)),
                                }
                            }
                            while let Some(frame) = socket.next().await {
                                let Ok(gloo_net::websocket::Message::Text(payload)) = frame else {
                                    continue;
                                };
                                let Ok(event) = serde_json::from_str::<serde_json::Value>(&payload)
                                else {
                                    continue;
                                };
                                match event["type"].as_str() {
                                    Some("message" | "message_updated" | "message_deleted") => {
                                        let Some(conversation_id) =
                                            event["conversation_id"].as_str()
                                        else {
                                            continue;
                                        };
                                        if active.get_untracked().as_deref()
                                            == Some(conversation_id)
                                        {
                                            match api::messages(&access_token, conversation_id, 0)
                                                .await
                                            {
                                                Ok(items) => {
                                                    set_has_older_messages.set(items.len() == 50);
                                                    set_messages.set(items);
                                                    if event["type"] == "message" {
                                                        if let Err(message) =
                                                            api::mark_conversation_read(
                                                                &access_token,
                                                                conversation_id,
                                                            )
                                                            .await
                                                        {
                                                            set_error.set(Some(message));
                                                        }
                                                    }
                                                }
                                                Err(message) => set_error.set(Some(message)),
                                            }
                                        }
                                        match api::conversations(&access_token).await {
                                            Ok(items) => set_conversations.set(items),
                                            Err(message) => set_error.set(Some(message)),
                                        }
                                    }
                                    Some("typing") => {
                                        if event["conversation_id"].as_str()
                                            == active.get_untracked().as_deref()
                                            && event["user_id"].as_str()
                                                != user
                                                    .get_untracked()
                                                    .as_ref()
                                                    .map(|user| user.id.as_str())
                                        {
                                            set_typing_notice
                                                .set(event["active"].as_bool().unwrap_or(false));
                                        }
                                    }
                                    Some("read_receipt") => {
                                        if let Some(conversation_id) =
                                            event["conversation_id"].as_str()
                                        {
                                            if active.get_untracked().as_deref()
                                                == Some(conversation_id)
                                            {
                                                match api::messages(
                                                    &access_token,
                                                    conversation_id,
                                                    0,
                                                )
                                                .await
                                                {
                                                    Ok(items) => {
                                                        set_has_older_messages
                                                            .set(items.len() == 50);
                                                        set_messages.set(items);
                                                    }
                                                    Err(message) => set_error.set(Some(message)),
                                                }
                                            }
                                        }
                                    }
                                    Some("error") => {
                                        leptos::logging::warn!(
                                            "WebSocket event rejected: {}",
                                            event["message"].as_str().unwrap_or("unknown error")
                                        );
                                    }
                                    Some("presence") => {
                                        if let (Some(user_id), Some(online)) =
                                            (event["user_id"].as_str(), event["online"].as_bool())
                                        {
                                            let user_id = user_id.to_owned();
                                            set_conversation_details.update(|details| {
                                                if let Some(details) = details {
                                                    if let Some(member) = details
                                                        .members
                                                        .iter_mut()
                                                        .find(|member| member.user_id == user_id)
                                                    {
                                                        member.online = online;
                                                    }
                                                }
                                            });
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            SOCKET_SENDER.with(|sender| *sender.borrow_mut() = None);
                            set_socket_connected.set(false);
                        }
                        Err(error) => {
                            set_socket_connected.set(false);
                            leptos::logging::warn!("WebSocket connection failed: {error}");
                        }
                    }
                    gloo_timers::future::TimeoutFuture::new(reconnect_delay).await;
                    reconnect_delay = (reconnect_delay * 2).min(15_000);
                }
                set_socket_connected.set(false);
            });
        } else {
            set_socket_connected.set(false);
            set_typing_notice.set(false);
        }
    });

    let authenticate = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        set_error.set(None);
        set_busy.set(true);
        let email_value = email.get_untracked();
        let password_value = password.get_untracked();
        let username_value = username.get_untracked();
        let display_value = display_name.get_untracked();
        let is_registering = registering.get_untracked();
        leptos::task::spawn_local(async move {
            let result = if is_registering {
                api::register(
                    &username_value,
                    &email_value,
                    &password_value,
                    &display_value,
                )
                .await
            } else {
                api::login(&email_value, &password_value).await
            };
            match result {
                Ok(auth) => {
                    store_tokens(&auth.access_token, &auth.refresh_token);
                    set_user.set(Some(auth.user));
                    set_token.set(Some(auth.access_token.clone()));
                    match api::conversations(&auth.access_token).await {
                        Ok(items) => set_conversations.set(items),
                        Err(message) => set_error.set(Some(message)),
                    }
                    set_password.set(String::new());
                }
                Err(message) => set_error.set(Some(message)),
            }
            set_busy.set(false);
        });
    };

    let search = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        set_error.set(None);
        if let Some(access_token) = token.get_untracked() {
            let search_term = query.get_untracked();
            if search_term.trim().len() < 2 {
                set_results.set(Vec::new());
                set_error.set(Some("Enter at least 2 characters to search.".to_owned()));
                return;
            }
            set_searching.set(true);
            leptos::task::spawn_local(async move {
                match api::search_users(&access_token, &search_term).await {
                    Ok(items) => set_results.set(items),
                    Err(message) => set_error.set(Some(message)),
                }
                set_searching.set(false);
            });
        }
    };

    let start_direct = move |member_id: String| {
        let Some(access_token) = token.get_untracked() else {
            return;
        };
        leptos::task::spawn_local(async move {
            match api::create_conversation(
                &access_token,
                &api::CreateConversation {
                    kind: "direct".to_owned(),
                    title: None,
                    member_ids: vec![member_id],
                },
            )
            .await
            {
                Ok(conversation) => {
                    set_active.set(Some(conversation.id.clone()));
                    match api::conversations(&access_token).await {
                        Ok(items) => set_conversations.set(items),
                        Err(message) => set_error.set(Some(message)),
                    }
                }
                Err(message) => set_error.set(Some(message)),
            }
        });
    };

    let create_group = move |_| {
        let Some(access_token) = token.get_untracked() else {
            return;
        };
        let title = group_title.get_untracked();
        let member_ids = selected_members.get_untracked();
        if creating_group.get_untracked() {
            return;
        }
        set_creating_group.set(true);
        leptos::task::spawn_local(async move {
            match api::create_conversation(
                &access_token,
                &api::CreateConversation {
                    kind: "group".to_owned(),
                    title: Some(title),
                    member_ids,
                },
            )
            .await
            {
                Ok(conversation) => {
                    set_active.set(Some(conversation.id.clone()));
                    set_group_title.set(String::new());
                    set_selected_members.set(Vec::new());
                    set_results.set(Vec::new());
                    match api::conversations(&access_token).await {
                        Ok(items) => set_conversations.set(items),
                        Err(message) => set_error.set(Some(message)),
                    }
                }
                Err(message) => set_error.set(Some(message)),
            }
            set_creating_group.set(false);
        });
    };

    let open_details = move |_| {
        let (Some(access_token), Some(conversation_id)) =
            (token.get_untracked(), active.get_untracked())
        else {
            return;
        };
        set_details_open.set(true);
        set_details_loading.set(true);
        set_error.set(None);
        leptos::task::spawn_local(async move {
            match api::conversation_details(&access_token, &conversation_id).await {
                Ok(details) => {
                    set_group_title_edit
                        .set(details.conversation.title.clone().unwrap_or_default());
                    set_conversation_details.set(Some(details));
                }
                Err(message) => set_error.set(Some(message)),
            }
            set_details_loading.set(false);
        });
    };

    let add_selected_members = move |_| {
        let (Some(access_token), Some(details)) =
            (token.get_untracked(), conversation_details.get_untracked())
        else {
            return;
        };
        let conversation_id = details.conversation.id;
        let current_members: Vec<String> = details
            .members
            .into_iter()
            .map(|member| member.user_id)
            .collect();
        let member_ids: Vec<String> = selected_members
            .get_untracked()
            .into_iter()
            .filter(|member_id| !current_members.contains(member_id))
            .collect();
        if member_ids.is_empty() || members_saving.get_untracked() {
            return;
        }
        set_members_saving.set(true);
        set_error.set(None);
        leptos::task::spawn_local(async move {
            for member_id in member_ids.iter() {
                if let Err(message) =
                    api::add_conversation_member(&access_token, &conversation_id, member_id).await
                {
                    set_error.set(Some(message));
                    break;
                }
            }
            set_selected_members.set(Vec::new());
            match api::conversation_details(&access_token, &conversation_id).await {
                Ok(details) => set_conversation_details.set(Some(details)),
                Err(message) => set_error.set(Some(message)),
            }
            set_members_saving.set(false);
        });
    };

    let save_group_title = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        let (Some(access_token), Some(details)) =
            (token.get_untracked(), conversation_details.get_untracked())
        else {
            return;
        };
        let title = group_title_edit.get_untracked().trim().to_owned();
        if title.is_empty() || group_title_saving.get_untracked() {
            return;
        }
        let conversation_id = details.conversation.id;
        set_group_title_saving.set(true);
        set_error.set(None);
        leptos::task::spawn_local(async move {
            match api::update_conversation(&access_token, &conversation_id, &title).await {
                Ok(()) => {
                    set_group_title_edit.set(title);
                    match api::conversation_details(&access_token, &conversation_id).await {
                        Ok(details) => set_conversation_details.set(Some(details)),
                        Err(message) => set_error.set(Some(message)),
                    }
                    match api::conversations(&access_token).await {
                        Ok(items) => set_conversations.set(items),
                        Err(message) => set_error.set(Some(message)),
                    }
                }
                Err(message) => set_error.set(Some(message)),
            }
            set_group_title_saving.set(false);
        });
    };

    let remove_group_member = move |target_id: String| {
        let (Some(access_token), Some(details)) =
            (token.get_untracked(), conversation_details.get_untracked())
        else {
            return;
        };
        if members_saving.get_untracked() {
            return;
        }
        let conversation_id = details.conversation.id;
        let is_self = user
            .get_untracked()
            .is_some_and(|current_user| current_user.id == target_id);
        set_members_saving.set(true);
        set_error.set(None);
        leptos::task::spawn_local(async move {
            match api::remove_conversation_member(&access_token, &conversation_id, &target_id).await
            {
                Ok(()) if is_self => {
                    set_active.set(None);
                    set_conversation_details.set(None);
                    set_details_open.set(false);
                    match api::conversations(&access_token).await {
                        Ok(items) => set_conversations.set(items),
                        Err(message) => set_error.set(Some(message)),
                    }
                }
                Ok(()) => match api::conversation_details(&access_token, &conversation_id).await {
                    Ok(details) => set_conversation_details.set(Some(details)),
                    Err(message) => set_error.set(Some(message)),
                },
                Err(message) => set_error.set(Some(message)),
            }
            set_members_saving.set(false);
        });
    };

    let send = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        if sending.get_untracked() {
            return;
        }
        let (Some(access_token), Some(conversation_id)) =
            (token.get_untracked(), active.get_untracked())
        else {
            return;
        };
        let body = draft.get_untracked().trim().to_owned();
        if body.is_empty() {
            return;
        }
        set_sending.set(true);
        set_draft.set(String::new());
        leptos::task::spawn_local(async move {
            match api::send_message(&access_token, &conversation_id, &body).await {
                Ok(message) => {
                    if active.get_untracked().as_deref() == Some(conversation_id.as_str()) {
                        set_messages.update(|items| {
                            if !items.iter().any(|existing| existing.id == message.id) {
                                items.push(message);
                            }
                        });
                    }
                    match api::conversations(&access_token).await {
                        Ok(items) => set_conversations.set(items),
                        Err(error) => set_error.set(Some(error)),
                    }
                }
                Err(message) => {
                    set_draft.set(body);
                    set_error.set(Some(message));
                }
            }
            set_sending.set(false);
        });
    };

    let load_older_messages = move |_| {
        let (Some(access_token), Some(conversation_id)) =
            (token.get_untracked(), active.get_untracked())
        else {
            return;
        };
        if loading_older_messages.get_untracked() || !has_older_messages.get_untracked() {
            return;
        }
        let offset = messages.get_untracked().len();
        set_loading_older_messages.set(true);
        leptos::task::spawn_local(async move {
            match api::messages(&access_token, &conversation_id, offset).await {
                Ok(older) => {
                    set_has_older_messages.set(older.len() == 50);
                    set_messages.update(|items| {
                        for message in older.into_iter().rev() {
                            if !items.iter().any(|item| item.id == message.id) {
                                items.insert(0, message);
                            }
                        }
                    });
                }
                Err(message) => set_error.set(Some(message)),
            }
            set_loading_older_messages.set(false);
        });
    };

    let open_profile = move |_| {
        if let Some(current_user) = user.get_untracked() {
            set_profile_display_name.set(current_user.display_name.unwrap_or_default());
            set_profile_bio.set(current_user.bio.unwrap_or_default());
            set_profile_open.set(true);
            set_error.set(None);
        }
    };

    let save_profile = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        let Some(access_token) = token.get_untracked() else {
            return;
        };
        let display_name = profile_display_name.get_untracked().trim().to_owned();
        let bio = profile_bio.get_untracked();
        set_profile_saving.set(true);
        set_error.set(None);
        leptos::task::spawn_local(async move {
            match api::update_profile(&access_token, &api::UpdateProfile { display_name, bio })
                .await
            {
                Ok(updated_user) => {
                    set_user.set(Some(updated_user));
                    set_profile_open.set(false);
                }
                Err(message) => set_error.set(Some(message)),
            }
            set_profile_saving.set(false);
        });
    };

    let upload_avatar = move |ev: web_sys::Event| {
        let Some(input) = ev
            .target()
            .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            return;
        };
        let Some(file) = input.files().and_then(|files| files.get(0)) else {
            return;
        };
        input.set_value("");

        if file.size() > (5 * 1024 * 1024) as f64 {
            set_error.set(Some("Choose an image smaller than 5 MB.".to_owned()));
            return;
        }
        if !matches!(
            file.unchecked_ref::<web_sys::Blob>().type_().as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) {
            set_error.set(Some("Choose a PNG, JPEG, GIF, or WebP image.".to_owned()));
            return;
        }
        let Some(access_token) = token.get_untracked() else {
            return;
        };
        set_avatar_uploading.set(true);
        set_error.set(None);
        leptos::task::spawn_local(async move {
            match api::upload_avatar(&access_token, file).await {
                Ok(updated_user) => set_user.set(Some(updated_user)),
                Err(message) => set_error.set(Some(message)),
            }
            set_avatar_uploading.set(false);
        });
    };

    let logout = move |_| {
        if let Some(refresh_token) = load_refresh_token() {
            leptos::task::spawn_local(async move {
                if let Err(message) = api::logout(&refresh_token).await {
                    leptos::logging::warn!("Server sign-out failed: {message}");
                }
            });
        }
        clear_token();
        set_token.set(None);
        set_user.set(None);
        set_active.set(None);
        set_conversations.set(Vec::new());
        set_messages.set(Vec::new());
    };

    view! {
        <Show
            when=move || user.get().is_some()
            fallback=move || view! {
                <main class="auth-shell">
                    <section class="auth-brand">
                        <div class="brand-logo">"z"</div>
                        <p class="eyebrow">"A calmer way to connect"</p>
                        <h1>"Good conversations start here."</h1>
                        <p>"A thoughtful space for the people who matter."</p>
                        <div class="brand-orbit orbit-one"></div>
                        <div class="brand-orbit orbit-two"></div>
                    </section>
                    <section class="auth-panel">
                        <div class="auth-form-wrap">
                            <span class="mobile-brand">"zuno"</span>
                            <p class="eyebrow">{move || if registering.get() { "CREATE YOUR ACCOUNT" } else { "WELCOME BACK" }}</p>
                            <h2>{move || if registering.get() { "Join your people." } else { "Sign in to Zuno." }}</h2>
                            <p class="muted">{move || if registering.get() { "Your conversations, all in one place." } else { "Pick up right where you left off." }}</p>
                            <form class="auth-form" on:submit=authenticate>
                                <Show when=move || registering.get()>
                                    <label>"Username"
                                        <input required minlength="3" maxlength="30" autocomplete="username"
                                            prop:value=move || username.get()
                                            on:input=move |ev| set_username.set(event_target_value(&ev)) />
                                    </label>
                                    <label>"Display name"
                                        <input required maxlength="100" autocomplete="name"
                                            prop:value=move || display_name.get()
                                            on:input=move |ev| set_display_name.set(event_target_value(&ev)) />
                                    </label>
                                </Show>
                                <label>"Email address"
                                    <input required type="email" autocomplete="email"
                                        prop:value=move || email.get()
                                        on:input=move |ev| set_email.set(event_target_value(&ev)) />
                                </label>
                                <label>"Password"
                                    <input required type="password" minlength="12" maxlength="128"
                                        autocomplete=move || if registering.get() { "new-password" } else { "current-password" }
                                        prop:value=move || password.get()
                                        on:input=move |ev| set_password.set(event_target_value(&ev)) />
                                </label>
                                <Show when=move || error.get().is_some()>
                                    <p class="error-banner" role="alert">{move || error.get().unwrap_or_default()}</p>
                                </Show>
                                <button class="primary-button" type="submit" disabled=move || busy.get()>
                                    {move || if busy.get() { "Please wait…" } else if registering.get() { "Create account" } else { "Sign in" }}
                                </button>
                            </form>
                            <p class="switch-auth">
                                {move || if registering.get() { "Already have an account?" } else { "New to Zuno?" }}
                                <button class="text-button" on:click=move |_| {
                                    set_registering.update(|value| *value = !*value);
                                    set_error.set(None);
                                }>{move || if registering.get() { "Sign in" } else { "Create an account" }}</button>
                            </p>
                        </div>
                    </section>
                </main>
            }
        >
            <main class="app-shell">
                <aside class="sidebar">
                    <header class="sidebar-header">
                        <a class="wordmark" href="/" aria-label="Zuno home"><span>"z"</span>"uno"</a>
                        <button class="icon-button" title="Sign out" on:click=logout>"↗"</button>
                    </header>
                    <div class="profile-chip">
                        <div class="avatar">
                            <Show
                                when=move || user.get().as_ref().is_some_and(|u| u.avatar_url.is_some())
                                fallback=move || view! { <span>{move || user.get().map(|u| initials(u.display_name.as_deref().unwrap_or(&u.username))).unwrap_or_default()}</span> }
                            >
                                <img
                                    src=move || user.get().and_then(|u| u.avatar_url).map(|path| api::asset_url(&path)).unwrap_or_default()
                                    alt="Profile photo"
                                />
                            </Show>
                        </div>
                        <div class="profile-copy">
                            <strong>{move || user.get().map(|u| u.display_name.unwrap_or(u.username)).unwrap_or_default()}</strong>
                            <button class="profile-edit-button" on:click=open_profile>"Edit profile"</button>
                        </div>
                    </div>
                    <Show when=move || error.get().is_some()>
                        <p class="error-banner app-error" role="alert">{move || error.get().unwrap_or_default()}</p>
                    </Show>
                    <form class="search-form" on:submit=search>
                        <input type="search" placeholder="Find people…" minlength="2"
                            prop:value=move || query.get()
                            on:input=move |ev| set_query.set(event_target_value(&ev)) />
                        <button type="submit" aria-label="Search users">"⌕"</button>
                    </form>
                    <Show when=move || !results.get().is_empty()>
                        <div class="search-results">
                            <p class="section-label">"PEOPLE"</p>
                            <For each=move || results.get() key=|person| person.id.clone() let:person>
                                {let person_id = person.id.clone(); let selected_person_id = person_id.clone(); let person_name = person.display_name.clone().unwrap_or_else(|| person.username.clone()); let avatar_name = person_name.clone(); let avatar_url = person.avatar_url.clone(); view! {
                                <div class="person-row">
                                    <div class="avatar avatar-small">
                                        {if let Some(url) = avatar_url {
                                            view! { <img src=api::asset_url(&url) alt=avatar_name.clone() /> }.into_any()
                                        } else {
                                            view! { <span>{initials(&avatar_name)}</span> }.into_any()
                                        }}
                                    </div>
                                    <span class="person-name">{person_name}</span>
                                    <button class="small-button" on:click={
                                        let id = person_id.clone();
                                        move |_| start_direct(id.clone())
                                    }>"Message"</button>
                                    <button class="add-member" title="Add to group" on:click={
                                        let id = person_id.clone();
                                        move |_| set_selected_members.update(|items| {
                                            if items.contains(&id) { items.retain(|item| item != &id); } else { items.push(id.clone()); }
                                        })
                                    }>{move || if selected_members.get().contains(&selected_person_id) { "✓" } else { "+" }}</button>
                                </div>
                                }}
                            </For>
                        </div>
                    </Show>
                    <Show when=move || !query.get().trim().is_empty() && !searching.get() && results.get().is_empty()>
                        <p class="empty-note search-empty">"No people found. Try another name or username."</p>
                    </Show>
                    <Show when=move || searching.get()>
                        <p class="empty-note search-empty">"Searching people…"</p>
                    </Show>
                    <div class="group-create">
                        <p class="section-label">"NEW GROUP"</p>
                        <input maxlength="100" placeholder="Group name"
                            prop:value=move || group_title.get()
                            on:input=move |ev| set_group_title.set(event_target_value(&ev)) />
                        <button class="secondary-button" disabled=move || creating_group.get() || group_title.get().trim().is_empty() || selected_members.get().is_empty()
                            on:click=create_group>{move || if creating_group.get() { "Creating…" } else { "Create group" }} <span>{move || selected_members.get().len()}</span></button>
                    </div>
                    <div class="conversation-list">
                        <div class="list-heading"><span class="section-label">"MESSAGES"</span><span>{move || conversations.get().len()}</span></div>
                        <Show when=move || conversations.get().is_empty()>
                            <p class="empty-note">"No conversations yet. Search for someone to say hello."</p>
                        </Show>
                        <For each=move || conversations.get() key=|item| item.id.clone() let:item>
                            <button class="conversation-item"
                                on:click={
                                    let id = item.id.clone();
                                    move |_| set_active.set(Some(id.clone()))
                                }>
                                <div class="avatar">
                                    {if let Some(url) = item.avatar_url.clone() {
                                        view! { <img src=api::asset_url(&url) alt=item.title.clone().unwrap_or_default() /> }.into_any()
                                    } else {
                                        view! { <span>{initials(item.title.as_deref().unwrap_or(if item.kind == "group" { "Group" } else { "Chat" }))}</span> }.into_any()
                                    }}
                                </div>
                                <div class="conversation-copy">
                                    <strong>{item.title.clone().unwrap_or_else(|| if item.kind == "group" { "Group conversation".into() } else { "Direct message".into() })}</strong>
                                    <span>{item.last_message.clone().unwrap_or_else(|| "Start a conversation".into())}</span>
                                </div>
                            </button>
                        </For>
                    </div>
                    <footer class="sidebar-footer"><span class=move || if socket_connected.get() { "status-pulse" } else { "offline-dot" }></span>{move || if socket_connected.get() { "Live connection" } else { "Reconnecting…" }}</footer>
                </aside>
                <section class="chat-panel">
                    <Show when=move || active.get().is_some() fallback=|| view! {
                        <div class="empty-chat">
                            <div class="empty-chat-art"><span>"z"</span><i></i><i></i><i></i></div>
                            <p class="eyebrow">"YOUR SPACE"</p>
                            <h2>"Make room for a good conversation."</h2>
                            <p>"Choose a chat or find someone new to get started."</p>
                        </div>
                    }>
                        <Show when=move || {
                            let selected_id = active.get();
                            conversations.get().into_iter().find(|item| Some(item.id.clone()) == selected_id).is_some()
                        }>
                            <header class="chat-header">
                                <div class="avatar">
                                    <Show
                                        when=move || active_conversation_avatar(&conversations.get(), active.get().as_deref()).is_some()
                                        fallback=move || view! { <span>{move || initials(&active_conversation_name(&conversations.get(), active.get().as_deref()))}</span> }
                                    >
                                        <img
                                            src=move || active_conversation_avatar(&conversations.get(), active.get().as_deref()).map(|url| api::asset_url(&url)).unwrap_or_default()
                                            alt=move || active_conversation_name(&conversations.get(), active.get().as_deref())
                                        />
                                    </Show>
                                </div>
                                <div class="chat-heading">
                                    <strong>{move || active_conversation_name(&conversations.get(), active.get().as_deref())}</strong>
                                    <span>
                                        <i class=move || presence_class(conversation_details.get(), user.get())></i>
                                        {move || presence_label(conversation_details.get(), user.get())}
                                        <span class="socket-state">{move || if socket_connected.get() { " · Live" } else { " · Reconnecting…" }}</span>
                                    </span>
                                </div>
                                <button class="icon-button" title="Conversation details" aria-label="Conversation details" on:click=open_details>"···"</button>
                            </header>
                            <div class="message-area">
                                <div class="date-divider"><span>"YOUR CONVERSATION"</span></div>
                                <Show when=move || has_older_messages.get()>
                                    <button class="load-older-button" disabled=move || loading_older_messages.get() on:click=load_older_messages>
                                        {move || if loading_older_messages.get() { "Loading older messages…" } else { "Load older messages" }}
                                    </button>
                                </Show>
                                <Show when=move || typing_notice.get()>
                                    <div class="typing-indicator">"Someone is typing…"</div>
                                </Show>
                                <Show when=move || messages.get().is_empty()>
                                    <div class="empty-messages">"It’s quiet here. Send the first message."</div>
                                </Show>
                                <For each=move || messages.get() key=|message| message.id.clone() let:message>
                                    <MessageRow message=message current_user=user token=token messages=set_messages error=set_error />
                                </For>
                            </div>
                            <form class="composer" on:submit=send>
                                <textarea rows="1" maxlength="10000" placeholder="Write a message…"
                                    prop:value=move || draft.get()
                                    on:input=move |ev| {
                                        set_draft.set(event_target_value(&ev));
                                        let Some(conversation_id) = active.get_untracked() else { return; };
                                        if !typing_sent.get_untracked() {
                                            set_typing_sent.set(true);
                                            send_typing_event(conversation_id.clone(), true);
                                        }
                                        let generation = typing_generation.get_untracked().wrapping_add(1);
                                        set_typing_generation.set(generation);
                                        leptos::task::spawn_local(async move {
                                            gloo_timers::future::TimeoutFuture::new(1200).await;
                                            if typing_generation.get_untracked() == generation {
                                                send_typing_event(conversation_id, false);
                                                set_typing_sent.set(false);
                                            }
                                        });
                                    }
                                    ></textarea>
                                <span class="composer-hint">"Use the arrow to send · Shift + Enter for a new line"</span>
                                <button class="send-button" type="submit" disabled=move || sending.get() || draft.get().trim().is_empty() aria-label="Send message">
                                    {move || if sending.get() { "…" } else { "↑" }}
                                </button>
                            </form>
                        </Show>
                    </Show>
                </section>
            </main>
        </Show>
        <Show when=move || profile_open.get()>
            <div class="profile-overlay">
                <section class="profile-dialog" role="dialog" aria-modal="true" aria-labelledby="profile-title">
                    <header>
                        <div>
                            <p class="eyebrow">"YOUR ACCOUNT"</p>
                            <h2 id="profile-title">"Edit profile"</h2>
                        </div>
                        <button class="icon-button" aria-label="Close profile" on:click=move |_| set_profile_open.set(false)>"×"</button>
                    </header>
                    <div class="profile-photo-row">
                        <div class="avatar profile-avatar">
                            <Show
                                when=move || user.get().as_ref().is_some_and(|u| u.avatar_url.is_some())
                                fallback=move || view! { <span>{move || user.get().map(|u| initials(u.display_name.as_deref().unwrap_or(&u.username))).unwrap_or_default()}</span> }
                            >
                                <img
                                    src=move || user.get().and_then(|u| u.avatar_url).map(|path| api::asset_url(&path)).unwrap_or_default()
                                    alt="Profile photo"
                                />
                            </Show>
                        </div>
                        <label class="secondary-button upload-button">
                            {move || if avatar_uploading.get() { "Uploading…" } else { "Change photo" }}
                            <input
                                type="file"
                                accept="image/png,image/jpeg,image/gif,image/webp"
                                disabled=move || avatar_uploading.get()
                                on:change=upload_avatar
                            />
                        </label>
                        <span class="profile-hint">"PNG, JPEG, GIF, or WebP · up to 5 MB"</span>
                    </div>
                    <form class="profile-form" on:submit=save_profile>
                        <label>"Display name"
                            <input
                                required
                                maxlength="100"
                                prop:value=move || profile_display_name.get()
                                on:input=move |ev| set_profile_display_name.set(event_target_value(&ev))
                            />
                        </label>
                        <label>"Bio"
                            <textarea
                                rows="4"
                                maxlength="500"
                                prop:value=move || profile_bio.get()
                                on:input=move |ev| set_profile_bio.set(event_target_value(&ev))
                            ></textarea>
                        </label>
                        <div class="profile-actions">
                            <button class="secondary-button" type="button" on:click=move |_| set_profile_open.set(false)>"Cancel"</button>
                            <button class="primary-button" type="submit" disabled=move || profile_saving.get() || profile_display_name.get().trim().is_empty()>
                                {move || if profile_saving.get() { "Saving…" } else { "Save changes" }}
                            </button>
                        </div>
                    </form>
                </section>
            </div>
        </Show>
        <Show when=move || details_open.get()>
            <div class="profile-overlay">
                <section class="profile-dialog group-dialog" role="dialog" aria-modal="true" aria-labelledby="group-title">
                    <header>
                        <div>
                            <p class="eyebrow">"CONVERSATION"</p>
                            <h2 id="group-title">{move || conversation_details.get().map(|details| details.conversation.title.unwrap_or_else(|| "Direct conversation".to_owned())).unwrap_or_else(|| "Loading…".to_owned())}</h2>
                        </div>
                        <button class="icon-button" aria-label="Close conversation details" on:click=move |_| set_details_open.set(false)>"×"</button>
                    </header>
                    <Show when=move || error.get().is_some()>
                        <p class="error-banner" role="alert">{move || error.get().unwrap_or_default()}</p>
                    </Show>
                    <Show when=move || details_loading.get()>
                        <p class="empty-note">"Loading conversation details…"</p>
                    </Show>
                    <Show when=move || conversation_details.get().as_ref().is_some_and(|details| details.conversation.kind == "group")>
                        <form class="profile-form group-title-form" on:submit=save_group_title>
                            <label>"Group name"
                                <input
                                    required
                                    maxlength="100"
                                    prop:value=move || group_title_edit.get()
                                    on:input=move |ev| set_group_title_edit.set(event_target_value(&ev))
                                />
                            </label>
                            <button class="secondary-button" type="submit" disabled=move || group_title_saving.get() || group_title_edit.get().trim().is_empty()>
                                {move || if group_title_saving.get() { "Saving…" } else { "Update name" }}
                            </button>
                        </form>
                    </Show>
                    <div class="member-list">
                        <p class="section-label">"MEMBERS"</p>
                        <For
                            each=move || conversation_details.get().map(|details| details.members).unwrap_or_default()
                            key=|member| member.user_id.clone()
                            let:member
                        >
                            {let member_id = member.user_id.clone(); let member_name = member.display_name.clone().unwrap_or_else(|| member.username.clone()); let member_is_owner = member.role == "owner"; view! {
                                <div class="member-row">
                                    <div class="avatar avatar-small">
                                        {if let Some(url) = member.avatar_url.clone() {
                                            view! { <img src=api::asset_url(&url) alt=member_name.clone() /> }.into_any()
                                        } else {
                                            view! { <span>{initials(&member_name)}</span> }.into_any()
                                        }}
                                    </div>
                                    <span class="member-name">{member_name.clone()}</span>
                                    <span class=if member.online { "member-online" } else { "member-offline" }>{if member.online { "Online" } else { "Offline" }}</span>
                                    <span class="member-role" title=member.joined_at.clone()>{member.role.clone()}</span>
                                    <Show when={
                                        let target = member_id.clone();
                                        move || {
                                            let Some(details) = conversation_details.get() else { return false; };
                                            let Some(current_user) = user.get() else { return false; };
                                            let current_role = details.members.iter().find(|item| item.user_id == current_user.id).map(|item| item.role.as_str());
                                            if target == current_user.id {
                                                !member_is_owner
                                            } else {
                                                matches!(current_role, Some("owner" | "admin"))
                                            }
                                        }
                                    }>
                                        <button class="small-button" disabled=move || members_saving.get() on:click={
                                            let target = member_id.clone();
                                            move |_| remove_group_member(target.clone())
                                        }>"Remove"</button>
                                    </Show>
                                </div>
                            }}
                        </For>
                    </div>
                    <Show when=move || conversation_details.get().as_ref().is_some_and(|details| details.conversation.kind == "group")>
                        <div class="group-member-actions">
                            <span class="profile-hint">{move || selected_members.get().len().to_string()}" selected"</span>
                            <button class="secondary-button" disabled=move || members_saving.get() || selected_members.get().is_empty() on:click=add_selected_members>
                                {move || if members_saving.get() { "Updating members…" } else { "Add selected people" }}
                            </button>
                        </div>
                        <form class="search-form group-member-search" on:submit=search>
                            <input
                                type="search"
                                placeholder="Find people to add…"
                                minlength="2"
                                prop:value=move || query.get()
                                on:input=move |ev| set_query.set(event_target_value(&ev))
                            />
                            <button type="submit" aria-label="Search people to add">"⌕"</button>
                        </form>
                        <Show when=move || searching.get()>
                            <p class="empty-note">"Searching people…"</p>
                        </Show>
                        <Show when=move || !searching.get() && !query.get().trim().is_empty() && results.get().is_empty()>
                            <p class="empty-note">"No people found."</p>
                        </Show>
                        <div class="group-search-results">
                            <For each=move || results.get() key=|person| person.id.clone() let:person>
                                {let person_id = person.id.clone(); let selected_person_id = person_id.clone(); let person_label = person.display_name.clone().unwrap_or_else(|| person.username.clone()); view! {
                                    <div class="person-row">
                                        <span class="person-name">{person_label}</span>
                                        <button class="small-button" on:click={
                                            let id = person_id.clone();
                                            move |_| set_selected_members.update(|items| {
                                                if items.contains(&id) { items.retain(|item| item != &id); } else { items.push(id.clone()); }
                                            })
                                        }>
                                            {move || if selected_members.get().contains(&selected_person_id) { "Selected" } else { "Select" }}
                                        </button>
                                    </div>
                                }}
                            </For>
                        </div>
                    </Show>
                    <Show when=move || conversation_details.get().as_ref().is_some_and(|details| details.conversation.kind == "direct")>
                        <p class="empty-note">"Direct conversations have no group membership controls."</p>
                    </Show>
                </section>
            </div>
        </Show>
    }
}

#[component]
fn MessageRow(
    message: api::Message,
    current_user: ReadSignal<Option<api::User>>,
    token: ReadSignal<Option<String>>,
    messages: WriteSignal<Vec<api::Message>>,
    error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let sender_id_for_class = message.sender_id.clone();
    let sender_id_for_receipt = message.sender_id.clone();
    let sender_id_for_actions = message.sender_id.clone();
    let edit_message_id = message.id.clone();
    let delete_message_id = message.id.clone();
    let message_body = message.body.clone();
    let fallback_body = message_body.clone();
    let is_deleted = message.body == "[message deleted]";
    let (editing, set_editing) = signal(false);
    let (edit_body, set_edit_body) = signal(message.body.clone());
    let (saving, set_saving) = signal(false);
    view! {
        <div class=move || if current_user.get().as_ref().is_some_and(|user| user.id == sender_id_for_class) { "message-row own-message" } else { "message-row" }>
            <div class="message-avatar">{initials(message.sender_display_name.as_deref().unwrap_or(&message.sender_username))}</div>
            <div class="message-content">
                <div class="message-meta">
                    <strong>{message.sender_display_name.clone().unwrap_or_else(|| message.sender_username.clone())}</strong>
                    <time>{short_time(&message.created_at)}</time>
                    <Show when=move || message.edited_at.is_some()><span class="edited-label">"Edited"</span></Show>
                </div>
                <Show when=move || editing.get() fallback=move || view! { <div class="message-bubble">{fallback_body.clone()}</div> }>
                    <textarea class="edit-message-input" rows="2" maxlength="10000"
                        prop:value=move || edit_body.get()
                        on:input=move |ev| set_edit_body.set(event_target_value(&ev))
                    ></textarea>
                    <div class="message-actions">
                        <button class="small-button" disabled=move || saving.get() on:click={
                            let message_id = edit_message_id.clone();
                            move |_| {
                                let (Some(access_token), body) = (token.get_untracked(), edit_body.get_untracked()) else { return; };
                                let body = body.trim().to_owned();
                                if body.is_empty() || saving.get_untracked() { return; }
                                let message_id = message_id.clone();
                                set_saving.set(true);
                                error.set(None);
                                leptos::task::spawn_local(async move {
                                    match api::edit_message(&access_token, &message_id, &body).await {
                                        Ok(updated) => {
                                            messages.update(|items| {
                                                if let Some(item) = items.iter_mut().find(|item| item.id == updated.id) {
                                                    item.body = updated.body;
                                                    item.edited_at = updated.edited_at;
                                                }
                                            });
                                            set_editing.set(false);
                                        }
                                        Err(message) => error.set(Some(message)),
                                    }
                                    set_saving.set(false);
                                });
                            }
                        }>
                            {move || if saving.get() { "Saving…" } else { "Save" }}
                        </button>
                        <button class="small-button" disabled=move || saving.get() on:click=move |_| set_editing.set(false)>"Cancel"</button>
                    </div>
                </Show>
                <Show when=move || current_user.get().as_ref().is_some_and(|user| user.id == sender_id_for_receipt)>
                    <span class="receipt">{read_receipt_label(message.read_count, message.recipient_count)}</span>
                </Show>
                <Show when=move || current_user.get().as_ref().is_some_and(|user| user.id == sender_id_for_actions) && !is_deleted && !editing.get()>
                    <div class="message-actions">
                        <button class="small-button" on:click={
                            let body = message_body.clone();
                            move |_| {
                                set_edit_body.set(body.clone());
                                set_editing.set(true);
                            }
                        }>"Edit"</button>
                        <button class="small-button delete-message-button" disabled=move || saving.get() on:click={
                            let message_id = delete_message_id.clone();
                            move |_| {
                                let Some(access_token) = token.get_untracked() else { return; };
                                if saving.get_untracked() { return; }
                                let message_id = message_id.clone();
                                let message_id = message_id.clone();
                                set_saving.set(true);
                                error.set(None);
                                leptos::task::spawn_local(async move {
                                    match api::delete_message(&access_token, &message_id).await {
                                        Ok(()) => messages.update(|items| {
                                            if let Some(item) = items.iter_mut().find(|item| item.id == message_id) {
                                                item.body = "[message deleted]".to_owned();
                                            }
                                        }),
                                        Err(message) => error.set(Some(message)),
                                    }
                                    set_saving.set(false);
                                });
                            }
                        }>"Delete"</button>
                    </div>
                </Show>
            </div>
        </div>
    }
}

fn load_token() -> Option<String> {
    leptos::web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item("zuno_access_token").ok().flatten())
}

fn load_refresh_token() -> Option<String> {
    leptos::web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item("zuno_refresh_token").ok().flatten())
}

fn store_tokens(access_token: &str, refresh_token: &str) {
    if let Some(storage) =
        leptos::web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        let _ = storage.set_item("zuno_access_token", access_token);
        let _ = storage.set_item("zuno_refresh_token", refresh_token);
    }
}

fn clear_token() {
    if let Some(storage) =
        leptos::web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        let _ = storage.remove_item("zuno_access_token");
        let _ = storage.remove_item("zuno_refresh_token");
    }
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

fn active_conversation_name(conversations: &[api::Conversation], selected: Option<&str>) -> String {
    conversations
        .iter()
        .find(|conversation| Some(conversation.id.as_str()) == selected)
        .and_then(|conversation| conversation.title.clone())
        .unwrap_or_else(|| "Conversation".to_owned())
}

fn active_conversation_avatar(
    conversations: &[api::Conversation],
    selected: Option<&str>,
) -> Option<String> {
    conversations
        .iter()
        .find(|conversation| Some(conversation.id.as_str()) == selected)
        .and_then(|conversation| conversation.avatar_url.clone())
}

fn presence_label(
    details: Option<api::ConversationDetails>,
    current_user: Option<api::User>,
) -> String {
    let Some(details) = details else {
        return "Presence unavailable".to_owned();
    };
    if details.conversation.kind == "group" {
        let online = details
            .members
            .iter()
            .filter(|member| member.online)
            .count();
        return format!("{online} online");
    }
    let Some(current_user) = current_user else {
        return "Presence unavailable".to_owned();
    };
    details
        .members
        .iter()
        .find(|member| member.user_id != current_user.id)
        .map(|member| if member.online { "Online" } else { "Offline" }.to_owned())
        .unwrap_or_else(|| "Presence unavailable".to_owned())
}

fn presence_class(
    details: Option<api::ConversationDetails>,
    current_user: Option<api::User>,
) -> &'static str {
    let Some(details) = details else {
        return "offline-dot";
    };
    let online = if details.conversation.kind == "group" {
        details.members.iter().any(|member| member.online)
    } else {
        current_user
            .as_ref()
            .and_then(|current_user| {
                details
                    .members
                    .iter()
                    .find(|member| member.user_id != current_user.id)
            })
            .is_some_and(|member| member.online)
    };
    if online {
        "online-dot"
    } else {
        "offline-dot"
    }
}

fn read_receipt_label(read_count: i64, recipient_count: i64) -> String {
    if read_count == 0 {
        "Sent".to_owned()
    } else if read_count >= recipient_count {
        "Read".to_owned()
    } else {
        format!("Read by {read_count}")
    }
}

fn send_typing_event(conversation_id: String, active: bool) {
    leptos::task::spawn_local(async move {
        let payload = serde_json::json!({
            "type": "typing",
            "conversation_id": conversation_id,
            "active": active,
        })
        .to_string();
        let result = SOCKET_SENDER.with(|sender| {
            sender.borrow_mut().as_mut().map(|socket| {
                std::pin::Pin::new(socket).start_send(gloo_net::websocket::Message::Text(payload))
            })
        });
        if let Some(Err(error)) = result {
            leptos::logging::warn!("Could not send typing state: {error}");
        }
    });
}

fn short_time(timestamp: &str) -> String {
    timestamp
        .split('T')
        .nth(1)
        .and_then(|time| time.get(..5))
        .unwrap_or("")
        .to_owned()
}
