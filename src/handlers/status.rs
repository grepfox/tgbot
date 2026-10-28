use std::time::Instant;
use crate::utils::InTopic;
use teloxide::prelude::*;
use teloxide::types::Message;

pub async fn status_cmd(bot: &Bot, msg: &Message) -> ResponseResult<()> {
    let t0 = Instant::now();
    match bot.send_message(msg.chat.id, "Pinging...").in_topic(msg).await {
        Ok(status_msg) => {
            let latency_ms = t0.elapsed().as_millis();
            let _ = bot
                .edit_message_text(
                    msg.chat.id,
                    status_msg.id,
                    format!("bot is up | ping: {latency_ms} ms"),
                )
                .await;
        }
        Err(_) => {
            let _ = bot.send_message(msg.chat.id, "bot is up").in_topic(msg).await;
        }
    }
    Ok(())
}
