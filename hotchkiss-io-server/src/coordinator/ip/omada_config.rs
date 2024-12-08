use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct OmadaConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}
