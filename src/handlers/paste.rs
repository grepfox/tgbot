use crate::state::AppState;
use crate::utils::safe_edit_message;
use std::path::{Path, PathBuf};
use teloxide::net::Download;
use teloxide::prelude::*;
use teloxide::types::{FileId, LinkPreviewOptions, Message, MessageId};
use tokio::fs::File;
use tokio::process::Command;

pub async fn upload_to_pastebin(
    client: &reqwest::Client,
    filename: &str,
    data: Vec<u8>,
) -> Result<String, String> {
    let part = reqwest::multipart::Part::bytes(data)
        .file_name(filename.to_string())
        .mime_str("text/plain")
        .map_err(|e| e.to_string())?;

    let form = reqwest::multipart::Form::new().part("file", part);

    let resp = client
        .post("https://paste.grepfox.online/")
        .multipart(form)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Failed to paste. Status code: {status}"));
    }

    let text = resp.text().await.map_err(|e| e.to_string())?;
    Ok(text.trim().to_string())
}

async fn edit_paste_success(bot: &Bot, chat_id: ChatId, message_id: MessageId, url: &str) {
    let raw_url = format!("{}/raw", url.trim_end_matches('/'));
    let text = format!("Pasted successfully: {url}\nRaw: {raw_url}");
    let link_opts = LinkPreviewOptions {
        is_disabled: true,
        url: None,
        prefer_small_media: false,
        prefer_large_media: false,
        show_above_text: false,
    };
    if let Err(e) = bot
        .edit_message_text(chat_id, message_id, text)
        .link_preview_options(link_opts)
        .await
    {
        eprintln!("Edit message error: {e}");
    }
}

pub async fn download_telegram_file(
    bot: &Bot,
    state: &AppState,
    file_id: &str,
    filename: &str,
) -> Result<PathBuf, String> {
    let temp_dir = std::env::temp_dir().join(format!("tg_dl_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .map_err(|e| e.to_string())?;
    let local_path = temp_dir.join(filename);

    let tg_file = bot
        .get_file(FileId(file_id.to_string()))
        .await
        .map_err(|e| e.to_string())?;
    let file_path = tg_file.path;

    if state.config.is_local_api {
        let mut container_file_path = file_path.clone();
        if let Some(idx) = file_path.find("/var/lib/telegram-bot-api/") {
            container_file_path = file_path[idx..].to_string();
        } else if !file_path.starts_with('/') {
            let token_str = format!("bot{}/", state.config.tg_token);
            let rel = if let Some(idx) = file_path.find(&token_str) {
                &file_path[idx + token_str.len()..]
            } else {
                &file_path
            };
            container_file_path = format!("/var/lib/telegram-bot-api/{}/{}", state.config.tg_token, rel);
        }

        let host_path = container_file_path.replace(
            "/var/lib/telegram-bot-api",
            "/home/gnsh/tgbot/telegram-bot-api-data",
        );

        if Path::new(&host_path).exists() {
            tokio::fs::copy(&host_path, &local_path)
                .await
                .map_err(|e| e.to_string())?;
        } else if Path::new(&container_file_path).exists() {
            tokio::fs::copy(&container_file_path, &local_path)
                .await
                .map_err(|e| e.to_string())?;
        } else {
            let output = Command::new("docker")
                .args([
                    "cp",
                    &format!("telegram-bot-api:{container_file_path}"),
                    local_path.to_str().unwrap_or_default(),
                ])
                .output()
                .await
                .map_err(|e| e.to_string())?;

            if !output.status.success() {
                let err = String::from_utf8_lossy(&output.stderr);
                return Err(format!("docker cp failed: {err}"));
            }
        }
    } else {
        let mut out_file = File::create(&local_path).await.map_err(|e| e.to_string())?;
        bot.download_file(&file_path, &mut out_file)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(local_path)
}

pub async fn paste_cmd(
    bot: &Bot,
    msg: &Message,
    args: &[String],
    state: &AppState,
) -> ResponseResult<()> {
    let mut file_info: Option<(String, String)> = None;

    if let Some(doc) = msg.document() {
        let name = doc
            .file_name
            .clone()
            .unwrap_or_else(|| "paste.txt".to_string());
        file_info = Some((doc.file.id.to_string(), name));
    } else if let Some(video) = msg.video() {
        let name = video
            .file_name
            .clone()
            .unwrap_or_else(|| format!("video_{}.mp4", &video.file.id.0[..6.min(video.file.id.0.len())]));
        file_info = Some((video.file.id.to_string(), name));
    } else if let Some(audio) = msg.audio() {
        let name = audio
            .file_name
            .clone()
            .unwrap_or_else(|| format!("audio_{}.mp3", &audio.file.id.0[..6.min(audio.file.id.0.len())]));
        file_info = Some((audio.file.id.to_string(), name));
    } else if let Some(photos) = msg.photo() {
        if let Some(photo) = photos.last() {
            let name = format!("photo_{}.jpg", &photo.file.id.0[..6.min(photo.file.id.0.len())]);
            file_info = Some((photo.file.id.to_string(), name));
        }
    } else if let Some(reply) = msg.reply_to_message() {
        if let Some(doc) = reply.document() {
            let name = doc
                .file_name
                .clone()
                .unwrap_or_else(|| "paste.txt".to_string());
            file_info = Some((doc.file.id.to_string(), name));
        } else if let Some(video) = reply.video() {
            let name = video
                .file_name
                .clone()
                .unwrap_or_else(|| format!("video_{}.mp4", &video.file.id.0[..6.min(video.file.id.0.len())]));
            file_info = Some((video.file.id.to_string(), name));
        } else if let Some(audio) = reply.audio() {
            let name = audio
                .file_name
                .clone()
                .unwrap_or_else(|| format!("audio_{}.mp3", &audio.file.id.0[..6.min(audio.file.id.0.len())]));
            file_info = Some((audio.file.id.to_string(), name));
        } else if let Some(photos) = reply.photo() {
            if let Some(photo) = photos.last() {
                let name = format!("photo_{}.jpg", &photo.file.id.0[..6.min(photo.file.id.0.len())]);
                file_info = Some((photo.file.id.to_string(), name));
            }
        }
    }

    let mut text_content = args.join(" ");
    if file_info.is_none() && text_content.is_empty() {
        if let Some(reply) = msg.reply_to_message() {
            if let Some(txt) = reply.text() {
                text_content = txt.to_string();
            } else if let Some(caption) = reply.caption() {
                text_content = caption.to_string();
            }
        }
    }

    if let Some((fid, fname)) = file_info {
        let status_msg = bot
            .send_message(msg.chat.id, "Fetching file from Telegram...")
            .await?;
        match download_telegram_file(bot, state, &fid, &fname).await {
            Ok(local_path) => {
                safe_edit_message(
                    bot,
                    msg.chat.id,
                    status_msg.id,
                    "Uploading file to pastebin...",
                    None,
                    None,
                )
                .await;

                match tokio::fs::read(&local_path).await {
                    Ok(bytes) => {
                        match upload_to_pastebin(&state.http_client, &fname, bytes).await {
                            Ok(url) => {
                                edit_paste_success(bot, msg.chat.id, status_msg.id, &url).await;
                            }
                            Err(e) => {
                                safe_edit_message(
                                    bot,
                                    msg.chat.id,
                                    status_msg.id,
                                    &format!("Error: {e}"),
                                    None,
                                    None,
                                )
                                .await;
                            }
                        }
                    }
                    Err(e) => {
                        safe_edit_message(
                            bot,
                            msg.chat.id,
                            status_msg.id,
                            &format!("Error reading file: {e}"),
                            None,
                            None,
                        )
                        .await;
                    }
                }
                let _ = tokio::fs::remove_file(&local_path).await;
                if let Some(p) = local_path.parent() {
                    let _ = tokio::fs::remove_dir(p).await;
                }
            }
            Err(e) => {
                safe_edit_message(
                    bot,
                    msg.chat.id,
                    status_msg.id,
                    &format!("Error: {e}"),
                    None,
                    None,
                )
                .await;
            }
        }
    } else if !text_content.is_empty() {
        let status_msg = bot
            .send_message(msg.chat.id, "Uploading paste...")
            .await?;
        match upload_to_pastebin(&state.http_client, "paste.txt", text_content.into_bytes()).await {
            Ok(url) => {
                edit_paste_success(bot, msg.chat.id, status_msg.id, &url).await;
            }
            Err(e) => {
                safe_edit_message(
                    bot,
                    msg.chat.id,
                    status_msg.id,
                    &format!("Error: {e}"),
                    None,
                    None,
                )
                .await;
            }
        }
    } else {
        bot.send_message(
            msg.chat.id,
            "Please provide text, reply to a message containing text/file, or upload a file to paste.",
        )
        .await?;
    }

    Ok(())
}
