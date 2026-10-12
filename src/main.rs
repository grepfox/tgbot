mod config;
mod handlers;
mod state;
mod utils;

use config::Config;
use state::AppState;
use teloxide::prelude::*;
use teloxide::types::BotCommand;
use tokio::sync::mpsc;
use url::Url;

#[tokio::main]
async fn main() {
    let config = Config::load().await;

    let (tx, rx) = mpsc::channel::<String>(100);
    let state = AppState::new(config.clone(), tx);

    let bot = if config.is_local_api {
        if let Some(ref local_url) = config.local_bot_api_url {
            println!("[System] Using LOCAL Telegram Bot API server at {local_url}");
            Bot::new(&config.tg_token).set_api_url(Url::parse(local_url).unwrap())
        } else {
            println!("[System] Using public Telegram Bot API server");
            Bot::new(&config.tg_token)
        }
    } else {
        println!("[System] Using public Telegram Bot API server");
        Bot::new(&config.tg_token)
    };

    let commands = vec![
        BotCommand::new("status", "Check if bot is up"),
        BotCommand::new("paste", "Paste text to a pastebin"),
        BotCommand::new("source_tracker", "Track LineageOS/YAAP commits"),
        BotCommand::new("mirror", "Mirror link/file to Google Drive"),
        BotCommand::new("tdl", "Manage your to-do list"),
        BotCommand::new("nowplaying", "Show currently playing on YouTube Music"),
        BotCommand::new(
            "neofetch",
            "Shows server specification in which bot is hosted on",
        ),
    ];
    let _ = bot.set_my_commands(commands).await;

    handlers::mirror::start_mirror_workers(bot.clone(), state.clone(), rx);

    println!("Bot is running...");

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(handlers::handle_message))
        .branch(Update::filter_callback_query().endpoint(handlers::handle_callback_query));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}
