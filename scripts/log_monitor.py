#!/usr/bin/env python3
"""Tail a JSON log, filter by level/message, send matches to Telegram.

Reads only bytes appended since the last run (byte-offset state),
handles log rotation by inode tracking, and guards against
concurrent runs with a lock file.

Forwards two classes of events:
  - Headline events (info-level): delta.trade_executed,
    ls.trade_executed, rebalance_executed.
  - Any error-level event ("something nasty is happening").
"""

from __future__ import annotations

import fcntl
import html
import json
import os
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path

# --- Config ---
LOG_FILE = Path("/var/log/arb-bot2.log")
STATE_DIR = Path("/var/lib/log_monitor")
STATE_FILE = STATE_DIR / "state.json"
LOCK_FILE = STATE_DIR / "lock"
HEADLINE_MSGS = frozenset({
    "delta.trade_executed",
    "ls.trade_executed",
    "rebalance_executed",
})
FORWARD_LEVELS = frozenset({"ERROR"})
MAX_CHUNK = 3500          # Telegram hard limit is 4096 chars
TELEGRAM_TIMEOUT = 10
# --------------


def load_state() -> tuple[int | None, int]:
    try:
        with STATE_FILE.open() as f:
            data = json.load(f)
        return data.get("inode"), int(data.get("offset", 0))
    except (FileNotFoundError, json.JSONDecodeError, ValueError):
        return None, 0


def save_state(inode: int, offset: int) -> None:
    """Atomic write so a crash mid-write can't corrupt state."""
    tmp = STATE_FILE.with_suffix(".json.tmp")
    tmp.write_text(json.dumps({"inode": inode, "offset": offset}))
    os.replace(tmp, STATE_FILE)


def send_telegram(token: str, chat_id: str, text: str) -> None:
    url = f"https://api.telegram.org/bot{token}/sendMessage"
    data = urllib.parse.urlencode({
        "chat_id": chat_id,
        "text": text,
        "parse_mode": "HTML",
        "disable_web_page_preview": "true",
    }).encode()
    req = urllib.request.Request(url, data=data, method="POST")
    with urllib.request.urlopen(req, timeout=TELEGRAM_TIMEOUT) as resp:
        resp.read()  # drain


def chunk_lines(lines: list[str], max_chunk: int) -> list[str]:
    """Pack lines into chunks <= max_chunk chars without splitting a line."""
    chunks: list[str] = []
    cur = ""
    for line in lines:
        candidate = cur + line + "\n"
        if len(candidate) > max_chunk and cur:
            chunks.append(cur)
            cur = line + "\n"
        else:
            cur = candidate
    if cur:
        chunks.append(cur)
    return chunks


def acquire_lock() -> int:
    fd = os.open(str(LOCK_FILE), os.O_CREAT | os.O_RDWR, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        os.close(fd)
        sys.exit(0)  # another instance running; exit silently
    return fd


def decide_start_offset(
    prev_inode: int | None, prev_offset: int, cur_inode: int, cur_size: int
) -> int:
    if prev_inode is None:
        return cur_size                  # first ever run: skip history
    if prev_inode != cur_inode:
        return 0                         # rotated: read new file from start
    if cur_size < prev_offset:
        return 0                         # truncated in place
    return prev_offset


def tx_hash_suffix(obj: dict) -> str:
    """Pull tx_hash from the parent `event` span if present."""
    span = obj.get("span") or {}
    tx = span.get("tx_hash")
    if not tx or tx == "-":
        return ""
    short = tx[:10] + "…" if len(tx) > 12 else tx
    return f" (tx {short})"


def render_trade_executed(strategy: str, fields: dict, suffix: str) -> str:
    path = fields.get("path", "?")
    predicted = fields.get("predicted_cspr")
    actual = fields.get("actual_cspr")
    slippage = fields.get("slippage_cspr")
    is_loss = fields.get("is_loss")
    glyph = "⚠️" if is_loss else "✅"
    head = f"{glyph} {strategy}.trade_executed{suffix}"
    if strategy == "delta":
        cond = f"long {fields.get('long_diff_pct')}%, short {fields.get('short_diff_pct')}%"
    else:
        cond = f"st {fields.get('st_diff_pct')}%"
    body = f"path={path}\n{cond}\npredicted={predicted} CSPR  actual={actual} CSPR  slippage={slippage} CSPR"
    return f"{head}\n{body}"


def render_rebalance_executed(fields: dict, suffix: str) -> str:
    action = fields.get("action", "?")
    head = f"🔧 rebalance_executed: {action}{suffix}"
    asset = fields.get("asset")
    pre = fields.get("pre_balance")
    target = fields.get("target_midpoint")
    ops = fields.get("ops", "")
    decision = []
    if asset is not None:
        line = f"asset={asset}"
        if pre is not None:
            line += f"  pre={pre}"
        if target is not None:
            line += f"  target={target}"
        decision.append(line)
    decision.append(f"ops={ops}")
    balances = (
        f"balances: cspr={fields.get('cspr_balance')}  "
        f"wcspr={fields.get('wcspr_balance')}  stcspr={fields.get('stcspr_balance')}  "
        f"long={fields.get('long_balance')}  short={fields.get('short_balance')}"
    )
    return head + "\n" + "\n".join(decision) + "\n" + balances


def render_error(obj: dict, fields: dict, suffix: str) -> str:
    msg = fields.get("message", "")
    # Drop the message field; render everything else as key=val.
    rendered = "  ".join(
        f"{k}={v}" for k, v in fields.items() if k != "message"
    )
    head = f"🚨 ERROR: {msg}{suffix}"
    return f"{head}\n{rendered}" if rendered else head


def render(obj: dict) -> str | None:
    """Return a rendered Telegram block for a matching log line, else None."""
    fields = obj.get("fields") or {}
    timestamp = obj.get("timestamp") or obj.get("time") or ""
    msg = fields.get("message", "")
    level = obj.get("level", "")
    suffix = tx_hash_suffix(obj)

    if msg == "delta.trade_executed":
        body = render_trade_executed("delta", fields, suffix)
    elif msg == "ls.trade_executed":
        body = render_trade_executed("ls", fields, suffix)
    elif msg == "rebalance_executed":
        body = render_rebalance_executed(fields, suffix)
    elif level in FORWARD_LEVELS:
        body = render_error(obj, fields, suffix)
    else:
        return None
    return f"[{timestamp}]\n{body}"


def filter_matches(complete: bytes) -> list[str]:
    matches: list[str] = []
    for raw in complete.splitlines():
        if not raw.strip():
            continue
        try:
            obj = json.loads(raw)
        except json.JSONDecodeError:
            continue
        rendered = render(obj)
        if rendered is None:
            continue
        matches.append(rendered)
    return matches


def main() -> int:
    token = os.environ.get("TELEGRAM_BOT_TOKEN")
    chat_id = os.environ.get("TELEGRAM_CHAT_ID")
    if not token or not chat_id:
        print("TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID must be set", file=sys.stderr)
        return 2
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    acquire_lock()  # held for the rest of process lifetime

    if not LOG_FILE.exists():
        return 0

    st = LOG_FILE.stat()
    cur_inode, cur_size = st.st_ino, st.st_size

    prev_inode, prev_offset = load_state()
    start_offset = decide_start_offset(prev_inode, prev_offset, cur_inode, cur_size)

    if start_offset >= cur_size:
        save_state(cur_inode, cur_size)
        return 0
    # Read exactly the new slice
    with LOG_FILE.open("rb") as f:
        f.seek(start_offset)
        new_data = f.read(cur_size - start_offset)

    # Trim trailing partial line; only advance offset past complete lines
    last_nl = new_data.rfind(b"\n")
    if last_nl == -1:
        # No complete line yet; don't advance, retry next run
        return 0

    complete = new_data[: last_nl + 1]
    next_offset = start_offset + len(complete)

    matches = filter_matches(complete)

    if not matches:
        save_state(cur_inode, next_offset)
        return 0

    escaped = [html.escape(m) for m in matches]
    chunks = chunk_lines(escaped, MAX_CHUNK)
    total = len(matches)

    # Send first, save state only on success — at-least-once delivery
    try:
        for i, body in enumerate(chunks, 1):
            header = f"<b>🔔 {total} events (part {i}/{len(chunks)})</b>"
            send_telegram(token, chat_id, f"{header}\n<pre>{body}</pre>")
            if i < len(chunks):
                time.sleep(1)  # respect Telegram rate limits
    except Exception as e:
        print(f"Telegram send failed, will retry next run: {e}", file=sys.stderr)
        return 1

    save_state(cur_inode, next_offset)
    return 0


if __name__ == "__main__":
    sys.exit(main())
