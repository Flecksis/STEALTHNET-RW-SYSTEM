import argparse
import hashlib
import http.client
import json
import os
import random
import secrets
import statistics
import subprocess
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CONTENT = {"sendMessage", "editMessageText", "sendPhoto", "editMessageMedia", "editMessageCaption",
           "sendDocument", "sendInvoice", "editMessageReplyMarkup", "sendAnimation"}


def psql(dsn, sql):
    out = subprocess.run(["psql", dsn, "-Atq", "-F", "\t", "-c", sql], check=True, capture_output=True, text=True)
    return [line.split("\t") for line in out.stdout.splitlines() if line]


def percentile(values, share):
    ordered = sorted(values)
    if not ordered:
        return None
    index = max(0, min(len(ordered) - 1, int(round(share * (len(ordered) - 1)))))
    return ordered[index]


class Stats:
    def __init__(self):
        self.lock = threading.Lock()
        self.timings = {}
        self.errors = {}

    def add(self, name, ms):
        with self.lock:
            self.timings.setdefault(name, []).append(ms)

    def fail(self, name, why):
        with self.lock:
            self.errors.setdefault(name, []).append(why)

    def rows(self, elapsed):
        out = []
        with self.lock:
            names = sorted(set(self.timings) | set(self.errors))
            for name in names:
                t = self.timings.get(name, [])
                e = self.errors.get(name, [])
                row = {"name": name, "count": len(t), "errors": len(e), "rps": round(len(t) / elapsed, 2) if elapsed else None}
                if t:
                    row.update({"p50": round(statistics.median(t), 1), "p95": round(percentile(t, 0.95), 1),
                                "p99": round(percentile(t, 0.99), 1), "max": round(max(t), 1)})
                if e:
                    row["error_sample"] = e[0][:200]
                out.append(row)
        return out


class Hub:
    def __init__(self):
        self.lock = threading.Condition()
        self.queue = []
        self.next_id = 1
        self.message_id = 1000
        self.waiting = {}
        self.callbacks = {}
        self.polls = 0
        self.calls = {}

    def push(self, update, chat):
        with self.lock:
            update["update_id"] = self.next_id
            self.next_id += 1
            slot = {"sent": None, "done": None, "ack": None, "methods": []}
            self.waiting[chat] = slot
            cb = update.get("callback_query")
            if cb:
                self.callbacks[cb["id"]] = chat
            self.queue.append(update)
            self.lock.notify_all()
            return slot

    def take(self, timeout):
        deadline = time.monotonic() + timeout
        with self.lock:
            self.polls += 1
            while not self.queue:
                left = deadline - time.monotonic()
                if left <= 0:
                    return []
                self.lock.wait(left)
            batch, self.queue = self.queue, []
            now = time.perf_counter()
            for update in batch:
                chat = None
                if "message" in update:
                    chat = update["message"]["chat"]["id"]
                elif "callback_query" in update:
                    chat = update["callback_query"]["from"]["id"]
                slot = self.waiting.get(chat)
                if slot is not None and slot["sent"] is None:
                    slot["sent"] = now
            return batch

    def seen(self, method, params):
        with self.lock:
            self.calls[method] = self.calls.get(method, 0) + 1
            chat = params.get("chat_id")
            if method == "answerCallbackQuery":
                chat = self.callbacks.pop(params.get("callback_query_id"), None)
                key = "ack"
            elif method in CONTENT:
                key = "done"
            else:
                return
            try:
                chat = int(chat) if chat is not None else None
            except (TypeError, ValueError):
                return
            slot = self.waiting.get(chat)
            if slot is None or slot["sent"] is None:
                return
            slot["methods"].append(method)
            if slot[key] is None:
                slot[key] = time.perf_counter()
                self.lock.notify_all()

    def wait(self, slot, timeout):
        deadline = time.monotonic() + timeout
        with self.lock:
            while slot["done"] is None:
                left = deadline - time.monotonic()
                if left <= 0:
                    break
                self.lock.wait(left)
        return slot


HUB = Hub()

MARK = "BCAST-MARK"


class Wire:
    def __init__(self):
        self.lock = threading.Lock()
        self.limit = 0
        self.window = []
        self.sends = []
        self.rejected = 0
        self.uploads = 0
        self.file_ids = 0

    def admit(self, now):
        with self.lock:
            if self.limit:
                cutoff = now - 1.0
                while self.window and self.window[0] < cutoff:
                    self.window.pop(0)
                if len(self.window) >= self.limit:
                    self.rejected += 1
                    return False
                self.window.append(now)
            return True

    def note(self, now, method, bulk, upload, file_id):
        with self.lock:
            self.sends.append((now, method, bulk))
            if upload:
                self.uploads += 1
            if file_id:
                self.file_ids += 1


WIRE = Wire()


class TelegramHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def reply(self, result):
        body = json.dumps({"ok": True, "result": result}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def fail(self, code, description, parameters=None):
        payload = {"ok": False, "error_code": code, "description": description}
        if parameters:
            payload["parameters"] = parameters
        body = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self.do_POST()

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        method = self.path.rsplit("/", 1)[-1].split("?")[0]
        params = {}
        ctype = self.headers.get("Content-Type", "")
        if raw[:1] in (b"{", b"["):
            try:
                params = json.loads(raw)
            except ValueError:
                params = {}
        elif "multipart" in ctype:
            text = raw.decode(errors="replace")
            for field in ("chat_id", "caption"):
                marker = f'name="{field}"'
                at = text.find(marker)
                if at >= 0:
                    tail = text[at + len(marker):].lstrip("\r\n")
                    params[field] = tail.split("\r\n--", 1)[0].strip()
            params["_upload"] = 'name="photo"; filename=' in text
        elif "x-www-form-urlencoded" in ctype:
            params = {k: v[0] for k, v in urllib.parse.parse_qs(raw.decode()).items()}
        if method == "getMe":
            return self.reply({"id": 999000, "is_bot": True, "first_name": "Load", "username": "load_bench_bot"})
        if method == "getWebhookInfo":
            return self.reply({"url": "", "pending_update_count": 0})
        if method == "getUpdates":
            return self.reply(HUB.take(min(float(params.get("timeout", 1)), 2.0)))
        if method in CONTENT:
            now = time.perf_counter()
            if not WIRE.admit(now):
                return self.fail(429, "Too Many Requests: retry after 1", {"retry_after": 1})
            body_text = str(params.get("text") or params.get("caption") or "")
            bulk = MARK in body_text
            upload = method == "sendPhoto" and bool(params.get("_upload"))
            file_id = method == "sendPhoto" and isinstance(params.get("photo"), str)
            WIRE.note(now, method, bulk, upload, file_id)
            if not bulk:
                HUB.seen(method, params)
            with HUB.lock:
                HUB.message_id += 1
                mid = HUB.message_id
            chat = params.get("chat_id") or 0
            try:
                chat = int(chat)
            except (TypeError, ValueError):
                chat = 0
            result = {"message_id": mid, "date": int(time.time()), "chat": {"id": chat, "type": "private"}, "text": ""}
            if method == "sendPhoto":
                result["photo"] = [{"file_id": "LOAD-FILE-1", "file_unique_id": "LOAD-U-1", "width": 800, "height": 400}]
            return self.reply(result)
        HUB.seen(method, params)
        if method == "getUserProfilePhotos":
            return self.reply({"total_count": 0, "photos": []})
        if method == "getChat":
            return self.reply({"id": int(params.get("chat_id") or 0), "type": "private"})
        if method == "createInvoiceLink":
            return self.reply("https://t.me/$invoice")
        return self.reply(True)


def tg_user(chat):
    return {"id": chat, "is_bot": False, "first_name": "Load", "last_name": str(chat), "language_code": "ru"}


def tg_message(chat, text, mid):
    msg = {"message_id": mid, "date": int(time.time()), "chat": {"id": chat, "type": "private"}, "from": tg_user(chat), "text": text}
    if text.startswith("/"):
        msg["entities"] = [{"type": "bot_command", "offset": 0, "length": len(text.split()[0])}]
    return msg


def tg_callback(chat, data, mid):
    return {"callback_query": {"id": secrets.token_hex(8), "from": tg_user(chat), "message": tg_message(chat, "экран", mid),
                               "chat_instance": "load", "data": data}}


class Http:
    def __init__(self, base, token=None, address=None):
        parsed = urllib.parse.urlparse(base)
        self.host = parsed.hostname
        self.port = parsed.port or 80
        self.conn = None
        self.token = token
        self.address = address

    def call(self, method, path, body=None):
        headers = {"Accept": "application/json", "Origin": f"http://{self.host}:{self.port}"}
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        if self.address:
            headers["X-Forwarded-For"] = self.address
        for attempt in range(2):
            if self.conn is None:
                self.conn = http.client.HTTPConnection(self.host, self.port, timeout=120)
            started = time.perf_counter()
            try:
                self.conn.request(method, path, body=data, headers=headers)
                response = self.conn.getresponse()
                payload = response.read()
                return response.status, payload, (time.perf_counter() - started) * 1000
            except (http.client.HTTPException, OSError):
                self.conn = None
                if attempt:
                    raise
        raise RuntimeError("unreachable")


def upload_raw(api, path, data, content_type):
    conn = http.client.HTTPConnection(api.host, api.port, timeout=120)
    headers = {"Content-Type": content_type, "Origin": f"http://{api.host}:{api.port}",
               "Authorization": f"Bearer {api.token}"}
    conn.request("POST", path, body=data, headers=headers)
    response = conn.getresponse()
    return response.status, response.read()


def sample_process(pid, stop, out):
    while not stop.is_set():
        r = subprocess.run(["ps", "-o", "%cpu=,rss=", "-p", str(pid)], capture_output=True, text=True)
        parts = r.stdout.split()
        if len(parts) == 2:
            out.append((float(parts[0]), int(parts[1])))
        stop.wait(1.0)


def broadcast(args):
    WIRE.limit = args.tg_limit
    start_telegram(args.tg_port)
    wait_polls(args.wait)
    time.sleep(args.settle)
    api = Http(args.base, admin_login(args.base, args.email, args.password))
    image_id = None
    if args.photo:
        with open(args.photo, "rb") as f:
            data = f.read()
        ctype = "image/png" if args.photo.endswith(".png") else "image/jpeg"
        status, payload = upload_raw(api, "/api/admin/m/broadcast/images", data, ctype)
        if status != 200:
            sys.exit(f"картинка не загружена: {status} {payload[:200]!r}")
        image_id = json.loads(payload)["id"]
    draft = {"text": f"{MARK} Проверка скорости рассылки", "audience": {"kind": args.audience}}
    if image_id:
        draft["image_id"] = image_id
    status, payload, _ = api.call("POST", "/api/admin/m/broadcast/broadcasts", draft)
    if status >= 400:
        sys.exit(f"рассылка не создана: {status} {payload[:300]!r}")
    bid = json.loads(payload)["id"]
    stats = Stats()
    stop = threading.Event()
    threads = []
    if args.bot_users:
        for chat, sid in pick_bot_clients(args.dsn, args.bot_users):
            threads.append(threading.Thread(target=bot_user, args=(stats, stop, chat, sid, args.bot_think, args.timeout, BOT_FLOW), daemon=True))
    samples = []
    if args.pid:
        threads.append(threading.Thread(target=sample_process, args=(args.pid, stop, samples), daemon=True))
    for t in threads:
        t.start()
    if args.bot_users:
        time.sleep(args.baseline)
    baseline = stats.rows(args.baseline) if args.bot_users else []
    stats_bc = Stats()
    if args.bot_users:
        stats.timings, stats.errors = {}, {}
    started = time.perf_counter()
    status, payload, ms = api.call("POST", f"/api/admin/m/broadcast/broadcasts/{bid}/send")
    if status >= 400:
        stop.set()
        sys.exit(f"рассылка не запущена: {status} {payload[:300]!r}")
    print(f"рассылка {bid} поставлена в очередь за {ms:.0f} мс: {payload[:160]!r}", flush=True)
    time.sleep(args.duration)
    elapsed = time.perf_counter() - started
    stop.set()
    api.call("POST", f"/api/admin/m/broadcast/broadcasts/{bid}/cancel")
    for t in threads:
        t.join(timeout=args.timeout + 5)
    with WIRE.lock:
        sends = list(WIRE.sends)
        rejected, uploads, file_ids = WIRE.rejected, WIRE.uploads, WIRE.file_ids
    bulk = [t - started for t, _, b in sends if b and t >= started]
    every = sorted(t for t, _, _ in sends if t >= started)
    per_second = {}
    for t in bulk:
        per_second[int(t)] = per_second.get(int(t), 0) + 1
    seconds = [per_second.get(i, 0) for i in range(5, max(6, int(elapsed) - 1))]
    peak = 0
    j = 0
    for i, t in enumerate(every):
        while every[j] < t - 1.0:
            j += 1
        peak = max(peak, i - j + 1)
    gaps = [b - a for a, b in zip(bulk, bulk[1:])]
    report = {
        "broadcast": bid, "elapsed": round(elapsed, 1), "bulk_sent": len(bulk),
        "bulk_rate": round(len(bulk) / elapsed, 2) if elapsed else None,
        "per_second_min": min(seconds) if seconds else None,
        "per_second_median": statistics.median(seconds) if seconds else None,
        "per_second_max": max(seconds) if seconds else None,
        "peak_any_second_all_sends": peak, "rejected_429": rejected,
        "uploads": uploads, "file_id_sends": file_ids,
        "max_gap_ms": round(max(gaps) * 1000, 1) if gaps else None,
        "gaps_over_500ms": sum(1 for g in gaps if g > 0.5),
        "cpu_avg": round(statistics.mean(c for c, _ in samples), 1) if samples else None,
        "cpu_max": max((c for c, _ in samples), default=None),
        "rss_max_mb": round(max((r for _, r in samples), default=0) / 1024, 1),
        "sent_in_db": int(psql(args.dsn, f"SELECT count(*) FROM broadcast_recipients WHERE broadcast_id = {int(bid)} AND state = 'sent'")[0][0]),
    }
    print(json.dumps(report, ensure_ascii=False, indent=1), flush=True)
    rows = stats.rows(elapsed)
    for title, data in (("бот без рассылки", baseline), ("бот во время рассылки", rows)):
        print(title, flush=True)
        for row in data:
            if "p50" in row:
                print(f"  {row['name']:36} n={row['count']:5d}  p50 {row['p50']:7.1f}  p95 {row['p95']:7.1f}  p99 {row['p99']:7.1f}  ошибок {row['errors']}", flush=True)
    if args.out:
        with open(args.out, "w") as f:
            json.dump({"report": report, "baseline": baseline, "during": rows, "bulk_times": bulk}, f, ensure_ascii=False)


def client_token(dsn, client_id, channel):
    token = secrets.token_hex(32)
    digest = hashlib.sha256(token.encode()).hexdigest()
    psql(dsn, f"""INSERT INTO client_sessions (client_id, token_hash, channel, scope, expires_at)
                  VALUES ({int(client_id)}, '\\x{digest}', '{channel}', 'full', now() + interval '1 day')""")
    return token


def pick_bot_clients(dsn, count):
    rows = psql(dsn, f"""
        SELECT i.value, s.id FROM client_identities i JOIN clients c ON c.id = i.client_id
        JOIN LATERAL (SELECT id FROM subscriptions s WHERE s.client_id = c.id AND s.ended_at IS NULL AND s.kind = 'paid'
                      ORDER BY s.id DESC LIMIT 1) s ON true
        WHERE i.kind = 'telegram' AND c.status = 'active' AND c.deleted_at IS NULL AND NOT c.bot_unreachable
        ORDER BY hashint8(c.id) LIMIT {int(count)}""")
    return [(int(chat), int(sid)) for chat, sid in rows]


def pick_web_clients(dsn, count):
    rows = psql(dsn, f"""
        SELECT c.id, s.id, i.value FROM clients c
        JOIN client_identities i ON i.client_id = c.id AND i.kind = 'telegram'
        JOIN LATERAL (SELECT id FROM subscriptions s WHERE s.client_id = c.id AND s.ended_at IS NULL AND s.kind = 'paid'
                      ORDER BY s.id DESC LIMIT 1) s ON true
        WHERE c.status = 'active' AND c.deleted_at IS NULL
        ORDER BY hashint8(c.id + 7) LIMIT {int(count)}""")
    return [(int(cid), int(sid), int(tg)) for cid, sid, tg in rows]


def init_data(bot_token, telegram_id):
    import hmac
    user = json.dumps({"id": telegram_id, "first_name": "Load", "language_code": "ru"}, separators=(",", ":"))
    pairs = {"auth_date": str(int(time.time())), "query_id": secrets.token_hex(8), "user": user}
    check = "\n".join(f"{k}={v}" for k, v in sorted(pairs.items()))
    secret = hmac.new(b"WebAppData", bot_token.encode(), hashlib.sha256).digest()
    pairs["hash"] = hmac.new(secret, check.encode(), hashlib.sha256).hexdigest()
    return urllib.parse.urlencode(pairs, quote_via=urllib.parse.quote)


def miniapp_login(api, bot_token, telegram_id):
    status, payload, ms = api.call("POST", "/api/client/auth/miniapp", {"init_data": init_data(bot_token, telegram_id)})
    if status != 200:
        return None, status, payload, ms
    return json.loads(payload).get("token"), status, payload, ms


BOT_FLOW = [
    ("бот: /start", lambda sid: ("text", "/start")),
    ("бот: главное меню", lambda sid: ("cb", "m")),
    ("бот: мои подписки", lambda sid: ("cb", "subs")),
    ("бот: моя подписка", lambda sid: ("cb", f"sub:{sid}")),
    ("бот: купить", lambda sid: ("cb", "buy")),
    ("бот: тариф", lambda sid: ("cb", "t:2")),
    ("бот: срок", lambda sid: ("cb", "p:5")),
    ("бот: оплата звёздами", lambda sid: ("cb", "pay:stars")),
]


def bot_step(stats, chat, label, kind, value, mid, timeout):
    update = {"message": tg_message(chat, value, mid)} if kind == "text" else tg_callback(chat, value, mid)
    slot = HUB.push(update, chat)
    HUB.wait(slot, timeout)
    if slot["sent"] is None:
        stats.fail(label, "бот не забрал обновление")
        return False
    if slot["done"] is None:
        if slot["ack"] is not None:
            stats.add(label + " (только ответ на кнопку)", (slot["ack"] - slot["sent"]) * 1000)
            return True
        stats.fail(label, "нет ответа")
        return False
    stats.add(label, (slot["done"] - slot["sent"]) * 1000)
    return True


def bot_user(stats, stop, chat, sid, think, timeout, flow):
    mid = random.randint(1, 10 ** 6)
    rng = random.Random(chat)
    first = True
    while not stop.is_set():
        for label, make in flow:
            if stop.is_set():
                return
            kind, value = make(sid)
            if kind == "text" and value == "/start" and not first:
                continue
            first = False
            mid += 1
            bot_step(stats, chat, label, kind, value, mid, timeout)
            stop.wait(rng.uniform(think * 0.5, think * 1.5))


WEB_FLOW = [
    ("{ch}: me", "GET", "/api/client/me", None),
    ("{ch}: подписки", "GET", "/api/client/subscriptions", None),
    ("{ch}: подписка", "GET", "/api/client/subscriptions/{sid}", None),
    ("{ch}: каталог", "GET", "/api/client/catalog", None),
    ("{ch}: расчёт цены", "POST", "/api/client/quote", {"tariff_id": 2, "period_id": 5, "extra_devices": 0}),
    ("{ch}: история оплат", "GET", "/api/client/payments", None),
]


def fake_address(seed):
    return f"198.18.{(seed >> 8) % 256}.{seed % 256}"


def web_user(stats, stop, base, token, channel, sid, think, bot_token=None, telegram_id=None):
    api = Http(base, token, fake_address(int(telegram_id or 0)))
    rng = random.Random(token)
    name = "кабинет" if channel == "cabinet" else "мини-приложение"
    while not stop.is_set():
        if channel == "miniapp" and bot_token:
            api.token = None
            try:
                fresh, status, payload, ms = miniapp_login(api, bot_token, telegram_id)
            except Exception as err:
                stats.fail("мини-приложение: вход", str(err))
                stop.wait(think)
                continue
            if fresh:
                api.token = fresh
                stats.add("мини-приложение: вход", ms)
            else:
                stats.fail("мини-приложение: вход", f"HTTP {status}: {payload[:120]!r}")
                api.token = token
        for label, method, path, body in WEB_FLOW:
            if stop.is_set():
                return
            label = label.format(ch=name)
            try:
                status, payload, ms = api.call(method, path.format(sid=sid), body)
            except Exception as err:
                stats.fail(label, str(err))
                continue
            if status >= 400:
                stats.fail(label, f"HTTP {status}: {payload[:120]!r}")
            else:
                stats.add(label, ms)
            stop.wait(rng.uniform(think * 0.5, think * 1.5))


ADMIN_FLOW = [
    ("админка: главная", "GET", "/api/admin/dashboard"),
    ("админка: лента", "GET", "/api/admin/feed?kind=highlights&limit=12"),
    ("админка: клиенты", "GET", "/api/admin/clients?limit=50"),
    ("админка: счётчики сегментов", "GET", "/api/admin/clients/segments"),
    ("админка: поиск клиента", "GET", "/api/admin/clients?limit=50&q=load"),
    ("админка: карточка клиента", "GET", "/api/admin/clients/{cid}"),
    ("админка: история клиента", "GET", "/api/admin/clients/{cid}/history?limit=5"),
    ("админка: платежи", "GET", "/api/admin/payments?limit=30"),
    ("админка: журнал действий", "GET", "/api/admin/audit?limit=50"),
    ("админка: тарифы", "GET", "/api/admin/tariffs"),
]


def admin_user(stats, stop, base, token, cid, think):
    api = Http(base, token)
    while not stop.is_set():
        for label, method, path in ADMIN_FLOW:
            if stop.is_set():
                return
            try:
                status, payload, ms = api.call(method, path.format(cid=cid))
            except Exception as err:
                stats.fail(label, str(err))
                continue
            if status >= 400:
                stats.fail(label, f"HTTP {status}: {payload[:120]!r}")
            else:
                stats.add(label, ms)
            stop.wait(think)


def remna_stats(remna):
    if not remna:
        return {}
    try:
        api = Http(remna)
        status, payload, _ = api.call("GET", "/__stats")
        return json.loads(payload) if status == 200 else {}
    except Exception:
        return {}


def remna_delta(before, after):
    out = {}
    for key, value in after.items():
        d = value - before.get(key, 0)
        if d:
            out[key] = d
    return out


def admin_login(base, email, password):
    api = Http(base)
    status, body, _ = api.call("POST", "/api/admin/auth/login", {"email": email, "password": password})
    if status != 200:
        sys.exit(f"вход в админку не удался: {status} {body[:200]!r}")
    return json.loads(body)["token"]


def start_telegram(port):
    server = ThreadingHTTPServer(("127.0.0.1", port), TelegramHandler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def wait_polls(timeout):
    deadline = time.monotonic() + timeout
    while HUB.polls < 2 and time.monotonic() < deadline:
        time.sleep(0.2)
    if HUB.polls < 2:
        sys.exit("бот не начал опрашивать getUpdates")


def run(args):
    stats = Stats()
    stop = threading.Event()
    threads = []
    if args.bot_users:
        start_telegram(args.tg_port)
        wait_polls(args.wait)
        chats = pick_bot_clients(args.dsn, args.bot_users)
        for chat, sid in chats:
            threads.append(threading.Thread(target=bot_user, args=(stats, stop, chat, sid, args.bot_think, args.timeout, BOT_FLOW), daemon=True))
    if args.web_users:
        for i, (cid, sid, tg) in enumerate(pick_web_clients(args.dsn, args.web_users)):
            channel = "cabinet" if i % 2 == 0 else "miniapp"
            token = client_token(args.dsn, cid, channel)
            threads.append(threading.Thread(target=web_user, args=(stats, stop, args.base, token, channel, sid, args.web_think, args.bot_token, tg), daemon=True))
    if args.admins:
        token = admin_login(args.base, args.email, args.password)
        cid = psql(args.dsn, "SELECT c.id FROM clients c JOIN payments p ON p.client_id = c.id AND p.status = 'paid' GROUP BY c.id HAVING count(*) BETWEEN 4 AND 8 ORDER BY c.id DESC LIMIT 1")[0][0]
        for _ in range(args.admins):
            threads.append(threading.Thread(target=admin_user, args=(stats, stop, args.base, token, cid, args.admin_think), daemon=True))
    before = remna_stats(args.remna)
    random.shuffle(threads)
    started = time.monotonic()
    for t in threads:
        t.start()
        time.sleep(args.ramp / max(1, len(threads)))
    time.sleep(max(0, args.duration - (time.monotonic() - started)))
    stop.set()
    elapsed = time.monotonic() - started
    for t in threads:
        t.join(timeout=args.timeout + 5)
    after = remna_stats(args.remna)
    rows = stats.rows(elapsed)
    for row in rows:
        if "p50" in row:
            print(f"{row['name']:42} n={row['count']:6d} {row['rps']:7.2f}/с  p50 {row['p50']:8.1f}  p95 {row['p95']:8.1f}  p99 {row['p99']:8.1f}  max {row['max']:8.1f}  ошибок {row['errors']}", flush=True)
        else:
            print(f"{row['name']:42} ошибок {row['errors']}: {row.get('error_sample')}", flush=True)
    calls = remna_delta(before, after)
    total = sum(calls.values())
    print(f"Remnawave: {total} запросов за {elapsed:.0f} с ({total / elapsed:.1f}/с)")
    for key, value in sorted(calls.items(), key=lambda kv: -kv[1]):
        print(f"  {key:60} {value}")
    if args.out:
        with open(args.out, "w") as f:
            json.dump({"elapsed": elapsed, "rows": rows, "remna": calls, "telegram": HUB.calls}, f, ensure_ascii=False, indent=1)


def log_size(path):
    try:
        return os.path.getsize(path)
    except OSError:
        return 0


def read_queries(path, start, end):
    with open(path, "rb") as f:
        f.seek(start)
        chunk = f.read(end - start)
    queries = []
    for line in chunk.splitlines():
        if b"sqlx::query" not in line:
            continue
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if event.get("target") != "sqlx::query":
            continue
        fields = event.get("fields", {})
        statement = fields.get("db.statement") or fields.get("summary") or fields.get("message") or ""
        span = event.get("span") or {}
        queries.append({"rid": span.get("request_id"), "sql": " ".join(statement.split())[:160],
                        "ms": fields.get("elapsed_secs", 0) * 1000 if isinstance(fields.get("elapsed_secs"), (int, float)) else None})
    return queries


def settle(path, quiet=0.4, limit=5.0):
    deadline = time.monotonic() + limit
    size = log_size(path)
    while time.monotonic() < deadline:
        time.sleep(quiet)
        now = log_size(path)
        if now == size:
            return now
        size = now
    return size


def count(args):
    rows = []

    def record(group, name, queries, calls, ms=None):
        heavy = sorted([q for q in queries if q["ms"] is not None], key=lambda q: -q["ms"])[:3]
        row = {"group": group, "name": name, "queries": len(queries), "remna": sum(calls.values()), "remna_calls": calls,
               "ms": ms, "db_ms": round(sum(q["ms"] or 0 for q in queries), 1), "top": heavy}
        rows.append(row)
        print(f"{group:16} {name:34} запросов к базе {len(queries):4d}  к Remnawave {row['remna']:3d}  "
              f"время базы {row['db_ms']:7.1f} мс" + (f"  ответ {ms:7.1f} мс" if ms is not None else ""), flush=True)
        if args.verbose:
            for q in queries:
                print(f"      {q['ms'] or 0:7.2f}  {q['sql']}")

    def http_action(api, group, name, method, path, body=None):
        before_calls = remna_stats(args.remna)
        start = settle(args.log)
        status, payload, ms = api.call(method, path, body)
        end = settle(args.log)
        calls = remna_delta(before_calls, remna_stats(args.remna))
        queries = read_queries(args.log, start, end)
        rid_queries = [q for q in queries if q["rid"]]
        if rid_queries:
            rids = {q["rid"] for q in rid_queries}
            queries = rid_queries if len(rids) == 1 else queries
        if status >= 400:
            print(f"{group:16} {name:34} HTTP {status}: {payload[:160]!r}")
        record(group, name, queries, calls, ms)

    if args.bot_users:
        start_telegram(args.tg_port)
        wait_polls(args.wait)
        idle_start = settle(args.log)
        time.sleep(3)
        idle = len(read_queries(args.log, idle_start, log_size(args.log)))
        print(f"фоновые запросы бота без обновлений: {idle} за 3 с")
        for chat, sid in pick_bot_clients(args.dsn, args.bot_users):
            mid = 100
            for label, make in BOT_FLOW:
                kind, value = make(sid)
                mid += 1
                before_calls = remna_stats(args.remna)
                start = settle(args.log)
                update = {"message": tg_message(chat, value, mid)} if kind == "text" else tg_callback(chat, value, mid)
                slot = HUB.push(update, chat)
                HUB.wait(slot, args.timeout)
                end = settle(args.log)
                calls = remna_delta(before_calls, remna_stats(args.remna))
                ms = (slot["done"] - slot["sent"]) * 1000 if slot["done"] and slot["sent"] else None
                record("бот", label, read_queries(args.log, start, end), calls, ms)
    if args.web_users:
        for i, (cid, sid, tg) in enumerate(pick_web_clients(args.dsn, args.web_users)):
            for channel in ("cabinet", "miniapp"):
                api = Http(args.base, client_token(args.dsn, cid, channel))
                name = "кабинет" if channel == "cabinet" else "мини-приложение"
                if channel == "miniapp" and args.bot_token:
                    saved = api.token
                    api.token = None
                    http_action(api, name, "вход", "POST", "/api/client/auth/miniapp", {"init_data": init_data(args.bot_token, tg)})
                    api.token = saved
                for label, method, path, body in WEB_FLOW:
                    http_action(api, name, label.format(ch=name).split(": ", 1)[1], method, path.format(sid=sid), body)
    if args.admins:
        api = Http(args.base, admin_login(args.base, args.email, args.password))
        cid = psql(args.dsn, "SELECT c.id FROM clients c JOIN payments p ON p.client_id = c.id AND p.status = 'paid' GROUP BY c.id HAVING count(*) BETWEEN 4 AND 8 ORDER BY c.id DESC LIMIT 1")[0][0]
        for label, method, path in ADMIN_FLOW:
            http_action(api, "админка", label.split(": ", 1)[1], method, path.format(cid=cid))
    if args.out:
        with open(args.out, "w") as f:
            json.dump(rows, f, ensure_ascii=False, indent=1)


def jobs(args):
    start_telegram(args.tg_port)
    row = psql(args.dsn, """
        SELECT c.id, s.id FROM clients c JOIN client_identities i ON i.client_id = c.id AND i.kind = 'telegram'
        JOIN subscriptions s ON s.client_id = c.id AND s.ended_at IS NULL
        WHERE c.status = 'active' AND c.deleted_at IS NULL AND NOT c.bot_unreachable ORDER BY c.id DESC LIMIT 1""")[0]
    payload = json.dumps({"kind": "notify", "template": "autorenew.reminder", "client_id": int(row[0]),
                          "params": {"amount": {"minor": 19900, "currency": "RUB"}, "balance": {"minor": 0, "currency": "RUB"},
                                     "missing": {"minor": 19900, "currency": "RUB"}, "expire_at": "2030-01-01T00:00:00Z",
                                     "amount_text": "199 ₽", "balance_off": False, "balance_text": "0 ₽",
                                     "subscription_id": int(row[1])}}, ensure_ascii=False)
    ids = []
    until = time.monotonic() + args.duration
    while time.monotonic() < until:
        ids.append(int(psql(args.dsn, f"INSERT INTO jobs (kind, payload) VALUES ('notify', '{payload}'::jsonb) RETURNING id")[0][0]))
        time.sleep(args.probe_every)
    listed = ",".join(map(str, ids))
    deadline = time.monotonic() + args.timeout
    while time.monotonic() < deadline:
        left = int(psql(args.dsn, f"SELECT count(*) FROM jobs WHERE id IN ({listed}) AND done_at IS NULL")[0][0])
        if left == 0:
            break
        time.sleep(1)
    waits = [float(r[0]) for r in psql(args.dsn, f"""
        SELECT extract(epoch FROM done_at - created_at) * 1000 FROM jobs WHERE id IN ({listed}) AND done_at IS NOT NULL""")]
    left = len(ids) - len(waits)
    if waits:
        print(f"задания: {len(waits)} выполнено, не выполнено {left}; ожидание p50 {statistics.median(waits):.0f} мс, "
              f"p95 {percentile(waits, 0.95):.0f} мс, максимум {max(waits):.0f} мс", flush=True)
    else:
        print(f"задания не выполнены: {left}", flush=True)
    if args.out:
        with open(args.out, "w") as f:
            json.dump({"waits": waits, "left": left}, f)


def main():
    parser = argparse.ArgumentParser(description="нагрузка на панель и подсчёт запросов на действие")
    parser.add_argument("mode", choices=["run", "count", "jobs", "broadcast"])
    parser.add_argument("--base", default=os.environ.get("PERF_BASE", "http://127.0.0.1:18124"))
    parser.add_argument("--dsn", default=os.environ.get("PERF_DSN", "postgresql://postgres@127.0.0.1:55432/sn_perf"))
    parser.add_argument("--email", default=os.environ.get("PERF_EMAIL"))
    parser.add_argument("--password", default=os.environ.get("PERF_PASSWORD"))
    parser.add_argument("--remna", default=os.environ.get("PERF_REMNA"))
    parser.add_argument("--tg-port", type=int, default=18125)
    parser.add_argument("--bot-token", default=os.environ.get("PERF_BOT_TOKEN"))
    parser.add_argument("--log", default=None)
    parser.add_argument("--bot-users", type=int, default=0)
    parser.add_argument("--bot-think", type=float, default=1.0)
    parser.add_argument("--web-users", type=int, default=0)
    parser.add_argument("--web-think", type=float, default=1.0)
    parser.add_argument("--admins", type=int, default=0)
    parser.add_argument("--admin-think", type=float, default=0.5)
    parser.add_argument("--duration", type=float, default=60)
    parser.add_argument("--ramp", type=float, default=5)
    parser.add_argument("--timeout", type=float, default=15)
    parser.add_argument("--wait", type=float, default=90)
    parser.add_argument("--probe-every", type=float, default=0.5)
    parser.add_argument("--tg-limit", type=int, default=0)
    parser.add_argument("--photo", default=None)
    parser.add_argument("--audience", default="all")
    parser.add_argument("--pid", type=int, default=0)
    parser.add_argument("--baseline", type=float, default=20)
    parser.add_argument("--settle", type=float, default=0)
    parser.add_argument("--verbose", action="store_true")
    parser.add_argument("--out", default=None)
    args = parser.parse_args()
    if args.mode == "count" and not args.log:
        sys.exit("для count нужен --log: журнал панели в формате json с sqlx::query=debug")
    if args.admins and (not args.email or not args.password):
        sys.exit("для админки нужны --email и --password")
    if args.mode == "run":
        run(args)
    elif args.mode == "jobs":
        jobs(args)
    elif args.mode == "broadcast":
        broadcast(args)
    else:
        count(args)


if __name__ == "__main__":
    main()
