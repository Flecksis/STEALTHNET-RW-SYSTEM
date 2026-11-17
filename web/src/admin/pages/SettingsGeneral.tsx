import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { BrandMark } from '../../lib/brand';
import type { Currency } from '../../lib/types';
import { useSession } from '../session';
import { fromRaw, toRaw } from '../tariffs/model';
import { Switch } from '../ui';
import '../form.css';
import { SettingsLayout } from './SettingsLayout';

type Core = {
  settings: {
    brand_name: string;
    support_url: string | null;
    public_url: string | null;
    invoice_ttl_minutes: number;
    remna_utc_offset_minutes: number;
    currency: Currency;
    topup: { min_minor: number; max_minor: number };
  };
};

type SiteData = { title: string | null; description: string | null; indexing: boolean; title_max: number; description_max: number };

type Work = {
  brand: string;
  support: string;
  ttl: string;
  offset: number;
  min: string;
  max: string;
  title: string;
  desc: string;
  indexing: boolean;
};

function chars(text: string): number {
  return [...text.trim().replace(/\s+/g, ' ')].length;
}

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

const OFFSETS = Array.from({ length: (14 * 60 * 2) / 30 + 1 }, (_, i) => -14 * 60 + i * 30);

function offsetLabel(minutes: number): string {
  const sign = minutes < 0 ? '−' : '+';
  const abs = Math.abs(minutes);
  const h = String(Math.floor(abs / 60)).padStart(2, '0');
  const m = String(abs % 60).padStart(2, '0');
  return `UTC${sign}${h}:${m}`;
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function SettingsGeneral() {
  const { me, reload } = useSession();
  const [core, setCore] = useState<Core['settings'] | null>(null);
  const [limits, setLimits] = useState<{ title: number; desc: number }>({ title: 70, desc: 300 });
  const [base, setBase] = useState<Work | null>(null);
  const [work, setWork] = useState<Work | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const [data, site] = await Promise.all([api<Core>('/api/admin/settings/core'), api<SiteData>('/api/admin/settings/site')]);
      const s = data.settings;
      const next: Work = {
        brand: s.brand_name,
        support: s.support_url ?? '',
        ttl: String(s.invoice_ttl_minutes),
        offset: s.remna_utc_offset_minutes,
        min: toRaw(s.topup.min_minor, s.currency),
        max: toRaw(s.topup.max_minor, s.currency),
        title: site.title ?? '',
        desc: site.description ?? '',
        indexing: site.indexing,
      };
      setLimits({ title: site.title_max, desc: site.description_max });
      setCore(s);
      setBase(next);
      setWork(next);
      setError(null);
    } catch (err) {
      setError(failText(err, 'Настройки не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  if (!core || !work || !base) {
    return (
      <SettingsLayout>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем настройки…</p>
        )}
      </SettingsLayout>
    );
  }

  const currency = core.currency;
  const dirty = JSON.stringify(work) !== JSON.stringify(base);
  const edit = (patch: Partial<Work>) => {
    setWork({ ...work, ...patch });
    setStatus(null);
  };
  const brandBad = work.brand.trim().length === 0;
  const supportBad = work.support.trim() !== '' && !/^(https:\/\/|tg:\/\/)\S+$/.test(work.support.trim());
  const ttl = Number.parseInt(work.ttl, 10);
  const ttlBad = Number.isNaN(ttl) || ttl < 5 || ttl > 24 * 60;
  const min = fromRaw(work.min, currency);
  const max = fromRaw(work.max, currency);
  const topupBad = min === 'bad' || max === 'bad' || min === null || max === null || min <= 0 || max < min;
  const titleLen = chars(work.title);
  const descLen = chars(work.desc);
  const titleBad = titleLen > limits.title;
  const descBad = descLen > limits.desc;
  const siteDirty = work.title !== base.title || work.desc !== base.desc || work.indexing !== base.indexing;

  const save = async () => {
    if (!dirty || busy) {
      return;
    }
    if (brandBad || supportBad || ttlBad || topupBad || titleBad || descBad) {
      setStatus({ text: 'Исправьте поля, отмеченные красным', bad: true });
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      const body: Record<string, unknown> = {};
      if (work.brand !== base.brand) {
        body.brand_name = work.brand.trim();
      }
      if (work.support !== base.support) {
        body.support_url = work.support.trim();
      }
      if (work.ttl !== base.ttl) {
        body.invoice_ttl_minutes = ttl;
      }
      if (work.offset !== base.offset) {
        body.remna_utc_offset_minutes = work.offset;
      }
      if (work.min !== base.min || work.max !== base.max) {
        body.topup = { min_minor: min, max_minor: max };
      }
      if (Object.keys(body).length > 0) {
        await api('/api/admin/settings/core', { method: 'PUT', body });
      }
      if (siteDirty) {
        await api('/api/admin/settings/site', {
          method: 'PUT',
          body: { title: work.title, description: work.desc, indexing: work.indexing },
        });
      }
      await load();
      if (work.brand !== base.brand) {
        await reload();
      }
      setStatus({ text: 'Сохранено', bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Настройки не сохранились'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const unit = currency === 'RUB' ? '₽' : '$';

  return (
    <SettingsLayout>
      <section className="sec" aria-labelledby="gen-h">
        <div className="sec-head">
          <h2 id="gen-h" className="section-title">
            Сервис
          </h2>
          <span className="muted">Так сервис называется в боте, кабинете, мини-приложении и письмах</span>
        </div>
        <div className="row">
          <label htmlFor="gen-brand" className="rl">
            Название
          </label>
          <div className="rc">
            <input id="gen-brand" className="field" maxLength={64} value={work.brand} aria-invalid={brandBad} onChange={(e) => edit({ brand: e.target.value })} disabled={busy} style={{ maxWidth: 360 }} />
            {brandBad ? <span className="err">Название не может быть пустым</span> : null}
          </div>
        </div>
        <div className="row">
          <label htmlFor="gen-support" className="rl">
            Ссылка на поддержку
          </label>
          <div className="rc">
            <input
              id="gen-support"
              className="field"
              maxLength={300}
              value={work.support}
              placeholder="https://t.me/… или tg://resolve?domain=…"
              aria-invalid={supportBad}
              aria-describedby="gen-support-hint"
              onChange={(e) => edit({ support: e.target.value })}
              disabled={busy}
            />
            <span id="gen-support-hint" className={supportBad ? 'err' : 'cap'}>
              {supportBad ? 'Ссылка начинается с https:// или tg://' : 'Пусто — клиенты пишут в поддержку только через бота и кабинет'}
            </span>
          </div>
        </div>
        <div className="row">
          <span className="rl">Адрес сервиса</span>
          <div className="rc">
            <span className="mono" style={{ paddingTop: 10 }}>
              {core.public_url ?? window.location.origin}
            </span>
            <span className="cap">Задаётся при установке вместе с сертификатом</span>
          </div>
        </div>
      </section>

      <section className="sec" aria-labelledby="seo-h">
        <div className="sec-head">
          <h2 id="seo-h" className="section-title">
            Сайт в поиске и в превью ссылок
          </h2>
          <span className="muted">Так главную страницу покажут Яндекс и Google, а ссылку на сайт — Telegram и другие мессенджеры</span>
        </div>
        <div className="row">
          <label htmlFor="seo-title" className="rl">
            Заголовок
          </label>
          <div className="rc">
            <input
              id="seo-title"
              className="field"
              maxLength={limits.title + 30}
              value={work.title}
              placeholder={work.brand.trim() || core.brand_name}
              aria-invalid={titleBad}
              aria-describedby="seo-title-hint"
              onChange={(e) => edit({ title: e.target.value })}
              disabled={busy}
            />
            <span id="seo-title-hint" className="cap">
              <span className={titleBad ? 'seo-count over' : 'seo-count'}>
                {titleLen} из {limits.title}
              </span>
              {' · '}Пусто — будет название сервиса. В поиске видно около 60 символов
            </span>
          </div>
        </div>
        <div className="row">
          <label htmlFor="seo-desc" className="rl">
            Описание
          </label>
          <div className="rc">
            <textarea
              id="seo-desc"
              className="field"
              rows={3}
              maxLength={limits.desc + 60}
              value={work.desc}
              placeholder="Одно-два предложения о сервисе"
              aria-invalid={descBad}
              aria-describedby="seo-desc-hint"
              onChange={(e) => edit({ desc: e.target.value })}
              disabled={busy}
            />
            <span id="seo-desc-hint" className="cap">
              <span className={descBad ? 'seo-count over' : 'seo-count'}>
                {descLen} из {limits.desc}
              </span>
              {' · '}В поиске видно около 160 символов, в Telegram — первые две-три строки
            </span>
          </div>
        </div>
        <div className="row">
          <span className="rl">Поиск</span>
          <div className="rc">
            <span className="gen-line" style={{ minHeight: 40 }}>
              <Switch
                on={work.indexing}
                onToggle={() => edit({ indexing: !work.indexing })}
                label="Показывать главную страницу в поиске"
                describedBy="seo-index-hint"
                disabled={busy}
              />
              <span>Показывать главную страницу в поиске</span>
            </span>
            <span id="seo-index-hint" className="cap">
              Кабинет, мини-приложение и админка в поиск не попадают никогда
            </span>
          </div>
        </div>
        <div className="row">
          <span className="rl">Как это выглядит</span>
          <div className="rc">
            <div className="seo-preview" aria-hidden="true">
              <BrandMark name={work.brand.trim() || core.brand_name} logo={me.logo_url} className="mark" />
              <span className="seo-text">
                <span className="seo-host">{hostOf(core.public_url ?? window.location.origin)}</span>
                <span className="seo-title">{work.title.trim() || work.brand.trim() || core.brand_name}</span>
                <span className={work.desc.trim() ? 'seo-desc' : 'seo-desc empty'}>{work.desc.trim() || 'Описание не задано'}</span>
              </span>
            </div>
            <span className="cap">Картинка в превью — логотип из раздела «Оформление»</span>
          </div>
        </div>
      </section>

      <section className="sec" aria-labelledby="inv-h">
        <div className="sec-head">
          <h2 id="inv-h" className="section-title">
            Счета и пополнение
          </h2>
          <span className="muted">Суммы в валюте сервиса, {currency === 'RUB' ? 'рублях' : 'долларах'}</span>
        </div>
        <div className="row">
          <label htmlFor="gen-ttl" className="rl">
            Счёт действует
          </label>
          <div className="rc">
            <span className="gen-line">
              <input id="gen-ttl" className="fld num" inputMode="numeric" value={work.ttl} aria-invalid={ttlBad} onChange={(e) => edit({ ttl: e.target.value.replace(/\D/g, '').slice(0, 4) })} disabled={busy} />
              <span className="muted">минут</span>
            </span>
            <span className={ttlBad ? 'err' : 'cap'}>{ttlBad ? 'От 5 минут до суток (1440)' : 'Потом неоплаченный счёт закрывается'}</span>
          </div>
        </div>
        <div className="row">
          <span className="rl">Пополнение баланса</span>
          <div className="rc">
            <span className="gen-line" style={{ flexWrap: 'wrap' }}>
              <span className="muted">от</span>
              <input className="fld num" style={{ width: 96 }} inputMode="decimal" value={work.min} aria-label="Минимальная сумма пополнения" aria-invalid={topupBad} onChange={(e) => edit({ min: e.target.value })} disabled={busy} />
              <span className="muted">до</span>
              <input className="fld num" style={{ width: 110 }} inputMode="decimal" value={work.max} aria-label="Максимальная сумма пополнения" aria-invalid={topupBad} onChange={(e) => edit({ max: e.target.value })} disabled={busy} />
              <span className="muted">{unit}</span>
            </span>
            {topupBad ? <span className="err">Минимум больше нуля и не больше максимума</span> : null}
          </div>
        </div>
      </section>

      <section className="sec" aria-labelledby="tz-h">
        <div className="sec-head">
          <h2 id="tz-h" className="section-title">
            Часовой пояс Remnawave
          </h2>
          <span className="muted">По нему считаются сутки в статистике трафика и подключений, которые отдаёт панель</span>
        </div>
        <div className="row">
          <label htmlFor="gen-tz" className="rl">
            Пояс панели
          </label>
          <div className="rc">
            <select id="gen-tz" className="field" value={work.offset} onChange={(e) => edit({ offset: Number(e.target.value) })} disabled={busy} style={{ maxWidth: 200 }}>
              {OFFSETS.map((m) => (
                <option key={m} value={m}>
                  {offsetLabel(m)}
                </option>
              ))}
            </select>
          </div>
        </div>
      </section>

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
          {status?.text ?? (dirty ? 'Есть несохранённые изменения' : '')}
        </span>
      </div>
    </SettingsLayout>
  );
}
