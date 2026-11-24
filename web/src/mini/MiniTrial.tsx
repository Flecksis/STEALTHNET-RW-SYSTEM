import { useEffect, useRef, useState } from 'react';
import { Link, useNavigate } from 'react-router';
import { ApiError } from '../lib/api';
import { days } from '../lib/format';
import { t } from '../lib/i18n';
import { loadTrial, NO_TRIAL, requestTrial, trialFacts, trialPlaces, trialTitle, type TrialCheck, type TrialOffer } from '../cabinet/trial';
import { BackHead, useMini } from './MiniApp';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function MiniTrial() {
  const { subs, reload } = useMini();
  const navigate = useNavigate();
  const [offer, setOffer] = useState<TrialOffer | null>(null);
  const [pick, setPick] = useState<number | null>(null);
  const [check, setCheck] = useState<TrialCheck | null>(null);
  const [busy, setBusy] = useState(false);
  const [started, setStarted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    loadTrial()
      .then((loaded) => {
        if (!alive.current) {
          return;
        }
        setOffer(loaded);
        const [only, other] = loaded.variants;
        if (only && !other) {
          setPick(only.id);
        }
      })
      .catch((err) => {
        if (alive.current) {
          setOffer(NO_TRIAL);
          setError(failText(err, t('Пробный период не загрузился')));
        }
      });
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    if (started && subs.length > 0) {
      navigate('/app', { replace: true });
    }
  }, [started, subs.length, navigate]);

  const start = async (variant: number, confirm?: string) => {
    setBusy(true);
    setError(null);
    try {
      const asked = await requestTrial(variant, confirm);
      if (asked) {
        setCheck(asked);
        return;
      }
      setCheck(null);
      setStarted(true);
      for (let i = 0; i < 10 && alive.current; i += 1) {
        await new Promise((r) => window.setTimeout(r, 1500));
        await reload();
      }
    } catch (err) {
      setError(failText(err, t('Пробный период не включился')));
      loadTrial()
        .then((loaded) => alive.current && setOffer(loaded))
        .catch(() => undefined);
    } finally {
      if (alive.current) {
        setBusy(false);
      }
    }
  };

  const choices = offer?.available ? offer.variants : [];
  const single = choices.length === 1 ? choices[0] : null;
  const picked = choices.find((choice) => choice.id === pick) ?? null;
  const singlePlaces = single ? trialPlaces(single) : null;

  return (
    <main className="mn-page trial">
      <BackHead title={t('Пробный период')} />
      {!offer ? <p className="mn-muted">{t('Загружаем…')}</p> : null}
      {offer && !offer.available && !started ? (
        <div className="mn-card mn-pad">
          <p>{t('Пробный период сейчас недоступен.')}</p>
          <Link to="/app/plans" className="mn-main sm press">
            {t('Выбрать тариф')}
          </Link>
        </div>
      ) : null}
      {single ? (
        <div className="mn-card mn-pad">
          <strong className="mn-big">{t('{days} бесплатно', { days: days(single.days) })}</strong>
          <span className="mn-muted">{trialFacts(single, false)}</span>
          {singlePlaces ? <span className="mn-muted">{singlePlaces}</span> : null}
        </div>
      ) : null}
      {choices.length > 1 ? (
        <section aria-labelledby="mn-trial-h" className="mn-trial">
          <h2 id="mn-trial-h" className="mn-trial-h">
            {t('Выберите пробный период')}
          </h2>
          <div role="radiogroup" aria-labelledby="mn-trial-h" className="mn-terms">
            {choices.map((choice, index) => {
              const places = trialPlaces(choice);
              return (
                <button key={choice.id} type="button" role="radio" aria-checked={pick === choice.id} className="mn-term press" onClick={() => setPick(choice.id)} disabled={busy || started}>
                  <span className="mn-dot" aria-hidden="true" />
                  <span className="mn-term-text">
                    <span className="mn-term-top">
                      <strong>{trialTitle(choice, index)}</strong>
                      <strong className="num">{days(choice.days)}</strong>
                    </span>
                    <span className="mn-term-sub">{trialFacts(choice, false)}</span>
                    {places ? <span className="mn-term-sub">{places}</span> : null}
                  </span>
                </button>
              );
            })}
          </div>
        </section>
      ) : null}
      {error ? <p className="mn-error">{error}</p> : null}
      {started ? (
        <p className="mn-note" role="status">
          {t('Пробный период оформляется. Подписка появится здесь через минуту.')}
        </p>
      ) : null}
      {check && picked ? (
        <>
          <p className="mn-note" aria-live="polite">
            {check.text}
          </p>
          <button type="button" className="mn-main press" disabled={busy} onClick={() => void start(picked.id, check.token)}>
            {busy ? t('Проверяем…') : t('Я не робот')}
          </button>
        </>
      ) : choices.length && !started ? (
        <button type="button" className="mn-main press" disabled={busy || !picked} onClick={() => picked && void start(picked.id)}>
          {busy ? t('Включаем…') : picked && choices.length > 1 ? t('Включить «{name}»', { name: trialTitle(picked, choices.indexOf(picked)) }) : choices.length > 1 ? t('Выберите вариант') : t('Включить пробный период')}
        </button>
      ) : null}
    </main>
  );
}
