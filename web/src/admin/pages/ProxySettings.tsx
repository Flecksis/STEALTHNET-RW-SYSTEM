import { useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { timeHM } from '../../lib/format';
import { Switch } from '../ui';
import './proxy.css';

export type ProxyRoute = 'telegram' | 'payments' | 'ai' | 'mail' | 'updates' | 'captcha';

type Health = {
  state: 'unknown' | 'up' | 'down';
  target: string | null;
  problem: string | null;
  since: string | null;
  checked_at: string | null;
  millis: number | null;
};

type EnvView = { server: string; username: string | null; password_set: boolean; routes: ProxyRoute[]; providers: string[] };

type PaymentOption = { id: string; title: string; enabled: boolean };

export type ProxyView = {
  source: 'none' | 'panel' | 'env';
  enabled: boolean;
  server: string | null;
  username: string | null;
  password_set: boolean;
  routes: ProxyRoute[];
  providers: string[];
  env: EnvView | null;
  secret_key: boolean;
  payment_options: PaymentOption[];
  telegram_api: string;
  health: Health;
};

type CheckItem = { route: ProxyRoute; title: string; host: string; ok: boolean; direct: boolean; millis: number | null; text: string };

type CheckView = { ok: boolean; proxy: string; items: CheckItem[] };

export type ProxyForm = {
  enabled: boolean;
  server: string;
  username: string;
  password: string;
  replacing: boolean;
  clear: boolean;
  routes: ProxyRoute[];
  chosen: boolean;
  providers: string[];
};

const ROUTES: Array<{ key: ProxyRoute; title: string; about: (view: ProxyView) => string }> = [
  {
    key: 'telegram',
    title: 'Telegram',
    about: (view) => `Бот, уведомления клиентам и администраторам, рассылки, копии в Telegram и оплата звёздами. Bot API: ${view.telegram_api}`,
  },
  { key: 'payments', title: 'Платёжки', about: () => 'Счета, проверки оплат и возвраты через API платёжных систем' },
  { key: 'ai', title: 'ИИ-чат поддержки', about: () => 'Запросы к OpenAI, Anthropic, Gemini и другим провайдерам из модуля «ИИ-чат поддержки»' },
  { key: 'mail', title: 'Почта через Resend', about: () => 'Только Resend. Письма по SMTP идут напрямую' },
  {
    key: 'updates',
    title: 'Проверка обновлений',
    about: () => 'Проверка новой версии на ghcr.io. Само обновление скачивает Docker на сервере, ему прокси задаётся в настройках Docker',
  },
  {
    key: 'captcha',
    title: 'Капча антибота',
    about: () => 'Проверка ответов капчи у Cloudflare или Яндекса. Сама капча открывается в браузере клиента, прокси её не касается',
  },
];

const SERVER = /^(socks5h?|https?):\/\/[^\s/?#]+\/?$/i;

function clean(server: string): string {
  return server.trim().replace(/\/$/, '').toLowerCase();
}

export function proxyForm(view: ProxyView): ProxyForm {
  return {
    enabled: view.enabled,
    server: view.server ?? '',
    username: view.username ?? '',
    password: '',
    replacing: false,
    clear: false,
    routes: [...view.routes],
    chosen: view.providers.length > 0,
    providers: [...view.providers],
  };
}

function sameList(a: string[], b: string[]): boolean {
  return a.length === b.length && [...a].sort().join(',') === [...b].sort().join(',');
}

export function proxyChanged(form: ProxyForm | null, view: ProxyView | null): boolean {
  if (!form || !view) {
    return false;
  }
  const start = proxyForm(view);
  return (
    form.enabled !== start.enabled ||
    clean(form.server) !== clean(start.server) ||
    form.username.trim() !== start.username.trim() ||
    form.password !== '' ||
    form.clear ||
    !sameList(form.routes, start.routes) ||
    !sameList(form.chosen ? form.providers : [], start.providers)
  );
}

function moved(form: ProxyForm, view: ProxyView): boolean {
  return view.source === 'panel' && view.password_set && clean(form.server) !== clean(view.server ?? '');
}

export function proxyProblem(form: ProxyForm | null): string | null {
  if (!form) {
    return null;
  }
  if (form.server.trim() !== '' && !SERVER.test(form.server.trim())) {
    return 'Адрес прокси начинается с socks5://, socks5h://, http:// или https:// и не содержит пути';
  }
  if (form.server.includes('@')) {
    return 'Логин и пароль укажите в отдельных полях, а не в адресе';
  }
  if (!form.enabled) {
    return null;
  }
  if (form.server.trim() === '') {
    return 'Укажите адрес прокси';
  }
  if (form.routes.length === 0) {
    return 'Отметьте, что пойдёт через прокси';
  }
  if (form.password !== '' && form.username.trim() === '') {
    return 'Для пароля прокси нужен и логин';
  }
  if (form.routes.includes('payments') && form.chosen && form.providers.length === 0) {
    return 'Отметьте платёжки, которые пойдут через прокси';
  }
  return null;
}

function body(form: ProxyForm): Record<string, unknown> {
  const out: Record<string, unknown> = {
    enabled: form.enabled,
    server: form.server.trim(),
    username: form.username.trim(),
    routes: form.routes,
    providers: form.routes.includes('payments') && form.chosen ? form.providers : [],
  };
  if (form.password !== '') {
    out.password = form.password;
  } else if (form.clear) {
    out.clear_password = true;
  }
  return out;
}

export async function saveProxy(form: ProxyForm): Promise<ProxyView> {
  return api<ProxyView>('/api/admin/settings/proxy', { method: 'PUT', body: body(form) });
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function healthLine(view: ProxyView, edited: boolean): { text: string; tone: 'ok' | 'bad' | 'muted' } {
  const active = view.source !== 'none' && view.enabled && view.routes.length > 0;
  if (!active) {
    return { text: 'Прокси не используется: все запросы идут напрямую', tone: 'muted' };
  }
  if (edited) {
    return { text: 'Сохраните изменения, и панель начнёт проверять новый прокси раз в минуту', tone: 'muted' };
  }
  const h = view.health;
  if (h.state === 'down') {
    const since = h.since ? `. Так с ${timeHM(h.since)}` : '';
    return { text: `${h.problem ?? 'Прокси не отвечает'}${since}`, tone: 'bad' };
  }
  if (h.state === 'up') {
    const at = h.checked_at ? `, проверено в ${timeHM(h.checked_at)}` : '';
    return { text: `Работает: ${h.target ?? 'сервис'} отвечает через прокси${at}`, tone: 'ok' };
  }
  if (h.problem) {
    return { text: `Последняя проверка не прошла: ${h.problem}`, tone: 'bad' };
  }
  return { text: 'Ещё не проверялся: панель проверяет прокси раз в минуту, а кнопка ниже — сразу', tone: 'muted' };
}

type Props = {
  view: ProxyView;
  form: ProxyForm;
  onChange: (form: ProxyForm) => void;
  onReset: () => Promise<void>;
  busy: boolean;
};

export function ProxySection({ view, form, onChange, onReset, busy }: Props) {
  const [check, setCheck] = useState<CheckView | null>(null);
  const [checking, setChecking] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const edited = proxyChanged(form, view);
  const problem = proxyProblem(form);
  const edit = (patch: Partial<ProxyForm>) => {
    onChange({ ...form, ...patch });
    setCheck(null);
    setNote(null);
  };
  const toggleRoute = (key: ProxyRoute) => {
    edit({ routes: form.routes.includes(key) ? form.routes.filter((r) => r !== key) : [...form.routes, key] });
  };
  const toggleProvider = (id: string) => {
    edit({ providers: form.providers.includes(id) ? form.providers.filter((p) => p !== id) : [...form.providers, id] });
  };
  const keepsPassword = view.password_set && !form.replacing && !form.clear && !moved(form, view) && view.source === 'panel';
  const health = healthLine(view, edited);
  const options = [...view.payment_options].sort((a, b) => Number(b.enabled) - Number(a.enabled));

  const run = async () => {
    if (problem && edited) {
      setNote(problem);
      return;
    }
    setChecking(true);
    setNote(null);
    setCheck(null);
    try {
      const request = edited || view.source === 'none' ? body(form) : { current: true };
      setCheck(await api<CheckView>('/api/admin/settings/proxy/check', { method: 'POST', body: request }));
    } catch (err) {
      setNote(failText(err, 'Проверка не запустилась'));
    } finally {
      setChecking(false);
    }
  };

  return (
    <section className="sec" id="proxy" aria-labelledby="proxy-h">
      <div className="sec-head">
        <h2 id="proxy-h" className="section-title">
          Исходящий прокси
        </h2>
        <span className="muted">
          Через него панель обращается к Telegram, платёжкам, ИИ и другим сервисам, если с сервера они недоступны. Remnawave и внешние модули всегда идут напрямую, входящие уведомления платёжек и вебхук бота прокси не касаются
        </span>
      </div>
      {view.source === 'env' && !edited ? (
        <div className="row">
          <span className="rl">Откуда настройки</span>
          <div className="rc">
            <span className="proxy-source">Из deploy/.env (SN_PROXY_URL)</span>
            <span className="cap">Чтобы задать прокси в панели, измените поля ниже и сохраните: настройки панели заменят .env, пока вы их не сбросите</span>
          </div>
        </div>
      ) : null}
      {view.source === 'panel' && view.env ? (
        <div className="row">
          <span className="rl">Откуда настройки</span>
          <div className="rc">
            <span className="proxy-source">Из панели</span>
            <span className="cap">В deploy/.env задан другой прокси, {view.env.server}: он не действует, пока в панели есть свои настройки</span>
            <span>
              <button type="button" className="b34 hov" onClick={() => void onReset()} disabled={busy || checking}>
                Вернуть настройки из .env
              </button>
            </span>
          </div>
        </div>
      ) : null}
      <div className="row">
        <span className="rl">Прокси</span>
        <div className="rc">
          <span className="gen-line" style={{ minHeight: 40 }}>
            <Switch on={form.enabled} onToggle={() => edit({ enabled: !form.enabled })} label="Использовать прокси" disabled={busy} />
            <span>{form.enabled ? 'Включён' : 'Выключен'}</span>
          </span>
        </div>
      </div>
      <div className="row">
        <label htmlFor="proxy-server" className="rl">
          Адрес
        </label>
        <div className="rc">
          <input
            id="proxy-server"
            className="field mono"
            value={form.server}
            placeholder="socks5://203.0.113.5:1080"
            autoComplete="off"
            spellCheck={false}
            aria-describedby="proxy-server-hint"
            onChange={(e) => edit({ server: e.target.value })}
            disabled={busy}
            style={{ maxWidth: 420 }}
          />
          <span id="proxy-server-hint" className="cap">
            socks5:// — имена сайтов узнаёт сервер панели, socks5h:// — сам прокси, http:// и https:// — HTTP-прокси. Если прокси запущен на этом же сервере, из контейнера он виден по адресу шлюза Docker, обычно 172.17.0.1
          </span>
        </div>
      </div>
      <div className="row">
        <label htmlFor="proxy-user" className="rl">
          Логин
        </label>
        <div className="rc">
          <input
            id="proxy-user"
            className="field mono"
            value={form.username}
            autoComplete="off"
            spellCheck={false}
            placeholder="Если прокси без логина, оставьте пустым"
            onChange={(e) => edit({ username: e.target.value })}
            disabled={busy}
            style={{ maxWidth: 420 }}
          />
        </div>
      </div>
      <div className="row">
        <label htmlFor="proxy-pass" className="rl">
          Пароль
        </label>
        <div className="rc">
          {keepsPassword ? (
            <span className="sf-secret">
              <span className="muted">Сохранён</span>
              <button type="button" className="b34 hov" onClick={() => edit({ replacing: true })} disabled={busy}>
                Заменить
              </button>
              <button type="button" className="b34 hov" onClick={() => edit({ clear: true })} disabled={busy}>
                Удалить
              </button>
            </span>
          ) : (
            <input
              id="proxy-pass"
              className="field mono"
              type="password"
              autoComplete="new-password"
              spellCheck={false}
              value={form.password}
              placeholder={form.clear ? 'Пароль удалится при сохранении' : 'Если прокси без пароля, оставьте пустым'}
              onChange={(e) => edit({ password: e.target.value, clear: false })}
              disabled={busy}
              style={{ maxWidth: 420 }}
            />
          )}
          <span className="cap">
            {!view.secret_key
              ? 'Не задан ключ шифрования SN_SECRET_KEY: пароль сохранить негде'
              : moved(form, view) && form.password === ''
                ? 'Адрес сменился: сохранённый пароль удалится, введите его заново'
                : view.source === 'env' && view.env?.password_set && !edited
                  ? 'Пароль задан в deploy/.env. Если перенесёте прокси в панель, введите его здесь'
                  : 'Хранится зашифрованным и в админке не показывается'}
          </span>
        </div>
      </div>
      <div className="row">
        <span className="rl">Через прокси</span>
        <div className="rc proxy-routes">
          {ROUTES.map((route) => (
            <div key={route.key} className="proxy-route">
              <label className="proxy-check">
                <input type="checkbox" checked={form.routes.includes(route.key)} onChange={() => toggleRoute(route.key)} disabled={busy} />
                <span>{route.title}</span>
              </label>
              <span className="cap">{route.about(view)}</span>
              {route.key === 'payments' && form.routes.includes('payments') ? (
                <div className="proxy-pay">
                  <span role="group" aria-label="Какие платёжки идут через прокси" className="segs">
                    <button type="button" aria-pressed={!form.chosen} onClick={() => edit({ chosen: false })} disabled={busy}>
                      Все
                    </button>
                    <button type="button" aria-pressed={form.chosen} onClick={() => edit({ chosen: true })} disabled={busy}>
                      Выбранные
                    </button>
                  </span>
                  {form.chosen ? (
                    <span className="proxy-list">
                      {options.map((option) => (
                        <label key={option.id} className="proxy-check">
                          <input type="checkbox" checked={form.providers.includes(option.id)} onChange={() => toggleProvider(option.id)} disabled={busy} />
                          <span>
                            {option.title}
                            {option.enabled ? '' : <span className="muted"> · выключена</span>}
                          </span>
                        </label>
                      ))}
                    </span>
                  ) : null}
                  <span className="cap">
                    Платёжка увидит адрес прокси, а не сервера. Если в её кабинете указан разрешённый адрес сервера, впишите туда адрес прокси или оставьте платёжку напрямую
                  </span>
                </div>
              ) : null}
            </div>
          ))}
        </div>
      </div>
      <div className="row">
        <span className="rl">Состояние</span>
        <div className="rc">
          <span className={`proxy-health ${health.tone}`} role="status">
            {health.text}
          </span>
          <span>
            <button type="button" className="btn-line hov" onClick={() => void run()} disabled={busy || checking}>
              {checking ? 'Проверяем…' : 'Проверить прокси'}
            </button>
          </span>
          {note ? (
            <span className="err" role="alert">
              {note}
            </span>
          ) : null}
          {check ? (
            <ul className="proxy-results" aria-label={`Проверка ${check.proxy}`}>
              {check.items.map((item, i) => (
                <li key={`${item.route}-${item.host}-${i}`} className={item.direct ? 'muted' : item.ok ? 'ok' : 'bad'}>
                  <span className="proxy-result-title">
                    {item.title}
                    {item.host ? <span className="mono"> {item.host}</span> : null}
                  </span>
                  <span>{item.direct ? item.text : item.ok ? `Работает, ${item.millis ?? 0} мс` : item.text}</span>
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      </div>
    </section>
  );
}
