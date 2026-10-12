pub mod help;
pub mod mirror;
pub mod neofetch;
pub mod nowplaying;
pub mod paste;
pub mod social;
pub mod status;
pub mod tdl;
pub mod tracker;

use crate::state::AppState;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, Message};

pub async fn handle_message(bot: Bot, msg: Message, state: AppState) -> ResponseResult<()> {
    let full_text = msg
        .text()
        .or_else(|| msg.caption())
        .unwrap_or_default()
        .trim();

    if full_text.starts_with('/') {
        let mut parts = full_text.split_whitespace();
        let cmd_with_slash = parts.next().unwrap_or_default();
        let cmd = cmd_with_slash
            .trim_start_matches('/')
            .split('@')
            .next()
            .unwrap_or_default();

        let args: Vec<String> = parts.map(|s| s.to_string()).collect();

        match cmd {
            "start" | "help" => {
                help::start_or_help(&bot, &msg).await?;
            }
            "status" => {
                status::status_cmd(&bot, &msg).await?;
            }
            "paste" => {
                paste::paste_cmd(&bot, &msg, &args, &state).await?;
            }
            "source_tracker" => {
                tracker::source_tracker_cmd(&bot, &msg).await?;
            }
            "mirror" => {
                mirror::mirror_cmd(&bot, &msg, &args, &state).await?;
            }
            "tdl" => {
                tdl::tdl_cmd(&bot, &msg, &args).await?;
            }
            "nowplaying" => {
                nowplaying::nowplaying_cmd(&bot, &msg, &state).await?;
            }
            "neofetch" => {
                neofetch::neofetch_cmd(&bot, &msg).await?;
            }
            _ => {}
        }
        return Ok(());
    }

    if social::detect_social_url(full_text).is_some() {
        social::social_media_handler(bot, msg, state).await?;
    }

    Ok(())
}

pub async fn handle_callback_query(
    bot: Bot,
    q: CallbackQuery,
    state: AppState,
) -> ResponseResult<()> {
    let data = q.data.as_deref().unwrap_or_default();
    if data.starts_with("track_") {
        tracker::source_tracker_callback(&bot, &q, &state).await?;
    } else if data.starts_with("cancel_") {
        mirror::cancel_callback(&bot, &q, &state).await?;
    }
    Ok(())
}
