import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { Switch } from '../ui';
import '../form.css';
import './admins.css';
import { SettingsLayout } from './SettingsLayout';

type Kind = 'support' | 'orders' | 'stars' | 'refunds' | 'withdrawals' | 'payments' | 'traffic' | 'nodes' | 'clients' | 'sales' | 'security';

type KindView = { kind: Kind; title: string; default_topic: string; on: boolean; topic: string | null };

type View = {
  chat_id: number | null;
  title: string | null;
  forum: boolean;
  bot_username: string | null;
  kinds: KindView[];
  test_pending: boolean;
};

type Route = { on: boolean; topic: string };

const ABOUT: Record<Kind, string> = {
  support: 'Новое обращение или новое сообщение клиента в старом',
  orders: 'Подписку не удалось выдать автоматически или платёжная система задержала оплату для проверки',
  stars: 'Оплата звёздами, которую не получилось сопоставить со счётом',
  refunds: 'Оплату заблокированного клиента вернули автоматически, или подписку после возврата нужно поменять вручную',
  withdrawals: 'Клиент попросил вывести заработанное по рефералке',
  payments: 'Спор или чарджбэк, возврат без привязки к оплате, деньги для ручного зачисления',
  traffic: 'Клиент превысил порог трафика за сутки или за неделю',
  nodes: 'Нода не отвечает дольше двух минут. Когда она снова в сети, придёт ещё одно сообщение',
  clients: 'Клиент зарегистрировался в боте, кабинете или мини-приложении. Новые за минуту приходят одним сообщением',
  sales: 'Оплачена покупка, продление, смена тарифа, дополнение или пополнение баланса. Новые за минуту приходят одним сообщением',
  security: 'Вход администратора по коду из приложения закрылся после неверных кодов: 5 за 15 минут или 20 за сутки. Пароль при этом ввели верно',
};

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function routesOf(view: View): Record<string, Route> {
  return Object.fromEntries(view.kinds.map((k) => [k.kind, { on: k.on, topic: k.topic ?? '' }]));
}

export function SettingsAlerts() {
  const [view, setView] = useState<View | null>(null);
  const [routes, setRoutes] = useState<Record<string, Route>>({});
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [link, setLink] = useState<{ link: string; expires_at: string } | null>(null);
  const [ask, setAsk] = useState(false);
  const waiting = useRef<number | null>(null);

  const load = useCallback(async () => {
    try {
      const next = await api<View>('/api/admin/alerts');
      setView(next);
      setRoutes(routesOf(next));
      setError(null);
      return next;
    } catch (err) {
      setError(failText(err, 'Настройки уведомлений не загрузились'));
      return null;
    }
  }, []);

  useEffect(() => {
    void load();
    return () => {
      if (waiting.current) {
        window.clearInterval(waiting.current);
      }
    };
  }, [load]);

  const stopWaiting = () => {
    if (waiting.current) {
      window.clearInterval(waiting.current);
      waiting.current = null;
    }
  };

  const connect = async () => {
    setBusy(true);
    setStatus(null);
    try {
      const res = await api<{ link: string; expires_at: string }>('/api/admin/alerts/link', { method: 'POST', body: {} });
      setLink(res);
      window.open(res.link, '_blank', 'noopener');
      stopWaiting();
      waiting.current = window.setInterval(async () => {
        const next = await api<View>('/api/admin/alerts').catch(() => null);
        if (next?.chat_id) {
          stopWaiting();
          setLink(null);
          setView(next);
          setRoutes(routesOf(next));
          setStatus({ text: `Группа «${next.title ?? 'без названия'}» подключена`, bad: false });
        }
      }, 3000);
    } catch (err) {
      setStatus({ text: failText(err, 'Ссылка для подключения не получена'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const disconnect = async () => {
    setBusy(true);
    try {
      const next = await api<View>('/api/admin/alerts/link', { method: 'DELETE' });
      setView(next);
      setRoutes(routesOf(next));
      setAsk(false);
      setStatus({ text: 'Группа отключена, уведомления больше не отправляются', bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Группа не отключилась'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const test = async () => {
    setBusy(true);
    try {
      await api('/api/admin/alerts/test', { method: 'POST', body: {} });
      setStatus({ text: 'Проверка отправится в группу в течение нескольких секунд', bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Проверку не удалось запросить'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!view) {
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      const next = await api<View>('/api/admin/alerts', {
        method: 'PUT',
        body: { routes: view.kinds.map((k) => ({ kind: k.kind, on: routes[k.kind]?.on ?? k.on, topic: routes[k.kind]?.topic ?? '' })) },
      });
      setView(next);
      setRoutes(routesOf(next));
      setStatus({ text: 'Сохранено', bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Настройки не сохранились'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  if (!view) {
    return (
      <SettingsLayout>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем…</p>
        )}
      </SettingsLayout>
    );
  }

  const linked = view.chat_id !== null;
  const edit = (kind: string, patch: Partial<Route>) => {
    setRoutes({ ...routes, [kind]: { ...(routes[kind] ?? { on: true, topic: '' }), ...patch } });
    setStatus(null);
  };

  return (
    <SettingsLayout>
      <div className="sec-stack">
        <section className="sec" aria-labelledby="alerts-h">
          <div className="sec-head">
            <h2 id="alerts-h" className="section-title">
              Группа для уведомлений
            </h2>
            <span className="muted">Бот пишет администраторам в одну группу Telegram о том, что требует внимания</span>
          </div>
          {linked ? (
            <div className="row adm-row">
              <span className="adm-who">
                <span className="adm-email">{view.title ?? 'Группа без названия'}</span>
                <span className="adm-status">{view.forum ? 'Темы включены: уведомления раскладываются по темам' : 'Темы выключены: все уведомления приходят в общий чат'}</span>
              </span>
              <span className="adm-acts">
                <button type="button" className="b34 hov" onClick={() => void test()} disabled={busy}>
                  Отправить проверку
                </button>
                {ask ? null : (
                  <button type="button" className="b34 hov" onClick={() => setAsk(true)} disabled={busy}>
                    Отключить
                  </button>
                )}
              </span>
            </div>
          ) : (
            <div className="row">
              <span className="rc">
                <ol className="alerts-steps">
                  <li>Создайте группу в Telegram. Чтобы раскладывать уведомления по темам, включите в её настройках «Темы».</li>
                  <li>Нажмите «Подключить группу» и выберите эту группу — Telegram добавит в неё бота{view.bot_username ? ` @${view.bot_username}` : ''}.</li>
                  <li>Сделайте бота администратором группы с правами «Отправка сообщений» и «Управление темами».</li>
                </ol>
                <span>
                  <button type="button" className="btn-main btn" onClick={() => void connect()} disabled={busy || !view.bot_username}>
                    {link ? 'Получить новую ссылку' : 'Подключить группу'}
                  </button>
                </span>
                {!view.bot_username ? <span className="cap">Сначала подключите бота в разделе «Настройки → Бот».</span> : null}
                {link ? (
                  <span className="cap">
                    Ждём, пока бота добавят в группу. Если Telegram не открылся,{' '}
                    <a className="link" href={link.link} target="_blank" rel="noreferrer">
                      откройте ссылку
                    </a>
                    . Она действует 30 минут.
                  </span>
                ) : null}
              </span>
            </div>
          )}
          {ask ? (
            <div className="row">
              <span className="rc">
                <span className="cap">Бот перестанет писать в группу «{view.title ?? 'без названия'}». Из самой группы его можно удалить вручную.</span>
                <span className="adm-new-fields">
                  <button type="button" className="btn-main btn" onClick={() => void disconnect()} disabled={busy}>
                    Отключить группу
                  </button>
                  <button type="button" className="b34 hov" onClick={() => setAsk(false)} disabled={busy}>
                    Отменить
                  </button>
                </span>
              </span>
            </div>
          ) : null}
        </section>

        <section className="sec" aria-labelledby="alerts-kinds-h">
          <div className="sec-head">
            <h2 id="alerts-kinds-h" className="section-title">
              Что присылать
            </h2>
            <span className="muted">
              {view.forum
                ? 'Тема — куда придёт уведомление. Одинаковое название — одна тема на несколько видов, пустое поле — общий чат. Темы бот создаст сам.'
                : 'Пока в группе нет тем, всё приходит в общий чат. Названия тем пригодятся, когда темы включите.'}
            </span>
          </div>
          {view.kinds.map((k) => {
            const route = routes[k.kind] ?? { on: k.on, topic: k.topic ?? '' };
            return (
              <div key={k.kind} className="row adm-row alerts-kind">
                <span className="adm-who">
                  <span className="adm-email">{k.title}</span>
                  <span className="adm-status">{ABOUT[k.kind]}</span>
                </span>
                <span className="adm-acts">
                  <input
                    className="field alerts-topic"
                    aria-label={`Тема для «${k.title}»`}
                    placeholder="Общий чат"
                    maxLength={64}
                    value={route.topic}
                    onChange={(e) => edit(k.kind, { topic: e.target.value })}
                    disabled={busy || !route.on}
                  />
                  <Switch on={route.on} onToggle={() => edit(k.kind, { on: !route.on })} label={k.title} disabled={busy} />
                </span>
              </div>
            );
          })}
        </section>

        <div className="save-row">
          <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
            {busy ? 'Сохраняем…' : 'Сохранить'}
          </button>
          <span role="status" className={status?.bad ? 'bad' : undefined}>
            {status?.text ?? ''}
          </span>
        </div>
      </div>
    </SettingsLayout>
  );
}
