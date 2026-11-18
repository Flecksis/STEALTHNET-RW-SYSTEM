import argparse
import json
import statistics
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ANSWERS = {"sendMessage", "editMessageText", "sendPhoto", "editMessageMedia", "editMessageCaption", "sendDocument"}


class Hub:
    def __init__(self):
        self.lock = threading.Condition()
        self.queue = []
        self.next_id = 1
        self.message_id = 1000
        self.delivered = {}
        self.answered = {}
        self.polls = 0

    def push(self, update, chat):
        with self.lock:
            update["update_id"] = self.next_id
            self.next_id += 1
            self.queue.append((update, chat))
            self.answered.pop(chat, None)
            self.lock.notify_all()

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
            for update, chat in batch:
                self.delivered[chat] = now
            return [u for u, _ in batch]

    def answer(self, chat):
        with self.lock:
            if chat in self.delivered and chat not in self.answered:
                self.answered[chat] = time.perf_counter() - self.delivered[chat]
                self.lock.notify_all()

    def wait_answer(self, chat, timeout=30):
        deadline = time.monotonic() + timeout
        with self.lock:
            while chat not in self.answered:
                left = deadline - time.monotonic()
                if left <= 0:
                    return None
                self.lock.wait(left)
            return self.answered[chat]


HUB = Hub()


def message(chat, text, mid):
    msg = {"message_id": mid, "date": int(time.time()), "chat": {"id": chat, "type": "private"},
           "from": {"id": chat, "is_bot": False, "first_name": "Perf", "language_code": "ru"}, "text": text}
    if text.startswith("/"):
        msg["entities"] = [{"type": "bot_command", "offset": 0, "length": len(text.split()[0])}]
    return msg


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, result):
        body = json.dumps({"ok": True, "result": result}).encode()
        self.send_response(200)
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
        try:
            params = json.loads(raw) if raw and raw[:1] in (b"{", b"[") else {}
        except ValueError:
            params = {}
        if method == "getMe":
            return self.reply({"id": 999000, "is_bot": True, "first_name": "Perf", "username": "perf_bench_bot"})
        if method == "getWebhookInfo":
            return self.reply({"url": "", "pending_update_count": 0})
        if method == "getUpdates":
            return self.reply(HUB.take(min(float(params.get("timeout", 1)), 2.0)))
        if method in ANSWERS:
            chat = params.get("chat_id")
            if chat is not None:
                HUB.answer(int(chat))
            with HUB.lock:
                HUB.message_id += 1
                mid = HUB.message_id
            return self.reply({"message_id": mid, "date": int(time.time()), "chat": {"id": int(chat or 0), "type": "private"}, "text": ""})
        if method == "getUserProfilePhotos":
            return self.reply({"total_count": 0, "photos": []})
        if method == "getChat":
            return self.reply({"id": int(params.get("chat_id") or 0), "type": "private"})
        return self.reply(True)


def psql(dsn, sql):
    out = subprocess.run(["psql", dsn, "-Atq", "-c", sql], check=True, capture_output=True, text=True)
    return [line for line in out.stdout.splitlines() if line]


def run_scenarios(dsn, count):
    chats = [int(v) for v in psql(dsn, f"""
        SELECT i.value FROM client_identities i JOIN clients c ON c.id = i.client_id
        JOIN subscriptions s ON s.client_id = c.id AND s.ended_at IS NULL AND s.kind = 'paid'
        WHERE i.kind = 'telegram' AND c.status = 'active' AND c.deleted_at IS NULL AND NOT c.bot_unreachable
        ORDER BY c.id DESC LIMIT {count}""")]
    fresh = [9100000000 + i for i in range(count)]
    results = {}

    def step(label, chat, update):
        HUB.push(update, chat)
        took = HUB.wait_answer(chat)
        results.setdefault(label, []).append(took)

    for i, chat in enumerate(chats):
        step("/start, клиент с подпиской", chat, {"message": message(chat, "/start", 10 * i + 1)})
        for data in ["buy", "t:2", "p:5"]:
            cb = {"id": f"cb{chat}{data}", "from": {"id": chat, "is_bot": False, "first_name": "Perf", "language_code": "ru"},
                  "message": message(chat, "меню", 10 * i + 2), "chat_instance": "perf", "data": data}
            step(f"кнопка {data}", chat, {"callback_query": cb})
    for i, chat in enumerate(fresh):
        step("/start, новый пользователь", chat, {"message": message(chat, "/start", 1)})
    return results


def main():
    parser = argparse.ArgumentParser(description="замер ответов бота через подставной Bot API")
    parser.add_argument("--port", type=int, default=18125)
    parser.add_argument("--dsn", default="postgresql://postgres@127.0.0.1:55432/sn_perf")
    parser.add_argument("--count", type=int, default=30)
    parser.add_argument("--wait", type=float, default=90)
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    deadline = time.monotonic() + args.wait
    while HUB.polls < 2 and time.monotonic() < deadline:
        time.sleep(0.2)
    if HUB.polls < 2:
        raise SystemExit("бот не начал опрашивать getUpdates")
    results = run_scenarios(args.dsn, args.count)
    for label, values in results.items():
        done = [v * 1000 for v in values if v is not None]
        lost = len(values) - len(done)
        if not done:
            print(f"{label:32} нет ответов ({lost})")
            continue
        done.sort()
        p95 = done[min(len(done) - 1, int(round(0.95 * (len(done) - 1))))]
        print(f"{label:32} p50 {statistics.median(done):7.1f}  p95 {p95:7.1f}  ответов {len(done)}, без ответа {lost}")
    server.shutdown()


if __name__ == "__main__":
    main()
