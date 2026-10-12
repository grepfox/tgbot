use serde::Deserialize;
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Default)]
struct CredentialsFile {
    #[serde(rename = "TG_TOKEN")]
    tg_token: Option<String>,
    #[serde(rename = "OWNER_USERNAME")]
    owner_username: Option<String>,
    #[serde(rename = "LASTFM_API_KEY")]
    lastfm_api_key: Option<String>,
    #[serde(rename = "LASTFM_USERNAME")]
    lastfm_username: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub tg_token: String,
    pub owner_username: String,
    pub lastfm_api_key: String,
    pub lastfm_username: String,
    pub is_local_api: bool,
    pub local_bot_api_url: Option<String>,
    pub cookies_file: Option<PathBuf>,
}

impl Config {
    pub async fn load() -> Self {
        let mut tg_token = env::var("TG_TOKEN").unwrap_or_default();
        let mut owner_username = env::var("OWNER_USERNAME").unwrap_or_else(|_| "grepfox".to_string());
        let mut lastfm_api_key = env::var("LASTFM_API_KEY").unwrap_or_default();
        let mut lastfm_username = env::var("LASTFM_USERNAME").unwrap_or_default();

        let creds_path = Path::new("credentials.json");
        if creds_path.exists() {
            if let Ok(contents) = tokio::fs::read_to_string(creds_path).await {
                if let Ok(creds) = serde_json::from_str::<CredentialsFile>(&contents) {
                    if let Some(token) = creds.tg_token {
                        if !token.is_empty() {
                            tg_token = token;
                        }
                    }
                    if let Some(user) = creds.owner_username {
                        if !user.is_empty() {
                            owner_username = user;
                        }
                    }
                    if let Some(key) = creds.lastfm_api_key {
                        if !key.is_empty() {
                            lastfm_api_key = key;
                        }
                    }
                    if let Some(user) = creds.lastfm_username {
                        if !user.is_empty() {
                            lastfm_username = user;
                        }
                    }
                }
            }
        }

        let cookies_path = Path::new("cookies.txt");
        let cookies_file = if cookies_path.exists() {
            Some(cookies_path.to_path_buf())
        } else {
            None
        };

        let mut is_local_api = false;
        let mut local_bot_api_url = None;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(1))
            .build()
            .unwrap_or_default();

        if let Ok(resp) = client.get("http://localhost:8081").send().await {
            if resp.status().is_success() || resp.status().is_client_error() || resp.status().is_server_error() {
                is_local_api = true;
                local_bot_api_url = Some("http://localhost:8081".to_string());
            }
        }

        Self {
            tg_token,
            owner_username,
            lastfm_api_key,
            lastfm_username,
            is_local_api,
            local_bot_api_url,
            cookies_file,
        }
    }
}
