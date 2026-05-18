# Telegram Bot

A versatile Telegram bot for mirroring files to Google Drive auto-downloading media from X/Twitter, YouTube and Instagram using `yt-dlp`, managing a to-do list and tracking LineageOS and YAAP commits.

## Setup

1. Install dependencies:
   ```bash
   pip install -r requirements.txt
   ```

2. Create a `credentials.json` file in the root directory:
   ```json
   {
     "TG_TOKEN": "your-telegram-bot-token",
     "OWNER_USERNAME": "your-telegram-username",
     "LASTFM_API_KEY": "your-lastfm-api-key",
     "LASTFM_USERNAME": "your-lastfm-username"
   }
   ```
   *(Alternatively, configure them as environment variables: `TG_TOKEN`, `OWNER_USERNAME`, `LASTFM_API_KEY`, `LASTFM_USERNAME`.)*

3. Place YouTube/Instagram cookies in `cookies.txt` in the root directory to bypass sign-in walls or rate limits.

## Running the Bot

Run the bot script:
```bash
python bot.py
```

To run a local Telegram Bot API Server (optional upto 2GB file upload support right now 50mb max):
```bash
./setup_local_bot_api.sh
```
