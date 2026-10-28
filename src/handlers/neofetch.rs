use crate::utils::escape_html;
use regex::Regex;
use std::collections::HashMap;
use crate::utils::InTopic;
use teloxide::prelude::*;
use teloxide::types::{Message, ParseMode};
use tokio::process::Command;

pub fn clean_neofetch_ansi(raw_bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw_bytes);
    let mut grid: HashMap<(i32, i32), char> = HashMap::new();
    let mut x: i32 = 0;
    let mut y: i32 = 0;
    let mut max_x: i32 = 0;
    let mut max_y: i32 = 0;

    let ansi_re = Regex::new(r"\x1b\[([?0-9;]*)([a-zA-Z])").unwrap();
    let mut last_idx = 0;

    for mat in ansi_re.find_iter(&text) {
        let normal_text = &text[last_idx..mat.start()];
        for ch in normal_text.chars() {
            if ch == '\n' {
                y += 1;
                x = 0;
            } else if ch == '\r' {
                x = 0;
            } else {
                grid.insert((x, y), ch);
                x += 1;
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }

        let caps = ansi_re.captures(mat.as_str()).unwrap();
        let args_str = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        let cmd = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
        last_idx = mat.end();

        let clean_args = args_str.trim_start_matches('?');
        let args: Vec<i32> = clean_args
            .split(';')
            .filter_map(|v| v.parse::<i32>().ok())
            .collect();

        match cmd {
            "A" => {
                let val = *args.first().unwrap_or(&1);
                y = 0.max(y - val);
            }
            "B" => {
                let val = *args.first().unwrap_or(&1);
                y += val;
                max_y = max_y.max(y);
            }
            "C" => {
                let val = *args.first().unwrap_or(&1);
                x += val;
                max_x = max_x.max(x);
            }
            "D" => {
                let val = *args.first().unwrap_or(&1);
                x = 0.max(x - val);
            }
            _ => {}
        }
    }

    let remaining_text = &text[last_idx..];
    for ch in remaining_text.chars() {
        if ch == '\n' {
            y += 1;
            x = 0;
        } else if ch == '\r' {
            x = 0;
        } else {
            grid.insert((x, y), ch);
            x += 1;
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }

    let mut lines = Vec::new();
    for cur_y in 0..=max_y {
        let mut line_chars = Vec::new();
        for cur_x in 0..=max_x {
            line_chars.push(*grid.get(&(cur_x, cur_y)).unwrap_or(&' '));
        }
        let line_str: String = line_chars.into_iter().collect();
        lines.push(line_str.trim_end().to_string());
    }

    while lines.last().map(|s| s.is_empty()).unwrap_or(false) {
        lines.pop();
    }

    lines.join("\n")
}

pub async fn neofetch_cmd(bot: &Bot, msg: &Message) -> ResponseResult<()> {
    let mut proc = Command::new("neofetch")
        .output()
        .await;

    if proc.is_err() {
        proc = Command::new("fastfetch").output().await;
    }

    match proc {
        Ok(output) => {
            let clean = clean_neofetch_ansi(&output.stdout);
            if clean.trim().is_empty() {
                bot.send_message(msg.chat.id, "Error: neofetch returned empty output.").in_topic(msg)
                    .await?;
                return Ok(());
            }

            let resp = format!("<pre>{}</pre>", escape_html(&clean));
            bot.send_message(msg.chat.id, resp).in_topic(msg)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Error running neofetch: {e}")).in_topic(msg)
                .await?;
        }
    }

    Ok(())
}
