#!/bin/bash

echo "========================================================="
echo "   Telegram Local Bot API Server Setup Helper"
echo "========================================================="
echo ""
echo "To run a local Telegram Bot API server, you need an API_ID and API_HASH."
echo "You can get them by logging into https://my.telegram.org under 'API development tools'."
echo ""
read -p "Enter your Telegram API_ID: " api_id
read -p "Enter your Telegram API_HASH: " api_hash

if [ -z "$api_id" ] || [ -z "$api_hash" ]; then
    echo "❌ Error: API_ID and API_HASH cannot be empty!"
    exit 1
fi

echo ""
echo "Stopping any existing local bot API container..."
docker rm -f telegram-bot-api 2>/dev/null

echo "Deploying Telegram Bot API container..."
docker run -d \
  --name telegram-bot-api \
  --restart=always \
  -p 8081:8081 \
  -v telegram-bot-api-data:/var/lib/telegram-bot-api \
  -e TELEGRAM_API_ID="$api_id" \
  -e TELEGRAM_API_HASH="$api_hash" \
  aiogram/telegram-bot-api \
  --local

if [ $? -eq 0 ]; then
  echo ""
  echo "========================================================="
  echo "Success! Telegram Bot API Server is running on port 8081."
  echo "========================================================="
  echo "========================================================="
else
  echo "❌ Error: Failed to start the Docker container."
fi
