use std::path::PathBuf;

use sqlx::PgPool;

use crate::realtime::RealtimeHub;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub jwt_secret: String,
    pub realtime: RealtimeHub,
    pub uploads_dir: PathBuf,
}
