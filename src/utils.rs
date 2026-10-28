use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use teloxide::prelude::*;
use teloxide::types::{ChatId, InlineKeyboardMarkup, MessageId, ParseMode};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

pub fn fmt_bytes(b: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut val = b as f64;
    for unit in units {
        if val < 1024.0 {
            return format!("{:.2} {}", val, unit);
        }
        val /= 1024.0;
    }
    format!("{:.2} PB", val)
}

pub fn fmt_speed(b: f64) -> String {
    format!("{}/s", fmt_bytes(b as u64))
}

pub fn fmt_eta(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{}s", seconds);
    }
    let m = seconds / 60;
    let s = seconds % 60;
    if m < 60 {
        return format!("{}m {}s", m, s);
    }
    let h = m / 60;
    let m = m % 60;
    format!("{}h {}m {}s", h, m, s)
}

pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub async fn safe_edit_message(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    text: &str,
    reply_markup: Option<InlineKeyboardMarkup>,
    parse_mode: Option<ParseMode>,
) {
    let mut req = bot.edit_message_text(chat_id, message_id, text);
    if let Some(rm) = reply_markup {
        req = req.reply_markup(rm);
    }
    if let Some(pm) = parse_mode {
        req = req.parse_mode(pm);
    }
    if let Err(e) = req.await {
        let err_str = e.to_string();
        if !err_str.contains("Message is not modified") {
            eprintln!("Edit message error: {err_str}");
        }
    }
}

pub fn zip_path(src_path: &Path, zip_name: &str) -> std::io::Result<PathBuf> {
    let parent = src_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let zip_path_out = parent.join(zip_name);

    let file = File::create(&zip_path_out)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o755);

    if src_path.is_dir() {
        for entry in WalkDir::new(src_path) {
            let entry = entry?;
            let path = entry.path();
            let name = path
                .strip_prefix(parent)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

            let name_str = name.to_string_lossy();
            if path.is_file() {
                zip.start_file(name_str, options)?;
                let mut f = File::open(path)?;
                let mut buffer = Vec::new();
                f.read_to_end(&mut buffer)?;
                zip.write_all(&buffer)?;
            } else if !name.as_os_str().is_empty() {
                zip.add_directory(name_str, options)?;
            }
        }
    } else {
        let file_name = src_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        zip.start_file(file_name, options)?;
        let mut f = File::open(src_path)?;
        let mut buffer = Vec::new();
        f.read_to_end(&mut buffer)?;
        zip.write_all(&buffer)?;
    }

    zip.finish()?;
    Ok(zip_path_out)
}

/// The forum topic a message was sent in, if any. `None` means the General topic
/// or a chat without topics.
pub fn topic_of(msg: &Message) -> Option<teloxide::types::ThreadId> {
    if msg.is_topic_message {
        msg.thread_id
    } else {
        None
    }
}

pub trait TopicPayload {
    fn set_topic(&mut self, thread: teloxide::types::ThreadId);
}

macro_rules! impl_topic_payload {
    ($($p:ty),*) => {$(
        impl TopicPayload for $p {
            fn set_topic(&mut self, thread: teloxide::types::ThreadId) {
                self.message_thread_id = Some(thread);
            }
        }
    )*};
}

impl_topic_payload!(
    teloxide::payloads::SendMessage,
    teloxide::payloads::SendPhoto,
    teloxide::payloads::SendVideo,
    teloxide::payloads::SendDocument,
    teloxide::payloads::SendMediaGroup
);

/// Send a reply into the same forum topic as `msg`, so it doesn't land in General.
pub trait InTopic: Sized {
    fn in_topic(self, msg: &Message) -> Self;
}

impl<R> InTopic for R
where
    R: teloxide::requests::HasPayload,
    R::Payload: TopicPayload,
{
    fn in_topic(mut self, msg: &Message) -> Self {
        if let Some(thread) = topic_of(msg) {
            self.payload_mut().set_topic(thread);
        }
        self
    }
}
