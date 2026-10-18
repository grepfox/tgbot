use crate::state::AppState;
use crate::utils::{escape_html, fmt_eta};
use regex::Regex;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Instant;
use teloxide::prelude::*;
use teloxide::types::{
    InlineKeyboardButton, InlineKeyboardMarkup, InputFile, InputMedia, InputMediaPhoto,
    InputMediaVideo, LinkPreviewOptions, Message, ParseMode, ReplyParameters,
};
use tokio::process::Command;
use url::Url;

// Public instances without JWT/Turnstile auth.
const COBALT_APIS: &[&str] = &["https://cobaltapi.cjs.nz"];

// Embed-fixer hosts that redirect straight to the media file for bot user agents.
const INSTAGRAM_EMBED_HOSTS: &[&str] = &["kkinstagram.com"];

pub fn detect_social_url(text: &str) -> Option<String> {
    let re = Regex::new(
        r"(?i)https?://(?:www\.)?(?:(?:twitter\.com|x\.com)/(?:i/)?\S+/status(?:es)?/\d+|t\.co/\S+|(?:youtube\.com/(?:watch|shorts|live)\S*|youtu\.be/\S+)|(?:instagram\.com|instagr\.am)/(?:p|reel|tv)/[A-Za-z0-9_-]+/?\S*)",
    )
    .unwrap();
    re.find(text).map(|m| m.as_str().to_string())
}

pub fn platform_of(url: &str) -> &'static str {
    let u = url.to_lowercase();
    if u.contains("youtube.com") || u.contains("youtu.be") {
        "YouTube"
    } else if u.contains("instagram.com") || u.contains("instagr.am") {
        "Instagram"
    } else if u.contains("twitter.com") || u.contains("x.com") || u.contains("t.co") {
        "X"
    } else {
        "Social"
    }
}

pub fn build_caption(
    info: &Value,
    platform: &str,
    source_url: Option<&str>,
    max_len: usize,
) -> String {
    let title = info
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim();
    let description = info
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim();
    let uploader = info
        .get("uploader")
        .or_else(|| info.get("channel"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim();

    if platform == "X" {
        let mut tweet_text = if !description.is_empty() {
            description.to_string()
        } else {
            title.to_string()
        };

        let tco_re = Regex::new(r"\s*https?://t\.co/[a-zA-Z0-9]+$").unwrap();
        tweet_text = tco_re.replace(&tweet_text, "").to_string();

        let space_re = Regex::new(r" {2,}").unwrap();
        tweet_text = space_re.replace_all(&tweet_text, "\n").to_string();

        if tweet_text.chars().count() > max_len {
            let truncated: String = tweet_text.chars().take(max_len).collect();
            tweet_text = format!("{truncated}...");
        }

        let mut lines = vec!["<b>app: X</b>".to_string()];
        if let Some(src) = source_url {
            lines.push(format!("<b>source:</b> <a href=\"{src}\">link</a>"));
        }

        if let Some(qrt) = info.get("_qrt") {
            let qrt_text_raw = qrt.get("text").and_then(|v| v.as_str()).unwrap_or_default();
            let mut qrt_text = tco_re.replace(qrt_text_raw, "").to_string();
            qrt_text = space_re.replace_all(&qrt_text, "\n").to_string();
            if qrt_text.chars().count() > max_len {
                let truncated: String = qrt_text.chars().take(max_len).collect();
                qrt_text = format!("{truncated}...");
            }
            let qrt_user = qrt.get("user_name").and_then(|v| v.as_str()).unwrap_or_default();
            if !qrt_user.is_empty() {
                lines.push(format!("user: <b>{} (Quoted)</b>", escape_html(qrt_user)));
            }
            lines.push(String::new());
            lines.push(format!("<blockquote>{}</blockquote>", escape_html(&qrt_text)));
            lines.push(String::new());
        }

        if !uploader.is_empty() {
            lines.push(format!("user: <b>{}</b>", escape_html(uploader)));
        }
        lines.push(String::new());
        lines.push(format!("<blockquote>{}</blockquote>", escape_html(&tweet_text)));
        lines.join("\n")
    } else {
        let mut caption_text = if !title.is_empty() {
            title.to_string()
        } else {
            description.to_string()
        };

        if caption_text.chars().count() > max_len {
            let truncated: String = caption_text.chars().take(max_len).collect();
            caption_text = format!("{truncated}...");
        }

        let mut lines = vec![format!("<b>app: {platform}</b>")];
        if let Some(src) = source_url {
            lines.push(format!("<b>source:</b> <a href=\"{src}\">link</a>"));
        }
        if !uploader.is_empty() {
            lines.push(format!("user: <b>{}</b>", escape_html(uploader)));
        }
        if !caption_text.is_empty() {
            lines.push(String::new());
            lines.push(format!("<blockquote>{}</blockquote>", escape_html(&caption_text)));
        }
        lines.join("\n")
    }
}

async fn extract_instagram_metadata_googlebot(client: &reqwest::Client, url_str: &str) -> Value {
    let req = client
        .get(url_str)
        .header(
            "User-Agent",
            "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
        );

    let html = match req.send().await {
        Ok(resp) if resp.status().is_success() => resp.text().await.unwrap_or_default(),
        _ => return Value::Null,
    };

    let meta_re = Regex::new(r"(?i)<meta\s+[^>]*>").unwrap();
    let prop_re = Regex::new(r#"(?i)(?:property|name)\s*=\s*["']([^"']+)["']"#).unwrap();
    let content_re = Regex::new(r#"(?i)content\s*=\s*["']([^"']+)["']"#).unwrap();

    let mut description = String::new();
    let mut title = String::new();
    let mut twitter_title = String::new();

    for mat in meta_re.find_iter(&html) {
        let tag = mat.as_str();
        if let (Some(pcaps), Some(ccaps)) = (prop_re.captures(tag), content_re.captures(tag)) {
            let prop = pcaps.get(1).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
            let content = ccaps.get(1).map(|m| m.as_str().trim()).unwrap_or_default();

            if prop == "og:description" || prop == "description" || prop == "twitter:description" {
                if content.len() > description.len() {
                    description = content.to_string();
                }
            } else if prop == "og:title" {
                if content.len() > title.len() {
                    title = content.to_string();
                }
            } else if prop == "twitter:title" {
                twitter_title = content.to_string();
            }
        }
    }

    let mut uploader = String::new();
    let title_uploader_re = Regex::new(r"^(.+?)\s+on\s+Instagram").unwrap();
    if let Some(caps) = title_uploader_re.captures(&title) {
        uploader = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
    } else if !twitter_title.is_empty() {
        let tw_uploader_re = Regex::new(r"^(.+?)\s*\(").unwrap();
        if let Some(caps) = tw_uploader_re.captures(&twitter_title) {
            uploader = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        }
    }

    let desc_quote_re = Regex::new(r#":\s*"(.*)"\s*\.?\s*$"#).unwrap();
    let mut post_text = String::new();
    if let Some(caps) = desc_quote_re.captures(&description) {
        post_text = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
    } else {
        let title_quote_re = Regex::new(r#":\s*"(.*)"\s*$"#).unwrap();
        if let Some(caps) = title_quote_re.captures(&title) {
            post_text = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        }
    }

    if post_text.is_empty() {
        post_text = description;
    }

    serde_json::json!({
        "title": post_text,
        "description": post_text,
        "uploader": if !uploader.is_empty() { uploader } else { "Instagram User".to_string() }
    })
}

struct CobaltItem {
    is_video: bool,
    path: PathBuf,
}

/// Detects media kind from the file's magic bytes: Some(true) = video, Some(false) = image.
fn sniff_media(bytes: &[u8]) -> Option<bool> {
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        return Some(!matches!(brand, b"avif" | b"avis" | b"heic" | b"heix" | b"mif1"));
    }
    if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(true);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(b"\x89PNG")
        || (bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
    {
        return Some(false);
    }
    None
}

async fn fetch_media(
    client: &reqwest::Client,
    media_url: &str,
    out_dir: &Path,
    stem: &str,
    user_agent: Option<&str>,
) -> Option<CobaltItem> {
    let mut req = client.get(media_url);
    if let Some(ua) = user_agent {
        req = req.header("User-Agent", ua);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    let is_video = sniff_media(&bytes)?;
    let ext = if is_video { "mp4" } else { "jpg" };
    let path = out_dir.join(format!("{stem}.{ext}"));
    tokio::fs::write(&path, &bytes).await.ok()?;
    Some(CobaltItem { is_video, path })
}

fn is_instagram_video_url(url_str: &str) -> bool {
    let u = url_str.to_lowercase();
    u.contains("/reel/") || u.contains("/reels/") || u.contains("/tv/")
}

async fn download_instagram_media_cobalt(
    client: &reqwest::Client,
    url_str: &str,
    out_dir: &Path,
) -> Option<Vec<CobaltItem>> {
    let payload = serde_json::json!({ "url": url_str });

    for api_url in COBALT_APIS {
        let base_url = if api_url.ends_with('/') {
            api_url.to_string()
        } else {
            format!("{api_url}/")
        };

        let req = client
            .post(&base_url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&payload);

        let resp = match req.send().await {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };

        let data = match resp.json::<Value>().await {
            Ok(d) => d,
            Err(_) => continue,
        };

        let status = data.get("status").and_then(|v| v.as_str()).unwrap_or_default();
        let mut downloaded = Vec::new();

        if status == "picker" {
            if let Some(items) = data.get("picker").and_then(|v| v.as_array()) {
                for (i, item) in items.iter().enumerate() {
                    if let Some(media_url) = item.get("url").and_then(|v| v.as_str()) {
                        if let Some(it) =
                            fetch_media(client, media_url, out_dir, &format!("insta_{i}"), None).await
                        {
                            downloaded.push(it);
                        }
                    }
                }
            }
        } else if matches!(status, "redirect" | "stream" | "tunnel" | "success") {
            if let Some(media_url) = data.get("url").and_then(|v| v.as_str()) {
                if let Some(it) = fetch_media(client, media_url, out_dir, "insta_single", None).await {
                    downloaded.push(it);
                }
            }
        }

        // A reel that only yields images got its thumbnail, not the video.
        if !downloaded.is_empty()
            && (!is_instagram_video_url(url_str) || downloaded.iter().any(|it| it.is_video))
        {
            return Some(downloaded);
        }
    }

    download_instagram_media_embed(client, url_str, out_dir).await
}

async fn download_instagram_media_embed(
    client: &reqwest::Client,
    url_str: &str,
    out_dir: &Path,
) -> Option<Vec<CobaltItem>> {
    let mut parsed = Url::parse(url_str).ok()?;
    for host in INSTAGRAM_EMBED_HOSTS {
        parsed.set_host(Some(host)).ok()?;
        let item = match fetch_media(
            client,
            parsed.as_str(),
            out_dir,
            "insta_single",
            Some("TelegramBot (like TwitterBot)"),
        )
        .await
        {
            Some(it) => it,
            None => continue,
        };
        if !item.is_video && is_instagram_video_url(url_str) {
            continue;
        }
        return Some(vec![item]);
    }
    None
}

async fn download_x_media_vxtwitter(
    client: &reqwest::Client,
    tweet_id: &str,
    out_dir: &Path,
) -> Option<(Vec<CobaltItem>, Value)> {
    let api_url = format!("https://api.vxtwitter.com/status/{tweet_id}");
    let resp = client.get(&api_url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let data = resp.json::<Value>().await.ok()?;

    let mut media_extended = data
        .get("media_extended")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let qrt_data = data.get("qrt").cloned();
    if let Some(ref qrt) = qrt_data {
        if let Some(qrt_media) = qrt.get("media_extended").and_then(|v| v.as_array()) {
            media_extended.extend(qrt_media.clone());
        }
    }

    let mut info = serde_json::json!({
        "id": tweet_id,
        "title": format!("{} - {}", data.get("user_name").and_then(|v| v.as_str()).unwrap_or_default(), data.get("text").and_then(|v| v.as_str()).unwrap_or_default()),
        "description": data.get("text").and_then(|v| v.as_str()).unwrap_or_default(),
        "uploader": data.get("user_name").and_then(|v| v.as_str()).unwrap_or_default(),
        "uploader_id": data.get("user_screen_name").and_then(|v| v.as_str()).unwrap_or_default(),
        "_qrt": qrt_data,
        "_has_media": !media_extended.is_empty()
    });

    let mut video_url: Option<String> = None;
    let mut photo_urls: Vec<String> = Vec::new();

    for item in &media_extended {
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or_default();
        let item_url = item.get("url").and_then(|v| v.as_str()).unwrap_or_default();
        if item_type == "video" || item_type == "gif" {
            if video_url.is_none() && !item_url.is_empty() {
                video_url = Some(item_url.to_string());
                if let Some(size) = item.get("size") {
                    info["width"] = size.get("width").cloned().unwrap_or(Value::from(0));
                    info["height"] = size.get("height").cloned().unwrap_or(Value::from(0));
                }
            }
        } else if item_type == "image" || item_type == "photo" {
            if !item_url.is_empty() {
                photo_urls.push(item_url.to_string());
            }
        }
    }

    if let Some(v_url) = video_url {
        let out_path = out_dir.join(format!("{tweet_id}.mp4"));
        let dl_resp = client
            .get(&v_url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .send()
            .await
            .ok()?;
        if dl_resp.status().is_success() {
            let bytes = dl_resp.bytes().await.ok()?;
            tokio::fs::write(&out_path, &bytes).await.ok()?;
            return Some((
                vec![CobaltItem {
                    is_video: true,
                    path: out_path,
                }],
                info,
            ));
        }
    }

    if !photo_urls.is_empty() {
        let mut downloaded = Vec::new();
        for (i, p_url) in photo_urls.iter().enumerate() {
            let out_path = out_dir.join(format!("x_img_{i}.jpg"));
            let dl_resp = client
                .get(p_url)
                .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                .send()
                .await;
            if let Ok(resp) = dl_resp {
                if resp.status().is_success() {
                    if let Ok(bytes) = resp.bytes().await {
                        if tokio::fs::write(&out_path, &bytes).await.is_ok() {
                            downloaded.push(CobaltItem {
                                is_video: false,
                                path: out_path,
                            });
                        }
                    }
                }
            }
        }
        if !downloaded.is_empty() {
            return Some((downloaded, info));
        }
    }

    Some((Vec::new(), info))
}

async fn download_x_media_fxtwitter(
    client: &reqwest::Client,
    tweet_id: &str,
    out_dir: &Path,
) -> Option<(Vec<CobaltItem>, Value)> {
    let api_url = format!("https://api.fxtwitter.com/status/{tweet_id}");
    let resp = client.get(&api_url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let data = resp.json::<Value>().await.ok()?;
    let tweet = data.get("tweet").or_else(|| data.get("status"))?;

    let author = tweet.get("author");
    let author_name = author
        .and_then(|a| a.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let screen_name = author
        .and_then(|a| a.get("screen_name"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let text = tweet.get("text").and_then(|v| v.as_str()).unwrap_or_default();

    fn media_all(t: &Value) -> Vec<Value> {
        t.get("media")
            .and_then(|m| m.get("all"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    }

    let quote = tweet.get("quote").cloned();
    let mut all_media = media_all(tweet);
    if let Some(ref q) = quote {
        all_media.extend(media_all(q));
    }

    let qrt = quote.map(|q| {
        serde_json::json!({
            "text": q.get("text").and_then(|v| v.as_str()).unwrap_or_default(),
            "user_name": q
                .get("author")
                .and_then(|a| a.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
        })
    });

    let mut info = serde_json::json!({
        "id": tweet_id,
        "title": format!("{author_name} - {text}"),
        "description": text,
        "uploader": author_name,
        "uploader_id": screen_name,
        "_qrt": qrt,
        "_has_media": !all_media.is_empty(),
    });

    let mut video_url: Option<String> = None;
    let mut photo_urls: Vec<String> = Vec::new();

    for item in &all_media {
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or_default();
        let item_url = item.get("url").and_then(|v| v.as_str()).unwrap_or_default();
        if item_url.is_empty() {
            continue;
        }
        if item_type == "video" || item_type == "gif" {
            if video_url.is_none() {
                video_url = Some(item_url.to_string());
                if let Some(w) = item.get("width").and_then(|v| v.as_u64()) {
                    info["width"] = Value::from(w);
                }
                if let Some(h) = item.get("height").and_then(|v| v.as_u64()) {
                    info["height"] = Value::from(h);
                }
            }
        } else if item_type == "photo" || item_type == "image" {
            photo_urls.push(item_url.to_string());
        }
    }

    if let Some(v_url) = video_url {
        let out_path = out_dir.join(format!("{tweet_id}.mp4"));
        if let Ok(dl_resp) = client
            .get(&v_url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .send()
            .await
        {
            if dl_resp.status().is_success() {
                if let Ok(bytes) = dl_resp.bytes().await {
                    if tokio::fs::write(&out_path, &bytes).await.is_ok() {
                        return Some((
                            vec![CobaltItem {
                                is_video: true,
                                path: out_path,
                            }],
                            info,
                        ));
                    }
                }
            }
        }
    }

    if !photo_urls.is_empty() {
        let mut downloaded = Vec::new();
        for (i, p_url) in photo_urls.iter().enumerate() {
            let out_path = out_dir.join(format!("x_img_{i}.jpg"));
            if let Ok(resp) = client
                .get(p_url)
                .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                .send()
                .await
            {
                if resp.status().is_success() {
                    if let Ok(bytes) = resp.bytes().await {
                        if tokio::fs::write(&out_path, &bytes).await.is_ok() {
                            downloaded.push(CobaltItem {
                                is_video: false,
                                path: out_path,
                            });
                        }
                    }
                }
            }
        }
        if !downloaded.is_empty() {
            return Some((downloaded, info));
        }
    }

    Some((Vec::new(), info))
}

async fn download_x_media(
    client: &reqwest::Client,
    tweet_id: &str,
    out_dir: &Path,
) -> Option<(Vec<CobaltItem>, Value)> {
    if let Some(result) = download_x_media_fxtwitter(client, tweet_id, out_dir).await {
        return Some(result);
    }
    download_x_media_vxtwitter(client, tweet_id, out_dir).await
}

pub async fn social_media_handler(
    bot: Bot,
    msg: Message,
    state: AppState,
) -> ResponseResult<()> {
    let text = msg
        .text()
        .or_else(|| msg.caption())
        .unwrap_or_default();

    let url = match detect_social_url(text) {
        Some(u) => u,
        None => return Ok(()),
    };

    let t0 = Instant::now();
    let platform = platform_of(&url);

    let status_msg = bot
        .send_message(msg.chat.id, format!("Fetching from {platform}..."))
        .reply_parameters(ReplyParameters::new(msg.id))
        .await?;

    let temp_dir = std::env::temp_dir().join(format!("tgsocial_{}", uuid::Uuid::new_v4()));
    let _ = tokio::fs::create_dir_all(&temp_dir).await;

    let mut downloaded_items: Option<Vec<CobaltItem>> = None;
    let mut media_info: Value = serde_json::json!({});
    let mut download_error: Option<String> = None;

    if platform == "X" {
        let tweet_id_re = Regex::new(r"/status/(\d+)").unwrap();
        if let Some(caps) = tweet_id_re.captures(&url) {
            let tweet_id = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            if let Some((items, info)) =
                download_x_media(&state.http_client, tweet_id, &temp_dir).await
            {
                let has_media = info
                    .get("_has_media")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                media_info = info;
                if !items.is_empty() {
                    downloaded_items = Some(items);
                } else if !has_media {
                    // Text-only post: nothing to download, just send the caption.
                    downloaded_items = Some(Vec::new());
                }
            }
        }
    }

    if downloaded_items.is_none() && platform == "Instagram" {
        if let Some(items) =
            download_instagram_media_cobalt(&state.http_client, &url, &temp_dir).await
        {
            let scraped =
                extract_instagram_metadata_googlebot(&state.http_client, &url).await;
            media_info = scraped;
            downloaded_items = Some(items);
        }
    }

    if downloaded_items.is_none() {
        let _ = bot
            .edit_message_text(msg.chat.id, status_msg.id, format!("Downloading from {platform}..."))
            .await;

        let ytdl_cmd = if Path::new("/home/gnsh/web/yted/venv/bin/yt-dlp").exists() {
            "/home/gnsh/web/yted/venv/bin/yt-dlp"
        } else {
            "yt-dlp"
        };

        let outtmpl = temp_dir.join("%(id)s.%(ext)s");
        let mut ytdl_args = vec![
            "--quiet",
            "--no-warnings",
            "--no-playlist",
            "--merge-output-format",
            "mp4",
            "--format",
            "bestvideo[height<=1080]+bestaudio/bv*[height<=1080]+ba/b[height<=1080]/bestvideo+bestaudio/b/best",
            "-o",
            outtmpl.to_str().unwrap_or(""),
            "--print-json",
        ];
        if platform == "YouTube" {
            ytdl_args.extend(["--extractor-args", "youtube:player_client=mweb,android,web,tv"]);
        }
        let cookie_str = state
            .config
            .cookies_file
            .as_ref()
            .and_then(|p| p.to_str())
            .unwrap_or_default();
        if !cookie_str.is_empty() {
            ytdl_args.push("--cookies");
            ytdl_args.push(cookie_str);
        }
        ytdl_args.push(&url);

        let output = Command::new(ytdl_cmd)
            .args(&ytdl_args)
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                if let Ok(info) = serde_json::from_slice::<Value>(&out.stdout) {
                    media_info = info;
                }

                let mut items = Vec::new();
                if let Ok(mut entries) = tokio::fs::read_dir(&temp_dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let path = entry.path();
                        if path.is_file() {
                            let ext = path
                                .extension()
                                .and_then(|e| e.to_str())
                                .unwrap_or_default()
                                .to_lowercase();
                            let is_video = matches!(
                                ext.as_str(),
                                "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v"
                            );
                            items.push(CobaltItem { is_video, path });
                        }
                    }
                }
                if !items.is_empty() {
                    downloaded_items = Some(items);
                }
            }
            Ok(out) => {
                download_error = Some(
                    String::from_utf8_lossy(&out.stderr)
                        .lines()
                        .rev()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("yt-dlp failed")
                        .trim()
                        .to_string(),
                );
                if platform == "Instagram" {
                    if let Some(items) =
                        download_instagram_media_cobalt(&state.http_client, &url, &temp_dir).await
                    {
                        let scraped =
                            extract_instagram_metadata_googlebot(&state.http_client, &url).await;
                        media_info = scraped;
                        downloaded_items = Some(items);
                    }
                }
            }
            Err(error) => download_error = Some(error.to_string()),
        }
    }

    let caption = build_caption(&media_info, platform, Some(&url), 900);

    let items = match downloaded_items {
        Some(it) => it,
        None => {
            let reason = download_error
                .as_deref()
                .unwrap_or("yt-dlp returned no media");
            let _ = bot
                .edit_message_text(
                    msg.chat.id,
                    status_msg.id,
                    format!("Could not fetch content from {platform}: {}", reason.chars().take(300).collect::<String>()),
                )
                .await;
            let _ = tokio::fs::remove_dir_all(&temp_dir).await;
            return Ok(());
        }
    };

    if items.is_empty() {
        let _ = bot.delete_message(msg.chat.id, status_msg.id).await;
        let link_opts = LinkPreviewOptions {
            is_disabled: true,
            url: None,
            prefer_small_media: false,
            prefer_large_media: false,
            show_above_text: false,
        };
        bot.send_message(msg.chat.id, caption)
            .parse_mode(ParseMode::Html)
            .reply_parameters(ReplyParameters::new(msg.id))
            .link_preview_options(link_opts)
            .await?;
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        return Ok(());
    }

    let _ = bot
        .edit_message_text(msg.chat.id, status_msg.id, "Uploading to Telegram...")
        .await;

    let upload_limit = if state.config.is_local_api {
        2000 * 1024 * 1024
    } else {
        50 * 1024 * 1024
    };

    let total_size: u64 = items
        .iter()
        .filter_map(|it| std::fs::metadata(&it.path).ok().map(|m| m.len()))
        .sum();

    if items.len() == 1 && total_size > upload_limit {
        let item = &items[0];
        let filename = item
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let _ = bot
            .edit_message_text(
                msg.chat.id,
                status_msg.id,
                format!(
                    "File is {} MB (> {} MB limit).\nUploading to Google Drive...",
                    total_size / (1024 * 1024),
                    upload_limit / (1024 * 1024)
                ),
            )
            .await;

        let _ = Command::new("rclone")
            .args([
                "copy",
                item.path.to_str().unwrap_or("."),
                "drive:tgbot/",
                "--drive-chunk-size",
                "64M",
            ])
            .output()
            .await;

        let link_out = Command::new("rclone")
            .args(["link", &format!("drive:tgbot/{filename}")])
            .output()
            .await;

        let rclone_url = match link_out {
            Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            Err(_) => String::new(),
        };

        if !rclone_url.is_empty() {
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

            let elapsed = t0.elapsed().as_secs();
            let _ = bot.delete_message(msg.chat.id, status_msg.id).await;

            let resp_text = format!(
                "✅ Download Complete (GDrive Mirror)!\n\nFile: `{filename}`\nSize: {} MB\nTime Taken: {}\n\n{caption}",
                total_size / (1024 * 1024),
                fmt_eta(elapsed)
            );

            bot.send_message(msg.chat.id, resp_text)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .reply_parameters(ReplyParameters::new(msg.id))
                .await?;
        }
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        return Ok(());
    }

    let _ = bot.delete_message(msg.chat.id, status_msg.id).await;

    if items.len() == 1 {
        let item = &items[0];
        let file = InputFile::file(&item.path);
        if item.is_video {
            let width = media_info.get("width").and_then(|v| v.as_u64()).map(|v| v as u32);
            let height = media_info.get("height").and_then(|v| v.as_u64()).map(|v| v as u32);
            let duration = media_info.get("duration").and_then(|v| v.as_u64()).map(|v| v as u32);

            let mut req = bot
                .send_video(msg.chat.id, file)
                .caption(&caption)
                .parse_mode(ParseMode::Html)
                .reply_parameters(ReplyParameters::new(msg.id))
                .supports_streaming(true);

            if let Some(w) = width {
                req = req.width(w);
            }
            if let Some(h) = height {
                req = req.height(h);
            }
            if let Some(d) = duration {
                req = req.duration(d);
            }
            let _ = req.await;
        } else {
            let _ = bot
                .send_photo(msg.chat.id, file)
                .caption(&caption)
                .parse_mode(ParseMode::Html)
                .reply_parameters(ReplyParameters::new(msg.id))
                .await;
        }
    } else {
        for chunk in items.chunks(10) {
            let mut media_group: Vec<InputMedia> = Vec::new();
            for (idx, it) in chunk.iter().enumerate() {
                let file = InputFile::file(&it.path);
                let item_caption = if idx == 0 { Some(caption.clone()) } else { None };
                if it.is_video {
                    let mut vid = InputMediaVideo::new(file).supports_streaming(true);
                    if let Some(cap) = item_caption {
                        vid = vid.caption(cap).parse_mode(ParseMode::Html);
                    }
                    media_group.push(InputMedia::Video(vid));
                } else {
                    let mut photo = InputMediaPhoto::new(file);
                    if let Some(cap) = item_caption {
                        photo = photo.caption(cap).parse_mode(ParseMode::Html);
                    }
                    media_group.push(InputMedia::Photo(photo));
                }
            }
            let _ = bot
                .send_media_group(msg.chat.id, media_group)
                .reply_parameters(ReplyParameters::new(msg.id))
                .await;
        }
    }

    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    Ok(())
}
