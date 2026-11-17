import { Fragment, useCallback, useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { plural } from '../../lib/format';
import { Icon } from '../../lib/icons';
import type { Currency } from '../../lib/types';
import { ConfirmDialog } from '../client/dialogs';
import { changedConfig, initialValues, missingRequired, SchemaFields, type FieldValues, type SecretState, type SettingField } from '../SchemaFields';
import { useSession } from '../session';
import { Switch } from '../ui';
import '../form.css';
import { SettingsLayout } from './SettingsLayout';

type Channels = { cabinet: boolean; bot: boolean; miniapp: boolean };

type Provider = {
  id: string;
  title: string;
  kind: string;
  currencies: Currency[];
  enabled: boolean;
  configured: boolean;
  accepts_http_webhooks: boolean;
  supports_recurring: boolean;
  channels?: Channels;
  settings: Record<string, unknown>;
};

const ALL_CHANNELS: Channels = { cabinet: true, bot: true, miniapp: true };

const CHANNELS: Array<[keyof Channels, string]> = [
  ['bot', 'Бот'],
  ['cabinet', 'Кабинет'],
  ['miniapp', 'Мини-приложение'],
];

type Renew = { charge_days_before: number; retry_days: number; enable_for_new: boolean; use_saved_card: boolean };

type Core = { settings: { currency: Currency; public_url: string | null; payment_order: { usd: string[]; rub: string[] } } };

type Work = {
  currency: Currency;
  shown: Record<string, boolean>;
  channels: Record<string, Channels>;
  order: { usd: string[]; rub: string[] };
  charge: string;
  retry: string;
  forNew: boolean;
  savedCard: boolean;
};

const ACCEPTS: Record<string, string> = {
  cryptopay: 'USDT, TON, BTC',
  heleket: 'Криптовалюта',
  lava: 'Карты Visa и Mastercard',
  stars: 'Своя цена в звёздах, задаётся в тарифе',
  gram: 'GRAM переводом на кошелёк TON',
  yookassa: 'Карты, СБП, автопродление',
  platega: 'СБП',
  yoomoney: 'Карты через форму ЮMoney',
  cloudpayments: 'Карты',
  overpay: 'Карты и СБП',
};

const HOOK_KINDS: Record<string, string[]> = { cloudpayments: ['check', 'pay', 'refund'] };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function key(currency: Currency): 'usd' | 'rub' {
  return currency === 'RUB' ? 'rub' : 'usd';
}

function fullOrder(saved: string[], providers: Provider[], currency: Currency): string[] {
  const ids = providers.filter((p) => p.kind !== 'balance' && p.currencies.includes(currency)).map((p) => p.id);
  return [...saved.filter((id) => ids.includes(id)), ...ids.filter((id) => !saved.includes(id))];
}

function daysWord(raw: string): string {
  const n = Number.parseInt(raw, 10);
  return plural(Number.isNaN(n) ? 0 : n, 'день', 'дня', 'дней');
}

function same(a: Work, b: Work): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

export function SettingsPayments() {
  const { reload } = useSession();
  const [providers, setProviders] = useState<Provider[] | null>(null);
  const [publicUrl, setPublicUrl] = useState<string | null>(null);
  const [base, setBase] = useState<Work | null>(null);
  const [work, setWork] = useState<Work | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);
  const [setup, setSetup] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [pay, core, renew] = await Promise.all([
        api<{ currency: Currency; order: string[]; providers: Provider[] }>('/api/admin/settings/payments'),
        api<Core>('/api/admin/settings/core'),
        api<Renew>('/api/admin/settings/autorenew'),
      ]);
      const list = pay.providers;
      const next: Work = {
        currency: pay.currency,
        shown: Object.fromEntries(list.map((p) => [p.id, p.enabled && p.configured])),
        channels: Object.fromEntries(list.map((p) => [p.id, p.channels ?? ALL_CHANNELS])),
        order: { usd: fullOrder(core.settings.payment_order.usd, list, 'USD'), rub: fullOrder(core.settings.payment_order.rub, list, 'RUB') },
        charge: String(renew.charge_days_before),
        retry: String(renew.retry_days),
        forNew: renew.enable_for_new,
        savedCard: renew.use_saved_card,
      };
      setProviders(list);
      setPublicUrl(core.settings.public_url);
      setBase(next);
      setWork(next);
      setLoadError(null);
    } catch (err) {
      setLoadError(failText(err, 'Настройки оплаты не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const dirty = base !== null && work !== null && !same(base, work);

  const edit = (patch: Partial<Work>) => {
    setWork((current) => (current ? { ...current, ...patch } : current));
    setStatus(null);
  };

  const visible = useMemo(() => (providers && work ? providers.filter((p) => p.kind !== 'balance' && p.currencies.includes(work.currency)) : []), [providers, work]);

  if (loadError && !work) {
    return (
      <SettingsLayout>
        <p className="err" role="alert">
          {loadError}
        </p>
      </SettingsLayout>
    );
  }

  if (!providers || !work || !base) {
    return (
      <SettingsLayout>
        <p className="muted">Загружаем настройки оплаты…</p>
      </SettingsLayout>
    );
  }

  const cur = key(work.currency);
  const shownOrder = work.order[cur].filter((id) => work.shown[id]);
  const titleOf = (id: string) => providers.find((p) => p.id === id)?.title ?? id;
  const yookassaOn = work.shown.yookassa === true;

  const move = (id: string, step: -1 | 1) => {
    const i = shownOrder.indexOf(id);
    const other = shownOrder[i + step];
    if (other === undefined) {
      return;
    }
    const full = [...work.order[cur]];
    const a = full.indexOf(id);
    const b = full.indexOf(other);
    full[a] = other;
    full[b] = id;
    edit({ order: { ...work.order, [cur]: full } });
  };

  const digits = (raw: string) => raw.replace(/\D/g, '').slice(0, 2);
  const numberOk = (raw: string) => raw !== '' && Number(raw) <= 30;

  const save = async (confirmed = false) => {
    if (!dirty || busy) {
      return;
    }
    if (!numberOk(work.charge) || !numberOk(work.retry)) {
      setStatus({ text: 'Дни автопродления: число от 0 до 30', bad: true });
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      const savedKey = key(base.currency);
      if (work.currency !== base.currency) {
        if (JSON.stringify(work.order[savedKey]) !== JSON.stringify(base.order[savedKey])) {
          await api('/api/admin/settings/payments', { method: 'PUT', body: { order: work.order[savedKey] } });
        }
        try {
          await api('/api/admin/settings/core', { method: 'PUT', body: { currency: work.currency, confirm_currency_change: confirmed } });
        } catch (err) {
          if (err instanceof ApiError && err.code === 'settings.currency_balances') {
            setConfirm(err.message);
            setBusy(false);
            return;
          }
          throw err;
        }
      }
      if (JSON.stringify(work.order[cur]) !== JSON.stringify(base.order[cur]) || work.currency !== base.currency) {
        await api('/api/admin/settings/payments', { method: 'PUT', body: { order: work.order[cur] } });
      }
      for (const p of providers) {
        const body: Record<string, unknown> = {};
        if (p.configured && work.shown[p.id] !== base.shown[p.id]) {
          body.enabled = work.shown[p.id];
        }
        if (JSON.stringify(work.channels[p.id]) !== JSON.stringify(base.channels[p.id])) {
          body.channels = work.channels[p.id];
        }
        if (Object.keys(body).length > 0) {
          await api(`/api/admin/payments/providers/${p.id}`, { method: 'PUT', body });
        }
      }
      if (work.charge !== base.charge || work.retry !== base.retry || work.forNew !== base.forNew || work.savedCard !== base.savedCard) {
        await api('/api/admin/settings/autorenew', {
          method: 'PUT',
          body: { charge_days_before: Number(work.charge), retry_days: Number(work.retry), enable_for_new: work.forNew, use_saved_card: work.savedCard },
        });
      }
      const currencyChanged = work.currency !== base.currency;
      await load();
      if (currencyChanged) {
        await reload();
      }
      setStatus({ text: 'Сохранено', bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Настройки не сохранились'), bad: true });
      await load().catch(() => undefined);
    } finally {
      setBusy(false);
    }
  };

  const configured = (next: Provider) => {
    setProviders(providers.map((p) => (p.id === next.id ? next : p)));
    const shown = next.enabled && next.configured;
    setBase({ ...base, shown: { ...base.shown, [next.id]: shown }, channels: { ...base.channels, [next.id]: next.channels ?? ALL_CHANNELS } });
    setWork({ ...work, shown: { ...work.shown, [next.id]: shown } });
  };

  const toggleChannel = (id: string, channel: keyof Channels) => {
    const current = work.channels[id] ?? ALL_CHANNELS;
    edit({ channels: { ...work.channels, [id]: { ...current, [channel]: !current[channel] } } });
  };

  const channelLine = (id: string, title: string) => {
    const current = work.channels[id] ?? ALL_CHANNELS;
    const none = !current.bot && !current.cabinet && !current.miniapp;
    return (
      <div className="pay-channels" role="group" aria-label={`Где показывать: ${title}`}>
        <span className="pay-ch-title muted">Где показывать</span>
        {CHANNELS.map(([channel, label]) => (
          <label key={channel} className="pay-check">
            <input type="checkbox" checked={current[channel]} onChange={() => toggleChannel(id, channel)} disabled={busy} />
            {label}
          </label>
        ))}
        {none ? <span className="warn-line">Нигде не показывается</span> : null}
      </div>
    );
  };

  const balance = providers.find((p) => p.kind === 'balance');

  const statusText = status?.text ?? (dirty ? 'Есть несохранённые изменения' : '');

  return (
    <SettingsLayout>
      <section className="sec pay-currency" aria-labelledby="cur-h">
        <div className="pay-currency-text">
          <h2 id="cur-h" className="section-title">
            Валюта сервиса
          </h2>
          <span className="muted">Цены, баланс клиентов и платёжные системы работают в одной валюте, без курсов и конвертации</span>
        </div>
        <div role="group" aria-labelledby="cur-h" className="segs">
          {(
            [
              ['USD', 'Доллары, $'],
              ['RUB', 'Рубли, ₽'],
            ] as const
          ).map(([code, label]) => (
            <button key={code} type="button" aria-pressed={work.currency === code} onClick={() => edit({ currency: code })} disabled={busy}>
              {label}
            </button>
          ))}
        </div>
      </section>

      <section className="sec" aria-labelledby="ps-h">
        <div className="sec-head">
          <h2 id="ps-h" className="section-title">
            Платёжные системы
          </h2>
          <span className="muted">Для валюты «{work.currency === 'RUB' ? 'Рубли' : 'Доллары'}»</span>
          <span className="pay-rule">
            Правила Telegram: в боте и мини-приложении цифровые товары, включая VPN-подписку, продаются только за звёзды. За другие способы оплаты там Telegram может заблокировать бота без
            предупреждения, поэтому карты и криптовалюту безопаснее оставить только в кабинете.
          </span>
        </div>
        {visible.map((p) => {
          const on = work.shown[p.id] === true;
          return (
            <Fragment key={p.id}>
              <div className="pay-row">
                <span className="pay-name">{p.title}</span>
                <span className="pay-what muted">{ACCEPTS[p.id] ?? ''}</span>
                <span className="pay-state" style={{ color: p.configured ? 'var(--ink-2)' : 'var(--warn)' }}>
                  {p.configured ? 'Подключена' : 'Не настроена'}
                </span>
                <span className={p.configured ? 'pay-switch' : 'pay-switch off'}>
                  <Switch on={on} onToggle={() => p.configured && edit({ shown: { ...work.shown, [p.id]: !on } })} label={`Показывать клиентам: ${p.title}`} disabled={!p.configured || busy} />
                </span>
                <button type="button" className="pay-setup hov" aria-expanded={setup === p.id} aria-label={`Настроить: ${p.title}`} onClick={() => setSetup(setup === p.id ? null : p.id)}>
                  Настроить
                </button>
              </div>
              {channelLine(p.id, p.title)}
              {setup === p.id ? <ProviderSetup provider={p} publicUrl={publicUrl} onClose={() => setSetup(null)} onSaved={configured} /> : null}
            </Fragment>
          );
        })}
        {balance ? (
          <>
            <div className="pay-row">
              <span className="pay-name">С баланса</span>
              <span className="pay-what muted">Оплата деньгами, которые клиент положил на баланс</span>
              <span className="pay-state" style={{ color: 'var(--ink-2)' }}>
                Включена
              </span>
            </div>
            {channelLine(balance.id, 'С баланса')}
          </>
        ) : null}
      </section>

      <div className="pay-pair">
        <section className="sec pay-card" aria-labelledby="ord-h">
          <div className="sec-head">
            <h2 id="ord-h" className="section-title">
              Порядок у клиента
            </h2>
            <span className="muted">
              Одинаковый в боте, кабинете и <span style={{ whiteSpace: 'nowrap' }}>мини-приложении</span>
            </span>
          </div>
          {shownOrder.length === 0 ? (
            <p className="pay-empty warn-line">Включите хотя бы одну платёжную систему</p>
          ) : (
            <ol className="pay-order">
              {shownOrder.map((id, i) => (
                <li key={id}>
                  <span className="num pay-pos">{i + 1}</span>
                  <span className="pay-oname">{titleOf(id)}</span>
                  <button type="button" className="pay-arrow" aria-label={`Выше: ${titleOf(id)}`} aria-disabled={i === 0} onClick={() => i > 0 && move(id, -1)} disabled={busy}>
                    <Icon name="arrow-up" size={18} />
                  </button>
                  <button
                    type="button"
                    className="pay-arrow"
                    aria-label={`Ниже: ${titleOf(id)}`}
                    aria-disabled={i === shownOrder.length - 1}
                    onClick={() => i < shownOrder.length - 1 && move(id, 1)}
                    disabled={busy}
                  >
                    <Icon name="arrow-down" size={18} />
                  </button>
                </li>
              ))}
            </ol>
          )}
          <p className="pay-foot cap">Оплата с баланса всегда последняя</p>
        </section>

        <section className="sec pay-card" aria-labelledby="ren-h">
          <div className="sec-head">
            <h2 id="ren-h" className="section-title">
              Автопродление
            </h2>
            <span className="muted">Списывается с баланса клиента</span>
          </div>
          <div className="pay-renew">
            <label className="pay-line">
              <span className="pay-lbl">Списывать за</span>
              <input className="fld num" inputMode="numeric" value={work.charge} onChange={(e) => edit({ charge: digits(e.target.value) })} aria-label="За сколько дней до окончания списывать" disabled={busy} />
              <span className="pay-lbl">{daysWord(work.charge)} до окончания</span>
            </label>
          </div>
          <div className="pay-renew">
            <label className="pay-line">
              <span className="pay-lbl">Повторять попытки</span>
              <input className="fld num" inputMode="numeric" value={work.retry} onChange={(e) => edit({ retry: digits(e.target.value) })} aria-label="Сколько дней повторять попытки" disabled={busy} />
              <span className="pay-lbl">{daysWord(work.retry)}</span>
            </label>
            <span className="cap">Потом автопродление выключится</span>
          </div>
          <div className="pay-renew pay-toggle">
            <span className="sf-text">
              <span className="pay-lbl">Включать новым клиентам</span>
              <span className="cap">Клиент может выключить его в кабинете</span>
            </span>
            <Switch on={work.forNew} onToggle={() => edit({ forNew: !work.forNew })} label="Включать автопродление новым клиентам" disabled={busy} />
          </div>
          {work.currency === 'RUB' ? (
            <div className="pay-renew pay-toggle">
              <span className="sf-text">
                <span className="pay-lbl">Списывать с сохранённой карты через ЮKassa</span>
                {yookassaOn ? <span className="cap">Если на балансе не хватает</span> : <span className="warn-line">Работает, когда ЮKassa включена</span>}
              </span>
              <span className={yookassaOn ? 'pay-switch' : 'pay-switch off'}>
                <Switch on={yookassaOn && work.savedCard} onToggle={() => yookassaOn && edit({ savedCard: !work.savedCard })} label="Списывать с сохранённой карты через ЮKassa" disabled={!yookassaOn || busy} />
              </span>
            </div>
          ) : null}
        </section>
      </div>

      <div className="save-row">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button
          type="button"
          className="btn-line hov"
          disabled={busy}
          onClick={() => {
            setWork(base);
            setStatus(null);
          }}
        >
          Отменить
        </button>
        <span role="status" className={status?.bad ? 'bad' : undefined}>
          {statusText}
          {status?.bad && status.text.startsWith('Сначала задайте цены') ? (
            <>
              {' '}
              <Link to="/admin/tariffs" className="hm-link">
                Открыть тарифы
              </Link>
            </>
          ) : null}
        </span>
      </div>

      {confirm ? (
        <ConfirmDialog
          title="Сменить валюту?"
          text={confirm}
          confirm="Сменить валюту"
          run={async () => {
            setConfirm(null);
            await save(true);
          }}
          onClose={() => setConfirm(null)}
        />
      ) : null}
    </SettingsLayout>
  );
}

type SetupProps = { provider: Provider; publicUrl: string | null; onClose: () => void; onSaved: (provider: Provider) => void };

function ProviderSetup({ provider, publicUrl, onClose, onSaved }: SetupProps) {
  const [schema, setSchema] = useState<SettingField[] | null>(null);
  const [config, setConfig] = useState<Record<string, unknown>>({});
  const [values, setValues] = useState<FieldValues>({});
  const [secrets, setSecrets] = useState<SecretState>({ clear: [], replacing: [] });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  useEffect(() => {
    api<{ provider: Provider; schema: SettingField[] }>(`/api/admin/payments/providers/${provider.id}`)
      .then((d) => {
        setSchema(d.schema);
        setConfig(d.provider.settings);
        setValues(initialValues(d.schema, d.provider.settings));
      })
      .catch((err) => setError(failText(err, 'Настройки системы не загрузились')));
  }, [provider.id]);

  const origin = (publicUrl ?? window.location.origin).replace(/\/+$/, '');
  const hooks = provider.accepts_http_webhooks
    ? (HOOK_KINDS[provider.id] ?? ['']).map((kind) => ({ kind, url: `${origin}/api/webhooks/pay/${provider.id}${kind ? `/${kind}` : ''}` }))
    : [];

  const copy = async (url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(url);
      window.setTimeout(() => setCopied(null), 2000);
    } catch {
      setError('Браузер не дал скопировать: выделите адрес вручную');
    }
  };

  const save = async () => {
    if (!schema) {
      return;
    }
    const lacking = missingRequired(schema, config, values, secrets);
    if (lacking.length) {
      setError(`Заполните: ${lacking.map((f) => f.label).join(', ')}`);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const saved = await api<{ provider: Provider; schema: SettingField[] }>(`/api/admin/payments/providers/${provider.id}`, {
        method: 'PUT',
        body: { config: changedConfig(schema, config, values), clear: secrets.clear },
      });
      onSaved(saved.provider);
      onClose();
    } catch (err) {
      setError(failText(err, 'Настройки системы не сохранились'));
      setBusy(false);
    }
  };

  return (
    <div className="pay-setup-panel" role="group" aria-label={`Настройка ${provider.title}`}>
      {schema === null && !error ? <p className="muted">Загружаем…</p> : null}
      {schema && schema.length === 0 ? (
        <p className="muted">
          {provider.kind === 'stars'
            ? 'Звёзды принимает бот сервиса: отдельных ключей не нужно. Система станет «Подключена», когда в настройках будет указан токен бота.'
            : 'У этой системы нет настроек.'}
        </p>
      ) : null}
      {schema && schema.length > 0 ? (
        <SchemaFields schema={schema} config={config} values={values} onChange={setValues} secrets={secrets} onSecrets={setSecrets} busy={busy} idPrefix={`pay-${provider.id}`} />
      ) : null}
      {hooks.length ? (
        <div className="sf-field" style={{ maxWidth: 'none' }}>
          <span className="sf-label">{hooks.length > 1 ? 'Адреса для уведомлений' : 'Адрес для уведомлений'}</span>
          {hooks.map((hook) => (
            <span key={hook.url} className="pay-hook">
              {hook.kind ? <span className="muted">{hook.kind}</span> : null}
              <code className="mono">{hook.url}</code>
              <button type="button" className="b34 hov" onClick={() => void copy(hook.url)}>
                {copied === hook.url ? 'Скопировано' : 'Скопировать'}
              </button>
            </span>
          ))}
          <span className="cap">Укажите в кабинете {provider.title}, чтобы оплаты засчитывались сразу</span>
        </div>
      ) : null}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="cc-btns" style={{ gap: 10 }}>
        {schema && schema.length > 0 ? (
          <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
            {busy ? 'Сохраняем…' : 'Сохранить'}
          </button>
        ) : null}
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy} style={{ padding: '0 16px' }}>
          {schema && schema.length > 0 ? 'Отменить' : 'Закрыть'}
        </button>
      </div>
    </div>
  );
}
