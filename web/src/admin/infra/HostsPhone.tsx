import { Fragment, useEffect, useState } from 'react';
import { api } from '../../lib/api';
import { Flag } from '../../lib/Flag';
import { PhoneHeader } from '../phone';
import { HostForm } from './HostForm';
import { OrderList } from './HostOrder';
import { addressText, failText, groupHosts, groupName, hostsSummary, inboundText, stateNote, switchHost, useHostsData, type HostRow } from './hosts';
import '../form.css';
import '../home/phone.css';
import './infra.css';

export function HostsPhone() {
  const { data, error, load } = useHostsData();
  const [open, setOpen] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [order, setOrder] = useState<string[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);

  useEffect(() => {
    if (!toast) {
      return;
    }
    const timer = window.setTimeout(() => setToast(null), 6000);
    return () => window.clearTimeout(timer);
  }, [toast]);

  const done = (text: string) => {
    setOpen(null);
    setAdding(false);
    setToast(text);
    void load();
  };

  const toggle = async (row: HostRow) => {
    setBusy(row.host.uuid);
    try {
      setToast(await switchHost(row));
      await load();
    } catch (err) {
      setToast(failText(err, 'Не получилось переключить хост'));
    } finally {
      setBusy(null);
    }
  };

  const saveOrder = async () => {
    if (!order) {
      return;
    }
    setBusy('order');
    try {
      await api('/api/admin/infra/hosts/order', { method: 'PUT', body: { uuids: order } });
      setOrder(null);
      setToast('Порядок сохранён');
      await load();
    } catch (err) {
      setToast(failText(err, 'Порядок не сохранился'));
    } finally {
      setBusy(null);
    }
  };

  const rows = data?.hosts ?? [];

  return (
    <div className="hp np hs-phone">
      <PhoneHeader title="Хосты" back={{ to: '/admin/infra', label: 'Назад' }} />
      <p className="np-sum" aria-live="polite">
        {hostsSummary(data, error)}
      </p>
      {error && data ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}

      {order && data ? (
        <section className="hp-group" aria-label="Порядок в приложении">
          <div className="hp-card">
            <OrderList phone data={data} order={order} setOrder={setOrder} busy={busy === 'order'} onSave={() => void saveOrder()} onCancel={() => setOrder(null)} />
          </div>
        </section>
      ) : (
        <>
          {adding && data ? (
            <section className="hp-group" aria-labelledby="hs-new">
              <h2 id="hs-new" className="hp-h2">
                Новый хост
              </h2>
              <div className="hp-card hs-pcard">
                <HostForm phone data={data} row={null} onCancel={() => setAdding(false)} onSaved={(host) => done(`Хост ${host.remark} создан`)} />
              </div>
            </section>
          ) : null}

          {(data ? groupHosts(data) : []).map((group) => {
            const state = stateNote(group.nodes);
            return (
              <section key={group.key || 'none'} className="hp-group" aria-label={groupName(group)}>
                <h2 className="hp-h2 hs-ph2">
                  {group.nodes.map((n) => (
                    <Flag key={n.uuid} code={n.country_code} />
                  ))}
                  <span>{groupName(group)}</span>
                  {state ? <span className={state.tone === 'bad' ? 'hs-pstate bad' : 'hs-pstate'}>{state.text}</span> : null}
                </h2>
                <ul className="hp-card hp-list np-ul">
                  {group.rows.map((row) => {
                    const isOpen = open === row.host.uuid;
                    return (
                      <Fragment key={row.host.uuid}>
                        <li className={row.host.isDisabled ? 'hs-prow hs-off' : 'hs-prow'}>
                          <button
                            type="button"
                            className="hs-pmain press"
                            aria-expanded={isOpen}
                            aria-controls={`host-p-${row.host.uuid}`}
                            onClick={() => {
                              setOpen(isOpen ? null : row.host.uuid);
                              setAdding(false);
                            }}
                          >
                            <span className="hp-title">{row.host.remark}</span>
                            <span className="hp-note muted">
                              <span className="mono hs-addr">{addressText(row.host)}</span> · {data ? inboundText(row, data) : ''}
                              {row.host.isDisabled ? ' · выключен' : row.host.isHidden ? ' · скрыт' : ''}
                            </span>
                          </button>
                          <span className="hs-pswitch">
                            <button
                              type="button"
                              role="switch"
                              className="sw"
                              aria-checked={!row.host.isDisabled}
                              aria-label={`Хост ${row.host.remark} включён`}
                              onClick={() => void toggle(row)}
                              disabled={busy !== null}
                            >
                              <span />
                            </button>
                          </span>
                        </li>
                        {isOpen && data ? (
                          <li className="hs-pedit" id={`host-p-${row.host.uuid}`}>
                            <HostForm
                              key={row.host.uuid}
                              phone
                              data={data}
                              row={row}
                              onCancel={() => setOpen(null)}
                              onSaved={(host) => done(`Хост ${host.remark}: изменения сохранены`)}
                              onDeleted={(remark) => done(`Хост ${remark} удалён`)}
                            />
                          </li>
                        ) : null}
                      </Fragment>
                    );
                  })}
                </ul>
              </section>
            );
          })}

          {data && rows.length === 0 && !adding ? <p className="hp-quiet hp-card">Хостов пока нет. Добавьте первый, чтобы клиенты увидели сервер в приложении.</p> : null}

          {data && !adding ? (
            <div className="hs-pbtns">
              <button
                type="button"
                className="np-ok press"
                onClick={() => {
                  setAdding(true);
                  setOpen(null);
                  window.scrollTo({ top: 0, behavior: 'smooth' });
                }}
              >
                Добавить хост
              </button>
              {rows.length > 1 ? (
                <button
                  type="button"
                  className="np-cancel press"
                  onClick={() => {
                    setOpen(null);
                    setOrder(rows.map((r) => r.host.uuid));
                  }}
                >
                  Изменить порядок
                </button>
              ) : null}
            </div>
          ) : null}
        </>
      )}

      <div role="status" className="np-toast-wrap">
        {toast ? <p className="np-toast">{toast}</p> : null}
      </div>
    </div>
  );
}
