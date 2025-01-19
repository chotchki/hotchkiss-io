use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    pub cloudflare_token: String,
    pub database_path: String,
    pub domain: String,
    pub log_path: String,
}
