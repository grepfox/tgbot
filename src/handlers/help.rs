use crate::utils::InTopic;
use teloxide::prelude::*;
use teloxide::types::Message;

pub async fn start_or_help(bot: &Bot, msg: &Message) -> ResponseResult<()> {
    let help_text = "\
Hello! I am grepfox_bot.

Commands:
/status - Check if bot is up
/paste <text> or reply to text - Paste text to a pastebin
/source_tracker - Track commits in past week for LineageOS and YAAP
/mirror <url> [-z] - Mirror a file to Google Drive (add -z to zip first)
/tdl <item> - Add to your to-do list
/tdl - Show your to-do list
/tdl del <n> - Delete item number n
/tdl clear - Clear your entire list
/nowplaying - Show what's playing on YT Music (@grepfox only)
/neofetch - Shows server specification in which bot is hosted on

Auto-download: Just send a YouTube, X/Twitter or Instagram link and I'll fetch the posts!";

    bot.send_message(msg.chat.id, help_text).in_topic(msg).await?;
    Ok(())
}
