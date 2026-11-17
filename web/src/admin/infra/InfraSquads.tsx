import { useCallback, useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { count, groupDigits, plural } from '../../lib/format';
import { Flag } from '../../lib/Flag';
import { InfraHead } from './InfraHead';
import { inboundLabel } from './InfraNodes';
import '../form.css';
import './infra.css';

type Inbound = { uuid: string; tag: string; kind: string; security: string | null; network: string | null; port: number | null };
type RawInbound = { uuid: string; tag: string; kind?: string; type?: string; security?: string | null; network?: string | null; port?: number | null };
type SquadData = {
  squad: { uuid: string; name: string; info: { membersCount: number; inboundsCount: number }; inbounds: RawInbound[] };
  tariffs: Array<{ id: number; name: string; on_sale: boolean }>;
};
type NodeLite = {
  node: { uuid: string; name: string; countryCode: string; address: string };
  state: 'online' | 'connecting' | 'down' | 'disabled';
  city: string | null;
  inbounds: Inbound[];
  profile_uuid: string | null;
};
type Host = {
  uuid: string;
  viewPosition: number;
  remark: string;
  address: string;
  isDisabled: boolean;
  isHidden: boolean;
  inbound: { configProfileUuid: string | null; configProfileInboundUuid: string | null };
  nodes: string[];
  internalSquads: { mode: string; squads: string[] } | null;
};
type HostDraft = { remark?: string; shown?: boolean };

const SQUAD_NAME = /^[A-Za-z0-9_ -]{2,30}$/;

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function normal(inbound: RawInbound): Inbound {
  return {
    uuid: inbound.uuid,
    tag: inbound.tag,
    kind: inbound.kind ?? inbound.type ?? '',
    security: inbound.security ?? null,
    network: inbound.network ?? null,
    port: inbound.port ?? null,
  };
}

function shownTo(host: Host, squad: string | null): boolean {
  if (host.isDisabled || host.isHidden) {
    return false;
  }
  const filter = host.internalSquads;
  if (!filter) {
    return true;
  }
  const listed = squad !== null && filter.squads.includes(squad);
  return filter.mode === 'ALLOW_ONLY' ? listed : !listed;
}

function hostOf(host: Host, inbounds: Set<string>): boolean {
  return host.inbound.configProfileInboundUuid !== null && inbounds.has(host.inbound.configProfileInboundUuid);
}

function nodesOfHost(host: Host, nodes: NodeLite[]): NodeLite[] {
  const bound = nodes.filter((n) => host.nodes.includes(n.node.uuid));
  if (bound.length) {
    return bound;
  }
  const byAddress = nodes.filter((n) => n.node.address === host.address);
  if (byAddress.length) {
    return byAddress;
  }
  const serving = nodes.filter((n) => n.inbounds.some((i) => i.uuid === host.inbound.configProfileInboundUuid));
  return serving.length === 1 ? serving : [];
}

function joinAnd(items: string[]): string {
  return items.length < 2 ? (items[0] ?? '') : `${items.slice(0, -1).join(', ')} и ${items[items.length - 1]}`;
}

function people(n: number): string {
  return count(n, 'клиент', 'клиента', 'клиентов');
}

export function InfraSquads() {
  const [squads, setSquads] = useState<SquadData[] | null>(null);
  const [nodes, setNodes] = useState<NodeLite[]>([]);
  const [hosts, setHosts] = useState<Host[] | null>([]);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [saved, setSaved] = useState<{ uuid: string; created: boolean } | null>(null);

  const load = useCallback(async (first: boolean) => {
    try {
      const [list, nodeData, hostList] = await Promise.all([
        api<SquadData[]>('/api/admin/infra/squads'),
        api<{ nodes: NodeLite[] }>('/api/admin/infra/nodes').catch(() => ({ nodes: [] as NodeLite[] })),
        api<Host[]>('/api/admin/infra/hosts').catch(() => null),
      ]);
      setSquads(list);
      setNodes(nodeData.nodes);
      setHosts(hostList ? [...hostList].sort((a, b) => a.viewPosition - b.viewPosition) : null);
      setError(null);
      if (first && list[0]) {
        setEditing((current) => current ?? list[0]?.squad.uuid ?? null);
      }
    } catch (err) {
      setError(failText(err, 'Сквады не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load(true);
  }, [load]);

  const open = (uuid: string) => {
    setSaved(null);
    setEditing(editing === uuid ? null : uuid);
  };

  const finish = async (uuid: string, created: boolean) => {
    setEditing(null);
    await load(false);
    setSaved({ uuid, created });
  };

  return (
    <div className="inf">
      <InfraHead
        current="squads"
        squads={squads?.length}
        action={
          <button type="button" className={editing === 'new' ? 'btn-line hov' : 'btn-main btn'} aria-expanded={editing === 'new'} onClick={() => open('new')}>
            Новый сквад
          </button>
        }
      />
      <p className="muted">Сквад — набор входов на нодах. Тариф даёт клиенту доступ к сквадам, клиент видит в приложении их серверы.</p>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <section className="inf-card" aria-label="Сквады">
        {editing === 'new' ? (
          <div className="sq-item">
            <div className="sq-row">
              <h2 className="inf-h2">Новый сквад</h2>
            </div>
            <SquadEditor squad={null} nodes={nodes} hosts={hosts} onCancel={() => setEditing(null)} onSaved={(uuid) => finish(uuid, true)} />
          </div>
        ) : null}
        {squads === null && !error ? <p className="inf-empty">Загружаем сквады…</p> : null}
        {squads?.length === 0 ? <p className="inf-empty">Сквадов пока нет. Создайте первый, чтобы выдать клиентам серверы.</p> : null}
        {squads?.map((item) => {
          const inbSet = new Set(item.squad.inbounds.map((i) => i.uuid));
          const own = nodes.filter((n) => n.inbounds.some((i) => inbSet.has(i.uuid)));
          const squadHosts = (hosts ?? []).filter((h) => hostOf(h, inbSet));
          const visible = [...new Set(squadHosts.filter((h) => shownTo(h, item.squad.uuid)).map((h) => h.remark.trim()).filter(Boolean))];
          const inbounds = item.squad.inbounds.length || item.squad.info.inboundsCount;
          const clients = item.squad.info.membersCount;
          const app =
            own.length === 0 ? { text: 'нет серверов', warn: true } : hosts === null ? { text: '—', warn: false } : visible.length === 0 ? { text: squadHosts.length ? 'все серверы скрыты' : 'нет хостов', warn: true } : { text: visible.join(', '), warn: false };
          const isOpen = editing === item.squad.uuid;
          return (
            <div key={item.squad.uuid} className="sq-item">
              <div className="sq-row">
                <div className="inf-col">
                  <h2 className="inf-h2">{item.squad.name}</h2>
                  <span className="muted">
                    {count(inbounds, 'вход', 'входа', 'входов')} · {clients ? people(clients) : 'клиентов пока нет'}
                  </span>
                </div>
                <div className="sq-facts">
                  <span className="muted">Ноды</span>
                  <span>
                    {own.length === 0 ? (
                      <span className="muted">—</span>
                    ) : (
                      own.map((n, i) => (
                        <span key={n.node.uuid}>
                          {i > 0 ? ', ' : ''}
                          {n.state === 'down' ? <span className="sq-bad">{n.node.name} не отвечает</span> : n.node.name}
                        </span>
                      ))
                    )}
                  </span>
                  <span className="muted">Тарифы</span>
                  <span>{item.tariffs.length ? item.tariffs.map((t) => t.name).join(', ') : <span className="muted">ни в одном</span>}</span>
                  <span className="muted">В приложении</span>
                  <span className={app.warn ? 'sq-warn' : undefined}>{app.text}</span>
                </div>
                <button type="button" className="b40 hov" aria-expanded={isOpen} aria-label={`${isOpen ? 'Свернуть' : 'Изменить'} сквад ${item.squad.name}`} onClick={() => open(item.squad.uuid)}>
                  {isOpen ? 'Свернуть' : 'Изменить'}
                </button>
              </div>
              {saved?.uuid === item.squad.uuid && !isOpen ? (
                <div className="sq-note-wrap">
                  <p role="status" className="sq-note">
                    {saved.created ? (
                      <>
                        <strong>Сквад создан.</strong> Добавьте его в тариф, чтобы клиенты получили доступ.{' '}
                        <Link to="/admin/tariffs" className="hm-link">
                          Открыть тарифы
                        </Link>
                      </>
                    ) : (
                      <>
                        <strong>Сохранено.</strong> Изменения отправлены в Remnawave.
                      </>
                    )}
                  </p>
                </div>
              ) : null}
              {isOpen ? <SquadEditor squad={item} nodes={nodes} hosts={hosts} onCancel={() => setEditing(null)} onSaved={(uuid) => finish(uuid, false)} /> : null}
            </div>
          );
        })}
      </section>
    </div>
  );
}

type EditorProps = {
  squad: SquadData | null;
  nodes: NodeLite[];
  hosts: Host[] | null;
  onCancel: () => void;
  onSaved: (uuid: string) => Promise<void>;
};

function SquadEditor({ squad, nodes, hosts, onCancel, onSaved }: EditorProps) {
  const squadId = squad?.squad.uuid ?? null;
  const original = useMemo(() => (squad?.squad.inbounds ?? []).map(normal), [squad]);
  const [name, setName] = useState(squad?.squad.name ?? '');
  const [chosen, setChosen] = useState<string[]>(original.map((i) => i.uuid));
  const [drafts, setDrafts] = useState<Record<string, HostDraft>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const isNew = squad === null;
  const clients = squad?.squad.info.membersCount ?? 0;

  const order = useMemo(() => {
    const start = new Set(original.map((i) => i.uuid));
    const mine = nodes.filter((n) => n.inbounds.some((i) => start.has(i.uuid)));
    return [...mine, ...nodes.filter((n) => !mine.includes(n))];
  }, [nodes, original]);

  const orphans = useMemo(() => {
    const served = new Set(nodes.flatMap((n) => n.inbounds.map((i) => i.uuid)));
    return original.filter((i) => !served.has(i.uuid));
  }, [nodes, original]);

  const shared = useMemo(() => {
    const byProfile = new Map<string, string[]>();
    for (const n of nodes) {
      if (n.profile_uuid && n.inbounds.length) {
        byProfile.set(n.profile_uuid, [...(byProfile.get(n.profile_uuid) ?? []), n.node.name]);
      }
    }
    return [...byProfile.values()].filter((names) => names.length > 1);
  }, [nodes]);

  const chosenSet = new Set(chosen);
  const hostRows = (hosts ?? [])
    .filter((h) => hostOf(h, chosenSet))
    .map((host) => {
      const draft = drafts[host.uuid] ?? {};
      const label = nodesOfHost(host, nodes)
        .map((n) => n.node.name)
        .join(', ');
      return { host, label: label || host.address, remark: draft.remark ?? host.remark, shown: draft.shown ?? shownTo(host, squadId) };
    });

  const toggle = (uuid: string) => setChosen(chosenSet.has(uuid) ? chosen.filter((x) => x !== uuid) : [...chosen, uuid]);
  const patchHost = (uuid: string, patch: HostDraft) => setDrafts({ ...drafts, [uuid]: { ...drafts[uuid], ...patch } });

  const trimmed = name.trim();
  const nameOk = SQUAD_NAME.test(trimmed);
  const remarkEmpty = hostRows.some((r) => r.remark.trim() === '');
  const blocked = !nameOk || (isNew && chosen.length === 0) || remarkEmpty;

  const caption = !trimmed
    ? 'Введите название сквада'
    : !nameOk
      ? 'Название сквада: латиница, цифры, пробел, дефис или _, до 30 символов'
      : isNew && chosen.length === 0
        ? 'Нужны название и хотя бы один вход'
        : remarkEmpty
          ? 'У каждого сервера должно быть название'
          : isNew
            ? 'Сквад можно будет выбрать в тарифе'
            : clients
              ? `Изменения сразу ${plural(clients, 'получит', 'получат', 'получат')} ${groupDigits(clients)} ${plural(clients, 'клиент', 'клиента', 'клиентов')} сквада`
              : 'В скваде пока нет клиентов';

  const hostNote =
    hosts === null
      ? { text: 'Remnawave не отдал список серверов', warn: false }
      : chosen.length === 0
        ? { text: 'Серверы появятся, когда отметите входы', warn: false }
        : hostRows.length === 0
          ? { text: 'У отмеченных входов нет хостов: добавьте их в Remnawave', warn: true }
          : hostRows.every((r) => !r.shown)
            ? { text: 'Все серверы скрыты: клиенты сквада не увидят ни одного', warn: true }
            : null;

  const save = async () => {
    if (blocked) {
      return;
    }
    setBusy(true);
    setError(null);
    const changes = hostRows.flatMap(({ host, remark, shown }) => {
      const change: { uuid: string; remark?: string; shown?: boolean } = { uuid: host.uuid };
      if (remark.trim() !== host.remark) {
        change.remark = remark.trim();
      }
      if (shown !== shownTo(host, squadId)) {
        change.shown = shown;
      }
      return change.remark !== undefined || change.shown !== undefined ? [change] : [];
    });
    try {
      const body = { name: trimmed, inbounds: chosen, hosts: changes };
      const result = isNew
        ? await api<{ uuid: string }>('/api/admin/infra/squads', { method: 'POST', body })
        : await api<{ uuid: string }>(`/api/admin/infra/squads/${squad.squad.uuid}`, { method: 'PUT', body });
      await onSaved(result.uuid);
    } catch (err) {
      setError(failText(err, 'Сквад не сохранился'));
      setBusy(false);
    }
  };

  return (
    <div className="sq-editor">
      <label className="sq-name">
        <span className="sq-legend">Название</span>
        <input
          className="field"
          value={name}
          maxLength={30}
          onChange={(e) => setName(e.target.value)}
          placeholder="Например, Europe"
          aria-invalid={trimmed !== '' && !nameOk}
          aria-describedby="sq-name-hint"
          disabled={busy}
        />
        <span id="sq-name-hint" className="cap">
          Только латиница, цифры, пробел, - и _
        </span>
      </label>
      <div className="sq-columns">
        <fieldset className="sq-set sq-inbounds">
          <legend className="sq-legend">Входы</legend>
          {order.map((n) => (
            <div key={n.node.uuid} role="group" aria-label={[n.node.name, n.city].filter(Boolean).join(', ')} className="sq-group">
              <span className="sq-node">
                <strong>
                  <Flag code={n.node.countryCode} />
                  {n.node.name}
                </strong>
                {n.city ? <span className="muted">{n.city}</span> : null}
                {n.state === 'down' ? <span className="sq-down">не отвечает</span> : null}
              </span>
              <span className="sq-checks">
                {n.inbounds.length ? (
                  n.inbounds.map((i) => (
                    <label key={i.uuid} className="sq-check">
                      <input type="checkbox" checked={chosenSet.has(i.uuid)} onChange={() => toggle(i.uuid)} disabled={busy} />
                      <span>{inboundLabel(i)}</span>
                    </label>
                  ))
                ) : (
                  <span className="muted">входов нет</span>
                )}
              </span>
            </div>
          ))}
          {orphans.length ? (
            <div role="group" aria-label="Входы без нод" className="sq-group">
              <span className="sq-node">
                <strong>Без ноды</strong>
              </span>
              <span className="sq-checks">
                {orphans.map((i) => (
                  <label key={i.uuid} className="sq-check">
                    <input type="checkbox" checked={chosenSet.has(i.uuid)} onChange={() => toggle(i.uuid)} disabled={busy} />
                    <span>{inboundLabel(i)}</span>
                  </label>
                ))}
              </span>
            </div>
          ) : null}
          {order.length === 0 && orphans.length === 0 ? <p className="sq-foot muted">В Remnawave пока нет нод с входами</p> : null}
          {shared.length ? (
            <p className="sq-foot cap">
              {shared.map((names) => `${joinAnd(names)} на одном профиле: вход отмечается сразу у всех`).join('. ')}
            </p>
          ) : null}
          {!isNew && chosen.length === 0 ? <p className="sq-foot sq-warn">Без входов у клиентов сквада не будет серверов</p> : null}
        </fieldset>
        <fieldset className="sq-set sq-hosts">
          <legend className="sq-legend">Что видят клиенты</legend>
          {hostRows.map(({ host, label, remark, shown }) => (
            <div key={host.uuid} className="sq-host">
              <span className="sq-code">{label}</span>
              <label className="sq-remark">
                <span className="sr-only">Название сервера {label} в приложении</span>
                <input className="field" value={remark} maxLength={100} onChange={(e) => patchHost(host.uuid, { remark: e.target.value })} aria-invalid={remark.trim() === ''} disabled={busy} />
              </label>
              <button
                type="button"
                role="switch"
                className="sq-show"
                aria-checked={shown}
                aria-label={`Показывать клиентам: ${remark.trim() || label}`}
                onClick={() => patchHost(host.uuid, { shown: !shown })}
                disabled={busy}
              >
                <span className="sw" aria-hidden="true">
                  <span />
                </span>
                <span>Показывать</span>
              </button>
            </div>
          ))}
          {hostNote ? <p className={hostNote.warn ? 'sq-foot sq-warn' : 'sq-foot muted'}>{hostNote.text}</p> : null}
        </fieldset>
      </div>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="sq-actions">
        <span className="cc-btns">
          <button type="button" className="btn-main btn" onClick={() => void save()} aria-disabled={blocked} disabled={busy}>
            {busy ? 'Сохраняем…' : isNew ? 'Создать сквад' : 'Сохранить'}
          </button>
          <button type="button" className="btn-line hov sq-cancel" onClick={onCancel} disabled={busy}>
            Отменить
          </button>
        </span>
        <span className="muted">{caption}</span>
      </div>
    </div>
  );
}
