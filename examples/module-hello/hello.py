import base64
import hashlib
import hmac
import html
import json
import os
import threading
import time
import urllib.error
import urllib.request
from collections import OrderedDict, deque
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

PANEL = os.environ.get("SN_PANEL_URL", "http://127.0.0.1:8080").rstrip("/")
API_KEY = os.environ["SN_API_KEY"]
SECRET = os.environ["SN_SIGNING_SECRET"]
HOST = os.environ.get("HELLO_HOST", "127.0.0.1")
PORT = int(os.environ.get("HELLO_PORT", "8090"))
MODULE = "hello"
KEY = base64.b64decode(SECRET.removeprefix("whsec_"))
TOLERANCE = 300

seen = OrderedDict()
recent = deque(maxlen=20)
lock = threading.Lock()


def signature_ok(msg_id, stamp, body, header):
    try:
        if abs(time.time() - int(stamp)) > TOLERANCE:
            return False
    except ValueError:
        return False
    signed = f"{msg_id}.{stamp}.".encode() + body
    expected = "v1," + base64.b64encode(hmac.new(KEY, signed, hashlib.sha256).digest()).decode()
    return any(hmac.compare_digest(expected, part) for part in header.split(" "))


def first_time(msg_id):
    with lock:
        if msg_id in seen:
            return False
        seen[msg_id] = True
        while len(seen) > 5000:
            seen.popitem(last=False)
        return True


def unpad(part):
    return base64.urlsafe_b64decode(part + "=" * (-len(part) % 4))


def page_token(token):
    try:
        head, body, sig = token.split(".")
        mac = hmac.new(KEY, f"{head}.{body}".encode(), hashlib.sha256).digest()
        if not hmac.compare_digest(base64.urlsafe_b64encode(mac).rstrip(b"=").decode(), sig):
            return None
        header, claims = json.loads(unpad(head)), json.loads(unpad(body))
    except (ValueError, AttributeError):
        return None
    if not isinstance(header, dict) or not isinstance(claims, dict):
        return None
    if header.get("alg") != "HS256" or claims.get("iss") != "stealthnet":
        return None
    if claims.get("aud") != f"module:{MODULE}" or claims.get("exp", 0) <= time.time():
        return None
    return claims


def api(method, path, payload=None):
    data = json.dumps(payload).encode() if payload is not None else None
    request = urllib.request.Request(PANEL + path, data=data, method=method)
    request.add_header("Authorization", f"Bearer {API_KEY}")
    request.add_header("Accept", "application/json")
    if data is not None:
        request.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(request, timeout=15) as response:
        return json.loads(response.read() or b"null")


def settings():
    return api("GET", "/api/admin/ext-modules/me")["settings"]


def handle(event):
    kind, data = event["type"], event["data"]
    try:
        if kind == "client_registered":
            text = settings().get("greeting")
            if text:
                api("POST", f"/api/admin/clients/{data['client_id']}/message", {"text": text})
        elif kind == "payment_paid":
            current = settings()
            if current.get("thank_after_payment") and current.get("thanks"):
                api("POST", f"/api/admin/clients/{data['client_id']}/message", {"text": current["thanks"]})
        result = "обработано"
    except urllib.error.HTTPError as err:
        result = f"панель ответила {err.code}"
    except (urllib.error.URLError, TimeoutError) as err:
        result = f"панель недоступна: {err}"
    except (KeyError, TypeError):
        result = "событие без нужных полей"
    with lock:
        recent.appendleft((time.strftime("%d.%m %H:%M:%S"), kind, event["id"], result))


PAGE = """<!doctype html>
<html lang="ru"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Привет: журнал</title>
<style>
body {{ font: 15px/1.5 system-ui, sans-serif; margin: 20px; color: #1d2433; }}
table {{ border-collapse: collapse; width: 100%; }}
td, th {{ text-align: left; padding: 6px 10px; border-top: 1px solid #e3e6ec; }}
.muted {{ color: #6b7385; }}
@media (prefers-color-scheme: dark) {{ body {{ color: #e8ebf2; background: #161a22; }} td, th {{ border-color: #2c3240; }} }}
</style></head><body>
<p>Страницу открыл: <b>{email}</b> <span class="muted">({role})</span></p>
<p class="muted" id="token">Токен действует до {until}</p>
<table><tr><th>Когда</th><th>Событие</th><th>Номер</th><th>Итог</th></tr>{rows}</table>
<script>
setInterval(function () {{ window.parent.postMessage({{ type: 'stealthnet:token' }}, '*'); }}, 240000);
window.addEventListener('message', function (event) {{
  if (event.data && event.data.type === 'stealthnet:token' && event.data.token) {{
    document.getElementById('token').textContent = 'Токен обновлён, действует до ' + new Date(event.data.expires_at).toLocaleTimeString();
  }}
}});
</script>
</body></html>"""


class Handler(BaseHTTPRequestHandler):
    def reply(self, code, body, kind="text/plain; charset=utf-8"):
        raw = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_POST(self):
        if urlparse(self.path).path != "/events":
            return self.reply(404, "нет такого адреса")
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        msg_id = self.headers.get("webhook-id", "")
        stamp = self.headers.get("webhook-timestamp", "")
        header = self.headers.get("webhook-signature", "")
        if not signature_ok(msg_id, stamp, body, header):
            return self.reply(401, "подпись не подошла")
        if not first_time(msg_id):
            return self.reply(200, "уже обработано")
        event = json.loads(body)
        threading.Thread(target=handle, args=(event,), daemon=True).start()
        return self.reply(200, "принято")

    def do_GET(self):
        url = urlparse(self.path)
        if url.path != "/admin":
            return self.reply(404, "нет такого адреса")
        claims = page_token(parse_qs(url.query).get("sn_token", [""])[0])
        if claims is None:
            return self.reply(403, "Откройте страницу из админки панели")
        with lock:
            rows = "".join(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>".format(*map(html.escape, row)) for row in recent
            )
        page = PAGE.format(
            email=html.escape(claims.get("email", "")),
            role=html.escape(claims.get("role", "")),
            until=time.strftime("%H:%M:%S", time.localtime(claims["exp"])),
            rows=rows or '<tr><td colspan="4" class="muted">Событий пока не было</td></tr>',
        )
        return self.reply(200, page, "text/html; charset=utf-8")

    def log_message(self, fmt, *args):
        print(f"{self.address_string()} {fmt % args}")


if __name__ == "__main__":
    print(f"Модуль «Привет» слушает http://{HOST}:{PORT}, панель: {PANEL}")
    ThreadingHTTPServer((HOST, PORT), Handler).serve_forever()
