use crate::state::AppState;
use crate::utils::escape_html;
use chrono::{Duration, Utc};
use serde_json::Value;
use crate::utils::InTopic;
use teloxide::prelude::*;
use teloxide::types::{
    CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, InputFile,
    LinkPreviewOptions, Message, ParseMode,
};

pub async fn source_tracker_cmd(bot: &Bot, msg: &Message) -> ResponseResult<()> {
    let keyboard = InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback("LineageOS", "track_LineageOS"),
        InlineKeyboardButton::callback("YAAP", "track_yaap"),
    ]]);

    bot.send_message(
        msg.chat.id,
        "Select the organization to track commits for the past week:",
    ).in_topic(msg)
    .reply_markup(keyboard)
    .await?;

    Ok(())
}

pub async fn source_tracker_callback(
    bot: &Bot,
    q: &CallbackQuery,
    state: &AppState,
) -> ResponseResult<()> {
    let _ = bot.answer_callback_query(q.id.clone()).await;

    let data_str = q.data.as_deref().unwrap_or_default();
    let parts: Vec<&str> = data_str.split('_').collect();
    if parts.len() < 2 {
        return Ok(());
    }
    let org = parts[1];

    let message = match &q.message {
        Some(m) => m,
        None => return Ok(()),
    };

    let chat_id = message.chat().id;
    let message_id = message.id();

    let _ = bot
        .edit_message_text(
            chat_id,
            message_id,
            format!("Fetching recent commits for {org}..."),
        )
        .await;

    let one_week_ago = (Utc::now() - Duration::days(7)).format("%Y-%m-%d").to_string();
    let base_url = format!("https://api.github.com/search/commits?q=org:{org}+committer-date:>{one_week_ago}");

    let mut commits: Vec<Value> = Vec::new();
    let mut page = 1;

    loop {
        let page_url = format!("{base_url}&per_page=100&page={page}");
        let req = state
            .http_client
            .get(&page_url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "tgbot");

        match req.send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    if let Ok(data) = resp.json::<Value>().await {
                        let items = data.get("items").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                        let items_len = items.len();
                        commits.extend(items);
                        if items_len < 100 || page >= 5 {
                            break;
                        }
                        page += 1;
                    } else {
                        break;
                    }
                } else {
                    if page == 1 {
                        let _ = bot
                            .edit_message_text(
                                chat_id,
                                message_id,
                                format!("Failed to fetch commits. GitHub API Status: {}", resp.status()),
                            )
                            .await;
                        return Ok(());
                    }
                    break;
                }
            }
            Err(e) => {
                let _ = bot
                    .edit_message_text(
                        chat_id,
                        message_id,
                        format!("Error fetching commits: {e}"),
                    )
                    .await;
                return Ok(());
            }
        }
    }

    if commits.is_empty() {
        let _ = bot
            .edit_message_text(
                chat_id,
                message_id,
                format!("No commits found in the past week for {org}."),
            )
            .await;
        return Ok(());
    }

    let mut html_msg = format!("Commits in the past week for <b>{}</b>:\n\n", escape_html(org));
    for item in &commits {
        let full_sha = item.get("sha").and_then(|v| v.as_str()).unwrap_or_default();
        let sha = if full_sha.len() >= 7 { &full_sha[..7] } else { full_sha };
        let commit_url = item.get("html_url").and_then(|v| v.as_str()).unwrap_or_default();
        let repo_name = item
            .get("repository")
            .and_then(|r| r.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or(org);
        let commit_msg = item
            .get("commit")
            .and_then(|c| c.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let first_line = commit_msg.lines().next().unwrap_or_default();

        html_msg.push_str(&format!(
            "- <a href=\"{commit_url}\">{sha}</a> ({}): {}\n",
            escape_html(repo_name),
            escape_html(first_line)
        ));
    }

    if html_msg.chars().count() > 4000 {
        let mut plain_msg = format!("Commits in the past week for {org}:\n\n");
        for item in &commits {
            let full_sha = item.get("sha").and_then(|v| v.as_str()).unwrap_or_default();
            let sha = if full_sha.len() >= 7 { &full_sha[..7] } else { full_sha };
            let commit_url = item.get("html_url").and_then(|v| v.as_str()).unwrap_or_default();
            let repo_name = item
                .get("repository")
                .and_then(|r| r.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or(org);
            let commit_msg = item
                .get("commit")
                .and_then(|c| c.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let first_line = commit_msg.lines().next().unwrap_or_default();

            plain_msg.push_str(&format!(
                "- {sha} ({repo_name}): {first_line}\n  Link: {commit_url}\n"
            ));
        }

        let temp_file_path = std::env::temp_dir().join(format!("{org}_recent_commits_{}.txt", uuid::Uuid::new_v4()));
        let _ = tokio::fs::write(&temp_file_path, &plain_msg).await;

        let _ = bot
            .edit_message_text(
                chat_id,
                message_id,
                "Too many commits to display! Sending as a file...",
            )
            .await;

        let input_file = InputFile::file(&temp_file_path).file_name(format!("{org}_recent_commits.txt"));
        let mut req = bot
            .send_document(chat_id, input_file)
            .caption(format!("Recent commits for {org}"));
        if let Some(m) = message.regular_message() {
            req = req.in_topic(m);
        }
        let _ = req.await;

        let _ = tokio::fs::remove_file(&temp_file_path).await;
    } else {
        let link_opts = LinkPreviewOptions {
            is_disabled: true,
            url: None,
            prefer_small_media: false,
            prefer_large_media: false,
            show_above_text: false,
        };
        let _ = bot
            .edit_message_text(chat_id, message_id, html_msg)
            .parse_mode(ParseMode::Html)
            .link_preview_options(link_opts)
            .await;
    }

    Ok(())
}
