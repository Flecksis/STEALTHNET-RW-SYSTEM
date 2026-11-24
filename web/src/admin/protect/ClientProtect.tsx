import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../../lib/api';
import { dateTime } from '../../lib/format';
import { useSession } from '../session';
import { ANTIBOT, exceedState, failText, LIMITS, reasonsText, usageText, type ClientMark, type Exceed, type Page } from './model';
import { protectSections } from './sections';
import './protect.css';

type Props = { clientId: number; phone?: boolean; onChanged?: () => void };

export function ClientProtect({ clientId, phone, onChanged }: Props) {
  const { me, can } = useSession();
  const [mark, setMark] = useState<ClientMark | null>(null);
  const [limited, setLimited] = useState<Exceed[]>([]);
  const [asking, setAsking] = useState<'mark' | number | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const writable = can('clients_write');
  const readable = can('clients_read');

  const { antibot, limits } = protectSections(me.modules, readable);

  const load = useCallback(async () => {
    if (!antibot && !limits) {
      setMark(null);
      setLimited([]);
      return;
    }
    const [found, page] = await Promise.all([
      antibot ? api<ClientMark>(`${ANTIBOT}/marks/${clientId}`).catch(() => null) : null,
      limits ? api<Page<Exceed>>(`${LIMITS}/exceeds?state=current&client=${clientId}&limit=20`).catch(() => null) : null,
    ]);
    setMark(found);
    setLimited((page?.items ?? []).filter((e) => exceedState(e).active));
  }, [clientId, antibot, limits]);

  useEffect(() => {
    void load();
  }, [load]);

  const act = async (work: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await work();
      setAsking(null);
      await load();
      onChanged?.();
    } catch (err) {
      setError(failText(err, 'Не получилось'));
    } finally {
      setBusy(false);
    }
  };

  const marked = mark?.marked ? mark : null;
  if (!marked && limited.length === 0) {
    return null;
  }
  const box = phone ? 'pc-group' : 'cc-sec cc-box';
  const title = phone ? 'hp-h2' : 'cc-h2';
  const body = phone ? 'hp-card pt-card pt-pitem' : 'pt-card';

  return (
    <>
      {marked ? (
        <section className={box} aria-labelledby="pt-ab-h">
          <h2 id="pt-ab-h" className={title}>
            Антибот
          </h2>
          <div className={body}>
            <span className="pt-card-text">
              Отмечен как подозрительный: {reasonsText(marked.reasons)}.{' '}
              {marked.confirmed_at ? `Прошёл проверку «Я не робот» ${dateTime(marked.confirmed_at).toLowerCase()}.` : ''}
            </span>
            <Link to="/admin/clients/antibot" className="pr-inline-link">
              Журнал антибота
            </Link>
            {writable ? (
              asking === 'mark' ? (
                <div className="pr-ask">
                  <span>
                    <b>Снять отметку?</b> Пробный период, дни по промокодам, подписки по промо-ссылкам, бонусы пригласившему, бесплатные вращения рулетки и конкурсы станут доступны.
                  </span>
                  <span className="pr-ask-btns">
                    <button type="button" className="b34 on-surface hov" disabled={busy} onClick={() => void act(() => api(`${ANTIBOT}/marks/${clientId}/clear`, { method: 'POST', body: {} }))}>
                      {busy ? 'Снимаем…' : 'Снять отметку'}
                    </button>
                    <button type="button" className="b34 on-surface hov" disabled={busy} onClick={() => setAsking(null)}>
                      Отменить
                    </button>
                  </span>
                </div>
              ) : (
                <button type="button" className="b36 hov pt-pbtn" onClick={() => setAsking('mark')}>
                  Снять отметку
                </button>
              )
            ) : null}
          </div>
        </section>
      ) : null}
      {limited.map((e) => (
        <section key={e.id} className={box} aria-labelledby={`pt-lm-h-${e.id}`}>
          <h2 id={`pt-lm-h-${e.id}`} className={title}>
            Лимит трафика
          </h2>
          <div className={body}>
            <span className="pt-card-text">
              {exceedState(e).text}: {usageText(e)}.
            </span>
            {writable ? (
              asking === e.id ? (
                <div className="pr-ask">
                  <span>
                    <b>Снять ограничение сейчас?</b> Подписка снова заработает, не дожидаясь конца окна.
                  </span>
                  <span className="pr-ask-btns">
                    <button type="button" className="b34 on-surface hov" disabled={busy} onClick={() => void act(() => api(`${LIMITS}/exceeds/${e.id}/lift`, { method: 'POST', body: {} }))}>
                      {busy ? 'Снимаем…' : 'Снять'}
                    </button>
                    <button type="button" className="b34 on-surface hov" disabled={busy} onClick={() => setAsking(null)}>
                      Отменить
                    </button>
                  </span>
                </div>
              ) : (
                <button type="button" className="b36 hov pt-pbtn" onClick={() => setAsking(e.id)}>
                  Снять ограничение
                </button>
              )
            ) : null}
          </div>
        </section>
      ))}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
    </>
  );
}
