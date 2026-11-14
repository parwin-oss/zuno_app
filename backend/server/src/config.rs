use shared::AppConfig;

pub fn load() -> Result<AppConfig, String> {
    dotenv::dotenv().ok();
    AppConfig::from_env()
}
