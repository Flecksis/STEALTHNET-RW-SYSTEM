import { useEffect, useRef, useState, type FormEvent } from 'react';
import { useLocation, useNavigate } from 'react-router';
import { dateLong, days } from '../lib/format';
import { t } from '../lib/i18n';
import { useCabinet } from './CabinetApp';
import { activatePromo, checkPromo, discountTitle, handedCode, promoFailText, promoTargets, type PromoFound } from './promo';
import type { Subs } from './SubSwitch';
import { subName, subState } from './subs';

type Step =
  | { kind: 'closed' }
  | { kind: 'form' }
  | { kind: 'discount'; found: PromoFound }
  | { kind: 'pick'; code: string; days: number }
  | { kind: 'none'; code: string; days: number }
  | { kind: 'done'; id: number; days: number; named: boolean };

export function PromoBox({ subs }: { subs: Subs }) {
  const { me, phone } = useCabinet();
  const location = useLocation();
  const navigate = useNavigate();
  const blocked = me.client.status === 'blocked';
  const allowed = !blocked && me.promo_codes === true && me.full_access !== false;
  const handed = useRef(allowed ? handedCode(location.state) : null);
  const [step, setStep] = useState<Step>(handed.current ? { kind: 'form' } : { kind: 'closed' });
  const [value, setValue] = useState(handed.current ?? '');
  const [target, setTarget] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const field = useRef<HTMLInputElement>(null);
  const focusNext = useRef(false);
  const list = subs.list ?? [];
  const targets = promoTargets(list);

  const activate = async (code: string, id: number, named: boolean) => {
    const done = await activatePromo(code, id);
    await subs.reload();
    if (subs.chosen?.subscription.id !== done.subscription_id) {
      subs.choose(done.subscription_id);
    }
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
      const fresh = promoTargets(await subs.reload());
      const [first, ...rest] = fresh;
      if (!first) {
        setStep({ kind: 'none', code: found.code, days: found.days });
        return;
      }
      if (rest.length === 0) {
        await activate(found.code, first.subscription.id, false);
        return;
      }
      const shown = subs.chosen?.subscription.id ?? null;
      setTarget(fresh.some((v) => v.subscription.id === shown) ? shown : first.subscription.id);
      setStep({ kind: 'pick', code: found.code, days: found.days });
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
    navigate(`${location.pathname}${location.search}`, { replace: true, state: null });
    void submit(code);
  });

  useEffect(() => {
    if (step.kind === 'form' && focusNext.current) {
      focusNext.current = false;
      field.current?.focus();
    }
  }, [step.kind]);

  if (!allowed) {
    return null;
  }

  const openForm = () => {
    focusNext.current = true;
    setError(null);
    setStep({ kind: 'form' });
  };

  const toPay = (code: string) => navigate(subs.link('/cabinet/pay'), { state: { promo: code } });

  if (step.kind === 'closed') {
    return (
      <button type="button" className="cb-text" onClick={openForm}>
        {t('Есть промокод?')}
      </button>
    );
  }

  if (step.kind === 'done') {
    const view = list.find((v) => v.subscription.id === step.id) ?? null;
    const expire = view?.subscription.mirror.expire_at ?? null;
    return (
      <p className="cb-result" role="status">
        <strong>{step.named && view ? t('Добавили {days} к «{name}».', { days: days(step.days), name: subName(view, list) }) : t('Добавили {days}.', { days: days(step.days) })}</strong>{' '}
        {expire ? t('Подписка действует до {date}.', { date: dateLong(expire, true) }) : null}
      </p>
    );
  }

  const buttons = phone ? 'cb-ask-btns col' : 'cb-ask-btns';
  const errorLine = error ? (
    <p className="cb-error" role="alert">
      {error}
    </p>
  ) : null;

  if (step.kind === 'discount') {
    return (
      <div className="cb-ask cb-promo-box" role="status">
        <p>
          <strong>{discountTitle(step.found)}</strong> {t('Скидка применится при оплате.')}
        </p>
        <span className={buttons}>
          <button type="button" className="cb-main sm" onClick={() => toPay(step.found.code)}>
            {targets.length > 0 ? t('Продлить со скидкой') : t('Выбрать тариф со скидкой')}
          </button>
          <button type="button" className="cb-cancel" onClick={openForm}>
            {t('Другой промокод')}
          </button>
        </span>
      </div>
    );
  }

  if (step.kind === 'none') {
    return (
      <div className="cb-ask cb-promo-box" role="status">
        <p>
          {list.length > 0
            ? t('Промокод {code} добавит {days}, но подписка ещё создаётся. Активируйте промокод через минуту.', { code: step.code, days: days(step.days) })
            : t('Промокод {code} добавит {days} к подписке, но подписки пока нет. Сначала оформите её, потом активируйте промокод.', { code: step.code, days: days(step.days) })}
        </p>
        <span className={buttons}>
          <button type="button" className="cb-cancel" onClick={openForm}>
            {t('Другой промокод')}
          </button>
        </span>
      </div>
    );
  }

  if (step.kind === 'pick') {
    const chosen = targets.find((v) => v.subscription.id === target) ?? null;
    return (
      <div className="cb-ask cb-promo-box">
        <p>
          <strong>{t('Промокод {code}: +{days}', { code: step.code, days: days(step.days) })}</strong> {t('К какой подписке добавить дни?')}
        </p>
        <div role="radiogroup" aria-label={t('К какой подписке добавить дни?')} className="cb-method-grid">
          {targets.map((v) => {
            const state = subState(v, blocked);
            return (
              <button key={v.subscription.id} type="button" role="radio" aria-checked={target === v.subscription.id} className="cb-method cb-target" onClick={() => setTarget(v.subscription.id)} disabled={busy}>
                <span className="cb-radio" aria-hidden="true" />
                <span className="cb-method-text">
                  <strong>{subName(v, list)}</strong>
                  <span className={`cb-target-note ${state.tone}`}>{state.text}</span>
                </span>
              </button>
            );
          })}
        </div>
        {errorLine}
        <span className={buttons}>
          <button type="button" className="cb-main sm" onClick={() => chosen && void confirm(step.code, chosen.subscription.id)} disabled={busy || !chosen}>
            {busy ? t('Добавляем…') : t('Добавить {days}', { days: days(step.days) })}
          </button>
          <button type="button" className="cb-cancel" onClick={openForm} disabled={busy}>
            {t('Отменить')}
          </button>
        </span>
      </div>
    );
  }

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    void submit(value);
  };

  return (
    <div className="cb-ask cb-promo-box">
      <form className="cb-promo" onSubmit={onSubmit}>
        <label className="cb-promo-field">
          <span className="sr-only">{t('Промокод')}</span>
          <input
            ref={field}
            value={value}
            onChange={(e) => {
              setValue(e.target.value.toUpperCase());
              setError(null);
            }}
            placeholder={t('Промокод')}
            maxLength={40}
            autoComplete="off"
            autoCapitalize="characters"
            spellCheck={false}
            disabled={busy}
          />
        </label>
        <button type="submit" className="cb-second" disabled={busy || !value.trim()}>
          {busy ? t('Проверяем…') : t('Применить')}
        </button>
      </form>
      {errorLine ?? <p className="cb-small cb-muted">{t('Скидку применим при оплате, а дни сразу добавим к подписке.')}</p>}
    </div>
  );
}
