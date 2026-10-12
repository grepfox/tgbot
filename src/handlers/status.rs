use std::time::Instant;
use teloxide::prelude::*;
use teloxide::types::Message;

pub async fn status_cmd(bot: &Bot, msg: &Message) -> ResponseResult<()> {
    let t0 = Instant::now();
    match bot.send_message(msg.chat.id, "Pinging...").await {
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
            let _ = bot.send_message(msg.chat.id, "bot is up").await;
        }
    }
    Ok(())
}
