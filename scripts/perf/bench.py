import argparse
import hashlib
import http.client
import json
import os
import secrets
import statistics
import subprocess
import sys
import time
import urllib.parse


def psql(dsn, sql):
    out = subprocess.run(["psql", dsn, "-Atq", "-F", "\t", "-c", sql], check=True, capture_output=True, text=True)
    return [line.split("\t") for line in out.stdout.splitlines() if line]


def one(dsn, sql):
    rows = psql(dsn, sql)
    return rows[0][0] if rows and rows[0] else None


class Client:
    def __init__(self, base):
        parsed = urllib.parse.urlparse(base)
        self.host = parsed.hostname
        self.port = parsed.port or 80
        self.conn = None
        self.token = None

    def connect(self):
        self.conn = http.client.HTTPConnection(self.host, self.port, timeout=600)

    def call(self, method, path, body=None):
        if self.conn is None:
            self.connect()
        headers = {"Accept": "application/json", "Origin": f"http://{self.host}:{self.port}"}
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        started = time.perf_counter()
        try:
            self.conn.request(method, path, body=data, headers=headers)
            response = self.conn.getresponse()
            payload = response.read()
        except (http.client.HTTPException, OSError):
            self.connect()
            started = time.perf_counter()
            self.conn.request(method, path, body=data, headers=headers)
            response = self.conn.getresponse()
            payload = response.read()
        elapsed = (time.perf_counter() - started) * 1000
        return response.status, payload, elapsed


def percentile(values, share):
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, int(round(share * (len(ordered) - 1)))))
    return ordered[index]


def samples(dsn):
    s = {}
    s["typical_client"] = one(dsn, """
        SELECT c.id FROM clients c JOIN payments p ON p.client_id = c.id AND p.status = 'paid'
        WHERE c.deleted_at IS NULL GROUP BY c.id HAVING count(*) BETWEEN 4 AND 8 ORDER BY c.id DESC LIMIT 1""")
    s["heavy_client"] = one(dsn, """
        SELECT client_id FROM payments GROUP BY client_id ORDER BY count(*) DESC LIMIT 1""")
    s["fresh_client"] = one(dsn, "SELECT id FROM clients WHERE deleted_at IS NULL ORDER BY id DESC LIMIT 1")
    s["telegram_id"] = one(dsn, "SELECT value FROM client_identities WHERE kind = 'telegram' ORDER BY id DESC LIMIT 1 OFFSET (SELECT count(*) * 5000 / 300000 FROM clients)")
    s["username"] = one(dsn, "SELECT meta ->> 'username' FROM client_identities WHERE kind = 'telegram' AND meta ? 'username' ORDER BY id DESC LIMIT 1 OFFSET (SELECT count(*) * 7000 / 300000 FROM clients)")
    s["email"] = one(dsn, "SELECT value FROM client_identities WHERE kind = 'email' ORDER BY id DESC LIMIT 1 OFFSET (SELECT count(*) * 3000 / 300000 FROM clients)")
    s["short_uuid"] = one(dsn, "SELECT short_uuid FROM subscriptions ORDER BY id DESC LIMIT 1 OFFSET (SELECT count(*) * 9000 / 300000 FROM clients)")
    s["sub_public"] = one(dsn, "SELECT public_id FROM subscriptions ORDER BY id DESC LIMIT 1 OFFSET (SELECT count(*) * 11000 / 300000 FROM clients)")
    s["last_name"] = one(dsn, "SELECT meta ->> 'last_name' FROM client_identities WHERE meta ? 'last_name' ORDER BY id DESC LIMIT 1 OFFSET (SELECT count(*) * 2000 / 300000 FROM clients)")
    s["full_name"] = one(dsn, """
        SELECT (meta ->> 'first_name') || ' ' || (meta ->> 'last_name') FROM client_identities
        WHERE meta ? 'last_name' ORDER BY id DESC LIMIT 1 OFFSET (SELECT count(*) * 4000 / 300000 FROM clients)""")
    s["open_ticket"] = one(dsn, "SELECT id FROM tickets WHERE status = 'open' ORDER BY id DESC LIMIT 1")
    s["payment"] = one(dsn, "SELECT id FROM payments WHERE status = 'paid' ORDER BY id DESC LIMIT 1 OFFSET (SELECT count(*) * 100 / 300000 FROM clients)")
    s["txid"] = one(dsn, "SELECT provider_txid FROM payments WHERE provider_txid IS NOT NULL ORDER BY id DESC LIMIT 1 OFFSET (SELECT count(*) * 300 / 300000 FROM clients)")
    s["webhook"] = one(dsn, "SELECT id FROM webhook_inbox ORDER BY id DESC LIMIT 1 OFFSET (SELECT count(*) * 50 / 300000 FROM clients)")
    s["contest"] = one(dsn, "SELECT contest_id FROM contest_entries GROUP BY contest_id ORDER BY count(*) DESC LIMIT 1")
    s["broadcast"] = one(dsn, "SELECT id FROM broadcasts ORDER BY total DESC LIMIT 1")
    s["referrer"] = one(dsn, "SELECT referrer_id FROM referral_links GROUP BY referrer_id ORDER BY count(*) DESC LIMIT 1")
    s["active_sub_client"] = one(dsn, """
        SELECT s.client_id FROM subscriptions s JOIN clients c ON c.id = s.client_id
        WHERE s.status = 'active' AND s.kind = 'paid' AND s.ended_at IS NULL AND c.status = 'active' AND c.deleted_at IS NULL
        ORDER BY s.id DESC LIMIT 1 OFFSET (SELECT count(*) * 200 / 300000 FROM clients)""")
    return s


def client_token(dsn, client_id, channel):
    token = secrets.token_hex(32)
    digest = hashlib.sha256(token.encode()).hexdigest()
    psql(dsn, f"""INSERT INTO client_sessions (client_id, token_hash, channel, scope, expires_at)
                  VALUES ({int(client_id)}, '\\x{digest}', '{channel}', 'full', now() + interval '1 day')""")
    return token


def admin_scenarios(s, months_ago):
    q = urllib.parse.quote
    yield "Клиенты", "список, первая страница", "GET", "/api/admin/clients?limit=50", None
    yield "Клиенты", "список, страница 50", "GET", "@clients_page50", None
    for segment in ["active", "trial", "no_subscription", "expiring_soon", "never_connected", "expired", "limited", "blocked"]:
        yield "Клиенты", f"сегмент {segment}", "GET", f"/api/admin/clients?limit=50&segment={segment}", None
    yield "Клиенты", "счётчики сегментов", "GET", "/api/admin/clients/segments", None
    yield "Клиенты", "поиск: Telegram ID", "GET", f"/api/admin/clients?limit=50&q={q(s['telegram_id'])}", None
    yield "Клиенты", "поиск: @username", "GET", f"/api/admin/clients?limit=50&q={q('@' + s['username'][:6])}", None
    yield "Клиенты", "поиск: почта", "GET", f"/api/admin/clients?limit=50&q={q(s['email'][:8])}", None
    yield "Клиенты", "поиск: фамилия", "GET", f"/api/admin/clients?limit=50&q={q(s['last_name'])}", None
    yield "Клиенты", "поиск: имя и фамилия", "GET", f"/api/admin/clients?limit=50&q={q(s['full_name'])}", None
    yield "Клиенты", "поиск: ссылка подписки", "GET", f"/api/admin/clients?limit=50&q={q('https://sub.example.com/' + s['short_uuid'])}", None
    yield "Клиенты", "поиск: UUID подписки", "GET", f"/api/admin/clients?limit=50&q={q(s['sub_public'])}", None
    yield "Клиенты", "поиск: нет совпадений", "GET", f"/api/admin/clients?limit=50&q={q('несуществующий')}", None
    yield "Клиенты", "поиск в шапке", "GET", f"/api/admin/clients?limit=8&q={q(s['last_name'][:4])}", None
    yield "Клиенты", "поиск + сегмент active", "GET", f"/api/admin/clients?limit=50&segment=active&q={q(s['last_name'])}", None
    for label, key in [("обычный", "typical_client"), ("с большой историей", "heavy_client")]:
        cid = s[key]
        yield "Карточка", f"карточка, {label}", "GET", f"/api/admin/clients/{cid}", None
        yield "Карточка", f"история 5 (компьютер), {label}", "GET", f"/api/admin/clients/{cid}/history?limit=5", None
        yield "Карточка", f"история 3 (телефон), {label}", "GET", f"/api/admin/clients/{cid}/history?limit=3", None
        yield "Карточка", f"история, ещё 20, {label}", "GET", f"@history_more:{cid}", None
    yield "Деньги", "платежи: ожидают", "GET", "/api/admin/payments?limit=30&status=pending", None
    yield "Деньги", "платежи: отменённые", "GET", "/api/admin/payments?limit=30&status=canceled", None
    yield "Деньги", "платежи: оплаченные", "GET", "/api/admin/payments?limit=30&status=paid", None
    yield "Деньги", "платежи: все", "GET", "/api/admin/payments?limit=30", None
    yield "Деньги", "платежи: все, страница 30", "GET", "@payments_page30", None
    yield "Деньги", "платёж по id", "GET", f"/api/admin/payments?limit=30&id={s['payment']}", None
    yield "Деньги", "варианты возврата", "GET", f"/api/admin/payments/{s['payment']}/refund", None
    yield "Деньги", "заказы на проверке", "GET", "/api/admin/orders/review", None
    yield "Деньги", "возвраты: ожидают", "GET", "/api/admin/refunds/pending", None
    yield "Деньги", "возвраты: действия", "GET", "/api/admin/refunds/actions", None
    yield "Деньги", "звёзды на проверке", "GET", "/api/admin/stars/review?limit=20", None
    yield "Деньги", "события на проверке", "GET", "/api/admin/events/review", None
    yield "Журнал платёжек", "все", "GET", "/api/admin/webhooks?limit=30", None
    yield "Журнал платёжек", "платёжка", "GET", "/api/admin/webhooks?limit=30&provider=platega", None
    yield "Журнал платёжек", "итог: сбой", "GET", "/api/admin/webhooks?limit=30&outcome=failed", None
    yield "Журнал платёжек", "требуют внимания", "GET", "/api/admin/webhooks?limit=30&outcome=attention", None
    yield "Журнал платёжек", "за сутки", "GET", "@webhooks_day", None
    yield "Журнал платёжек", "за неделю", "GET", "@webhooks_week", None
    yield "Журнал платёжек", "поиск по номеру платежа", "GET", f"/api/admin/webhooks?limit=30&q={q(s['txid'])}", None
    yield "Журнал платёжек", "поиск: нет совпадений", "GET", f"/api/admin/webhooks?limit=30&q={q('nothing-like-this')}", None
    yield "Журнал платёжек", "карточка уведомления", "GET", f"/api/admin/webhooks/{s['webhook']}", None
    yield "Поддержка", "ждут ответа", "GET", "/api/admin/support/tickets?filter=waiting&limit=50", None
    yield "Поддержка", "отвеченные", "GET", "/api/admin/support/tickets?filter=answered&limit=50", None
    yield "Поддержка", "закрытые", "GET", "/api/admin/support/tickets?filter=closed&limit=50", None
    yield "Поддержка", "поиск в закрытых", "GET", f"/api/admin/support/tickets?filter=closed&limit=50&q={q(s['last_name'])}", None
    yield "Поддержка", "обращение", "GET", f"/api/admin/support/tickets/{s['open_ticket']}", None
    yield "Главная", "dashboard", "GET", "/api/admin/dashboard", None
    for kind in ["highlights", "payments", "support", "nodes"]:
        yield "Главная", f"feed {kind}", "GET", f"/api/admin/feed?kind={kind}&limit=12", None
    yield "Главная", "badges", "GET", "/api/admin/badges", None
    yield "Главная", "последние платежи", "GET", "/api/admin/payments?limit=5", None
    yield "Главная", "обращения ждут", "GET", "/api/admin/support/tickets?filter=waiting&limit=3", None
    yield "Аналитика", "отчёт 12 месяцев", "GET", "/api/admin/m/analytics/report", None
    yield "Аналитика", "отчёт 3 месяца", "GET", "/api/admin/m/analytics/report?months=3", None
    yield "Аналитика", "отчёт за всё время", "GET", "/api/admin/m/analytics/report?all=true", None
    yield "Промокоды", "список", "GET", "/api/admin/m/promo/codes?limit=50", None
    yield "Промокоды", "активные", "GET", "/api/admin/m/promo/codes?limit=50&status=active", None
    yield "Рефералы", "сводка", "GET", "/api/admin/m/referral/summary", None
    yield "Рефералы", "рефереры", "GET", "/api/admin/m/referral/referrers?limit=20", None
    yield "Рефералы", "заявки на вывод", "GET", "/api/admin/m/referral/withdrawals?status=pending&limit=100", None
    yield "Рефералы", "история выводов", "GET", "/api/admin/m/referral/withdrawals?status=paid&limit=20", None
    yield "Рефералы", "клиент-реферер", "GET", f"/api/admin/m/referral/clients/{s['referrer']}", None
    yield "Рассылки", "список", "GET", "/api/admin/m/broadcast/broadcasts?limit=30", None
    yield "Рассылки", "рассылка на 280 тыс.", "GET", f"/api/admin/m/broadcast/broadcasts/{s['broadcast']}", None
    for kind in ["all", "active", "expired", "trial", "no_subscription", "never_connected"]:
        yield "Рассылки", f"охват: {kind}", "POST", "/api/admin/m/broadcast/audience", {"audience": {"kind": kind}}
    yield "Рассылки", "охват: истекает за 3 дня", "POST", "/api/admin/m/broadcast/audience", {"audience": {"kind": "expiring", "days": 3}}
    yield "Рассылки", "охват: тариф", "POST", "/api/admin/m/broadcast/audience", {"audience": {"kind": "tariff", "tariff_id": 2}}
    yield "Рассылки", "авторассылки", "GET", "/api/admin/m/broadcast/auto", None
    yield "Конкурсы", "список", "GET", "/api/admin/m/contests/contests?limit=30", None
    yield "Конкурсы", "конкурс", "GET", f"/api/admin/m/contests/contests/{s['contest']}", None
    yield "Конкурсы", "участники", "GET", f"/api/admin/m/contests/contests/{s['contest']}/entries?limit=50", None
    yield "Выгрузки", "участники конкурса CSV", "GET", f"/api/admin/m/contests/contests/{s['contest']}/entries.csv", None
    yield "Подарки", "список", "GET", "/api/admin/m/gifts/gifts?limit=50", None
    yield "Промо-ссылки", "список", "GET", "/api/admin/m/promolinks/links?limit=50", None
    yield "Рулетка", "вращения", "GET", "/api/admin/m/roulette/spins?limit=50", None
    yield "Защита", "антибот: отметки", "GET", "/api/admin/m/antibot/marks?limit=30", None
    yield "Защита", "антибот: журнал", "GET", "/api/admin/m/antibot/journal?limit=30", None
    yield "Защита", "лимиты: превышения", "GET", "/api/admin/m/limits/exceeds?limit=30", None
    yield "Прочее", "me", "GET", "/api/admin/me", None
    yield "Прочее", "тарифы", "GET", "/api/admin/tariffs", None
    yield "Прочее", "модули", "GET", "/api/admin/modules", None
    yield "Прочее", "система", "GET", "/api/admin/system", None
    yield "Прочее", "предупреждения", "GET", "/api/admin/system/warnings", None


def client_scenarios(channel):
    name = "Кабинет" if channel == "cabinet" else "Мини-приложение"
    yield name, "me", "GET", "/api/client/me", None
    yield name, "подписки", "GET", "/api/client/subscriptions", None
    yield name, "каталог", "GET", "/api/client/catalog", None
    yield name, "история оплат", "GET", "/api/client/payments", None
    yield name, "пополнение", "GET", "/api/client/topup", None
    yield name, "обращения", "GET", "/api/client/support/tickets", None
    yield name, "рефералы", "GET", "/api/client/m/referral/me", None
    yield name, "расчёт цены", "POST", "/api/client/quote", {"tariff_id": 2, "period_id": 5, "extra_devices": 0}


def resolve(api, path):
    if not path.startswith("@"):
        return path
    if path == "@clients_page50":
        after = None
        for _ in range(50):
            status, body, _ = api.call("GET", "/api/admin/clients?limit=50" + (f"&after={after}" if after else ""))
            after = json.loads(body).get("next")
        return f"/api/admin/clients?limit=50&after={after}"
    if path == "@payments_page30":
        after = None
        for _ in range(30):
            status, body, _ = api.call("GET", "/api/admin/payments?limit=30" + (f"&after={after}" if after else ""))
            after = json.loads(body).get("next")
        return f"/api/admin/payments?limit=30&after={after}"
    if path.startswith("@history_more:"):
        cid = path.split(":", 1)[1]
        status, body, _ = api.call("GET", f"/api/admin/clients/{cid}/history?limit=5")
        nxt = json.loads(body).get("next")
        return f"/api/admin/clients/{cid}/history?limit=20" + (f"&after={nxt}" if nxt else "")
    if path in ("@webhooks_day", "@webhooks_week"):
        days = 1 if path == "@webhooks_day" else 7
        since = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() - days * 86400))
        return f"/api/admin/webhooks?limit=30&from={urllib.parse.quote(since)}"
    raise ValueError(path)


def measure(api, scenarios, runs, warmup, only):
    results = []
    for group, name, method, path, body in scenarios:
        if only and only.lower() not in (group + " " + name).lower():
            continue
        try:
            path = resolve(api, path)
        except Exception as err:
            results.append({"group": group, "name": name, "path": path, "error": f"не подготовлен: {err}"})
            print(f"{group:18} {name:40} не подготовлен: {err}", flush=True)
            continue
        timings = []
        status = None
        size = 0
        failure = None
        for i in range(warmup + runs):
            status, payload, elapsed = api.call(method, path, body)
            size = len(payload)
            if status >= 400:
                failure = payload[:300].decode(errors="replace")
                break
            if i >= warmup:
                timings.append(elapsed)
        row = {"group": group, "name": name, "method": method, "path": path, "status": status, "bytes": size}
        if failure:
            row["error"] = failure
            print(f"{group:18} {name:40} HTTP {status}: {failure}", flush=True)
        else:
            row["p50"] = round(statistics.median(timings), 1)
            row["p95"] = round(percentile(timings, 0.95), 1)
            row["max"] = round(max(timings), 1)
            print(f"{group:18} {name:40} p50 {row['p50']:8.1f}  p95 {row['p95']:8.1f}  {size:8d} Б", flush=True)
        results.append(row)
    return results


def heavy(api, dsn, runs, only):
    results = []

    def wanted(group, name):
        return not only or only.lower() in (group + " " + name).lower()

    def record(group, name, timings, size, error=None):
        row = {"group": group, "name": name, "bytes": size}
        if error:
            row["error"] = error
            print(f"{group:18} {name:40} {error}", flush=True)
        elif timings:
            row["p50"] = round(statistics.median(timings), 1)
            row["p95"] = round(percentile(timings, 0.95), 1)
            row["max"] = round(max(timings), 1)
            print(f"{group:18} {name:40} p50 {row['p50']:8.1f}  p95 {row['p95']:8.1f}  {size:8d} Б", flush=True)
        results.append(row)

    if wanted("Аналитика", "полный пересчёт"):
        timings, size, error = [], 0, None
        for _ in range(runs):
            psql(dsn, "DELETE FROM analytics_state")
            status, payload, ms = api.call("GET", "/api/admin/m/analytics/report")
            size = len(payload)
            if status >= 400:
                error = f"HTTP {status}: {payload[:200].decode(errors='replace')}"
                break
            timings.append(ms)
        record("Аналитика", "полный пересчёт", timings, size, error)

    if wanted("Рассылки", "создание и постановка в очередь"):
        created, launched, size, error = [], [], 0, None
        for _ in range(runs):
            status, payload, ms = api.call("POST", "/api/admin/m/broadcast/broadcasts",
                                           {"text": "Проверка скорости рассылки", "audience": {"kind": "all"}})
            if status >= 400:
                error = f"HTTP {status}: {payload[:200].decode(errors='replace')}"
                break
            created.append(ms)
            bid = json.loads(payload)["id"]
            status, payload, ms = api.call("POST", f"/api/admin/m/broadcast/broadcasts/{bid}/send")
            size = len(payload)
            if status >= 400:
                error = f"HTTP {status}: {payload[:200].decode(errors='replace')}"
            else:
                launched.append(ms)
            total = one(dsn, f"SELECT count(*) FROM broadcast_recipients WHERE broadcast_id = {int(bid)}")
            psql(dsn, f"DELETE FROM broadcasts WHERE id = {int(bid)}")
            if error:
                break
        record("Рассылки", "создание черновика", created, 0, error)
        record("Рассылки", f"отправка всем: постановка в очередь {total} получателей" if not error else "отправка всем", launched, size, error)
    return results


def main():
    parser = argparse.ArgumentParser(description="замер скорости API панели на большой базе")
    parser.add_argument("--base", default=os.environ.get("PERF_BASE", "http://127.0.0.1:18124"))
    parser.add_argument("--dsn", default=os.environ.get("PERF_DSN", "postgresql://postgres@127.0.0.1:55432/sn_perf"))
    parser.add_argument("--email", default=os.environ.get("PERF_EMAIL"))
    parser.add_argument("--password", default=os.environ.get("PERF_PASSWORD"))
    parser.add_argument("--runs", type=int, default=15)
    parser.add_argument("--warmup", type=int, default=2)
    parser.add_argument("--only", default=None)
    parser.add_argument("--part", choices=["admin", "client", "all"], default="all")
    parser.add_argument("--out", default=None)
    args = parser.parse_args()
    if not args.email or not args.password:
        sys.exit("нужны --email и --password (или PERF_EMAIL и PERF_PASSWORD)")
    s = samples(args.dsn)
    print(json.dumps(s, ensure_ascii=False), flush=True)
    results = []
    if args.part in ("admin", "all"):
        api = Client(args.base)
        status, body, _ = api.call("POST", "/api/admin/auth/login", {"email": args.email, "password": args.password})
        if status != 200:
            sys.exit(f"вход не удался: {status} {body[:200]!r}")
        api.token = json.loads(body)["token"]
        results += measure(api, admin_scenarios(s, 12), args.runs, args.warmup, args.only)
        results += heavy(api, args.dsn, max(1, min(3, args.runs)), args.only)
    if args.part in ("client", "all"):
        for channel in ["cabinet", "miniapp"]:
            api = Client(args.base)
            api.token = client_token(args.dsn, s["active_sub_client"], channel)
            results += measure(api, client_scenarios(channel), args.runs, args.warmup, args.only)
    if args.out:
        with open(args.out, "w") as f:
            json.dump({"samples": s, "results": results}, f, ensure_ascii=False, indent=1)


if __name__ == "__main__":
    main()
