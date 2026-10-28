use crate::state::AppState;
use crate::utils::escape_html;
use serde_json::Value;
use crate::utils::InTopic;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, InputFile, Message, ParseMode};
use url::Url;

pub async fn nowplaying_cmd(bot: &Bot, msg: &Message, state: &AppState) -> ResponseResult<()> {
    let sender_username = msg
        .from
        .as_ref()
        .and_then(|u| u.username.as_deref())
        .unwrap_or_default();
    if sender_username != state.config.owner_username {
        bot.send_message(
            msg.chat.id,
            format!("This command is only for @{}.", state.config.owner_username),
        ).in_topic(msg)
        .await?;
        return Ok(());
    }

    if state.config.lastfm_api_key.is_empty() || state.config.lastfm_username.is_empty() {
        let help = "\
<b>Last.fm not configured.</b>

Set <code>LASTFM_API_KEY</code> and <code>LASTFM_USERNAME</code> in credentials.json or environment.
Get a free API key at https://www.last.fm/api/account/create";
        bot.send_message(msg.chat.id, help).in_topic(msg)
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }

    let url = format!(
        "https://ws.audioscrobbler.com/2.0/?method=user.getrecenttracks&user={}&api_key={}&format=json&limit=1",
        state.config.lastfm_username, state.config.lastfm_api_key
    );

    let resp = match state.http_client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Failed to reach Last.fm: {e}")).in_topic(msg)
                .await?;
            return Ok(());
        }
    };

    let data = match resp.json::<Value>().await {
        Ok(d) => d,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Failed to parse Last.fm response: {e}")).in_topic(msg)
                .await?;
            return Ok(());
        }
    };

    let tracks = data
        .get("recenttracks")
        .and_then(|r| r.get("track"))
        .and_then(|t| t.as_array());

    let track = match tracks.and_then(|t| t.first()) {
        Some(t) => t,
        None => {
            bot.send_message(msg.chat.id, "No recent tracks found on Last.fm.").in_topic(msg)
                .await?;
            return Ok(());
        }
    };

    let is_now = track
        .get("@attr")
        .and_then(|a| a.get("nowplaying"))
        .and_then(|v| v.as_str())
        .map(|v| v == "true")
        .unwrap_or(false);

    let title = track
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown");

    let artist = track
        .get("artist")
        .and_then(|a| a.get("#text"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    let album = track
        .get("album")
        .and_then(|a| a.get("#text"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    let mut thumb_url: Option<String> = None;
    if let Some(images) = track.get("image").and_then(|i| i.as_array()) {
        for img in images.iter().rev() {
            if let Some(text) = img.get("#text").and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    thumb_url = Some(text.to_string());
                    break;
                }
            }
        }
    }

    let status = if is_now { "Now Playing" } else { "Last Played" };
    let mut lines = vec![format!("<b>{status} — YT Music</b>\n")];
    lines.push(format!("<b>{}</b>", escape_html(title)));
    if !artist.is_empty() {
        lines.push(format!("Artist: {}", escape_html(artist)));
    }
    if !album.is_empty() {
        lines.push(format!("Album: {}", escape_html(album)));
    }
    let caption = lines.join("\n");

    let query = format!("{artist} {title}").trim().to_string();
    let query_encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let ytm_url = format!("https://music.youtube.com/search?q={query_encoded}");

    let parsed_url = Url::parse(&ytm_url).unwrap_or_else(|_| Url::parse("https://music.youtube.com").unwrap());
    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::url(
        "Listen on YT Music",
        parsed_url,
    )]]);

    if let Some(thumb) = &thumb_url {
        if let Ok(photo_url) = Url::parse(thumb) {
            let res = bot
                .send_photo(msg.chat.id, InputFile::url(photo_url)).in_topic(msg)
                .caption(&caption)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard.clone())
                .await;
            if res.is_ok() {
                return Ok(());
            }
        }
    }

    bot.send_message(msg.chat.id, caption).in_topic(msg)
        .parse_mode(ParseMode::Html)
        .reply_markup(keyboard)
        .await?;

    Ok(())
}
