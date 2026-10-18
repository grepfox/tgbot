use crate::config::Config;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::Instant;
use teloxide::types::{ChatId, MessageId};
use tokio::sync::{mpsc, Mutex, RwLock};

#[derive(Clone)]
#[allow(dead_code)]
pub struct MirrorTask {
    pub task_id: String,
    pub url: Option<String>,
    pub tg_file_id: Option<String>,
    pub filename: String,
    pub file_size: u64,
    pub do_zip: bool,
    pub user_name: String,
    pub user_id: u64,
    pub chat_id: ChatId,
    pub status_msg_id: MessageId,
    pub cancelled: Arc<AtomicBool>,
    pub start_time: Instant,
    pub pid: Arc<Mutex<Option<u32>>>,
    pub action_text: String,
    pub file_label: String,
}

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub tasks: Arc<RwLock<HashMap<String, MirrorTask>>>,
    pub queue_tx: mpsc::Sender<String>,
    pub active_count: Arc<AtomicUsize>,
    pub http_client: reqwest::Client,
}

impl AppState {
    pub fn new(config: Config, queue_tx: mpsc::Sender<String>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36")
            .build()
            .unwrap_or_default();

        Self {
            config,
            tasks: Arc::new(RwLock::new(HashMap::new())),
            queue_tx,
            active_count: Arc::new(AtomicUsize::new(0)),
            http_client,
        }
    }
}
