use std::env;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub jwt_secret: String,
    pub aws_region: String,
    pub aws_access_key_id: String,
    pub aws_secret_access_key: String,
    pub s3_bucket_name: String,
    pub s3_endpoint: Option<String>,
    pub worker_concurrency: usize,
    pub su_username: Option<String>,
    pub su_password: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
        let port = env::var("PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3000);
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let jwt_secret = env::var("JWT_SECRET").expect("JWT_SECRET must be set");
        let aws_region = env::var("AWS_REGION").expect("AWS_REGION must be set");
        let aws_access_key_id = env::var("AWS_ACCESS_KEY_ID").expect("AWS_ACCESS_KEY_ID must be set");
        let aws_secret_access_key = env::var("AWS_SECRET_ACCESS_KEY").expect("AWS_SECRET_ACCESS_KEY must be set");
        let s3_bucket_name = env::var("S3_BUCKET_NAME").expect("S3_BUCKET_NAME must be set");
        let s3_endpoint = env::var("S3_ENDPOINT").ok();
        let su_username = env::var("SU_USERNAME").ok();
        let su_password = env::var("SU_PASSWORD").ok();

        Self {
            host,
            port,
            database_url,
            jwt_secret,
            aws_region,
            aws_access_key_id,
            aws_secret_access_key,
            s3_bucket_name,
            s3_endpoint,
            worker_concurrency: env::var("WORKER_CONCURRENCY")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
            su_username,
            su_password,
        }
    }
}

pub static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn get_config() -> &'static Config {
    CONFIG.get_or_init(Config::from_env)
}
