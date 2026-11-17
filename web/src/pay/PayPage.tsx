import { useCallback, useEffect, useState } from 'react';
import { useParams } from 'react-router';
import { api, ApiError } from '../lib/api';
import { BrandMark } from '../lib/brand';
import { money, plural, type Money } from '../lib/format';
import { usePhone } from '../lib/media';
import { QrCode } from '../lib/qr';
import { DEFAULT_SITE, useSite } from '../lib/site';
import { readStored, useSurface, useSystemTheme, type Theme } from '../lib/theme';
import './pay.css';

type PayState = 'pending' | 'paid' | 'expired' | 'closed';

type PayView = {
  status: PayState;
  provider: 'gram';
  amount: Money;
  expires_at: string | null;
  gram: {
    amount: string;
    nano: string;
    address: string;
    comment: string;
    ton_link: string;
    tonkeeper_link: string;
  };
  back: { bot: string | null; cabinet: string | null };
  checked?: 'found' | 'not_found' | 'failed' | 'too_often';
};

const CHECK_PAUSE_MS = 5000;

const CHECK_TEXT: Record<'not_found' | 'failed' | 'too_often', string> = {
  not_found: 'Перевод пока не найден. Если вы только что отправили GRAM, подождите минуту: сеть подтверждает перевод, страница проверит его сама.',
  failed: 'Сеть TON сейчас не ответила. Попробуйте через минуту, проверка идёт и без вас.',
  too_often: 'Проверяем не чаще раза в 5 секунд. Нажмите чуть позже.',
};

const POLL_MS = 5000;

function grams(value: string): string {
  return value.replace('.', ',');
}

function minutesLeft(iso: string | null, now: number): number | null {
  if (!iso) {
    return null;
  }
  const at = Date.parse(iso);
  if (Number.isNaN(at)) {
    return null;
  }
  return Math.max(0, Math.ceil((at - now) / 60_000));
}

function leftText(minutes: number): string {
  if (minutes >= 60) {
    const hours = Math.floor(minutes / 60);
    const rest = minutes % 60;
    return rest ? `${hours} ч ${rest} мин` : `${hours} ${plural(hours, 'час', 'часа', 'часов')}`;
  }
  return `${minutes} мин`;
}

function CopyRow({ label, value, shown, warn }: { label: string; value: string; shown?: string; warn?: string }) {
  const [done, setDone] = useState(false);
  const [failed, setFailed] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setDone(true);
      setFailed(false);
      window.setTimeout(() => setDone(false), 1600);
    } catch {
      setFailed(true);
    }
  };
  return (
    <div className="gp-row">
      <div className="gp-row-head">
        <span className="gp-label">{label}</span>
        <button type="button" className="gp-copy" onClick={() => void copy()}>
          {done ? 'Скопировано' : 'Скопировать'}
        </button>
      </div>
      <span className="gp-value mono">{shown ?? value}</span>
      {warn ? <span className="gp-warn">{warn}</span> : null}
      {failed ? <span className="gp-muted">Браузер не дал скопировать: выделите текст вручную</span> : null}
    </div>
  );
}

function CheckPaid({ id, onView }: { id: string; onView: (view: PayView) => void }) {
  const [busy, setBusy] = useState(false);
  const [resting, setResting] = useState(false);
  const [answer, setAnswer] = useState<string | null>(null);
  const run = async () => {
    setBusy(true);
    setAnswer(null);
    try {
      const data = await api<PayView>(`/api/public/pay/${encodeURIComponent(id)}/check`, { method: 'POST' });
      onView(data);
      if (data.checked && data.checked !== 'found') {
        setAnswer(CHECK_TEXT[data.checked]);
      }
    } catch (err) {
      setAnswer(err instanceof ApiError ? err.message : 'Не получилось проверить. Проверьте интернет.');
    } finally {
      setBusy(false);
      setResting(true);
      window.setTimeout(() => setResting(false), CHECK_PAUSE_MS);
    }
  };
  return (
    <div className="gp-check">
      <button type="button" className="gp-btn line" onClick={() => void run()} disabled={busy || resting}>
        {busy ? 'Проверяем…' : 'Я оплатил, проверить'}
      </button>
      {answer ? (
        <p className="gp-muted" role="status">
          {answer}
        </p>
      ) : null}
    </div>
  );
}

function BackLinks({ back }: { back: PayView['back'] }) {
  if (!back.bot && !back.cabinet) {
    return null;
  }
  return (
    <div className="gp-actions">
      {back.bot ? (
        <a className="gp-btn" href={back.bot}>
          Вернуться в бот
        </a>
      ) : null}
      {back.cabinet ? (
        <a className={back.bot ? 'gp-btn line' : 'gp-btn'} href={back.cabinet}>
          Открыть кабинет
        </a>
      ) : null}
    </div>
  );
}

export function PayPage() {
  const { id = '' } = useParams();
  const site = useSite() ?? DEFAULT_SITE;
  const system = useSystemTheme();
  const phone = usePhone();
  const choice = site.appearance.client_theme;
  const stored = readStored('sn.client.theme');
  const personal: Theme | null = stored === 'dark' || stored === 'light' ? stored : null;
  const theme: Theme = choice === 'dark' || choice === 'light' ? choice : (personal ?? system);
  useSurface(phone ? 'cabinet-phone' : 'cabinet', theme, site.appearance.client === 'mono' ? 'b' : 'a');

  const [view, setView] = useState<PayView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const load = useCallback(async () => {
    try {
      const data = await api<PayView>(`/api/public/pay/${encodeURIComponent(id)}`);
      setView(data);
      setError(null);
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) {
        setError('Счёт не найден. Откройте оплату заново из бота или кабинета.');
      } else {
        setError(err instanceof ApiError ? err.message : 'Не получилось загрузить счёт. Проверьте интернет.');
      }
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load]);

  const pending = view?.status === 'pending' || view?.status === 'expired';
  useEffect(() => {
    if (!pending) {
      return;
    }
    const timer = window.setInterval(() => {
      setNow(Date.now());
      void load();
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [pending, load]);

  const brand = (
    <div className="gp-brand">
      <BrandMark name={site.brand_name} logo={site.logo_url} className="gp-mark" />
      <span>{site.brand_name.toUpperCase()}</span>
    </div>
  );

  if (!view) {
    return (
      <main className="gp">
        <div className="gp-box">
          {brand}
          {error ? (
            <section className="gp-card">
              <h1 className="gp-title">Оплата в GRAM</h1>
              <p className="gp-muted" role="alert">
                {error}
              </p>
            </section>
          ) : (
            <p className="gp-muted">Загружаем счёт…</p>
          )}
        </div>
      </main>
    );
  }

  const left = minutesLeft(view.expires_at, now);

  if (view.status === 'paid') {
    return (
      <main className="gp">
        <div className="gp-box">
          {brand}
          <section className="gp-card" aria-live="polite">
            <h1 className="gp-title">Оплата получена</h1>
            <p className="gp-sub">
              {grams(view.gram.amount)} GRAM за {money(view.amount)}. Подписка обновится в течение минуты, подтверждение придёт в бот.
            </p>
            <BackLinks back={view.back} />
          </section>
        </div>
      </main>
    );
  }

  if (view.status !== 'pending') {
    return (
      <main className="gp">
        <div className="gp-box">
          {brand}
          <section className="gp-card">
            <h1 className="gp-title">{view.status === 'expired' ? 'Срок счёта истёк' : 'Счёт закрыт'}</h1>
            <p className="gp-sub">
              {view.status === 'expired'
                ? 'Курс для этого счёта больше не действует. Если GRAM с этим комментарием уже отправлены, мы всё равно засчитаем перевод: сеть проверяется ещё сутки, а кнопка ниже проверит сразу. Если не отправляли, оформите оплату заново.'
                : 'По этому счёту оплатить уже нельзя. Оформите оплату заново в боте или кабинете.'}
            </p>
            <span className="gp-value mono">{view.gram.comment}</span>
            {view.status === 'expired' ? <CheckPaid id={id} onView={setView} /> : null}
            <BackLinks back={view.back} />
          </section>
        </div>
      </main>
    );
  }

  return (
    <main className="gp">
      <div className="gp-box">
        {brand}
        <section className="gp-card" aria-labelledby="gp-title">
          <h1 id="gp-title" className="gp-title">
            Оплата в GRAM
          </h1>
          <div className="gp-sum">
            <span className="gp-big num">{grams(view.gram.amount)} GRAM</span>
            <span className="gp-muted">{money(view.amount)}</span>
          </div>
          <p className="gp-status" role="status">
            <span className="gp-dot" aria-hidden="true" />
            Ждём перевод. Страница обновится сама, когда он придёт
          </p>
          {left !== null ? <p className="gp-muted">Курс закреплён ещё {leftText(left)}</p> : null}
          <div className="gp-qr">
            <QrCode text={view.gram.ton_link} size={phone ? 200 : 220} label="QR-код для оплаты в кошельке TON" />
          </div>
          <div className="gp-actions">
            <a className="gp-btn" href={view.gram.ton_link}>
              Открыть кошелёк
            </a>
            <a className="gp-btn line" href={view.gram.tonkeeper_link} target="_blank" rel="noreferrer">
              Открыть в Tonkeeper
            </a>
          </div>
          <CheckPaid id={id} onView={setView} />
        </section>
        <section className="gp-card gp-list" aria-label="Данные для перевода вручную">
          <CopyRow label="Сумма" value={view.gram.amount} shown={`${grams(view.gram.amount)} GRAM`} />
          <CopyRow label="Адрес" value={view.gram.address} />
          <CopyRow
            label="Комментарий"
            value={view.gram.comment}
            warn="Обязательно укажите комментарий: без него оплата не найдётся"
          />
        </section>
        <p className="gp-note">
          Переводите GRAM (бывший Toncoin) в сети TON. Сумма — не меньше указанной. Обычно перевод доходит за минуту.
        </p>
      </div>
    </main>
  );
}
