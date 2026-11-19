import { NBSP, dateTime, plural, timeHM } from '../../lib/format';

export type UpdatesMode = 'polling' | 'webhook';

type Fallback = { at: string; reason: 'rejected' | 'undelivered'; error?: string | null };

type Snapshot = {
  checked_at: string | null;
  pending: number | null;
  last_error_at: string | null;
  last_error: string | null;
  broken_since: string | null;
  registered_at: string | null;
};

export type Updates = {
  mode: UpdatesMode;
  fallback: Fallback | null;
  address: string | null;
  blocker: string | null;
  custom_api: boolean;
  telegram: Snapshot | null;
  queue: { pending: number; oldest_at: string | null; last_received_at: string | null };
};

const MODES: Array<[UpdatesMode, string]> = [
  ['polling', 'Опрос'],
  ['webhook', 'Вебхук'],
];

const DAY_MS = 86_400_000;

function blockerText(code: string): string {
  switch (code) {
    case 'bot.webhook_needs_https':
      return 'Не задан адрес сервиса с https («Настройки → Общие»). Домен задаётся при установке или командой stealthnet domain на сервере';
    case 'bot.webhook_port':
      return 'Telegram отправляет вебхуки только на порты 443, 80, 88 и 8443, а в адресе сервиса другой порт';
    default:
      return 'Не задан ключ шифрования SN_SECRET_KEY, секрет вебхука негде хранить';
  }
}

function modeText(mode: UpdatesMode, customApi: boolean): string {
  if (mode === 'polling') {
    return 'Бот сам спрашивает Telegram о новых сообщениях. Работает на любом сервере, адрес с https и открытые порты не нужны';
  }
  return customApi
    ? 'Ваш сервер Bot API сам присылает сообщения на адрес сервиса, бот не ждёт опроса. Серверу Bot API нужен доступ к этому адресу'
    : 'Telegram сам присылает сообщения на адрес сервиса, бот не ждёт опроса. Лучше для большой базы клиентов. Нужны адрес сервиса с https и открытый порт 443';
}

function minutes(n: number): string {
  return `${n}${NBSP}${plural(n, 'минуту', 'минуты', 'минут')}`;
}

function since(iso: string): number {
  return Math.max(1, Math.round((Date.now() - Date.parse(iso)) / 60_000));
}

type Delivery = { head: string; tone: 'ok' | 'quiet' | 'bad'; meta?: string };

function delivery(telegram: Snapshot | null): Delivery {
  if (!telegram?.checked_at) {
    return { head: 'Ждём первой проверки Telegram', tone: 'quiet' };
  }
  const checked = `проверено в ${timeHM(telegram.checked_at)}`;
  if (telegram.broken_since) {
    return {
      head: `Telegram не может доставить обновления уже ${minutes(since(telegram.broken_since))}`,
      tone: 'bad',
      meta: checked,
    };
  }
  return { head: 'Telegram доставляет обновления', tone: 'ok', meta: checked };
}

function details(updates: Updates): string[] {
  const out: string[] = [];
  const telegram = updates.telegram;
  if (telegram?.last_error_at && Date.now() - Date.parse(telegram.last_error_at) < DAY_MS) {
    const error = telegram.last_error ? `: «${telegram.last_error}»` : '';
    out.push(`Последняя ошибка доставки ${dateTime(telegram.last_error_at).toLowerCase()}${error}`);
  }
  if (telegram?.pending) {
    out.push(`Ждут доставки в Telegram: ${telegram.pending}`);
  }
  if (updates.queue.pending > 0) {
    const from = updates.queue.oldest_at ? `, самое раннее с ${timeHM(updates.queue.oldest_at)}` : '';
    out.push(`Ждут обработки в панели: ${updates.queue.pending}${from}`);
  } else if (updates.queue.last_received_at) {
    out.push(`Последнее обновление пришло ${dateTime(updates.queue.last_received_at).toLowerCase()}`);
  }
  return out;
}

function fallbackText(fallback: Fallback): { head: string; text: string } {
  const error = fallback.error ? ` Ответ Telegram: «${fallback.error}».` : '';
  if (fallback.reason === 'rejected') {
    return {
      head: `Вебхук не включился ${dateTime(fallback.at).toLowerCase()}`,
      text: `Telegram не принял адрес сервиса, бот работает опросом.${error} Проверьте, что адрес открывается снаружи и сертификат действителен, и снова выберите «Вебхук».`,
    };
  }
  return {
    head: `Бот вернулся к опросу ${dateTime(fallback.at).toLowerCase()}`,
    text: `Telegram долго не мог доставить обновления на вебхук, поэтому бот сам переключился на опрос. Сообщения клиентов не потерялись.${error} Когда адрес сервиса снова будет открываться снаружи, выберите «Вебхук».`,
  };
}

type Props = {
  updates: Updates;
  mode: UpdatesMode;
  busy: boolean;
  onMode: (mode: UpdatesMode) => void;
  onDismiss: () => void;
};

export function BotUpdates({ updates, mode, busy, onMode, onDismiss }: Props) {
  const blocked = updates.blocker !== null;
  const saved = updates.mode;
  const shown = delivery(updates.telegram);
  const lines = details(updates);
  const fallback = updates.fallback ? fallbackText(updates.fallback) : null;
  return (
    <section className="sec" aria-labelledby="bot-updates-h">
      <div className="sec-head">
        <h2 id="bot-updates-h" className="section-title">
          Получение обновлений
        </h2>
        <span className="muted">Как сообщения и нажатия кнопок клиентов попадают в панель</span>
      </div>
      <div className="row">
        <span className="rl">Способ</span>
        <div className="rc">
          <span role="group" aria-label="Способ получения обновлений" className="segs">
            {MODES.map(([key, label]) => (
              <button
                key={key}
                type="button"
                aria-pressed={mode === key}
                onClick={() => onMode(key)}
                disabled={busy || (key === 'webhook' && blocked && saved !== 'webhook')}
              >
                {label}
              </button>
            ))}
          </span>
          <span className="cap">{modeText(mode, updates.custom_api)}</span>
          {blocked && saved !== 'webhook' ? <span className="cap">Вебхук сейчас не включить. {blockerText(updates.blocker ?? '')}.</span> : null}
        </div>
      </div>
      {saved === 'webhook' && blocked ? (
        <div className="row">
          <span className="rl">Вебхук</span>
          <div className="rc">
            <span className="bot-state">
              <strong className="bot-warn" aria-live="polite">
                Выбран, но сейчас не работает
              </strong>
            </span>
            <span className="cap">{blockerText(updates.blocker ?? '')}. Пока это так, бот получает обновления опросом.</span>
          </div>
        </div>
      ) : null}
      {saved === 'webhook' && !blocked ? (
        <div className="row">
          <span className="rl">Вебхук</span>
          <div className="rc">
            <span className="bot-state">
              <strong className={`bot-${shown.tone}`} aria-live="polite">
                {shown.head}
              </strong>
              {shown.meta ? <span className="muted">{shown.meta}</span> : null}
            </span>
            {updates.address ? <span className="cap mono bot-hook-url">{updates.address}</span> : null}
            {lines.map((line) => (
              <span key={line} className="cap">
                {line}
              </span>
            ))}
            {updates.telegram?.broken_since ? (
              <span className="cap">Если сбой продлится 10{NBSP}минут, бот сам вернётся к опросу, сообщения не потеряются. Об этом придёт уведомление в группу администраторов, в тему «Бот».</span>
            ) : null}
          </div>
        </div>
      ) : null}
      {fallback && saved === 'polling' ? (
        <div className="row">
          <span className="rl">Вебхук</span>
          <div className="rc">
            <div role="status" className="bot-ask">
              <span>
                <strong>{fallback.head}.</strong> {fallback.text}
              </span>
              <span className="bot-ask-btns">
                <button type="button" className="b34 on-surface" onClick={onDismiss} disabled={busy}>
                  Понятно
                </button>
              </span>
            </div>
          </div>
        </div>
      ) : null}
    </section>
  );
}
