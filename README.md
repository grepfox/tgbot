# Telegram Bot

A Telegram bot for mirroring files to Google Drive, auto-downloading media from X/Twitter, YouTube and Instagram using `yt-dlp`, managing a to-do list and tracking LineageOS and YAAP commits.

## Setup

1. Create a `credentials.json` file in the root directory:
   ```json
   {
     "TG_TOKEN": "your-telegram-bot-token",
     "OWNER_USERNAME": "your-telegram-username",
     "LASTFM_API_KEY": "your-lastfm-api-key",
     "LASTFM_USERNAME": "your-lastfm-username"
   }
   ```
   *(Alternatively, configure them as environment variables: `TG_TOKEN`, `OWNER_USERNAME`, `LASTFM_API_KEY`, `LASTFM_USERNAME`.)*

2. Place YouTube/Instagram cookies in `cookies.txt` in the root directory to bypass sign-in walls or rate limits.

## Running the Bot

### Using Docker Compose

```bash
docker compose up -d --build
```

### Using Cargo Directly

```bash
cargo run --release
```

To run a local Telegram Bot API Server:
```bash
./setup_local_bot_api.sh
```
