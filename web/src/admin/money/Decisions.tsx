import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { Link, useLocation } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { count, dateLong, money, type Money } from '../../lib/format';
import { Avatar } from '../Avatar';
import { Dialog } from '../client/dialogs';
import { amountText } from '../client/history';
import type { Amount, OrderBrief } from '../client/types';
import { agoLong, minutesSince, orderLine, personName, spanText, type Person } from '../home/model';
import { useSession } from '../session';
import { GroupFrame, type Outcome } from './group';
import { StarsReview } from './StarsReview';

type Key = 'refunds' | 'orders' | 'stars' | 'charges' | 'requests';

export const TODO_ANCHOR: Record<Key, string> = {
  refunds: 'todo-refunds',
  orders: 'todo-orders',
  stars: 'todo-stars',
  charges: 'todo-charges',
  requests: 'todo-requests',
};

type Shown = { visible: boolean; count: number };

type Report = (key: Key, shown: Shown) => void;

type GroupProps = { report: Report; changed: () => void; round: number };

type Ask = {
  title: string;
  text: string;
  confirm: string;
  danger?: boolean;
  note?: string;
  run: (note: string) => Promise<Outcome>;
};

type OrderAction = 'redeliver' | 'recalculate' | 'mark_fulfilled';

type OrderItem = {
  order_id: number;
  client_id: number;
  kind: string;
  tariff: string | null;
  total: Money;
  stars: number | null;
  reason: string | null;
  refund_pending: boolean;
  job: { id: number; status: string; attempts: number; error: string | null; updated_at: string } | null;
  updated_at: string;
  actions: OrderAction[];
  client: Person | null;
  order: OrderBrief | null;
};

type RefundAction = {
  request_id: number;
  payment_id: number;
  client_id: number;
  action: 'keep' | 'cut' | 'disable';
  amount: Amount;
  attempts: number;
  error: string | null;
  confirmed_at: string | null;
  client: Person | null;
  order: OrderBrief | null;
  provider_title: string | null;
};

type RefundRequest = {
  request_id: number;
  payment_id: number;
  amount: Amount;
  subscription_action: 'keep' | 'cut' | 'disable';
  reason: string | null;
  held: boolean;
  requested_at: string;
  client_id: number | null;
  client: Person | null;
  order: OrderBrief | null;
  provider_title: string | null;
};

type Charge = {
  payment_id: number;
  client_id: number;
  client: Person | null;
  order: OrderBrief | null;
  provider_title: string;
  amount: Amount;
  reason: string | null;
  charged_at: string;
  closes_at: string | null;
};

type Watch = { id: number; name: string; mark: string | null; since: number };

const WATCH_STEP = 3000;
const SETTLE_AFTER = 6000;
const WATCH_LIMIT = 45_000;
const IN_WORK = new Set(['queued', 'running']);

const ORDER_LABEL: Record<OrderAction, string> = {
  redeliver: 'Повторить выдачу',
  recalculate: 'Пересчитать и выдать',
  mark_fulfilled: 'Отметить выданным',
};

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function waited(iso: string | null): string {
  const minutes = minutesSince(iso);
  if (minutes === null) {
    return '';
  }
  return minutes < 1 ? 'ждёт меньше минуты' : `ждёт ${spanText(minutes)}`;
}

function useList<T>(path: string, fallback: string, round: number) {
  const [items, setItems] = useState<T[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async (): Promise<T[] | null> => {
    try {
      const list = await api<T[]>(path);
      setItems(list);
      setError(null);
      return list;
    } catch (err) {
      setItems((prev) => prev ?? []);
      setError(failText(err, fallback));
      return null;
    }
  }, [path, fallback]);
  useEffect(() => {
    void load();
  }, [load, round]);
  return { items, error, load };
}

function useReport(key: Key, report: Report, items: unknown[] | null, outcome: Outcome | null, error: string | null) {
  const size = items?.length ?? 0;
  const loaded = items !== null;
  useEffect(() => {
    if (loaded) {
      report(key, { visible: size > 0 || outcome !== null || error !== null, count: size });
    }
  }, [key, report, loaded, size, outcome, error]);
}

function AskDialog({ ask, onClose, onDone }: { ask: Ask; onClose: () => void; onDone: (outcome: Outcome) => void }) {
  const [note, setNote] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fieldId = useId();
  const go = async () => {
    setBusy(true);
    setError(null);
    try {
      onDone(await ask.run(note.trim()));
    } catch (err) {
      setError(failText(err, 'Не получилось'));
      setBusy(false);
    }
  };
  return (
    <Dialog title={ask.title} busy={busy} onClose={onClose}>
      <p className="muted">{ask.text}</p>
      {ask.note ? (
        <div className="field-row">
          <label htmlFor={fieldId}>{ask.note}</label>
          <textarea id={fieldId} className="field" rows={3} maxLength={500} value={note} onChange={(event) => setNote(event.target.value)} disabled={busy} autoFocus />
        </div>
      ) : null}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button type="button" className={ask.danger ? 'btn-bad' : 'btn-main btn'} onClick={() => void go()} disabled={busy || (ask.note !== undefined && !note.trim())}>
          {busy ? 'Выполняем…' : ask.confirm}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}

function Who({ clientId, person, line }: { clientId: number | null; person: Person | null; line: string }) {
  const name = clientId ? personName(person, clientId) : 'Клиент не найден';
  const body = (
    <>
      <Avatar id={clientId ?? 0} name={name} className="mo-ava" />
      <span className="mo-text">
        <span className="mo-name">{name}</span>
        <span className="muted">{line}</span>
      </span>
    </>
  );
  return clientId ? (
    <Link to={`/admin/clients/${clientId}`} className="mo-who hov">
      {body}
    </Link>
  ) : (
    <span className="mo-who">{body}</span>
  );
}

function purchase(order: OrderBrief | null, fallback: string): string {
  return order ? orderLine(order) : fallback;
}

function Orders({ report, changed, round }: GroupProps) {
  const { items, error, load } = useList<OrderItem>('/api/admin/orders/review', 'Заказы на проверке не загрузились', round);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  const [watch, setWatch] = useState<Watch | null>(null);
  useReport('orders', report, items, outcome, error);

  useEffect(() => {
    if (!watch) {
      return;
    }
    const timer = window.setTimeout(async () => {
      const list = await load();
      if (!list) {
        setWatch(null);
        return;
      }
      const item = list.find((o) => o.order_id === watch.id);
      const job = item?.job ?? null;
      const fresh = job !== null && job.updated_at !== watch.mark;
      if (!item) {
        setOutcome({ text: `${watch.name}: заказ выдан`, tone: 'ok' });
        setWatch(null);
        changed();
      } else if (job && fresh && !IN_WORK.has(job.status)) {
        setOutcome({ text: `${watch.name}: заказ снова на проверке. Причина: ${job.error ?? item.reason ?? 'не указана'}`, tone: 'warn' });
        setWatch(null);
        changed();
      } else if (job && fresh && job.status === 'queued' && job.attempts > 0 && job.error) {
        setOutcome({ text: `${watch.name}: сразу выдать не получилось, система повторит сама. Причина: ${job.error}`, tone: 'warn' });
        setWatch(null);
      } else if (Date.now() - watch.since > WATCH_LIMIT) {
        setOutcome({ text: `${watch.name}: заказ в очереди на выдачу. Если он не выдастся, то вернётся сюда с причиной`, tone: 'warn' });
        setWatch(null);
      } else {
        setWatch({ ...watch });
      }
    }, WATCH_STEP);
    return () => window.clearTimeout(timer);
  }, [watch, load, changed]);

  if (!items || (items.length === 0 && !outcome && !error)) {
    return null;
  }

  const queue = (item: OrderItem, name: string, path: string, text: (body: { recalculated?: boolean }) => string) => async () => {
    const mark = item.job?.updated_at ?? null;
    const body = await api<{ recalculated?: boolean }>(path, { method: 'POST' });
    setWatch({ id: item.order_id, name, mark, since: Date.now() });
    void load();
    return { text: text(body), tone: 'ok' as const };
  };

  const open = (item: OrderItem, action: OrderAction) => {
    const name = personName(item.client, item.client_id);
    const base = `/api/admin/orders/${item.order_id}`;
    if (action === 'redeliver') {
      setAsk({
        title: 'Повторить выдачу?',
        text: 'Заказ снова уйдёт на выдачу, как после оплаты. Если выдать не получится, он вернётся сюда с причиной.',
        confirm: 'Повторить выдачу',
        run: queue(item, name, `${base}/redeliver`, (body) =>
          body.recalculated ? `${name}: план пересчитан, заказ выдаётся…` : `${name}: заказ выдаётся…`,
        ),
      });
    } else if (action === 'recalculate') {
      setAsk({
        title: 'Пересчитать и выдать?',
        text: 'План выдачи посчитается заново от текущего срока подписки в Remnawave, и заказ снова уйдёт на выдачу. Если срок в панели уже продлили этой оплатой, лучше отметить заказ выданным.',
        confirm: 'Пересчитать и выдать',
        run: queue(item, name, `${base}/recalculate`, () => `${name}: план пересчитан, заказ выдаётся…`),
      });
    } else {
      setAsk({
        title: 'Отметить заказ выданным?',
        text: 'Заказ закроется без выдачи: панель возьмёт срок подписки из Remnawave, клиент получит сообщение о выдаче. Отмечайте, если подписку уже выдали вручную. Пояснение увидят другие администраторы.',
        confirm: 'Отметить выданным',
        note: 'Пояснение',
        run: async (note) => {
          await api(`${base}/mark-fulfilled`, { method: 'POST', body: { note } });
          await load();
          return { text: `${name}: заказ отмечен выданным`, tone: 'ok' };
        },
      });
    }
  };

  return (
    <GroupFrame
      id={TODO_ANCHOR.orders}
      title="Заказы на ручной проверке"
      total={items.length}
      lead="Оплата пришла, но заказ не выдался сам. Повторите выдачу, пересчитайте её от срока в Remnawave или отметьте заказ выданным, если подписку уже выдали вручную."
      outcome={outcome}
      error={error}
    >
      {items.map((item) => {
        const busy = watch?.id === item.order_id || (item.job !== null && IN_WORK.has(item.job.status));
        return (
          <div key={item.order_id} className="mo-row">
            <Who clientId={item.client_id} person={item.client} line={purchase(item.order, item.tariff ?? 'Заказ')} />
            <span className="mo-sum">
              <strong className="num">{item.stars ? count(item.stars, 'звезда', 'звезды', 'звёзд') : money(item.total)}</strong>
              <span className="muted">{waited(item.updated_at)}</span>
            </span>
            <span className="mo-act mo-acts">
              {busy ? <span className="muted">Выдаётся…</span> : null}
              {item.actions
                .filter((action) => !busy || action === 'mark_fulfilled')
                .map((action) => (
                  <button key={action} type="button" className="btn-line hov" onClick={() => open(item, action)}>
                    {ORDER_LABEL[action]}
                  </button>
                ))}
            </span>
            {item.reason ? <span className="muted mo-reason">Причина: {item.reason}</span> : null}
          </div>
        );
      })}
      {ask ? (
        <AskDialog
          ask={ask}
          onClose={() => setAsk(null)}
          onDone={(result) => {
            setAsk(null);
            setOutcome(result);
            changed();
          }}
        />
      ) : null}
    </GroupFrame>
  );
}

function actionText(action: RefundAction['action']): string {
  return action === 'cut' ? 'укоротить срок подписки' : 'отключить подписку';
}

function Refunds({ report, changed, round }: GroupProps) {
  const { items, error, load } = useList<RefundAction>('/api/admin/refunds/actions', 'Незавершённые возвраты не загрузились', round);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  useReport('refunds', report, items, outcome, error);
  if (!items || (items.length === 0 && !outcome && !error)) {
    return null;
  }

  const retry = (item: RefundAction) => {
    const name = personName(item.client, item.client_id);
    setAsk({
      title: 'Повторить действие с подпиской?',
      text: `Система ещё раз попробует ${actionText(item.action)}. Деньги второй раз не возвращаются.`,
      confirm: 'Повторить',
      run: async () => {
        const body = await api<{ outcome: 'done' | 'retry' | 'failed' | 'skipped' }>(`/api/admin/refunds/${item.request_id}/action/retry`, { method: 'POST' });
        const list = await load();
        const left = list?.find((r) => r.request_id === item.request_id);
        switch (body.outcome) {
          case 'done':
            return { text: `${name}: подписка изменена, возврат завершён`, tone: 'ok' };
          case 'skipped':
            return { text: `${name}: менять подписку уже не нужно`, tone: 'ok' };
          case 'retry':
            return { text: `${name}: сразу не получилось, система повторит сама`, tone: 'warn' };
          default:
            return { text: `${name}: снова не получилось. Причина: ${left?.error ?? 'не указана'}`, tone: 'warn' };
        }
      },
    });
  };

  const close = (item: RefundAction) => {
    const name = personName(item.client, item.client_id);
    setAsk({
      title: 'Закрыть возврат?',
      text: 'Система больше не будет менять подписку по этому возврату. Напишите, что сделано с подпиской: пояснение увидят другие администраторы.',
      confirm: 'Закрыть',
      note: 'Что сделано с подпиской',
      run: async (note) => {
        await api(`/api/admin/refunds/${item.request_id}/action/close`, { method: 'POST', body: { note } });
        await load();
        return { text: `${name}: возврат закрыт`, tone: 'ok' };
      },
    });
  };

  return (
    <GroupFrame
      id={TODO_ANCHOR.refunds}
      title="Возвраты, которые не завершились"
      total={items.length}
      lead="Деньги клиенту вернули, но подписку после возврата изменить не получилось. Повторите действие или закройте возврат с пояснением, что сделано с подпиской."
      outcome={outcome}
      error={error}
    >
      {items.map((item) => (
        <div key={item.request_id} className="mo-row">
          <Who
            clientId={item.client_id}
            person={item.client}
            line={[purchase(item.order, 'Заказ'), item.provider_title].filter(Boolean).join(' · ')}
          />
          <span className="mo-sum">
            <strong className="num">{amountText(item.amount)}</strong>
            <span className="muted">{waited(item.confirmed_at)}</span>
          </span>
          <span className="mo-act mo-acts">
            <button type="button" className="btn-line hov" onClick={() => retry(item)}>
              Повторить
            </button>
            <button type="button" className="btn-line hov" onClick={() => close(item)}>
              Закрыть
            </button>
          </span>
          <span className="muted mo-reason">
            Не получилось {actionText(item.action)}
            {item.error ? `: ${item.error}` : ''}
          </span>
        </div>
      ))}
      {ask ? (
        <AskDialog
          ask={ask}
          onClose={() => setAsk(null)}
          onDone={(result) => {
            setAsk(null);
            setOutcome(result);
            changed();
          }}
        />
      ) : null}
    </GroupFrame>
  );
}

function Requests({ report, changed, round }: GroupProps) {
  const { items, error, load } = useList<RefundRequest>('/api/admin/refunds/pending?stuck=true', 'Заявки на возврат не загрузились', round);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  useReport('requests', report, items, outcome, error);
  if (!items || (items.length === 0 && !outcome && !error)) {
    return null;
  }

  const confirm = (item: RefundRequest) => {
    const name = personName(item.client, item.client_id);
    const where = item.provider_title ? `кабинете ${item.provider_title}` : 'кабинете платёжной системы';
    setAsk({
      title: 'Подтвердить возврат вручную?',
      text: `Панель запишет возврат как сделанный${item.subscription_action === 'keep' ? '' : ' и изменит подписку по заявке'}. Подтверждайте, только если возврат виден в ${where}. Пояснение увидят другие администраторы.`,
      confirm: 'Подтвердить',
      note: 'Пояснение',
      run: async (note) => {
        await api(`/api/admin/refunds/${item.request_id}/confirm`, { method: 'POST', body: { note } });
        await load();
        return { text: `${name}: возврат подтверждён вручную`, tone: 'ok' };
      },
    });
  };

  const cancel = (item: RefundRequest) => {
    const name = personName(item.client, item.client_id);
    const where = item.provider_title ? `кабинете ${item.provider_title}` : 'кабинете платёжной системы';
    setAsk({
      title: 'Отменить заявку на возврат?',
      text: `Заявка закроется без возврата${item.held ? ', удержанная сумма вернётся на баланс клиента' : ''}. Отменяйте, если в ${where} возврата нет. Пояснение увидят другие администраторы.`,
      confirm: 'Отменить заявку',
      danger: true,
      note: 'Почему заявка отменяется',
      run: async (reason) => {
        await api(`/api/admin/refunds/${item.request_id}/cancel`, { method: 'POST', body: { reason } });
        await load();
        return { text: `${name}: заявка на возврат отменена`, tone: 'ok' };
      },
    });
  };

  return (
    <GroupFrame
      id={TODO_ANCHOR.requests}
      title="Заявки на возврат без ответа"
      total={items.length}
      lead="Платёжная система не ответила на заявку больше суток. Проверьте возврат в её кабинете: если деньги ушли клиенту, подтвердите заявку, если нет — отмените."
      outcome={outcome}
      error={error}
    >
      {items.map((item) => (
        <div key={item.request_id} className="mo-row">
          <Who
            clientId={item.client_id}
            person={item.client}
            line={[purchase(item.order, 'Заказ'), item.provider_title].filter(Boolean).join(' · ')}
          />
          <span className="mo-sum">
            <strong className="num">{amountText(item.amount)}</strong>
            <span className="muted">{waited(item.requested_at)}</span>
          </span>
          <span className="mo-act mo-acts">
            <button type="button" className="btn-line hov" onClick={() => confirm(item)}>
              Подтвердить вручную
            </button>
            <button type="button" className="btn-line hov" onClick={() => cancel(item)}>
              Отменить
            </button>
          </span>
          {item.reason || item.held ? (
            <span className="muted mo-reason">
              {[item.reason ? `Причина возврата: ${item.reason}` : null, item.held ? 'Сумма удержана с баланса клиента до ответа' : null].filter(Boolean).join('. ')}
            </span>
          ) : null}
        </div>
      ))}
      {ask ? (
        <AskDialog
          ask={ask}
          onClose={() => setAsk(null)}
          onDone={(result) => {
            setAsk(null);
            setOutcome(result);
            changed();
          }}
        />
      ) : null}
    </GroupFrame>
  );
}

function Charges({ report, changed, round }: GroupProps) {
  const { items, error, load } = useList<Charge>('/api/admin/charges/review', 'Списания на проверке не загрузились', round);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  useReport('charges', report, items, outcome, error);
  if (!items || (items.length === 0 && !outcome && !error)) {
    return null;
  }

  const paid = (item: Charge) => {
    const name = personName(item.client, item.client_id);
    setAsk({
      title: 'Отметить списание оплаченным?',
      text: `Отмечайте, только если в кабинете ${item.provider_title} видно, что деньги списаны. Подписка продлится так же, как после обычного автопродления, клиент получит сообщение. Пояснение увидят другие администраторы.`,
      confirm: 'Отметить оплаченным',
      note: 'Пояснение',
      run: async (comment) => {
        const body = await api<{ result: 'accepted' | 'already_applied' }>(`/api/admin/payments/${item.payment_id}/mark-paid`, {
          method: 'POST',
          body: { comment },
        });
        await load();
        return body.result === 'already_applied'
          ? { text: `${name}: оплата уже была засчитана`, tone: 'ok' }
          : { text: `${name}: списание засчитано, заказ передан на выдачу. Если он не выдастся, то появится в заказах на проверке`, tone: 'ok' };
      },
    });
  };

  return (
    <GroupFrame
      id={TODO_ANCHOR.charges}
      title="Списания с карты на проверке"
      total={items.length}
      lead="Касса не сообщила, прошло ли списание за автопродление. Проверьте платёж в её кабинете: если деньги списаны, отметьте его оплаченным. Если нет, ничего делать не нужно: платёж закроется сам."
      outcome={outcome}
      error={error}
    >
      {items.map((item) => (
        <div key={item.payment_id} className="mo-row">
          <Who clientId={item.client_id} person={item.client} line={['Автопродление', purchase(item.order, ''), item.provider_title].filter(Boolean).join(' · ')} />
          <span className="mo-sum">
            <strong className="num">{amountText(item.amount)}</strong>
            <span className="muted">{agoLong(item.charged_at)}</span>
          </span>
          <span className="mo-act mo-acts">
            <button type="button" className="btn-line hov" onClick={() => paid(item)}>
              Отметить оплаченным
            </button>
          </span>
          <span className="muted mo-reason">
            {[item.reason ? `Причина: ${item.reason}` : null, item.closes_at ? `Если денег не списали, платёж закроется сам ${dateLong(item.closes_at)}` : null]
              .filter(Boolean)
              .join('. ')}
          </span>
        </div>
      ))}
      {ask ? (
        <AskDialog
          ask={ask}
          onClose={() => setAsk(null)}
          onDone={(result) => {
            setAsk(null);
            setOutcome(result);
            changed();
          }}
        />
      ) : null}
    </GroupFrame>
  );
}

export function Decisions({ onChanged }: { onChanged: () => void }) {
  const { can, refreshBadges } = useSession();
  const { hash } = useLocation();
  const [shown, setShown] = useState<Partial<Record<Key, Shown>>>({});
  const scrolled = useRef<string | null>(null);
  const canMoney = can('money');
  const canRefunds = can('refunds');

  const report = useCallback<Report>((key, next) => {
    setShown((prev) => {
      const old = prev[key];
      return old && old.visible === next.visible && old.count === next.count ? prev : { ...prev, [key]: next };
    });
  }, []);

  const [round, setRound] = useState(0);
  const later = useRef<number | null>(null);
  const changed = useCallback(() => {
    onChanged();
    refreshBadges();
    setRound((n) => n + 1);
    if (later.current !== null) {
      window.clearTimeout(later.current);
    }
    later.current = window.setTimeout(() => {
      later.current = null;
      onChanged();
      refreshBadges();
      setRound((n) => n + 1);
    }, SETTLE_AFTER);
  }, [onChanged, refreshBadges]);

  useEffect(
    () => () => {
      if (later.current !== null) {
        window.clearTimeout(later.current);
      }
    },
    [],
  );

  const stars = useCallback((visible: boolean, size: number) => report('stars', { visible, count: size }), [report]);

  const keys = useMemo(() => {
    const list: Key[] = [];
    if (canRefunds) {
      list.push('refunds');
    }
    if (canMoney) {
      list.push('orders', 'stars', 'charges');
    }
    if (canRefunds) {
      list.push('requests');
    }
    return list;
  }, [canMoney, canRefunds]);
  const ready = keys.every((key) => shown[key] !== undefined);
  const visible = Object.values(shown).some((s) => s?.visible);
  const total = Object.values(shown).reduce((sum, s) => sum + (s?.count ?? 0), 0);

  useEffect(() => {
    const target = hash.replace(/^#/, '');
    const key = keys.find((k) => TODO_ANCHOR[k] === target);
    if (!key || !ready || scrolled.current === hash || !shown[key]?.visible) {
      return;
    }
    scrolled.current = hash;
    document.getElementById(target)?.scrollIntoView({ block: 'start' });
  }, [hash, keys, ready, shown]);

  if (!canMoney && !canRefunds) {
    return null;
  }

  return (
    <section className="card mn-money mo-todo" aria-labelledby="todo-h" hidden={!visible}>
      <div className="mo-head">
        <h2 id="todo-h" className="section-title">
          Ждут решения{total > 0 ? ` · ${total}` : ''}
        </h2>
      </div>
      {canRefunds ? <Refunds report={report} changed={changed} round={round} /> : null}
      {canMoney ? <Orders report={report} changed={changed} round={round} /> : null}
      {canMoney ? <StarsReview id={TODO_ANCHOR.stars} onChanged={changed} onState={stars} round={round} /> : null}
      {canMoney ? <Charges report={report} changed={changed} round={round} /> : null}
      {canRefunds ? <Requests report={report} changed={changed} round={round} /> : null}
    </section>
  );
}
