use crate::state::{AppState, MirrorTask};
use crate::utils::{escape_html, fmt_bytes, fmt_eta, fmt_speed, safe_edit_message, zip_path};
use regex::Regex;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use teloxide::prelude::*;
use teloxide::types::{
    CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message, ParseMode,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
use url::Url;

const MAX_WORKERS: usize = 4;

pub fn extract_gdrive_info(url: &str) -> (Option<String>, bool) {
    let folder_re1 = Regex::new(r"/folders/([a-zA-Z0-9_-]+)").unwrap();
    if let Some(caps) = folder_re1.captures(url) {
        return (caps.get(1).map(|m| m.as_str().to_string()), true);
    }

    let folder_re2 = Regex::new(r"id=([a-zA-Z0-9_-]+)").unwrap();
    if url.contains("drive.google.com/drive/folders/") {
        if let Some(caps) = folder_re2.captures(url) {
            return (caps.get(1).map(|m| m.as_str().to_string()), true);
        }
    }

    let file_re1 = Regex::new(r"/file/d/([a-zA-Z0-9_-]+)").unwrap();
    if let Some(caps) = file_re1.captures(url) {
        return (caps.get(1).map(|m| m.as_str().to_string()), false);
    }

    let file_re2 = Regex::new(r"[?&]id=([a-zA-Z0-9_-]+)").unwrap();
    if let Some(caps) = file_re2.captures(url) {
        return (caps.get(1).map(|m| m.as_str().to_string()), false);
    }

    (None, false)
}

pub async fn resolve_download_filename(client: &reqwest::Client, url_str: &str) -> String {
    let cd_re1 = Regex::new(r#"(?i)filename\*=(?:UTF-8''|)([^;]+)"#).unwrap();
    let cd_re2 = Regex::new(r#"(?i)filename=([^;]+)"#).unwrap();

    let parse_cd = |cd: &str| -> Option<String> {
        if let Some(caps) = cd_re1.captures(cd) {
            let raw = caps.get(1).map(|m| m.as_str().trim().trim_matches('"'))?;
            if let Ok(decoded) = urlencoding::decode(raw) {
                return Some(decoded.into_owned());
            }
        }
        if let Some(caps) = cd_re2.captures(cd) {
            let raw = caps.get(1).map(|m| m.as_str().trim().trim_matches('"'))?;
            return Some(raw.to_string());
        }
        None
    };

    if let Ok(resp) = client.head(url_str).send().await {
        if let Some(cd) = resp.headers().get("content-disposition").and_then(|v| v.to_str().ok()) {
            if let Some(name) = parse_cd(cd) {
                return name;
            }
        }
        let final_url = resp.url().as_str();
        if let Ok(parsed) = Url::parse(final_url) {
            if let Some(seg) = parsed.path_segments().and_then(|mut s| s.next_back()) {
                if !seg.is_empty() {
                    if let Ok(dec) = urlencoding::decode(seg) {
                        return dec.into_owned();
                    }
                }
            }
        }
    }

    if let Ok(parsed) = Url::parse(url_str) {
        if let Some(seg) = parsed.path_segments().and_then(|mut s| s.next_back()) {
            if !seg.is_empty() {
                if let Ok(dec) = urlencoding::decode(seg) {
                    return dec.into_owned();
                }
            }
        }
    }

    format!("download_{}", chrono::Utc::now().timestamp())
}

pub async fn mirror_cmd(
    bot: &Bot,
    msg: &Message,
    args: &[String],
    state: &AppState,
) -> ResponseResult<()> {
    let target_msg = msg.reply_to_message().unwrap_or(msg);

    let mut tg_file_id: Option<String> = None;
    let mut filename = "Unknown".to_string();
    let mut file_size: u64 = 0;

    if let Some(doc) = target_msg.document() {
        tg_file_id = Some(doc.file.id.to_string());
        filename = doc.file_name.clone().unwrap_or_else(|| "Unknown".to_string());
        file_size = doc.file.size as u64;
    } else if let Some(video) = target_msg.video() {
        tg_file_id = Some(video.file.id.to_string());
        filename = video.file_name.clone().unwrap_or_else(|| {
            format!("video_{}.mp4", &video.file.id.0[..6.min(video.file.id.0.len())])
        });
        file_size = video.file.size as u64;
    } else if let Some(audio) = target_msg.audio() {
        tg_file_id = Some(audio.file.id.to_string());
        filename = audio.file_name.clone().unwrap_or_else(|| {
            format!("audio_{}.mp3", &audio.file.id.0[..6.min(audio.file.id.0.len())])
        });
        file_size = audio.file.size as u64;
    } else if let Some(photos) = target_msg.photo() {
        if let Some(photo) = photos.last() {
            tg_file_id = Some(photo.file.id.to_string());
            filename = format!("photo_{}.jpg", &photo.file.id.0[..6.min(photo.file.id.0.len())]);
            file_size = photo.file.size as u64;
        }
    }

    let caption = msg.caption().unwrap_or_default();
    let do_zip = args.iter().any(|a| a == "-z") || caption.contains("-z");

    let mut url: Option<String> = None;
    if tg_file_id.is_none() {
        let url_args: Vec<&String> = args.iter().filter(|a| *a != "-z").collect();
        if url_args.is_empty() {
            bot.send_message(
                msg.chat.id,
                "Usage: `/mirror <URL> [-z]`\nOr reply to a file/document/video/photo with `/mirror [-z]`",
            )
            .parse_mode(ParseMode::MarkdownV2)
            .await?;
            return Ok(());
        }
        url = Some(url_args[0].clone());
    }

    let task_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let active = state.active_count.load(Ordering::SeqCst);
    let slots_free = MAX_WORKERS.saturating_sub(active);

    let zip_note = if do_zip { "  |  Zip: ON" } else { "" };
    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "Cancel",
        format!("cancel_{task_id}"),
    )]]);

    let initial_text = if slots_free > 0 {
        format!("Starting mirror...{zip_note}")
    } else {
        format!(
            "Added to queue\nActive: {}/{MAX_WORKERS}{zip_note}",
            active
        )
    };

    let status_msg = bot
        .send_message(msg.chat.id, initial_text)
        .reply_markup(keyboard)
        .await?;

    let user_name = msg
        .from
        .as_ref()
        .map(|u| u.first_name.clone())
        .unwrap_or_else(|| "User".to_string());
    let user_id = msg.from.as_ref().map(|u| u.id.0).unwrap_or(0);

    let task = MirrorTask {
        task_id: task_id.clone(),
        url,
        tg_file_id,
        filename,
        file_size,
        do_zip,
        user_name,
        user_id,
        chat_id: msg.chat.id,
        status_msg_id: status_msg.id,
        cancelled: Arc::new(AtomicBool::new(false)),
        start_time: Instant::now(),
        pid: Arc::new(Mutex::new(None)),
        action_text: "Mirroring".to_string(),
        file_label: "Filename".to_string(),
    };

    {
        let mut tasks_lock = state.tasks.write().await;
        tasks_lock.insert(task_id.clone(), task);
    }

    let _ = state.queue_tx.send(task_id).await;
    Ok(())
}

pub async fn cancel_callback(
    bot: &Bot,
    q: &CallbackQuery,
    state: &AppState,
) -> ResponseResult<()> {
    let data_str = q.data.as_deref().unwrap_or_default();
    let task_id = data_str.strip_prefix("cancel_").unwrap_or_default();

    let task_opt = {
        let tasks_lock = state.tasks.read().await;
        tasks_lock.get(task_id).cloned()
    };

    if let Some(task) = task_opt {
        if q.from.id.0 != task.user_id {
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("You didn't start this task!")
                .show_alert(true)
                .await;
            return Ok(());
        }

        let _ = bot.answer_callback_query(q.id.clone()).text("Task cancelled!").await;
        task.cancelled.store(true, Ordering::SeqCst);

        if let Some(pid) = *task.pid.lock().await {
            let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).output().await;
        }

        safe_edit_message(
            bot,
            task.chat_id,
            task.status_msg_id,
            "Task Cancelled by user!",
            None,
            None,
        )
        .await;
    } else {
        let _ = bot
            .answer_callback_query(q.id.clone())
            .text("Task not found or already finished.")
            .await;
    }

    Ok(())
}

async fn generate_link_and_finish(
    bot: &Bot,
    chat_id: teloxide::types::ChatId,
    status_msg_id: teloxide::types::MessageId,
    filename: &str,
    start_time: Instant,
) {
    safe_edit_message(
        bot,
        chat_id,
        status_msg_id,
        "Status: Generating public links...",
        None,
        None,
    )
    .await;

    let output = Command::new("rclone")
        .args(["link", &format!("drive:tgbot/{filename}")])
        .output()
        .await;

    let rclone_url = match output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(_) => String::new(),
    };

    let elapsed = start_time.elapsed().as_secs();
    let time_taken_str = format!("\nTime Taken: {}", fmt_eta(elapsed));

    if !rclone_url.is_empty() && rclone_url.contains("drive.google.com") {
        let mut dl_url = rclone_url.clone();
        if rclone_url.contains("/file/d/") {
            if let Some(part) = rclone_url.split("/d/").nth(1) {
                if let Some(fid) = part.split('/').next() {
                    dl_url = format!("https://drive.google.com/uc?export=download&id={fid}");
                }
            }
        }

        let view_url_parsed = Url::parse(&rclone_url).unwrap_or_else(|_| Url::parse("https://drive.google.com").unwrap());
        let dl_url_parsed = Url::parse(&dl_url).unwrap_or_else(|_| Url::parse("https://drive.google.com").unwrap());

        let keyboard = InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::url("👁 View File", view_url_parsed),
            InlineKeyboardButton::url("⬇️ Download File", dl_url_parsed),
        ]]);

        let text = format!(
            "Mirror Complete!\n\nFile: <code>{}</code>{time_taken_str}",
            escape_html(filename)
        );
        safe_edit_message(bot, chat_id, status_msg_id, &text, Some(keyboard), Some(ParseMode::Html)).await;
    } else if !rclone_url.is_empty() {
        if let Ok(link_parsed) = Url::parse(&rclone_url) {
            let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::url(
                "Open Link",
                link_parsed,
            )]]);
            let text = format!(
                "Mirror Complete!\n\nFile: <code>{}</code>{time_taken_str}",
                escape_html(filename)
            );
            safe_edit_message(bot, chat_id, status_msg_id, &text, Some(keyboard), Some(ParseMode::Html)).await;
        } else {
            let text = format!(
                "Mirror Complete!\n\nFile: <code>{}</code>{time_taken_str}\n\n<i>Note: Could not generate public link.</i>",
                escape_html(filename)
            );
            safe_edit_message(bot, chat_id, status_msg_id, &text, None, Some(ParseMode::Html)).await;
        }
    } else {
        let text = format!(
            "Mirror Complete!\n\nFile: <code>{}</code>{time_taken_str}\n\n<i>Note: Could not generate public link.</i>",
            escape_html(filename)
        );
        safe_edit_message(bot, chat_id, status_msg_id, &text, None, Some(ParseMode::Html)).await;
    }
}

async fn run_and_parse_rclone(
    cmd_args: &[&str],
    task: &MirrorTask,
    bot: &Bot,
) -> Result<i32, String> {
    let mut args = cmd_args.to_vec();
    let gdrive_opts = [
        "--drive-chunk-size",
        "64M",
        "--tpslimit",
        "10",
        "--tpslimit-burst",
        "10",
        "--drive-pacer-min-sleep",
        "100ms",
        "--drive-acknowledge-abuse",
        "--low-level-retries",
        "10",
        "--retries",
        "5",
        "--use-json-log",
        "--log-level",
        "INFO",
    ];
    args.extend_from_slice(&gdrive_opts);

    let mut child = Command::new("rclone")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;

    if let Some(pid) = child.id() {
        let mut p = task.pid.lock().await;
        *p = Some(pid);
    }

    let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
    let mut reader = BufReader::new(stdout).lines();

    let mut last_update = Instant::now();
    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "Cancel",
        format!("cancel_{}", task.task_id),
    )]]);

    while let Ok(Some(line)) = reader.next_line().await {
        if task.cancelled.load(Ordering::SeqCst) {
            let _ = child.kill().await;
            break;
        }

        if let Ok(entry) = serde_json::from_str::<Value>(&line) {
            if entry.get("level").and_then(|v| v.as_str()) != Some("info") {
                continue;
            }
            if let Some(stats) = entry.get("stats") {
                if last_update.elapsed().as_secs() >= 4 {
                    last_update = Instant::now();

                    let bytes_done = stats.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                    let total_bytes = stats.get("totalBytes").and_then(|v| v.as_u64()).unwrap_or(0);
                    let speed = stats.get("speed").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let upload_speed = stats.get("uploadSpeed").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let eta_secs = stats.get("eta").and_then(|v| v.as_u64()).unwrap_or(0);

                    let pct = if total_bytes > 0 {
                        ((bytes_done as f64 / total_bytes as f64) * 100.0) as u64
                    } else {
                        0
                    };
                    let filled = (pct / 10).min(10) as usize;
                    let bar = format!("{}{}", "⬢".repeat(filled), "○".repeat(10 - filled));

                    let mut lines = vec![
                        format!("<b>{}:</b> <code>{}</code>", escape_html(&task.file_label), escape_html(&task.filename)),
                        String::new(),
                        format!("Task By {} ( #ID{} )", escape_html(&task.user_name), task.user_id),
                        format!("┣ [{bar}] {pct}%"),
                        format!("┣ Processed → {} of {}", fmt_bytes(bytes_done), fmt_bytes(total_bytes)),
                        format!("┣ Status → {}", escape_html(&task.action_text)),
                    ];

                    if upload_speed > 0.0 {
                        lines.push(format!("┣ Download → {}", fmt_speed(speed)));
                        lines.push(format!("┣ Upload → {}", fmt_speed(upload_speed)));
                    } else {
                        lines.push(format!("┣ Speed → {}", fmt_speed(speed)));
                    }
                    lines.push(format!("┗ ETA → {}", fmt_eta(eta_secs)));

                    let text = lines.join("\n");
                    safe_edit_message(
                        bot,
                        task.chat_id,
                        task.status_msg_id,
                        &text,
                        Some(keyboard.clone()),
                        Some(ParseMode::Html),
                    )
                    .await;
                }
            }
        }
    }

    let status = child.wait().await.map_err(|e| e.to_string())?;
    {
        let mut p = task.pid.lock().await;
        *p = None;
    }
    Ok(status.code().unwrap_or(-1))
}

async fn run_aria2c_download(
    url_str: &str,
    dl_dir: &Path,
    filename: &str,
    task: &MirrorTask,
    bot: &Bot,
) -> Result<i32, String> {
    let mut child = Command::new("aria2c")
        .args([
            url_str,
            "--dir",
            dl_dir.to_str().unwrap_or("."),
            "--out",
            filename,
            "--file-allocation=none",
            "--console-log-level=notice",
            "--summary-interval=1",
            "--max-connection-per-server=8",
            "--split=8",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;

    if let Some(pid) = child.id() {
        let mut p = task.pid.lock().await;
        *p = Some(pid);
    }

    let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
    let mut reader = BufReader::new(stdout).lines();

    let mut last_update = Instant::now();
    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "Cancel",
        format!("cancel_{}", task.task_id),
    )]]);

    let aria_re = Regex::new(
        r"\[#\w+\s+([\d.]+\w+)/([\d.]+\w+)\((\d+)%\)(?:[^\]]*DL:([\d.]+\w+))?(?:[^\]]*ETA:(\S+))?\]",
    )
    .unwrap();

    while let Ok(Some(line)) = reader.next_line().await {
        if task.cancelled.load(Ordering::SeqCst) {
            let _ = child.kill().await;
            break;
        }

        if let Some(caps) = aria_re.captures(&line) {
            if last_update.elapsed().as_secs() >= 4 {
                last_update = Instant::now();

                let done_s = caps.get(1).map(|m| m.as_str()).unwrap_or("0");
                let total_s = caps.get(2).map(|m| m.as_str()).unwrap_or("0");
                let pct: u64 = caps.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
                let spd_s = caps.get(4).map(|m| m.as_str()).unwrap_or("N/A");
                let eta_s = caps.get(5).map(|m| m.as_str()).unwrap_or("N/A");

                let filled = (pct / 10).min(10) as usize;
                let bar = format!("{}{}", "⯂".repeat(filled), "○".repeat(10 - filled));

                let lines = vec![
                    format!("<b>{}:</b> <code>{}</code>", escape_html(&task.file_label), escape_html(filename)),
                    String::new(),
                    format!("Task By {} ( #ID{} )", escape_html(&task.user_name), task.user_id),
                    format!("┊ [{bar}] {pct}%"),
                    format!("┊ Processed → {done_s} of {total_s}"),
                    "┊ Status → Downloading".to_string(),
                    format!("┊ Speed → {spd_s}/s"),
                    format!("┗ ETA → {eta_s}"),
                ];

                safe_edit_message(
                    bot,
                    task.chat_id,
                    task.status_msg_id,
                    &lines.join("\n"),
                    Some(keyboard.clone()),
                    Some(ParseMode::Html),
                )
                .await;
            }
        }
    }

    let status = child.wait().await.map_err(|e| e.to_string())?;
    {
        let mut p = task.pid.lock().await;
        *p = None;
    }
    Ok(status.code().unwrap_or(-1))
}

pub async fn process_mirror(bot: &Bot, state: &AppState, task_id: &str) -> Result<(), String> {
    let mut task = {
        let tasks_lock = state.tasks.read().await;
        tasks_lock.get(task_id).cloned().ok_or("Task not found")?
    };

    if task.cancelled.load(Ordering::SeqCst) {
        return Ok(());
    }

    if let Some(ref tg_file_id) = task.tg_file_id {
        task.action_text = "Downloading".to_string();
        task.file_label = "Filename".to_string();

        safe_edit_message(
            bot,
            task.chat_id,
            task.status_msg_id,
            "Status: Fetching file details from Telegram...",
            None,
            None,
        )
        .await;

        let dl_dir = PathBuf::from("downloads").join(&task.task_id);
        tokio::fs::create_dir_all(&dl_dir).await.map_err(|e| e.to_string())?;
        let local_path = dl_dir.join(&task.filename);

        let dl_res = crate::handlers::paste::download_telegram_file(
            bot,
            state,
            tg_file_id,
            &task.filename,
        )
        .await;

        match dl_res {
            Ok(src_path) => {
                if src_path != local_path {
                    let _ = tokio::fs::rename(&src_path, &local_path).await;
                }
            }
            Err(e) => {
                let _ = tokio::fs::remove_dir_all(&dl_dir).await;
                return Err(format!("Telegram file transfer failed: {e}"));
            }
        }

        if task.cancelled.load(Ordering::SeqCst) {
            let _ = tokio::fs::remove_dir_all(&dl_dir).await;
            return Ok(());
        }

        let upload_src = if task.do_zip {
            let zip_name = if task.filename.ends_with(".zip") {
                task.filename.clone()
            } else {
                format!("{}.zip", task.filename)
            };
            safe_edit_message(bot, task.chat_id, task.status_msg_id, "Status: Zipping...", None, None).await;
            let zipped = zip_path(&local_path, &zip_name).map_err(|e| e.to_string())?;
            task.filename = zip_name;
            zipped
        } else {
            local_path.clone()
        };

        task.action_text = "Uploading to Drive".to_string();
        let cmd = ["copy", upload_src.to_str().unwrap_or("."), "drive:tgbot/", "--stats", "1s"];
        let ret = run_and_parse_rclone(&cmd, &task, bot).await?;
        let _ = tokio::fs::remove_dir_all(&dl_dir).await;

        if task.cancelled.load(Ordering::SeqCst) {
            return Ok(());
        }

        if ret == 0 {
            generate_link_and_finish(bot, task.chat_id, task.status_msg_id, &task.filename, task.start_time).await;
        } else {
            safe_edit_message(bot, task.chat_id, task.status_msg_id, "Error: Failed to upload file to Drive.", None, None).await;
        }
        return Ok(());
    }

    let url = task.url.clone().unwrap_or_default();

    if url.to_lowercase().starts_with("magnet:") {
        let dl_dir = PathBuf::from("downloads").join(&task.task_id);
        tokio::fs::create_dir_all(&dl_dir).await.map_err(|e| e.to_string())?;

        task.file_label = "Torrent".to_string();
        task.action_text = "Downloading".to_string();
        task.filename = "Resolving...".to_string();

        let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
            "Cancel",
            format!("cancel_{}", task.task_id),
        )]]);
        safe_edit_message(bot, task.chat_id, task.status_msg_id, "Starting aria2c...", Some(keyboard), None).await;

        let ret = run_aria2c_download(&url, &dl_dir, "torrent_data", &task, bot).await?;
        if task.cancelled.load(Ordering::SeqCst) || ret != 0 {
            let _ = tokio::fs::remove_dir_all(&dl_dir).await;
            if !task.cancelled.load(Ordering::SeqCst) {
                safe_edit_message(bot, task.chat_id, task.status_msg_id, "Error: aria2c download failed.", None, None).await;
            }
            return Ok(());
        }

        let mut final_name = task.task_id.clone();
        if let Ok(mut entries) = tokio::fs::read_dir(&dl_dir).await {
            if let Ok(Some(entry)) = entries.next_entry().await {
                final_name = entry.file_name().to_string_lossy().to_string();
            }
        }

        let upload_src = if task.do_zip {
            let zip_name = if final_name.ends_with(".zip") {
                final_name.clone()
            } else {
                format!("{final_name}.zip")
            };
            safe_edit_message(bot, task.chat_id, task.status_msg_id, "Status: Zipping...", None, None).await;
            let zipped = zip_path(&dl_dir, &zip_name).map_err(|e| e.to_string())?;
            final_name = zip_name;
            zipped
        } else {
            dl_dir.clone()
        };

        task.filename = final_name.clone();
        task.action_text = "Uploading to Drive".to_string();
        let cmd = ["copy", upload_src.to_str().unwrap_or("."), "drive:tgbot/", "--stats", "1s"];
        let ret = run_and_parse_rclone(&cmd, &task, bot).await?;
        let _ = tokio::fs::remove_dir_all(&dl_dir).await;

        if task.cancelled.load(Ordering::SeqCst) {
            return Ok(());
        }
        if ret == 0 {
            generate_link_and_finish(bot, task.chat_id, task.status_msg_id, &final_name, task.start_time).await;
        } else {
            safe_edit_message(bot, task.chat_id, task.status_msg_id, "Error: Failed to upload to Drive.", None, None).await;
        }
        return Ok(());
    }

    let (file_id_opt, is_folder) = extract_gdrive_info(&url);

    if let Some(file_id) = file_id_opt {
        if is_folder {
            let dl_dir = PathBuf::from("downloads").join(&task.task_id);
            tokio::fs::create_dir_all(&dl_dir).await.map_err(|e| e.to_string())?;

            let folder_name = format!("Folder_{file_id}");
            let local_folder = dl_dir.join(&folder_name);
            tokio::fs::create_dir_all(&local_folder).await.map_err(|e| e.to_string())?;

            task.filename = folder_name.clone();
            task.action_text = "Downloading Folder".to_string();
            task.file_label = "Foldername".to_string();

            let cmd = [
                "copy",
                "drive:",
                local_folder.to_str().unwrap_or("."),
                "--drive-root-folder-id",
                &file_id,
                "--drive-acknowledge-abuse",
                "--stats",
                "1s",
            ];
            let ret = run_and_parse_rclone(&cmd, &task, bot).await?;

            if task.cancelled.load(Ordering::SeqCst) {
                let _ = tokio::fs::remove_dir_all(&dl_dir).await;
                return Ok(());
            }

            if ret != 0 {
                let _ = tokio::fs::remove_dir_all(&dl_dir).await;
                safe_edit_message(bot, task.chat_id, task.status_msg_id, "Error: Failed to download Google Drive folder.", None, None).await;
                return Ok(());
            }

            let zip_name = format!("{folder_name}.zip");
            safe_edit_message(bot, task.chat_id, task.status_msg_id, "Status: Zipping...", None, None).await;
            let zipped = zip_path(&local_folder, &zip_name).map_err(|e| e.to_string())?;

            task.filename = zip_name.clone();
            task.action_text = "Uploading to Drive".to_string();
            let up_cmd = ["copy", zipped.to_str().unwrap_or("."), "drive:tgbot/", "--stats", "1s"];
            let up_ret = run_and_parse_rclone(&up_cmd, &task, bot).await?;
            let _ = tokio::fs::remove_dir_all(&dl_dir).await;

            if task.cancelled.load(Ordering::SeqCst) {
                return Ok(());
            }
            if up_ret == 0 {
                generate_link_and_finish(bot, task.chat_id, task.status_msg_id, &zip_name, task.start_time).await;
            } else {
                safe_edit_message(bot, task.chat_id, task.status_msg_id, "Error: Failed to upload zipped folder to Drive.", None, None).await;
            }
            return Ok(());
        } else {
            task.action_text = "Copying to Drive".to_string();
            task.file_label = "Filename".to_string();
            task.filename = "...".to_string();

            safe_edit_message(bot, task.chat_id, task.status_msg_id, "Copying file to Drive...", None, None).await;

            let tmp_remote = format!("drive:tgbot/tmp_{}/", task.task_id);
            let cp_out = Command::new("rclone")
                .args(["backend", "copyid", "drive:", &file_id, &tmp_remote])
                .output()
                .await;

            let success = cp_out.map(|o| o.status.success()).unwrap_or(false);
            if !success || task.cancelled.load(Ordering::SeqCst) {
                let _ = Command::new("rclone").args(["purge", &tmp_remote]).output().await;
                if !task.cancelled.load(Ordering::SeqCst) {
                    safe_edit_message(bot, task.chat_id, task.status_msg_id, "Error: Failed to copy Google Drive file.", None, None).await;
                }
                return Ok(());
            }

            let ls_out = Command::new("rclone")
                .args(["lsjson", &tmp_remote])
                .output()
                .await;

            let mut real_name: Option<String> = None;
            if let Ok(out) = ls_out {
                if let Ok(items) = serde_json::from_slice::<Vec<Value>>(&out.stdout) {
                    if let Some(first) = items.first() {
                        real_name = first.get("Name").and_then(|v| v.as_str()).map(|s| s.to_string());
                    }
                }
            }

            let file_name = real_name.unwrap_or_else(|| format!("file_{file_id}"));
            task.filename = file_name.clone();

            if task.do_zip {
                let dl_dir = PathBuf::from("downloads").join(&task.task_id);
                tokio::fs::create_dir_all(&dl_dir).await.map_err(|e| e.to_string())?;

                task.action_text = "Downloading for zip".to_string();
                let dl_cmd = ["copy", &tmp_remote, dl_dir.to_str().unwrap_or("."), "--stats", "1s"];
                let dl_ret = run_and_parse_rclone(&dl_cmd, &task, bot).await?;
                let _ = Command::new("rclone").args(["purge", &tmp_remote]).output().await;

                if task.cancelled.load(Ordering::SeqCst) || dl_ret != 0 {
                    let _ = tokio::fs::remove_dir_all(&dl_dir).await;
                    if !task.cancelled.load(Ordering::SeqCst) {
                        safe_edit_message(bot, task.chat_id, task.status_msg_id, "Error: Failed to download for zipping.", None, None).await;
                    }
                    return Ok(());
                }

                let local_path = dl_dir.join(&file_name);
                let zip_name = if file_name.ends_with(".zip") {
                    file_name.clone()
                } else {
                    format!("{file_name}.zip")
                };
                safe_edit_message(bot, task.chat_id, task.status_msg_id, "Status: Zipping...", None, None).await;
                let zipped = zip_path(&local_path, &zip_name).map_err(|e| e.to_string())?;

                task.filename = zip_name.clone();
                task.action_text = "Uploading to Drive".to_string();
                let up_cmd = ["copy", zipped.to_str().unwrap_or("."), "drive:tgbot/", "--stats", "1s"];
                let up_ret = run_and_parse_rclone(&up_cmd, &task, bot).await?;
                let _ = tokio::fs::remove_dir_all(&dl_dir).await;

                if task.cancelled.load(Ordering::SeqCst) {
                    return Ok(());
                }
                if up_ret == 0 {
                    generate_link_and_finish(bot, task.chat_id, task.status_msg_id, &zip_name, task.start_time).await;
                } else {
                    safe_edit_message(bot, task.chat_id, task.status_msg_id, "Error: Failed to upload to Drive.", None, None).await;
                }
            } else {
                let src_path = format!("{tmp_remote}{file_name}");
                let dst_path = format!("drive:tgbot/{file_name}");
                let mv_out = Command::new("rclone")
                    .args(["moveto", &src_path, &dst_path, "--drive-acknowledge-abuse"])
                    .output()
                    .await;

                let _ = Command::new("rclone").args(["purge", &tmp_remote]).output().await;
                if task.cancelled.load(Ordering::SeqCst) {
                    return Ok(());
                }

                if mv_out.map(|o| o.status.success()).unwrap_or(false) {
                    generate_link_and_finish(bot, task.chat_id, task.status_msg_id, &file_name, task.start_time).await;
                } else {
                    safe_edit_message(bot, task.chat_id, task.status_msg_id, "Error: Failed to move file to final location.", None, None).await;
                }
            }
            return Ok(());
        }
    }

    let resolved_name = resolve_download_filename(&state.http_client, &url).await;
    let dl_dir = PathBuf::from("downloads").join(&task.task_id);
    tokio::fs::create_dir_all(&dl_dir).await.map_err(|e| e.to_string())?;

    task.filename = resolved_name.clone();
    task.action_text = "Downloading".to_string();
    task.file_label = "Filename".to_string();

    let ret = run_aria2c_download(&url, &dl_dir, &resolved_name, &task, bot).await?;
    if task.cancelled.load(Ordering::SeqCst) || ret != 0 {
        let _ = tokio::fs::remove_dir_all(&dl_dir).await;
        if !task.cancelled.load(Ordering::SeqCst) {
            safe_edit_message(bot, task.chat_id, task.status_msg_id, "Error: Download failed.", None, None).await;
        }
        return Ok(());
    }

    let local_path = dl_dir.join(&resolved_name);
    let (upload_src, upload_name) = if task.do_zip {
        let zip_name = if resolved_name.ends_with(".zip") {
            resolved_name.clone()
        } else {
            format!("{resolved_name}.zip")
        };
        safe_edit_message(bot, task.chat_id, task.status_msg_id, "Status: Zipping...", None, None).await;
        let zipped = zip_path(&local_path, &zip_name).map_err(|e| e.to_string())?;
        (zipped, zip_name)
    } else {
        (local_path, resolved_name.clone())
    };

    task.filename = upload_name.clone();
    task.action_text = "Uploading to Drive".to_string();
    let cmd = ["copy", upload_src.to_str().unwrap_or("."), "drive:tgbot/", "--stats", "1s"];
    let ret = run_and_parse_rclone(&cmd, &task, bot).await?;
    let _ = tokio::fs::remove_dir_all(&dl_dir).await;

    if task.cancelled.load(Ordering::SeqCst) {
        return Ok(());
    }
    if ret == 0 {
        generate_link_and_finish(bot, task.chat_id, task.status_msg_id, &upload_name, task.start_time).await;
    } else {
        safe_edit_message(bot, task.chat_id, task.status_msg_id, "Error: Failed to upload to Drive.", None, None).await;
    }

    Ok(())
}

pub fn start_mirror_workers(bot: Bot, state: AppState, rx: tokio::sync::mpsc::Receiver<String>) {
    let rx_arc = Arc::new(tokio::sync::Mutex::new(rx));
    for _ in 0..MAX_WORKERS {
        let b = bot.clone();
        let s = state.clone();
        let r = rx_arc.clone();
        tokio::spawn(async move {
            loop {
                let task_id = {
                    let mut lock = r.lock().await;
                    lock.recv().await
                };

                let task_id = match task_id {
                    Some(id) => id,
                    None => break,
                };

                let is_valid = {
                    let tasks = s.tasks.read().await;
                    tasks.get(&task_id).map(|t| !t.cancelled.load(Ordering::SeqCst)).unwrap_or(false)
                };

                if is_valid {
                    s.active_count.fetch_add(1, Ordering::SeqCst);
                    let res = process_mirror(&b, &s, &task_id).await;
                    s.active_count.fetch_sub(1, Ordering::SeqCst);

                    if let Err(e) = res {
                        let task_opt = {
                            let tasks = s.tasks.read().await;
                            tasks.get(&task_id).cloned()
                        };
                        if let Some(task) = task_opt {
                            safe_edit_message(
                                &b,
                                task.chat_id,
                                task.status_msg_id,
                                &format!("Error: <code>{}</code>", escape_html(&e)),
                                None,
                                Some(ParseMode::Html),
                            )
                            .await;
                        }
                    }
                }

                let mut tasks = s.tasks.write().await;
                tasks.remove(&task_id);
            }
        });
    }
}
