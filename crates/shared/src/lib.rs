use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub jwt_secret: String,
    pub cors_origins: Vec<String>,
    pub upload_dir: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, String> {
        let host = std::env::var("APP_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned());
        let port = std::env::var("APP_PORT")
            .or_else(|_| std::env::var("PORT"))
            .unwrap_or_else(|_| "3000".to_owned())
            .parse::<u16>()
            .map_err(|_| "APP_PORT must be a valid port number".to_owned())?;
        let database_url =
            std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL must be set".to_owned())?;
        let jwt_secret =
            std::env::var("JWT_SECRET").map_err(|_| "JWT_SECRET must be set".to_owned())?;
        if jwt_secret.len() < 32 {
            return Err("JWT_SECRET must be at least 32 bytes long".to_owned());
        }
        let cors_origins = std::env::var("CORS_ORIGINS")
            .unwrap_or_else(|_| "http://127.0.0.1:8080,http://localhost:8080".to_owned())
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_owned)
            .collect();
        let upload_dir =
            std::env::var("UPLOAD_DIR").unwrap_or_else(|_| "/tmp/zuno-uploads".to_owned());

        Ok(Self {
            host,
            port,
            database_url,
            jwt_secret,
            cors_origins,
            upload_dir,
        })
    }
}
