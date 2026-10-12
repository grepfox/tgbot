use crate::utils::escape_html;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use teloxide::prelude::*;
use teloxide::types::{Message, ParseMode};

const TDL_FILE: &str = "tdl_data.json";

fn tdl_load() -> HashMap<String, Vec<String>> {
    if !Path::new(TDL_FILE).exists() {
        return HashMap::new();
    }
    match fs::read_to_string(TDL_FILE) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

fn tdl_save(data: &HashMap<String, Vec<String>>) {
    if let Ok(content) = serde_json::to_string_pretty(data) {
        let _ = fs::write(TDL_FILE, content);
    }
}

fn tdl_render(items: &[String]) -> String {
    if items.is_empty() {
        return "<b>Your To-Do List</b>\n\n<i>Nothing here yet. Add something with /tdl &lt;item&gt;</i>"
            .to_string();
    }
    let lines: Vec<String> = items
        .iter()
        .enumerate()
        .map(|(i, item)| format!("  {}. {}", i + 1, escape_html(item)))
        .collect();
    let body = lines.join("\n");
    format!("<b>Your To-Do List</b>\n<pre>{body}</pre>")
}

pub async fn tdl_cmd(bot: &Bot, msg: &Message, args: &[String]) -> ResponseResult<()> {
    let user_id = match msg.from.as_ref() {
        Some(user) => user.id.to_string(),
        None => return Ok(()),
    };

    let mut data = tdl_load();
    let mut items = data.get(&user_id).cloned().unwrap_or_default();

    if let Some(first_arg) = args.first() {
        if first_arg.eq_ignore_ascii_case("clear") {
            data.insert(user_id, Vec::new());
            tdl_save(&data);
            bot.send_message(msg.chat.id, "To-do list cleared.")
                .parse_mode(ParseMode::Html)
                .await?;
            return Ok(());
        }

        if first_arg.eq_ignore_ascii_case("del") {
            if args.len() < 2 {
                bot.send_message(msg.chat.id, "Usage: /tdl del <number>")
                    .parse_mode(ParseMode::Html)
                    .await?;
                return Ok(());
            }

            match args[1].parse::<usize>() {
                Ok(num) if num >= 1 && num <= items.len() => {
                    let removed = items.remove(num - 1);
                    data.insert(user_id, items.clone());
                    tdl_save(&data);
                    let resp = format!(
                        "Removed: <code>{}</code>\n\n{}",
                        escape_html(&removed),
                        tdl_render(&items)
                    );
                    bot.send_message(msg.chat.id, resp)
                        .parse_mode(ParseMode::Html)
                        .await?;
                }
                _ => {
                    bot.send_message(
                        msg.chat.id,
                        format!("No item #{} in your list.", args[1]),
                    )
                    .parse_mode(ParseMode::Html)
                    .await?;
                }
            }
            return Ok(());
        }

        let item = args.join(" ");
        items.push(item.clone());
        data.insert(user_id, items.clone());
        tdl_save(&data);
        let resp = format!(
            "Added: <code>{}</code>\n\n{}",
            escape_html(&item),
            tdl_render(&items)
        );
        bot.send_message(msg.chat.id, resp)
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }

    bot.send_message(msg.chat.id, tdl_render(&items))
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}
