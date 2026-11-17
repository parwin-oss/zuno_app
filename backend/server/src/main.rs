mod auth;
mod chat;
mod config;
mod error;
mod realtime;
mod state;
mod users;

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use error::ApiError;
use shared::AppConfig;
use sqlx::postgres::PgPoolOptions;
use state::AppState;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::ServeDir;
use tracing_subscriber::EnvFilter;

async fn live() -> &'static str {
    "ok"
}

async fn ready(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<&'static str, ApiError> {
    sqlx::query("SELECT 1").execute(&state.db).await?;
    Ok("ok")
}

const MAX_AVATAR_BYTES: usize = 5 * 1024 * 1024;

fn app(config: &AppConfig, state: AppState) -> Result<Router, String> {
    let origins = config
        .cors_origins
        .iter()
        .map(|origin| {
            origin
                .parse()
                .map_err(|_| format!("CORS_ORIGINS contains an invalid origin: {origin}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
        ])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
        ]);

    let frontend_dir = ServeDir::new("/app/frontend/web/dist")
        .append_index_html_on_directories(true);

    Ok(Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/ws", get(realtime::websocket))
        .route("/api/auth/register", post(auth::register))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/refresh", post(auth::refresh))
        .route("/api/auth/me", get(auth::me))
        .route("/api/users/me", get(users::get_me).patch(users::update_me))
        .route(
            "/api/users/me/avatar",
            post(users::upload_avatar).route_layer(DefaultBodyLimit::max(MAX_AVATAR_BYTES)),
        )
        .route("/api/users/search", get(users::search))
        .route(
            "/api/conversations",
            get(chat::list_conversations).post(chat::create_conversation),
        )
        .route(
            "/api/conversations/{id}",
            get(chat::get_conversation).patch(chat::update_conversation),
        )
        .route("/api/conversations/{id}/members", post(chat::add_member))
        .route(
            "/api/conversations/{id}/members/{user_id}",
            axum::routing::delete(chat::remove_member),
        )
        .route(
            "/api/conversations/{id}/messages",
            get(chat::list_messages).post(chat::send_message),
        )
        .route(
            "/api/conversations/{id}/read",
            post(chat::mark_conversation_read),
        )
        .route(
            "/api/messages/{id}",
            axum::routing::patch(chat::edit_message).delete(chat::delete_message),
        )
        .route("/api/messages/{id}/read", post(chat::mark_read))
        .layer(cors)
        .nest_service("/uploads", ServeDir::new(&config.upload_dir))
        .fallback_service(frontend_dir)
        .with_state(state))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let config = config::load()?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;

    let state = AppState {
        db: pool,
        jwt_secret: config.jwt_secret.clone(),
        realtime: realtime::RealtimeHub::default(),
        uploads_dir: config.upload_dir.clone().into(),
    };
    let app = app(&config, state)?;
    let address = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(address, "Zuno API listening");
    axum::serve(listener, app).await?;
    Ok(())
}