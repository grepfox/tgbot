import os
import re
import html
import json
import asyncio
import requests
import datetime
import uuid
import shutil
import zipfile
import tempfile
import urllib.parse
from pathlib import Path
from telegram import Update, InlineKeyboardButton, InlineKeyboardMarkup, LinkPreviewOptions, InputMediaPhoto, BotCommand
from telegram.ext import Application, CommandHandler, MessageHandler, ContextTypes, CallbackQueryHandler, filters
import time
import yt_dlp
CREDENTIALS_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'credentials.json')

TOKEN = os.environ.get("TG_TOKEN", "")
OWNER_USERNAME = os.environ.get("OWNER_USERNAME", "grepfox")
LASTFM_API_KEY = os.environ.get("LASTFM_API_KEY", "")
LASTFM_USERNAME = os.environ.get("LASTFM_USERNAME", "")

if os.path.exists(CREDENTIALS_FILE):
    try:
        with open(CREDENTIALS_FILE, 'r') as f:
            creds = json.load(f)
            TOKEN = creds.get("TG_TOKEN", TOKEN)
            OWNER_USERNAME = creds.get("OWNER_USERNAME", OWNER_USERNAME)
            LASTFM_API_KEY = creds.get("LASTFM_API_KEY", LASTFM_API_KEY)
            LASTFM_USERNAME = creds.get("LASTFM_USERNAME", LASTFM_USERNAME)
    except Exception as e:
        print(f"Error loading credentials.json: {e}")

COOKIES_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'cookies.txt')

MAX_WORKERS = 4
mirror_queue = None
active_count = 0
tasks_state = {}

TDL_FILE = 'tdl_data.json'

def _tdl_load():
    try:
        with open(TDL_FILE, 'r') as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return {}

def _tdl_save(data):
    with open(TDL_FILE, 'w') as f:
        json.dump(data, f, indent=2)

def _tdl_render(items):
    """Return an HTML message showing the to-do list in a <pre> code block."""
    if not items:
        return "<b>Your To-Do List</b>\n\n<i>Nothing here yet. Add something with /tdl &lt;item&gt;</i>"
    lines = [f"  {i+1}. {html.escape(item)}" for i, item in enumerate(items)]
    body = "\n".join(lines)
    return f"<b>Your To-Do List</b>\n<pre>{body}</pre>"

async def safe_edit_message(status_msg, new_text, reply_markup=None):
    try:
        await status_msg.edit_text(new_text, reply_markup=reply_markup, parse_mode="Markdown")
    except Exception as e:
        if "Message is not modified" not in str(e):
            print(f"Edit message error: {e}")

async def mirror_worker(application: Application):
    global active_count
    while True:
        task_id = await mirror_queue.get()
        if task_id in tasks_state and not tasks_state[task_id].get('cancelled'):
            active_count += 1
            try:
                await process_mirror(task_id)
            except Exception as e:
                state = tasks_state.get(task_id, {})
                status_msg = state.get('status_msg')
                if status_msg:
                    await safe_edit_message(status_msg, f"❌ Error: {e}")
            finally:
                active_count -= 1
                if task_id in tasks_state:
                    del tasks_state[task_id]
        else:
            if task_id in tasks_state:
                del tasks_state[task_id]
        mirror_queue.task_done()

async def post_init(application: Application):
    global mirror_queue
    mirror_queue = asyncio.Queue()
    for _ in range(MAX_WORKERS):
        asyncio.create_task(mirror_worker(application))

    commands = [
        BotCommand("status", "Check if bot is up"),
        BotCommand("paste", "Paste text to a pastebin"),
        BotCommand("source_tracker", "Track LineageOS/YAAP commits"),
        BotCommand("mirror", "Mirror link/file to Google Drive"),
        BotCommand("tdl", "Manage your to-do list"),
        BotCommand("nowplaying", "Show currently playing on YouTube Music")
    ]
    await application.bot.set_my_commands(commands)

async def start(update: Update, context: ContextTypes.DEFAULT_TYPE):
    help_text = (
        "Hello! I am grepfox_bot.\n\n"
        "Commands:\n"
        "/status - Check if bot is up\n"
        "/paste <text> or reply to text - Paste text to a pastebin\n"
        "/source_tracker - Track commits in past week for LineageOS and YAAP\n"
        "/mirror <url> [-z] - Mirror a file to Google Drive (add -z to zip first)\n"
        "/tdl <item> - Add to your to-do list\n"
        "/tdl - Show your to-do list\n"
        "/tdl del <n> - Delete item number n\n"
        "/tdl clear - Clear your entire list\n"
        "/nowplaying - Show what's playing on YT Music (@grepfox only)\n"
        "\nAuto-download: Just send a YouTube, X/Twitter or Instagram link and I'll fetch the posts!"
    )
    await update.message.reply_text(help_text)

async def status(update: Update, context: ContextTypes.DEFAULT_TYPE):
    await update.message.reply_text("bot is up")

async def paste(update: Update, context: ContextTypes.DEFAULT_TYPE):
    text = " ".join(context.args)
    if not text and update.message.reply_to_message:
        text = update.message.reply_to_message.text or update.message.reply_to_message.caption

    if not text:
        await update.message.reply_text("Please provide text or reply to a message containing text to paste.")
        return

    try:

        response = requests.post("https://dpaste.com/api/v2/", data={"content": text})
        if response.status_code in [200, 201]:
            await update.message.reply_text(f"Pasted successfully: {response.text.strip()}")
        else:
            await update.message.reply_text(f"Failed to paste. Status code: {response.status_code}")
    except Exception as e:
        await update.message.reply_text(f"Error: {e}")

async def source_tracker(update: Update, context: ContextTypes.DEFAULT_TYPE):
    keyboard = [
        [
            InlineKeyboardButton("LineageOS", callback_data="track_LineageOS"),
            InlineKeyboardButton("YAAP", callback_data="track_yaap"),
        ]
    ]
    reply_markup = InlineKeyboardMarkup(keyboard)
    await update.message.reply_text(
        "Select the organization to track commits for the past week:",
        reply_markup=reply_markup
    )

async def source_tracker_callback(update: Update, context: ContextTypes.DEFAULT_TYPE):
    query = update.callback_query
    await query.answer()

    org = query.data.split("_")[1]
    await query.edit_message_text(f"Fetching recent commits for {org}...")

    one_week_ago = (datetime.datetime.utcnow() - datetime.timedelta(days=7)).strftime('%Y-%m-%d')
    url = f"https://api.github.com/search/commits?q=org:{org}+committer-date:>{one_week_ago}"
    headers = {"Accept": "application/vnd.github+json"}

    try:
        commits = []
        page = 1
        while True:
            page_url = f"{url}&per_page=100&page={page}"
            response = requests.get(page_url, headers=headers)
            if response.status_code == 200:
                data = response.json()
                items = data.get('items', [])
                commits.extend(items)
                if len(items) < 100 or page >= 5:
                    break
                page += 1
            else:
                if page == 1:
                    await query.edit_message_text(f"Failed to fetch commits. GitHub API Status: {response.status_code}")
                    return
                break

        if not commits:
            await query.edit_message_text(f"No commits found in the past week for {org}.")
            return

        msg = f"Commits in the past week for <b>{html.escape(org)}</b>:\n\n"
        for item in commits:
            sha = item['sha'][:7]
            commit_url = item['html_url']
            repo_name = item['repository']['name'] if 'repository' in item else org
            message = item['commit']['message'].split('\n')[0]
            msg += f"- <a href=\"{commit_url}\">{sha}</a> ({html.escape(repo_name)}): {html.escape(message)}\n"

        if len(msg) > 4000:
            plain_msg = f"Commits in the past week for {org}:\n\n"
            for item in commits:
                sha = item['sha'][:7]
                commit_url = item['html_url']
                repo_name = item['repository']['name'] if 'repository' in item else org
                message = item['commit']['message'].split('\n')[0]
                plain_msg += f"- {sha} ({repo_name}): {message}\n  Link: {commit_url}\n"

            file_path = f"{org}_commits.txt"
            with open(file_path, "w", encoding="utf-8") as f:
                f.write(plain_msg)

            await query.edit_message_text(f"Too many commits to display! Sending as a file...")
            with open(file_path, "rb") as f:
                await context.bot.send_document(
                    chat_id=update.effective_chat.id,
                    document=f,
                    filename=f"{org}_recent_commits.txt",
                    caption=f"Recent commits for {org}"
                )
            os.remove(file_path)
        else:
            await query.edit_message_text(
                msg, 
                parse_mode='HTML', 
                link_preview_options=LinkPreviewOptions(is_disabled=True)
            )
    except Exception as e:
         await query.edit_message_text(f"Error fetching commits: {e}")

def extract_gdrive_info(url):

    folder_match = re.search(r'/folders/([a-zA-Z0-9_-]+)', url)
    if folder_match:
        return folder_match.group(1), True

    folder_match_2 = re.search(r'id=([a-zA-Z0-9_-]+)', url)
    if folder_match_2 and "drive.google.com/drive/folders/" in url:
        return folder_match_2.group(1), True

    file_match = re.search(r'/file/d/([a-zA-Z0-9_-]+)', url)
    if file_match:
        return file_match.group(1), False

    id_match = re.search(r'[?&]id=([a-zA-Z0-9_-]+)', url)
    if id_match:
        return id_match.group(1), False

    return None, False

async def mirror(update: Update, context: ContextTypes.DEFAULT_TYPE):
    args = context.args if context.args else []
    if not args:
        await update.message.reply_text("Usage: /mirror <URL> [-z]")
        return

    do_zip = '-z' in args
    url_args = [a for a in args if a != '-z']
    if not url_args:
        await update.message.reply_text("Usage: /mirror <URL> [-z]")
        return
    url = url_args[0]

    task_id = str(uuid.uuid4())[:8]
    queue_pos = mirror_queue.qsize() + 1
    slots_free = MAX_WORKERS - active_count

    zip_note = "  |  Zip: ON" if do_zip else ""
    keyboard = [[InlineKeyboardButton("🚫 Cancel", callback_data=f"cancel_{task_id}")]]
    if slots_free > 0:
        status_msg = await update.message.reply_text(
            f"⏳ Starting mirror...{zip_note}",
            reply_markup=InlineKeyboardMarkup(keyboard)
        )
    else:
        status_msg = await update.message.reply_text(
            f"⏳ Added to queue\nPosition: {queue_pos}  |  Active: {active_count}/{MAX_WORKERS}{zip_note}",
            reply_markup=InlineKeyboardMarkup(keyboard)
        )

    tasks_state[task_id] = {
        'url': url,
        'do_zip': do_zip,
        'user_name': update.effective_user.first_name,
        'user_id': update.effective_user.id,
        'status_msg': status_msg,
        'cancelled': False,
        'process': None
    }

    await mirror_queue.put(task_id)

async def cancel_callback(update: Update, context: ContextTypes.DEFAULT_TYPE):
    query = update.callback_query
    task_id = query.data.split("_")[1]

    if task_id in tasks_state:
        if update.effective_user.id != tasks_state[task_id]['user_id']:
            await query.answer("You didn't start this task!", show_alert=True)
            return

        await query.answer("Task cancelled!")
        tasks_state[task_id]['cancelled'] = True
        proc = tasks_state[task_id].get('process')
        if proc:
            try:
                proc.terminate()
            except Exception:
                pass

        status_msg = tasks_state[task_id]['status_msg']
        await safe_edit_message(status_msg, "🚫 Task Cancelled by user!")
    else:
        await query.answer("Task not found or already finished.")

def fmt_bytes(b):
    for unit in ('B', 'KB', 'MB', 'GB', 'TB'):
        if b < 1024:
            return f"{b:.2f} {unit}"
        b /= 1024
    return f"{b:.2f} PB"

def fmt_speed(b):
    return fmt_bytes(b) + "/s"

def fmt_eta(seconds):
    seconds = int(seconds)
    if seconds < 60:
        return f"{seconds}s"
    m, s = divmod(seconds, 60)
    if m < 60:
        return f"{m}m {s}s"
    h, m = divmod(m, 60)
    return f"{h}h {m}m {s}s"

async def run_and_parse_rclone(cmd_args, task_id):
    state = tasks_state[task_id]

    full_cmd = cmd_args + ['--use-json-log', '--log-level', 'INFO']
    process = await asyncio.create_subprocess_exec(
        *full_cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT
    )
    state['process'] = process

    last_update = time.time()
    filename = state.get('filename', 'Unknown')
    file_label = state.get('file_label', 'Filename')
    action_text = state.get('action_text', 'Mirroring')
    user_name = state['user_name']
    user_id = state['user_id']
    status_msg = state['status_msg']

    while True:
        if state.get('cancelled'):
            break

        line = await process.stdout.readline()
        if not line:
            break

        line_str = line.decode('utf-8', errors='ignore').strip()
        if not line_str:
            continue

        try:
            entry = json.loads(line_str)
        except json.JSONDecodeError:
            continue

        if entry.get('level') != 'info':
            continue

        stats = entry.get('stats')
        if not stats:
            continue

        now = time.time()
        if now - last_update < 1:
            continue
        last_update = now

        try:
            bytes_done  = stats.get('bytes', 0)
            total_bytes = stats.get('totalBytes', 0)
            speed       = stats.get('speed', 0)
            upload_speed= stats.get('uploadSpeed', 0)
            eta_secs    = stats.get('eta') or 0
            transferring = stats.get('transferring', [])

            pct = int((bytes_done / total_bytes * 100)) if total_bytes else 0
            filled = pct // 10
            bar = "⬢" * filled + "○" * (10 - filled)

            lines = [
                f"*{file_label}:* `{filename}`",
                "",
                f"Task By {user_name} ( #ID{user_id} )",
                f"┣ [{bar}] {pct}%",
                f"┣ Processed → {fmt_bytes(bytes_done)} of {fmt_bytes(total_bytes)}",
                f"┣ Status → {action_text}",
            ]

            if upload_speed > 0:
                lines.append(f"┣ Download → {fmt_speed(speed)}")
                lines.append(f"┣ Upload → {fmt_speed(upload_speed)}")
            else:
                lines.append(f"┣ Speed → {fmt_speed(speed)}")

            lines.append(f"┗ ETA → {fmt_eta(eta_secs)}")

            msg_text = "\n".join(lines)
            keyboard = [[InlineKeyboardButton("🚫 Cancel", callback_data=f"cancel_{task_id}")]]
            await safe_edit_message(status_msg, msg_text, reply_markup=InlineKeyboardMarkup(keyboard))
        except Exception:
            pass

    await process.wait()
    return process.returncode

async def zip_path(src_path, zip_name, status_msg):
    """Zip a file or directory into <zip_name>.zip beside it. Returns the zip path."""
    await safe_edit_message(status_msg, "⏳ Status: Zipping...")
    zip_path_out = src_path + '.zip' if not src_path.endswith('.zip') else src_path

    zip_path_out = os.path.join(os.path.dirname(src_path), zip_name)
    loop = asyncio.get_event_loop()
    def _zip():
        with zipfile.ZipFile(zip_path_out, 'w', zipfile.ZIP_DEFLATED) as zf:
            if os.path.isdir(src_path):
                for root, dirs, files in os.walk(src_path):
                    for file in files:
                        fp = os.path.join(root, file)
                        arcname = os.path.relpath(fp, os.path.dirname(src_path))
                        zf.write(fp, arcname)
            else:
                zf.write(src_path, os.path.basename(src_path))
    await loop.run_in_executor(None, _zip)
    return zip_path_out

async def generate_link_and_finish(status_msg, filename):
    await safe_edit_message(status_msg, "⏳ Status: Generating public links...")

    link_process = await asyncio.create_subprocess_exec(
        'rclone', 'link', f'drive:tgbot/{filename}',
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE
    )
    stdout, stderr = await link_process.communicate()
    rclone_url = stdout.decode('utf-8', errors='ignore').strip()

    if rclone_url and "drive.google.com" in rclone_url:
        view_url = rclone_url
        dl_url = rclone_url
        if "/file/d/" in rclone_url:
            file_id = rclone_url.split("/d/")[1].split("/")[0]
            dl_url = f"https://drive.google.com/uc?export=download&id={file_id}"

        keyboard = [
            [
                InlineKeyboardButton("👁 View File", url=view_url),
                InlineKeyboardButton("⬇️ Download File", url=dl_url),
            ]
        ]
        reply_markup = InlineKeyboardMarkup(keyboard)
        await safe_edit_message(
            status_msg,
            f"✅ Mirror Complete!\n\nFile: `{filename}`",
            reply_markup=reply_markup
        )
    else:
        if rclone_url:
            keyboard = [[InlineKeyboardButton("🔗 Open Link", url=rclone_url)]]
            await safe_edit_message(status_msg, f"✅ Mirror Complete!\n\nFile: `{filename}`", reply_markup=InlineKeyboardMarkup(keyboard))
        else:
            await safe_edit_message(status_msg, f"✅ Mirror Complete!\n\nFile: `{filename}`\n\n_Note: Could not generate public link._")

async def gdrive_copyid_to_temp(file_id, task_id):
    """
    Copy a GDrive file by ID into a unique temp subdir on Drive, then read back
    the real filename via lsjson. Returns (proc_returncode, filename_or_None).
    Temp dir: drive:tgbot/tmp_{task_id}/
    """
    tmp_remote = f'drive:tgbot/tmp_{task_id}/'
    cp_proc = await asyncio.create_subprocess_exec(
        'rclone', 'backend', 'copyid', 'drive:', file_id, tmp_remote,
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT
    )
    await cp_proc.wait()
    if cp_proc.returncode != 0:
        out = await cp_proc.stdout.read()
        print(f"[DEBUG] copyid failed: {out.decode('utf-8', errors='ignore').strip()}")
        return cp_proc.returncode, None

    ls_proc = await asyncio.create_subprocess_exec(
        'rclone', 'lsjson', tmp_remote,
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE
    )
    ls_out, _ = await ls_proc.communicate()
    try:
        items = json.loads(ls_out.decode('utf-8', errors='ignore'))
        if items:
            return 0, items[0]['Name']
    except Exception:
        pass
    return 0, None

async def process_magnet(task_id):
    state = tasks_state[task_id]
    status_msg = state['status_msg']
    url = state['url']
    dl_dir = os.path.join('downloads', task_id)
    os.makedirs(dl_dir, exist_ok=True)

    state['filename'] = 'Resolving...'
    state['file_label'] = 'Torrent'
    state['action_text'] = 'Downloading'

    keyboard = [[InlineKeyboardButton("🚫 Cancel", callback_data=f"cancel_{task_id}")]]
    await safe_edit_message(status_msg, "⏳ Starting aria2c...", reply_markup=InlineKeyboardMarkup(keyboard))

    process = await asyncio.create_subprocess_exec(
        'aria2c', url, '--dir', dl_dir, '--seed-time=0',
        '--file-allocation=none', '--console-log-level=notice', '--summary-interval=1',
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT
    )
    state['process'] = process
    last_update = time.time()
    filename = None

    while True:
        if state.get('cancelled'):
            break
        line = await process.stdout.readline()
        if not line:
            break
        line_str = line.decode('utf-8', errors='ignore').strip()
        if not line_str:
            continue

        dl_match = re.search(r'Download complete: (.+)', line_str)
        if dl_match:
            filename = os.path.basename(dl_match.group(1).strip())
            state['filename'] = filename

        m = re.search(r'\[#\w+ ([\d.]+\w+)/([\d.]+\w+)\((\d+)%\)(?:[^\]]*DL:([\d.]+\w+))?(?:[^\]]*ETA:(\S+))?\]', line_str)
        if m:
            now = time.time()
            if now - last_update >= 1:
                last_update = now
                done_s, total_s, pct_s, spd_s, eta_s = m.groups()
                pct = int(pct_s)
                bar = '⬢' * (pct // 10) + '○' * (10 - pct // 10)
                fname = filename or 'Resolving...'
                msg_text = '\n'.join([
                    f"*Torrent:* `{fname}`", '',
                    f"Task By {state['user_name']} ( #ID{state['user_id']} )",
                    f"┣ [{bar}] {pct}%",
                    f"┣ Processed → {done_s} of {total_s}",
                    f"┣ Status → Downloading",
                    f"┣ Speed → {(spd_s or 'N/A') + '/s'}",
                    f"┗ ETA → {eta_s or 'N/A'}",
                ])
                await safe_edit_message(status_msg, msg_text, reply_markup=InlineKeyboardMarkup(keyboard))

    await process.wait()

    if state.get('cancelled') or process.returncode != 0:
        import shutil; shutil.rmtree(dl_dir, ignore_errors=True)
        if not state.get('cancelled'):
            await safe_edit_message(status_msg, "❌ Error: aria2c download failed.")
        return

    fname = filename or task_id
    upload_src = dl_dir
    upload_name = fname

    if state.get('do_zip') and not state.get('cancelled'):
        zip_filename = (fname if fname.endswith('.zip') else fname + '.zip')
        zipped = await zip_path(dl_dir, zip_filename, status_msg)
        upload_src = zipped
        upload_name = zip_filename

    state['filename'] = upload_name
    state['action_text'] = 'Uploading to Drive'
    state['file_label'] = 'Torrent'

    if state.get('do_zip') and not state.get('cancelled'):

        cmd = ['rclone', 'copy', upload_src, 'drive:tgbot/', '--stats', '1s']
    else:
        cmd = ['rclone', 'copy', upload_src, 'drive:tgbot/', '--stats', '1s']

    retcode = await run_and_parse_rclone(cmd, task_id)
    shutil.rmtree(dl_dir, ignore_errors=True)
    if state.get('do_zip'):
        try:
            os.remove(upload_src)
        except Exception:
            pass

    if state.get('cancelled'): return
    if retcode == 0:
        await generate_link_and_finish(status_msg, upload_name)
    else:
        await safe_edit_message(status_msg, "❌ Error: Failed to upload to Drive.")

async def run_aria2c_download(url, dl_dir, filename, task_id):
    """Download a URL via aria2c with live progress. Returns returncode."""
    state = tasks_state[task_id]
    status_msg = state['status_msg']
    keyboard = [[InlineKeyboardButton("🚫 Cancel", callback_data=f"cancel_{task_id}")]]

    process = await asyncio.create_subprocess_exec(
        'aria2c', url,
        '--dir', dl_dir, '--out', filename,
        '--file-allocation=none',
        '--console-log-level=notice',
        '--summary-interval=1',
        '--max-connection-per-server=8',
        '--split=8',
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT
    )
    state['process'] = process
    last_update = time.time()

    while True:
        if state.get('cancelled'):
            break
        line = await process.stdout.readline()
        if not line:
            break
        line_str = line.decode('utf-8', errors='ignore').strip()
        if not line_str:
            continue

        m = re.search(
            r'\[#\w+ ([\d.]+\w+)/([\d.]+\w+)\((\d+)%\)(?:[^\]]*DL:([\d.]+\w+))?(?:[^\]]*ETA:(\S+))?\]',
            line_str
        )
        if m:
            now = time.time()
            if now - last_update >= 1:
                last_update = now
                done_s, total_s, pct_s, spd_s, eta_s = m.groups()
                pct = int(pct_s)
                bar = '⯂' * (pct // 10) + '○' * (10 - pct // 10)
                msg_text = '\n'.join([
                    f"*{state.get('file_label','Filename')}:* `{filename}`", '',
                    f"Task By {state['user_name']} ( #ID{state['user_id']} )",
                    f"┊ [{bar}] {pct}%",
                    f"┊ Processed → {done_s} of {total_s}",
                    f"┊ Status → Downloading",
                    f"┊ Speed → {(spd_s or 'N/A') + '/s'}",
                    f"┗ ETA → {eta_s or 'N/A'}",
                ])
                await safe_edit_message(status_msg, msg_text,
                                        reply_markup=InlineKeyboardMarkup(keyboard))

    await process.wait()
    return process.returncode

async def _download_then_zip_upload(task_id, url, filename, label):
    """Download a URL via aria2c to a temp dir, optionally zip, then upload to Drive."""
    state = tasks_state[task_id]
    status_msg = state['status_msg']
    do_zip = state.get('do_zip', False)
    dl_dir = os.path.join('downloads', task_id)
    os.makedirs(dl_dir, exist_ok=True)

    state['filename'] = filename
    state['action_text'] = 'Downloading'
    state['file_label'] = label

    retcode = await run_aria2c_download(url, dl_dir, filename, task_id)
    if state.get('cancelled'):
        shutil.rmtree(dl_dir, ignore_errors=True)
        return
    if retcode != 0:
        shutil.rmtree(dl_dir, ignore_errors=True)
        await safe_edit_message(status_msg, "❌ Error: Download failed.")
        return

    local_path = os.path.join(dl_dir, filename)
    upload_src = local_path
    upload_name = filename

    if do_zip:
        zip_filename = filename + '.zip' if not filename.endswith('.zip') else filename
        upload_src = await zip_path(local_path, zip_filename, status_msg)
        upload_name = zip_filename

    state['filename'] = upload_name
    state['action_text'] = 'Uploading to Drive'
    cmd = ['rclone', 'copy', upload_src, 'drive:tgbot/', '--stats', '1s']
    retcode = await run_and_parse_rclone(cmd, task_id)
    shutil.rmtree(dl_dir, ignore_errors=True)

    if state.get('cancelled'): return
    if retcode == 0:
        await generate_link_and_finish(status_msg, upload_name)
    else:
        await safe_edit_message(status_msg, "❌ Error: Failed to upload to Drive.")

async def _copy_folder_then_zip_upload(task_id, file_id):
    """Copy a GDrive folder locally, optionally zip it, then re-upload."""
    state = tasks_state[task_id]
    status_msg = state['status_msg']
    do_zip = state.get('do_zip', False)
    dl_dir = os.path.join('downloads', task_id)
    os.makedirs(dl_dir, exist_ok=True)

    folder_name = f"Folder_{file_id}"
    local_folder = os.path.join(dl_dir, folder_name)
    os.makedirs(local_folder, exist_ok=True)

    state['filename'] = folder_name
    state['action_text'] = 'Downloading Folder'
    state['file_label'] = 'Foldername'

    cmd = ['rclone', 'copy', 'drive:', local_folder,
           '--drive-root-folder-id', file_id, '--drive-acknowledge-abuse', '--stats', '1s']
    retcode = await run_and_parse_rclone(cmd, task_id)
    if state.get('cancelled'):
        shutil.rmtree(dl_dir, ignore_errors=True)
        return
    if retcode != 0:
        shutil.rmtree(dl_dir, ignore_errors=True)
        await safe_edit_message(status_msg, "❌ Error: Failed to download Google Drive folder.")
        return

    zip_filename = folder_name + '.zip'
    zipped = await zip_path(local_folder, zip_filename, status_msg)
    upload_name = zip_filename

    state['filename'] = upload_name
    state['action_text'] = 'Uploading to Drive'
    cmd = ['rclone', 'copy', zipped, 'drive:tgbot/', '--stats', '1s']
    retcode = await run_and_parse_rclone(cmd, task_id)
    shutil.rmtree(dl_dir, ignore_errors=True)

    if state.get('cancelled'): return
    if retcode == 0:
        await generate_link_and_finish(status_msg, upload_name)
    else:
        await safe_edit_message(status_msg, "❌ Error: Failed to upload zipped folder to Drive.")

async def process_mirror(task_id):
    state = tasks_state[task_id]
    url = state['url']
    status_msg = state['status_msg']
    do_zip = state.get('do_zip', False)

    if url.lower().startswith('magnet:'):
        print(f"[DEBUG] Detected magnet link: {url[:80]}")
        await process_magnet(task_id)
        return

    print(f"[DEBUG] process_mirror url={url[:80]}")
    file_id, is_folder = extract_gdrive_info(url)
    print(f"[DEBUG] gdrive file_id={file_id}, is_folder={is_folder}")

    if file_id and is_folder:
        if do_zip:
            await _copy_folder_then_zip_upload(task_id, file_id)
        else:
            filename = f"Folder_{file_id}"
            state['filename'] = filename
            state['action_text'] = 'Copying Folder'
            state['file_label'] = 'Foldername'
            cmd = ['rclone', 'copy', 'drive:', f'drive:tgbot/{file_id}',
                   '--drive-root-folder-id', file_id, '--drive-acknowledge-abuse', '--stats', '1s']
            retcode = await run_and_parse_rclone(cmd, task_id)
            if state.get('cancelled'): return
            if retcode == 0:
                await generate_link_and_finish(status_msg, filename)
            else:
                await safe_edit_message(status_msg, "❌ Error: Failed to copy Google Drive folder.")
        return

    if file_id and not is_folder:
        state['action_text'] = 'Copying to Drive'
        state['file_label'] = 'Filename'
        state['filename'] = '...'
        await safe_edit_message(status_msg, "⏳ Copying file to Drive...")

        retcode, filename = await gdrive_copyid_to_temp(file_id, task_id)
        tmp_remote = f'drive:tgbot/tmp_{task_id}/'

        if state.get('cancelled'):

            await asyncio.create_subprocess_exec('rclone', 'purge', tmp_remote)
            return
        if retcode != 0 or filename is None:
            await asyncio.create_subprocess_exec('rclone', 'purge', tmp_remote)
            await safe_edit_message(status_msg, "❌ Error: Failed to copy Google Drive file.")
            return

        state['filename'] = filename

        if do_zip:

            dl_dir = os.path.join('downloads', task_id)
            os.makedirs(dl_dir, exist_ok=True)
            state['action_text'] = 'Downloading for zip'
            cmd = ['rclone', 'copy', tmp_remote, dl_dir, '--stats', '1s']
            dl_ret = await run_and_parse_rclone(cmd, task_id)

            await asyncio.create_subprocess_exec('rclone', 'purge', tmp_remote)
            if state.get('cancelled'):
                shutil.rmtree(dl_dir, ignore_errors=True)
                return
            if dl_ret != 0:
                shutil.rmtree(dl_dir, ignore_errors=True)
                await safe_edit_message(status_msg, "❌ Error: Failed to download for zipping.")
                return
            local_path = os.path.join(dl_dir, filename)
            zip_filename = filename + '.zip' if not filename.endswith('.zip') else filename
            zipped = await zip_path(local_path, zip_filename, status_msg)
            upload_name = zip_filename
            state['filename'] = upload_name
            state['action_text'] = 'Uploading to Drive'
            cmd = ['rclone', 'copy', zipped, 'drive:tgbot/', '--stats', '1s']
            retcode = await run_and_parse_rclone(cmd, task_id)
            shutil.rmtree(dl_dir, ignore_errors=True)
            if state.get('cancelled'): return
            if retcode == 0:
                await generate_link_and_finish(status_msg, upload_name)
            else:
                await safe_edit_message(status_msg, "❌ Error: Failed to upload to Drive.")
        else:

            src_path = f'drive:tgbot/tmp_{task_id}/{filename}'
            dst_path = f'drive:tgbot/{filename}'
            mv_proc = await asyncio.create_subprocess_exec(
                'rclone', 'moveto', src_path, dst_path,
                '--drive-acknowledge-abuse',
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT
            )
            state['process'] = mv_proc
            mv_out, _ = await mv_proc.communicate()

            await asyncio.create_subprocess_exec('rclone', 'purge', tmp_remote)
            if state.get('cancelled'): return
            if mv_proc.returncode == 0:
                await generate_link_and_finish(status_msg, filename)
            else:
                err = mv_out.decode('utf-8', errors='ignore').strip()
                print(f"[DEBUG] moveto failed: {err}")
                await safe_edit_message(status_msg, "❌ Error: Failed to move file to final location.")
        return

    filename = url.split('/')[-1].split('?')[0] or f"download_{int(datetime.datetime.utcnow().timestamp())}"
    if do_zip:
        await _download_then_zip_upload(task_id, url, filename, 'Filename')
    else:

        state['filename'] = filename
        state['action_text'] = 'Downloading'
        state['file_label'] = 'Filename'
        dl_dir = os.path.join('downloads', task_id)
        os.makedirs(dl_dir, exist_ok=True)
        retcode = await run_aria2c_download(url, dl_dir, filename, task_id)
        if state.get('cancelled'):
            shutil.rmtree(dl_dir, ignore_errors=True)
            return
        if retcode != 0:
            shutil.rmtree(dl_dir, ignore_errors=True)
            await safe_edit_message(status_msg, "❌ Error: Download failed.")
            return
        state['filename'] = filename
        state['action_text'] = 'Uploading to Drive'
        cmd = ['rclone', 'copy', os.path.join(dl_dir, filename), 'drive:tgbot/', '--stats', '1s']
        retcode = await run_and_parse_rclone(cmd, task_id)
        shutil.rmtree(dl_dir, ignore_errors=True)
        if state.get('cancelled'): return
        if retcode == 0:
            await generate_link_and_finish(status_msg, filename)
        else:
            await safe_edit_message(status_msg, "❌ Error: Failed to upload to Drive.")

async def tdl(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = str(update.effective_user.id)
    args = context.args

    data = _tdl_load()
    items = data.get(user_id, [])

    if args and args[0].lower() == 'clear':
        data[user_id] = []
        _tdl_save(data)
        await update.message.reply_text("\u2705 To-do list cleared.", parse_mode='HTML')
        return

    if args and args[0].lower() == 'del':
        if len(args) < 2 or not args[1].isdigit():
            await update.message.reply_text("Usage: /tdl del <number>", parse_mode='HTML')
            return
        idx = int(args[1]) - 1
        if idx < 0 or idx >= len(items):
            await update.message.reply_text(f"\u274c No item #{args[1]} in your list.", parse_mode='HTML')
            return
        removed = items.pop(idx)
        data[user_id] = items
        _tdl_save(data)
        await update.message.reply_text(
            f"\u2705 Removed: <code>{html.escape(removed)}</code>\n\n{_tdl_render(items)}",
            parse_mode='HTML'
        )
        return

    if args:
        item = ' '.join(args)
        items.append(item)
        data[user_id] = items
        _tdl_save(data)
        await update.message.reply_text(
            f"\u2705 Added: <code>{html.escape(item)}</code>\n\n{_tdl_render(items)}",
            parse_mode='HTML'
        )
        return

    await update.message.reply_text(_tdl_render(items), parse_mode='HTML')

def _fetch_exact_ytmusic_url(query: str) -> str:
    """Extract yt music links"""
    try:
        from ytmusicapi import YTMusic
        ytm = YTMusic()
        search_results = ytm.search(query, filter="songs")
        if search_results and 'videoId' in search_results[0]:
            return f"https://music.youtube.com/watch?v={search_results[0]['videoId']}"
        search_results = ytm.search(query, filter="videos")
        if search_results and 'videoId' in search_results[0]:
            return f"https://music.youtube.com/watch?v={search_results[0]['videoId']}"
    except Exception as e:
        print(f"Error searching YTMusic for exact link: {e}")
    return None

async def nowplaying(update: Update, context: ContextTypes.DEFAULT_TYPE):
    if update.effective_user.username != OWNER_USERNAME:
        await update.message.reply_text("This command is only for @grepfox.")
        return

    if not LASTFM_API_KEY or not LASTFM_USERNAME:
        await update.message.reply_text(
            "<b>Last.fm not configured.</b>\n\n"
            "Edit bot.py and set <code>LASTFM_API_KEY</code> and <code>LASTFM_USERNAME</code>.\n"
            "Get a free API key at https://www.last.fm/api/account/create",
            parse_mode='HTML'
        )
        return

    url = (
        'https://ws.audioscrobbler.com/2.0/'
        f'?method=user.getrecenttracks'
        f'&user={LASTFM_USERNAME}'
        f'&api_key={LASTFM_API_KEY}'
        f'&format=json&limit=1'
    )

    loop = asyncio.get_event_loop()
    try:
        resp = await loop.run_in_executor(
            None, lambda: requests.get(url, timeout=10)
        )
        resp.raise_for_status()
        data = resp.json()
    except Exception as e:
        await update.message.reply_text(f"Failed to reach Last.fm: {e}")
        return

    tracks = data.get('recenttracks', {}).get('track', [])
    if not tracks:
        await update.message.reply_text("No recent tracks found on Last.fm.")
        return

    track = tracks[0]
    is_now = track.get('@attr', {}).get('nowplaying') == 'true'
    title   = track.get('name', 'Unknown')
    artist  = track.get('artist', {}).get('#text', '')
    album   = track.get('album',  {}).get('#text', '')
    images  = track.get('image', [])
    thumb_url = next((i['#text'] for i in reversed(images) if i.get('#text')), None)

    status = "\U0001f3b5 Now Playing" if is_now else "\U0001f552 Last Played"
    lines = [f"<b>{status} — YT Music</b>\n"]
    lines.append(f"<b>{html.escape(title)}</b>")
    if artist:
        lines.append(f"Artist: {html.escape(artist)}")
    if album:
        lines.append(f"Album: {html.escape(album)}")
    caption = '\n'.join(lines)

    query = f"{artist} {title}".strip()
    import urllib.parse
    ytm_url = f"https://music.youtube.com/search?q={urllib.parse.quote(query)}"

    # Attempt to resolve the exact song link asynchronously
    try:
        exact_url = await loop.run_in_executor(
            None, _fetch_exact_ytmusic_url, query
        )
        if exact_url:
            ytm_url = exact_url
    except Exception as e:
        print(f"Executor error fetching exact link: {e}")

    keyboard = InlineKeyboardMarkup([[InlineKeyboardButton("Listen on YT Music", url=ytm_url)]])

    try:
        if thumb_url:
            await update.message.reply_photo(
                photo=thumb_url, caption=caption, parse_mode='HTML', reply_markup=keyboard
            )
        else:
            await update.message.reply_text(caption, parse_mode='HTML', reply_markup=keyboard)
    except Exception:
        await update.message.reply_text(caption, parse_mode='HTML', reply_markup=keyboard)

_SOCIAL_PATTERNS = re.compile(
    r'https?://(?:www\.)?'
    r'(?:'
    r'(?:twitter\.com|x\.com)/(?:i/)?\S+/status(?:es)?/\d+'
    r'|t\.co/\S+'
    r'|(?:youtube\.com/(?:watch|shorts|live)\S*|youtu\.be/\S+)'
    r'|(?:instagram\.com|instagr\.am)/(?:p|reel|tv)/[A-Za-z0-9_-]+/?\S*'
    r')',
    re.IGNORECASE
)

def _detect_social_url(text: str):
    """Return the first social media URL found in text, or None."""
    if not text:
        return None
    m = _SOCIAL_PATTERNS.search(text)
    return m.group(0) if m else None

def _platform_of(url: str) -> str:
    """Return a human-readable platform name."""
    u = url.lower()
    if 'youtube.com' in u or 'youtu.be' in u:
        return 'YouTube'
    if 'instagram.com' in u or 'instagr.am' in u:
        return 'Instagram'
    if 'twitter.com' in u or 'x.com' in u or 't.co' in u:
        return 'X'
    return 'Social'

def _ydl_opts_for(platform: str, download: bool, out_dir: str = '') -> dict:
    """Build yt-dlp options tuned per platform."""
    base = {
        'quiet': True,
        'no_warnings': True,
        'noplaylist': True,
        'writethumbnail': False,
        'writesubtitles': False,
        'writeautomaticsub': False,

        'socket_timeout': 30,
    }

    if os.path.isfile(COOKIES_FILE):
        base['cookiefile'] = COOKIES_FILE

    if not download:
        base['skip_download'] = True
        base['extract_flat'] = False
    else:
        base['outtmpl'] = os.path.join(out_dir, '%(id)s.%(ext)s')
        base['merge_output_format'] = 'mp4'

        base['format'] = (
            'bestvideo[height<=1080]+bestaudio'
            '/best[height<=1080]'
            '/best'
        )

    if platform == 'X':
        base['ignore_no_formats_error'] = True
        base['http_headers'] = {
            'User-Agent': (
                'Mozilla/5.0 (Windows NT 10.0; Win64; x64) '
                'AppleWebKit/537.36 (KHTML, like Gecko) '
                'Chrome/124.0.0.0 Safari/537.36'
            )
        }

    return base

def _extract_info_sync(url: str, platform: str):
    """
    Run yt-dlp in info-only mode (no download) to get metadata.
    Returns (info_dict, error_str).  info_dict is None on failure.
    For X posts where yt-dlp finds no video, info_dict['_no_video'] = True
    but there may still be images — the handler will attempt download anyway.
    """
    opts = _ydl_opts_for(platform, download=False)
    if platform == 'X':
        opts['ignore_no_formats_error'] = True
    try:
        with yt_dlp.YoutubeDL(opts) as ydl:
            info = ydl.extract_info(url, download=False)
            if platform == 'X':

                formats = info.get('formats') or []
                has_video = any(
                    f.get('vcodec', 'none') not in ('none', None)
                    for f in formats
                )
                if not has_video:
                    info['_no_video'] = True
            return info, None
    except Exception as e:
        return None, str(e)

def _download_best_media_sync(url: str, out_dir: str, platform: str,
                               permissive: bool = False,
                               progress_hook=None):
    """
    Download best video (<=1080p) or image from URL into out_dir.
    Returns (file_path, info_dict, error_str).
    file_path is None on failure.
    Picks video files first; among multiple files picks the largest.
    If permissive=True, uses format='best' (no codec restrictions).
    If progress_hook is set, it is added to yt-dlp's progress_hooks.
    """
    VIDEO_EXTS = {'.mp4', '.mkv', '.webm', '.mov', '.avi', '.m4v'}
    IMAGE_EXTS = {'.jpg', '.jpeg', '.png', '.webp', '.gif'}

    opts = _ydl_opts_for(platform, download=True, out_dir=out_dir)
    if permissive:
        opts['format'] = 'best'
    if progress_hook:
        opts['progress_hooks'] = [progress_hook]
    try:
        with yt_dlp.YoutubeDL(opts) as ydl:
            info = ydl.extract_info(url, download=True)

        all_files = [f for f in Path(out_dir).iterdir() if f.is_file()]
        if not all_files:
            return None, None, 'No file produced by yt-dlp'

        videos = [f for f in all_files if f.suffix.lower() in VIDEO_EXTS]
        images = [f for f in all_files if f.suffix.lower() in IMAGE_EXTS]
        candidates = videos or images or all_files
        best = max(candidates, key=lambda f: f.stat().st_size)
        return str(best), info, None

    except Exception as e:
        return None, None, str(e)

def _download_x_images_sync(info: dict, out_dir: str):
    """
    For X posts where yt-dlp has no video formats, try to grab all actual
    tweet images directly from pbs.twimg.com (public CDN, no auth needed).
    Tweet images appear in info['thumbnails'] with pbs.twimg.com/media/ URLs.
    Returns a list of local file paths.
    """
    thumbnails = info.get('thumbnails') or []

    media_urls = []
    seen_urls = set()
    for t in thumbnails:
        url = t.get('url') or ''
        if 'pbs.twimg.com/media/' in url:
            base_url = url.split('?')[0]
            if base_url not in seen_urls:
                seen_urls.add(base_url)
                media_urls.append(url)

    if not media_urls:
        return []

    headers = {'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36'}
    paths = []
    for i, raw_url in enumerate(media_urls):
        url = raw_url.split('?')[0] + '?format=jpg&name=large'
        try:
            resp = requests.get(url, timeout=20, headers=headers)
            if resp.status_code == 200:
                ct = resp.headers.get('content-type', '')
                ext = '.png' if 'png' in ct else '.webp' if 'webp' in ct else '.jpg'
                path = os.path.join(out_dir, f'x_img_{i}{ext}')
                with open(path, 'wb') as f:
                    f.write(resp.content)
                paths.append(path)
        except Exception:
            continue
    return paths

def _build_caption(info: dict, platform: str, max_len: int = 900) -> str:
    """
    Build a plain-label caption (no emojis) from yt-dlp info.
    Format:
      app: <Platform>
      user: <uploader>

      <blockquote>title / tweet text</blockquote>
    """
    title = (info.get('title') or '').strip()
    description = (info.get('description') or '').strip()
    uploader = (info.get('uploader') or info.get('channel') or '').strip()

    if platform == 'X':
        tweet_text = description or title

        tweet_text = html.unescape(tweet_text)

        tweet_text = re.sub(r'\s*https?://t\.co/[a-zA-Z0-9]+$', '', tweet_text).strip()

        tweet_text = re.sub(r' {2,}', '\n', tweet_text)

        if len(tweet_text) > max_len:
            tweet_text = tweet_text[:max_len] + '...'
        lines = ['<b>app: X</b>']
        if uploader:
            lines.append(f'user: <b>{html.escape(uploader)}</b>')
        lines.append('')

        quoted = f'<blockquote>{html.escape(tweet_text)}</blockquote>'
        lines.append(quoted)
        return '\n'.join(lines)
    else:
        caption_text = title
        if not caption_text and description:
            caption_text = description
        if caption_text and len(caption_text) > max_len:
            caption_text = caption_text[:max_len] + '...'
        lines = [f'<b>app: {platform}</b>']
        if uploader:
            lines.append(f'user: <b>{html.escape(uploader)}</b>')
        if caption_text:
            lines.append('')
            lines.append(f'<blockquote>{html.escape(caption_text)}</blockquote>')
        return '\n'.join(lines)

async def social_media_handler(update: Update, context: ContextTypes.DEFAULT_TYPE):
    """Auto-triggered when a message contains a social media URL."""
    message = update.effective_message
    if not message:
        return

    text = message.text or message.caption or ''
    url = _detect_social_url(text)
    if not url:
        return

    platform = _platform_of(url)

    status_msg = await message.reply_text(
        f'Fetching from {platform}...',
        reply_to_message_id=message.message_id
    )

    loop = asyncio.get_event_loop()
    out_dir = tempfile.mkdtemp(prefix='tgsocial_')

    try:

        try:
            info, err = await asyncio.wait_for(
                loop.run_in_executor(None, _extract_info_sync, url, platform),
                timeout=60
            )
        except asyncio.TimeoutError:
            info, err = None, 'Request timed out'

        if info is None:
            print(f'[social] extract_info failed for {platform}: {err}')
            await status_msg.edit_text(
                f'Could not fetch content from {platform}.\n'
                f'Reason: {html.escape(err or "unknown error")}'
            )
            return

        caption = _build_caption(info, platform)

        no_video = info.get('_no_video', False)

        if platform == 'X' and no_video:

            img_paths = await loop.run_in_executor(
                None, _download_x_images_sync, info, out_dir
            )
            if img_paths:
                await status_msg.delete()
                if len(img_paths) == 1:

                    with open(img_paths[0], 'rb') as f:
                        await message.reply_photo(
                            photo=f,
                            caption=caption,
                            parse_mode='HTML',
                            reply_to_message_id=message.message_id
                        )
                else:

                    media_group = []
                    files_to_close = []
                    for i, path in enumerate(img_paths):
                        f = open(path, 'rb')
                        files_to_close.append(f)
                        if i == 0:
                            media_group.append(InputMediaPhoto(media=f, caption=caption, parse_mode='HTML'))
                        else:
                            media_group.append(InputMediaPhoto(media=f))
                    try:
                        await message.reply_media_group(
                            media=media_group,
                            reply_to_message_id=message.message_id
                        )
                    finally:
                        for f in files_to_close:
                            f.close()
            else:

                await status_msg.delete()
                await message.reply_text(
                    caption,
                    parse_mode='HTML',
                    reply_to_message_id=message.message_id,
                    link_preview_options=LinkPreviewOptions(is_disabled=True)
                )
            return

        if platform == 'X':
            formats = info.get('formats', [])
            has_media = any(
                f.get('vcodec', 'none') not in ('none', None)
                or (f.get('ext') in ('jpg', 'jpeg', 'png', 'webp') and f.get('url'))
                for f in formats
            ) if formats else bool(info.get('url'))
            if not has_media:
                await status_msg.delete()
                await message.reply_text(
                    caption,
                    parse_mode='HTML',
                    reply_to_message_id=message.message_id,
                    link_preview_options=LinkPreviewOptions(is_disabled=True)
                )
                return

        await status_msg.edit_text(f'Downloading from {platform}...')

        _last_edit = [0.0]

        def make_progress_hook(ev_loop, msg, plat):
            def hook(d):
                if d['status'] != 'downloading':
                    return
                now = time.time()
                if now - _last_edit[0] < 2:
                    return
                _last_edit[0] = now

                done  = d.get('downloaded_bytes') or 0
                total = d.get('total_bytes') or d.get('total_bytes_estimate') or 0
                speed = d.get('speed') or 0
                eta   = int(d.get('eta') or 0)

                pct    = int(done / total * 100) if total else 0
                filled = pct // 10
                bar    = '⬢' * filled + '○' * (10 - filled)

                lines = [
                    f'Downloading from {plat}...',
                    '',
                    f'[{bar}] {pct}%',
                    f'Processed: {fmt_bytes(done)}' + (f' of {fmt_bytes(total)}' if total else ''),
                    f'Speed: {fmt_speed(speed)}',
                    f'ETA: {fmt_eta(eta)}',
                ]

                async def _edit():
                    try:
                        await msg.edit_text('\n'.join(lines))
                    except Exception:
                        pass

                asyncio.run_coroutine_threadsafe(_edit(), ev_loop)
            return hook

        hook = make_progress_hook(loop, status_msg, platform)
        file_path, dl_info, dl_err = await loop.run_in_executor(
            None, _download_best_media_sync, url, out_dir, platform, False, hook
        )

        if dl_info:
            caption = _build_caption(dl_info, platform)

        if not file_path or not os.path.exists(file_path):
            print(f'[social] download failed for {platform}: {dl_err}')
            if platform == 'X':

                await status_msg.delete()
                await message.reply_text(
                    caption,
                    parse_mode='HTML',
                    reply_to_message_id=message.message_id,
                    link_preview_options=LinkPreviewOptions(is_disabled=True)
                )
                return

            thumb = next(
                (t['url'] for t in (info.get('thumbnails') or []) if t.get('url')),
                None
            )
            if thumb:
                await status_msg.delete()
                await message.reply_photo(
                    photo=thumb,
                    caption=caption,
                    parse_mode='HTML',
                    reply_to_message_id=message.message_id
                )
            else:
                await status_msg.edit_text(
                    f'Could not download media from {platform}.\n'
                    f'Reason: {html.escape(dl_err or "unknown error")}'
                )
            return

        await status_msg.edit_text(f'Uploading to Telegram...')
        ext = os.path.splitext(file_path)[1].lower()
        file_size = os.path.getsize(file_path)

        is_local_api = "localhost:8081" in context.bot.base_url
        upload_limit = 2000 * 1024 * 1024 if is_local_api else 50 * 1024 * 1024

        if file_size > upload_limit:
            if is_local_api:
                await status_msg.edit_text(
                    f'File too large for Telegram even with Local Server ({file_size // (1024 * 1024)} MB > 2000 MB).\n\n'
                    + caption,
                    parse_mode='HTML'
                )
                return

            await status_msg.edit_text(f'File is {file_size // (1024 * 1024)} MB (> 50 MB limit).\nUploading to Google Drive...')
            filename = os.path.basename(file_path)

            copy_proc = await asyncio.create_subprocess_exec(
                'rclone', 'copy', file_path, 'drive:tgbot/',
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE
            )
            await copy_proc.communicate()

            link_process = await asyncio.create_subprocess_exec(
                'rclone', 'link', f'drive:tgbot/{filename}',
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE
            )
            stdout, stderr = await link_process.communicate()
            rclone_url = stdout.decode('utf-8', errors='ignore').strip()

            if rclone_url:
                view_url = rclone_url
                dl_url = rclone_url
                if "drive.google.com" in rclone_url and "/file/d/" in rclone_url:
                    file_id = rclone_url.split("/d/")[1].split("/")[0]
                    dl_url = f"https://drive.google.com/uc?export=download&id={file_id}"

                keyboard = [
                    [
                        InlineKeyboardButton("👁 View File", url=view_url),
                        InlineKeyboardButton("⬇️ Download File", url=dl_url),
                    ]
                ]
                reply_markup = InlineKeyboardMarkup(keyboard)
                await status_msg.delete()
                await message.reply_text(
                    f"✅ Download Complete (GDrive Mirror)!\n\nFile: `{filename}`\nSize: {file_size // (1024 * 1024)} MB\n\n" + caption,
                    parse_mode='HTML',
                    reply_markup=reply_markup,
                    reply_to_message_id=message.message_id
                )
            else:
                await status_msg.edit_text(
                    f"❌ Could not upload file or generate link.\nFile size: {file_size // (1024 * 1024)} MB is too large for standard Telegram Bot API."
                )
            return

        is_image = ext in {'.jpg', '.jpeg', '.png', '.webp', '.gif'}

        with open(file_path, 'rb') as f:
            await status_msg.delete()
            if is_image:
                await message.reply_photo(
                    photo=f,
                    caption=caption,
                    parse_mode='HTML',
                    reply_to_message_id=message.message_id
                )
            else:
                src = dl_info or info
                await message.reply_video(
                    video=f,
                    caption=caption,
                    parse_mode='HTML',
                    reply_to_message_id=message.message_id,
                    width=src.get('width') or 0,
                    height=src.get('height') or 0,
                    duration=int(src.get('duration') or 0),
                    supports_streaming=True
                )

    except Exception as e:
        print(f'[social_media_handler] Unhandled error: {e}')
        try:
            await status_msg.edit_text(
                f'Error processing {platform} link: {html.escape(str(e))}'
            )
        except Exception:
            pass
    finally:
        shutil.rmtree(out_dir, ignore_errors=True)

def main():

    import urllib.request
    import urllib.error
    use_local = False
    try:
        with urllib.request.urlopen("http://localhost:8081", timeout=1) as response:
            use_local = True
    except urllib.error.HTTPError:

        use_local = True
    except Exception:
        pass

    builder = Application.builder().token(TOKEN).post_init(post_init)
    if use_local:
        print("[System] Using LOCAL Telegram Bot API server at http://localhost:8081")
        builder = builder.base_url("http://localhost:8081/bot").local_mode(True)
    else:
        print("[System] Using public Telegram Bot API server")

    application = builder.build()

    application.add_handler(CommandHandler("start", start))
    application.add_handler(CommandHandler("help", start))
    application.add_handler(CommandHandler("status", status))
    application.add_handler(CommandHandler("paste", paste))
    application.add_handler(CommandHandler("source_tracker", source_tracker))
    application.add_handler(CallbackQueryHandler(source_tracker_callback, pattern="^track_"))
    application.add_handler(CallbackQueryHandler(cancel_callback, pattern="^cancel_"))
    application.add_handler(CommandHandler("mirror", mirror))
    application.add_handler(CommandHandler("tdl", tdl))
    application.add_handler(CommandHandler("nowplaying", nowplaying))

    application.add_handler(
        MessageHandler(
            (filters.TEXT | filters.CAPTION) & ~filters.COMMAND,
            social_media_handler
        )
    )

    print("Bot is running...")
    application.run_polling(drop_pending_updates=True)

if __name__ == '__main__':
    main()
