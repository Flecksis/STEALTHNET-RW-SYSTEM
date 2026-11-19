import { NBSP, plural, timeHM } from '../../lib/format';
import { Switch } from '../ui';

export type FloodSettings = {
  enabled: boolean;
  start_interval_secs: number;
  messages_per_minute: number;
  block_after: number;
  block_minutes: number;
  registrations_per_minute: number;
};

export type FloodStats = {
  at: string | null;
  admitted: number;
  dropped: number;
  blocked_new: number;
  blocked_now: number;
  signups: number;
  refused: number;
};

export type FloodForm = {
  enabled: boolean;
  start: string;
  messages: string;
  after: string;
  minutes: string;
  signups: string;
};

type Field = Exclude<keyof FloodForm, 'enabled'>;

const RANGES: Record<Field, [number, number]> = {
  start: [1, 60],
  messages: [5, 600],
  after: [0, 1000],
  minutes: [1, 1440],
  signups: [0, 100000],
};

export function floodForm(settings: FloodSettings): FloodForm {
  return {
    enabled: settings.enabled,
    start: String(settings.start_interval_secs),
    messages: String(settings.messages_per_minute),
    after: String(settings.block_after),
    minutes: String(settings.block_minutes),
    signups: String(settings.registrations_per_minute),
  };
}

function bad(form: FloodForm, field: Field): boolean {
  const raw = form[field];
  const [min, max] = RANGES[field];
  const value = Number(raw);
  return raw === '' || !Number.isInteger(value) || value < min || value > max;
}

export function floodInvalid(form: FloodForm): boolean {
  return (Object.keys(RANGES) as Field[]).some((field) => bad(form, field));
}

export function floodSettings(form: FloodForm): FloodSettings {
  return {
    enabled: form.enabled,
    start_interval_secs: Number(form.start),
    messages_per_minute: Number(form.messages),
    block_after: Number(form.after),
    block_minutes: Number(form.minutes),
    registrations_per_minute: Number(form.signups),
  };
}

export function floodChanged(form: FloodForm, saved: FloodSettings): boolean {
  const now = floodForm(saved);
  return (Object.keys(now) as Array<keyof FloodForm>).some((key) => now[key] !== form[key]);
}

function count(n: number, one: string, few: string, many: string): string {
  return `${n}${NBSP}${plural(n, one, few, many)}`;
}

function statsLine(stats: FloodStats | null): string | null {
  if (!stats?.at) {
    return null;
  }
  const parts = [
    `пропущено ${count(stats.dropped, 'обновление', 'обновления', 'обновлений')}`,
    `заблокировано ${count(stats.blocked_new, 'аккаунт', 'аккаунта', 'аккаунтов')}`,
    `новых клиентов ${stats.signups}`,
  ];
  if (stats.refused > 0) {
    parts.push(`отложено регистраций ${stats.refused}`);
  }
  const blocked = stats.blocked_now > 0 ? ` Сейчас без ответа: ${count(stats.blocked_now, 'аккаунт', 'аккаунта', 'аккаунтов')}.` : '';
  return `За минуту до ${timeHM(stats.at)}: ${parts.join(', ')}.${blocked}`;
}

type Props = {
  form: FloodForm;
  stats: FloodStats | null;
  busy: boolean;
  onChange: (patch: Partial<FloodForm>) => void;
};

export function BotFlood({ form, stats, busy, onChange }: Props) {
  const digits = (field: Field, max: number) => (e: React.ChangeEvent<HTMLInputElement>) =>
    onChange({ [field]: e.target.value.replace(/\D/g, '').slice(0, max) });
  const off = !form.enabled || busy;
  const line = statsLine(stats);
  return (
    <section className="sec" aria-labelledby="bot-flood-h">
      <div className="sec-head">
        <h2 id="bot-flood-h" className="section-title">
          Защита от флуда
        </h2>
        <span className="muted">Поток сообщений от одного аккаунта или наплыв новых не мешает боту отвечать остальным клиентам</span>
      </div>
      <div className="row">
        <span className="rl">Защита</span>
        <div className="rc">
          <span className="gen-line" style={{ minHeight: 40 }}>
            <Switch on={form.enabled} onToggle={() => onChange({ enabled: !form.enabled })} label="Защищать бота от флуда" describedBy="flood-on-hint" disabled={busy} />
            <span>Защищать бота от флуда</span>
          </span>
          <span id="flood-on-hint" className="cap">
            Оплаты, нажатия кнопок и сообщения в поддержку бот разбирает первыми. Лишнее от одного аккаунта пропускает молча
          </span>
          {line ? <span className="cap">{line}</span> : null}
        </div>
      </div>
      <div className="row">
        <label htmlFor="flood-start" className="rl">
          Команда /start
        </label>
        <div className="rc">
          <span className="gen-line">
            <span className="muted">не чаще раза в</span>
            <input id="flood-start" className="fld num" inputMode="numeric" value={form.start} aria-invalid={bad(form, 'start')} onChange={digits('start', 2)} disabled={off} />
            <span className="muted">{plural(Number(form.start) || 0, 'секунду', 'секунды', 'секунд')}</span>
          </span>
          <span className={bad(form, 'start') ? 'err' : 'cap'}>
            {bad(form, 'start') ? 'От 1 до 60 секунд' : 'Тот же /start с той же ссылкой в течение 30 секунд бот не обрабатывает заново'}
          </span>
        </div>
      </div>
      <div className="row">
        <label htmlFor="flood-messages" className="rl">
          Сообщений от аккаунта
        </label>
        <div className="rc">
          <span className="gen-line">
            <input id="flood-messages" className="fld num" inputMode="numeric" value={form.messages} aria-invalid={bad(form, 'messages')} onChange={digits('messages', 3)} disabled={off} />
            <span className="muted">в минуту</span>
          </span>
          <span className={bad(form, 'messages') ? 'err' : 'cap'}>
            {bad(form, 'messages') ? 'От 5 до 600 в минуту' : 'Нажатий кнопок можно вдвое больше. Короткий всплеск бот пропускает'}
          </span>
        </div>
      </div>
      <div className="row">
        <label htmlFor="flood-after" className="rl">
          Временная блокировка
        </label>
        <div className="rc">
          <span className="gen-line" style={{ flexWrap: 'wrap' }}>
            <span className="muted">после</span>
            <input id="flood-after" className="fld num" inputMode="numeric" value={form.after} aria-invalid={bad(form, 'after')} onChange={digits('after', 4)} disabled={off} />
            <span className="muted">лишних сообщений за минуту — на</span>
            <input className="fld num" inputMode="numeric" value={form.minutes} aria-label="На сколько минут блокировать" aria-invalid={bad(form, 'minutes')} onChange={digits('minutes', 4)} disabled={off} />
            <span className="muted">{plural(Number(form.minutes) || 0, 'минуту', 'минуты', 'минут')}</span>
          </span>
          <span className={bad(form, 'after') || bad(form, 'minutes') ? 'err' : 'cap'}>
            {bad(form, 'after')
              ? 'Лишних сообщений — от 0 до 1000, 0 — не блокировать'
              : bad(form, 'minutes')
                ? 'Блокировка — от 1 минуты до суток (1440)'
                : 'Аккаунт получит одно предупреждение, администраторы — уведомление в тему «Бот». Оплаты проходят и во время блокировки'}
          </span>
        </div>
      </div>
      <div className="row">
        <label htmlFor="flood-signups" className="rl">
          Новых клиентов
        </label>
        <div className="rc">
          <span className="gen-line">
            <input id="flood-signups" className="fld num" inputMode="numeric" value={form.signups} aria-invalid={bad(form, 'signups')} onChange={digits('signups', 6)} disabled={off} />
            <span className="muted">в минуту</span>
          </span>
          <span className={bad(form, 'signups') ? 'err' : 'cap'}>
            {bad(form, 'signups')
              ? 'От 0 до 100000, 0 — без предела'
              : 'Сверх предела бот просит вернуться через минуту и не создаёт клиента. Когда за минуту приходит больше половины предела, модуль «Антибот» отмечает новых: пробный период и промо-ссылки — после проверки. 0 — без предела'}
          </span>
        </div>
      </div>
    </section>
  );
}
