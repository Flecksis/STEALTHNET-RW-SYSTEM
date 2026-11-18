import { useEffect, useRef, useState, type FormEvent } from 'react';
import { Link, useLocation, useNavigate } from 'react-router';
import { api } from '../lib/api';
import { dateLong, days } from '../lib/format';
import { t } from '../lib/i18n';
import type { SubscriptionView } from '../cabinet/model';
import { activatePromo, checkPromo, discountTitle, handedCode, promoFailText, promoTargets, type PromoFound } from '../cabinet/promo';
import { liveSubs, remember, subName, subState } from '../cabinet/subs';
import { BackHead, MINI_SUB_KEY, useMini } from './MiniApp';

type Step =
  | { kind: 'form' }
  | { kind: 'discount'; found: PromoFound }
  | { kind: 'pick'; code: string; days: number; list: SubscriptionView[] }
  | { kind: 'none'; code: string; days: number; waiting: boolean }
  | { kind: 'done'; id: number; days: number; named: boolean };

export function MiniPromo() {
  const { me, subs, view, reload } = useMini();
  const location = useLocation();
  const navigate = useNavigate();
  const blocked = me.client.status === 'blocked';
  const allowed = !blocked && me.promo_codes === true && me.full_access !== false;
  const handed = useRef(allowed ? handedCode(location.state) : null);
  const [step, setStep] = useState<Step>({ kind: 'form' });
  const [value, setValue] = useState(handed.current ?? '');
  const [target, setTarget] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const activate = async (code: string, id: number, named: boolean) => {
    const done = await activatePromo(code, id);
    remember(MINI_SUB_KEY, done.subscription_id);
    await reload();
    setValue('');
    setStep({ kind: 'done', id: done.subscription_id, days: done.days, named });
  };

  const submit = async (raw: string) => {
    const code = raw.trim().toUpperCase();
    if (!code || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const found = await checkPromo(code);
      if (found.type !== 'days') {
        setStep({ kind: 'discount', found });
        return;
      }
      const list = liveSubs(await api<SubscriptionView[]>('/api/client/subscriptions'));
      const fresh = promoTargets(list);
      const [first, ...rest] = fresh;
      if (!first) {
        setStep({ kind: 'none', code: found.code, days: found.days, waiting: list.length > 0 });
        return;
      }
      if (rest.length === 0) {
        await activate(found.code, first.subscription.id, false);
        return;
      }
      const shown = view?.subscription.id ?? null;
      setTarget(fresh.some((v) => v.subscription.id === shown) ? shown : first.subscription.id);
      setStep({ kind: 'pick', code: found.code, days: found.days, list });
    } catch (err) {
      setError(promoFailText(err));
    } finally {
      setBusy(false);
    }
  };

  const confirm = async (code: string, id: number) => {
    setBusy(true);
    setError(null);
    try {
      await activate(code, id, true);
    } catch (err) {
      setError(promoFailText(err));
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    const code = handed.current;
    if (!code) {
      return;
    }
    handed.current = null;
    navigate(location.pathname, { replace: true, state: null });
    void submit(code);
  });

  const openForm = () => {
    setError(null);
    setStep({ kind: 'form' });
  };

  if (!allowed) {
    return (
      <main className="mn-page">
        <BackHead title={t('Промокод')} />
        <p className="mn-muted">{blocked ? t('Аккаунт заблокирован. Напишите в поддержку.') : t('Промокоды сейчас не принимаются')}</p>
      </main>
    );
  }

  const errorLine = error ? (
    <p className="mn-error" role="alert">
      {error}
    </p>
  ) : null;

  const body = () => {
    if (step.kind === 'done') {
      const done = subs.find((v) => v.subscription.id === step.id) ?? null;
      const expire = done?.subscription.mirror.expire_at ?? null;
      return (
        <div className="mn-card mn-pad" role="status">
          <strong>{step.named && done ? t('Добавили {days} к «{name}».', { days: days(step.days), name: subName(done, subs) }) : t('Добавили {days}.', { days: days(step.days) })}</strong>
          {expire ? <span className="mn-ink2">{t('Подписка действует до {date}.', { date: dateLong(expire, true) })}</span> : null}
          <Link to="/app" className="mn-main sm press">
            {t('На главную')}
          </Link>
        </div>
      );
    }
    if (step.kind === 'discount') {
      return (
        <div className="mn-card mn-pad" role="status">
          <strong>{discountTitle(step.found)}</strong>
          <span className="mn-ink2">{t('Скидка применится при оплате.')}</span>
          <button type="button" className="mn-main sm press" onClick={() => navigate('/app/plans', { state: { promo: step.found.code } })}>
            {promoTargets(subs).length > 0 ? t('Продлить со скидкой') : t('Выбрать тариф со скидкой')}
          </button>
          <button type="button" className="mn-link press" onClick={openForm}>
            {t('Другой промокод')}
          </button>
        </div>
      );
    }
    if (step.kind === 'none') {
      return (
        <div className="mn-card mn-pad" role="status">
          <span className="mn-ink2">
            {step.waiting
              ? t('Промокод {code} добавит {days}, но подписка ещё создаётся. Активируйте промокод через минуту.', { code: step.code, days: days(step.days) })
              : t('Промокод {code} добавит {days} к подписке, но подписки пока нет. Сначала оформите её, потом активируйте промокод.', { code: step.code, days: days(step.days) })}
          </span>
          {step.waiting ? null : (
            <Link to="/app/plans" className="mn-main sm press">
              {t('Выбрать тариф')}
            </Link>
          )}
          <button type="button" className="mn-link press" onClick={openForm}>
            {t('Другой промокод')}
          </button>
        </div>
      );
    }
    if (step.kind === 'pick') {
      const targets = promoTargets(step.list);
      const chosen = targets.find((v) => v.subscription.id === target) ?? null;
      return (
        <>
          <section aria-label={t('К какой подписке добавить дни?')} className="mn-pay">
            <div className="mn-pay-head">
              <h2>{t('Промокод {code}: +{days}. К какой подписке их добавить?', { code: step.code, days: days(step.days) })}</h2>
            </div>
            <div role="radiogroup" aria-label={t('К какой подписке добавить дни?')} className="mn-terms">
              {targets.map((v) => {
                const state = subState(v, blocked);
                return (
                  <button key={v.subscription.id} type="button" role="radio" aria-checked={target === v.subscription.id} className="mn-term press" onClick={() => setTarget(v.subscription.id)} disabled={busy}>
                    <span className="mn-dot" aria-hidden="true" />
                    <span className="mn-term-text">
                      <strong className="mn-target-name">{subName(v, step.list)}</strong>
                      <span className={`mn-term-sub ${state.tone}`}>{state.text}</span>
                    </span>
                  </button>
                );
              })}
            </div>
          </section>
          {errorLine}
          <button type="button" className="mn-main press" onClick={() => chosen && void confirm(step.code, chosen.subscription.id)} disabled={busy || !chosen}>
            {busy ? t('Добавляем…') : t('Добавить {days}', { days: days(step.days) })}
          </button>
          <button type="button" className="mn-link press mn-promo-back" onClick={openForm} disabled={busy}>
            {t('Отменить')}
          </button>
        </>
      );
    }
    const onSubmit = (e: FormEvent) => {
      e.preventDefault();
      void submit(value);
    };
    return (
      <>
        <form className="mn-promo" onSubmit={onSubmit}>
          <input
            value={value}
            onChange={(e) => {
              setValue(e.target.value.toUpperCase());
              setError(null);
            }}
            placeholder={t('Промокод')}
            aria-label={t('Промокод')}
            maxLength={40}
            autoComplete="off"
            autoCapitalize="characters"
            spellCheck={false}
            disabled={busy}
          />
          <button type="submit" className="mn-btn2 press" disabled={busy || !value.trim()}>
            {busy ? t('Проверяем…') : t('Применить')}
          </button>
        </form>
        {errorLine ?? (
          <p className="mn-notes">
            <span>{t('Скидку применим при оплате, а дни сразу добавим к подписке.')}</span>
          </p>
        )}
      </>
    );
  };

  return (
    <main className="mn-page">
      <BackHead title={t('Промокод')} />
      {body()}
    </main>
  );
}
