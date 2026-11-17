import { Fragment, useState } from 'react';
import { api } from '../../lib/api';
import { Flag } from '../../lib/Flag';
import { usePhone } from '../../lib/media';
import { HostForm } from './HostForm';
import { OrderList } from './HostOrder';
import { HostsPhone } from './HostsPhone';
import { InfraHead } from './InfraHead';
import {
  addressText,
  failText,
  groupHosts,
  groupName,
  hostsCount,
  hostsSummary,
  inboundOf,
  inboundText,
  stateNote,
  switchHost,
  useHostsData,
  type HostRow,
} from './hosts';
import '../form.css';
import './infra.css';

export function InfraHosts() {
  const phone = usePhone();
  return phone ? <HostsPhone /> : <InfraHostsDesktop />;
}

function InfraHostsDesktop() {
  const { data, error, load } = useHostsData();
  const [editing, setEditing] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [order, setOrder] = useState<string[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const toggle = async (row: HostRow) => {
    setBusy(row.host.uuid);
    setNote(null);
    try {
      setNote(await switchHost(row));
      await load();
    } catch (err) {
      setNote(failText(err, 'Не получилось переключить хост'));
    } finally {
      setBusy(null);
    }
  };

  const saveOrder = async () => {
    if (!order) {
      return;
    }
    setBusy('order');
    setNote(null);
    try {
      await api('/api/admin/infra/hosts/order', { method: 'PUT', body: { uuids: order } });
      setOrder(null);
      setNote('Порядок сохранён');
      await load();
    } catch (err) {
      setNote(failText(err, 'Порядок не сохранился'));
    } finally {
      setBusy(null);
    }
  };

  const groups = data ? groupHosts(data) : [];
  const rows = data?.hosts ?? [];

  return (
    <div className="inf">
      <InfraHead
        current="hosts"
        hosts={data?.hosts.length}
        action={
          <button
            type="button"
            className={adding ? 'btn-line hov' : 'btn-main btn'}
            aria-expanded={adding}
            aria-controls="add-host"
            onClick={() => {
              setAdding(!adding);
              setOrder(null);
            }}
            disabled={!data}
          >
            Добавить хост
          </button>
        }
      />
      <p className="muted">Хост — адрес сервера с названием, под которым клиент видит его в приложении. Порядок хостов — порядок серверов у клиента.</p>

      {adding && data ? (
        <section id="add-host" className="inf-card inf-add" aria-label="Новый хост">
          <div className="inf-col" style={{ gap: 2 }}>
            <h2 className="inf-h2">Новый хост</h2>
            <span className="muted">Если вход есть только на одной ноде, название, адрес и порт подставятся из неё</span>
          </div>
          <HostForm
            data={data}
            row={null}
            onCancel={() => setAdding(false)}
            onSaved={(host) => {
              setAdding(false);
              setNote(`Хост ${host.remark} создан`);
              void load();
            }}
          />
        </section>
      ) : null}

      <div className="inf-col" style={{ gap: 10 }}>
        <p className="inf-summary" aria-live="polite">
          {hostsSummary(data, error)}
        </p>
        {note ? (
          <p role="status" className="cap" style={{ color: 'var(--ink-2)' }}>
            {note}
          </p>
        ) : null}
        {error && data ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : null}
      </div>

      {data && rows.length > 0 ? (
        <section className="inf-card" aria-label="Хосты">
          {order ? (
            <OrderList data={data} order={order} setOrder={setOrder} busy={busy === 'order'} onSave={() => void saveOrder()} onCancel={() => setOrder(null)} />
          ) : (
            <>
              <div className="hs-bar">
                <span className="muted">По нодам, внутри — в порядке приложения</span>
                <button
                  type="button"
                  className="b36 hov"
                  onClick={() => {
                    setEditing(null);
                    setAdding(false);
                    setNote(null);
                    setOrder(rows.map((r) => r.host.uuid));
                  }}
                  disabled={rows.length < 2}
                >
                  Изменить порядок
                </button>
              </div>
              <div className="inf-scroll">
                <div role="table" aria-label="Хосты" className="hs-table">
                  <div role="row" className="hs-row head">
                    <span role="columnheader">Хост</span>
                    <span role="columnheader">Адрес</span>
                    <span role="columnheader">Вход</span>
                    <span role="columnheader">Включён</span>
                    <span role="columnheader">
                      <span className="sr-only">Действия</span>
                    </span>
                  </div>
                  {groups.map((group) => {
                    const state = stateNote(group.nodes);
                    return (
                      <div key={group.key || 'none'} role="rowgroup" className="hs-group">
                        <div role="row" className="hs-ghead">
                          <span role="rowheader" className="hs-gname">
                            {group.nodes.map((n) => (
                              <Flag key={n.uuid} code={n.country_code} />
                            ))}
                            <strong>{groupName(group)}</strong>
                            {state ? <span className={state.tone === 'bad' ? 'sq-bad' : 'muted'}>{state.text}</span> : null}
                            {group.rows.length > 1 ? <span className="muted">{hostsCount(group.rows.length)}</span> : null}
                          </span>
                        </div>
                        {group.rows.map((row) => {
                          const isOpen = editing === row.host.uuid;
                          const shown = row.squads.filter((s) => s.shown).map((s) => s.name);
                          const sub = row.host.isHidden ? 'скрыт от клиентов' : shown.length ? `${shown.length === 1 ? 'сквад' : 'сквады'} ${shown.join(', ')}` : 'ни в одном скваде';
                          return (
                            <Fragment key={row.host.uuid}>
                              <div role="row" className={['hs-row', isOpen ? 'open' : '', row.host.isDisabled ? 'hs-off' : ''].join(' ').trim()}>
                                <span role="cell" className="inf-col">
                                  <strong className="hs-remark">{row.host.remark}</strong>
                                  <span className={!row.host.isHidden && shown.length === 0 ? 'inf-sub hs-warn' : 'inf-sub'}>{sub}</span>
                                </span>
                                <span role="cell" className="inf-col">
                                  <span className="mono hs-addr">{addressText(row.host)}</span>
                                  {row.host.sni ? <span className="inf-sub mono hs-addr">SNI {row.host.sni}</span> : null}
                                </span>
                                <span role="cell" className={inboundOf(row, data) ? 'hs-ink2' : 'muted'}>
                                  {inboundText(row, data)}
                                </span>
                                <span role="cell" className="hs-switch">
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
                                  <span className="muted">{busy === row.host.uuid ? '…' : row.host.isDisabled ? 'выключен' : ''}</span>
                                </span>
                                <span role="cell" className="hs-act">
                                  <button
                                    type="button"
                                    className="b36 hov"
                                    aria-expanded={isOpen}
                                    aria-controls={`host-edit-${row.host.uuid}`}
                                    aria-label={`${isOpen ? 'Свернуть' : 'Изменить'} хост ${row.host.remark}`}
                                    onClick={() => {
                                      setEditing(isOpen ? null : row.host.uuid);
                                      setNote(null);
                                    }}
                                  >
                                    {isOpen ? 'Свернуть' : 'Изменить'}
                                  </button>
                                </span>
                              </div>
                              {isOpen ? (
                                <div role="row" className="hs-details" id={`host-edit-${row.host.uuid}`}>
                                  <div role="cell">
                                    <HostForm
                                      key={row.host.uuid}
                                      data={data}
                                      row={row}
                                      onCancel={() => setEditing(null)}
                                      onSaved={(host) => {
                                        setEditing(null);
                                        setNote(`Хост ${host.remark}: изменения сохранены`);
                                        void load();
                                      }}
                                      onDeleted={(remark) => {
                                        setEditing(null);
                                        setNote(`Хост ${remark} удалён`);
                                        void load();
                                      }}
                                    />
                                  </div>
                                </div>
                              ) : null}
                            </Fragment>
                          );
                        })}
                      </div>
                    );
                  })}
                </div>
              </div>
            </>
          )}
        </section>
      ) : null}

      {data && rows.length === 0 && !adding ? (
        <section className="inf-card">
          <p className="inf-empty hs-empty">Хостов пока нет. Добавьте первый, чтобы клиенты увидели сервер в приложении.</p>
        </section>
      ) : null}
    </div>
  );
}
