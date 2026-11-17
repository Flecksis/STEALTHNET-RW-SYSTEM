import { useEffect, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { api } from '../../lib/api';
import { useSession } from '../session';

type Props = { current: 'nodes' | 'squads' | 'hosts' | 'billing'; nodes?: number; squads?: number; hosts?: number; action?: ReactNode };

let panelCache: string | null | undefined;

export function useRemnaPanel(): string | null {
  const { can } = useSession();
  const [panel, setPanel] = useState<string | null>(panelCache ?? null);
  useEffect(() => {
    if (!can('settings') || panelCache !== undefined) {
      return;
    }
    api<{ base_url?: string | null; url?: string | null }>('/api/admin/settings/remnawave')
      .then((r) => {
        panelCache = r.base_url ?? r.url ?? null;
        setPanel(panelCache);
      })
      .catch(() => {
        panelCache = null;
      });
  }, [can]);
  return panel;
}

export function InfraHead({ current, nodes, squads, hosts, action }: Props) {
  const panel = useRemnaPanel();
  const [squadCount, setSquadCount] = useState<number | null>(squads ?? null);
  const [nodeCount, setNodeCount] = useState<number | null>(nodes ?? null);
  const [hostCount, setHostCount] = useState<number | null>(hosts ?? null);

  useEffect(() => {
    if (squads !== undefined) {
      setSquadCount(squads);
      return;
    }
    api<unknown[]>('/api/admin/infra/squads')
      .then((list) => setSquadCount(list.length))
      .catch(() => setSquadCount(null));
  }, [squads]);

  useEffect(() => {
    if (nodes !== undefined) {
      setNodeCount(nodes);
      return;
    }
    api<{ nodes: unknown[] }>('/api/admin/infra/nodes')
      .then((r) => setNodeCount(r.nodes.length))
      .catch(() => setNodeCount(null));
  }, [nodes]);

  useEffect(() => {
    if (hosts !== undefined) {
      setHostCount(hosts);
      return;
    }
    if (current === 'hosts') {
      return;
    }
    api<{ hosts: unknown[] }>('/api/admin/infra/hosts')
      .then((r) => setHostCount(r.hosts.length))
      .catch(() => setHostCount(null));
  }, [hosts, current]);

  const tabs: Array<[Props['current'], string, string, number | null]> = [
    ['nodes', 'Ноды', '/admin/infra', nodeCount],
    ['squads', 'Сквады', '/admin/infra/squads', squadCount],
    ['hosts', 'Хосты', '/admin/infra/hosts', hostCount],
    ['billing', 'Оплата серверов', '/admin/infra/billing', null],
  ];

  return (
    <div className="inf-col" style={{ gap: 14 }}>
      <div className="inf-head">
        <h1 className="page-title tight" style={{ flex: '1 1 auto' }}>
          Инфраструктура
        </h1>
        {panel ? (
          <a className="inf-panel" href={panel} target="_blank" rel="noreferrer">
            Открыть в Remnawave
          </a>
        ) : null}
        {action}
      </div>
      <nav aria-label="Подразделы инфраструктуры" className="chips">
        {tabs.map(([key, label, to, n]) => (
          <Link key={key} to={to} className="chip inf-tab" aria-current={current === key ? 'page' : undefined}>
            <span>{label}</span>
            {n !== null ? <span className="chip-n">{n}</span> : null}
          </Link>
        ))}
      </nav>
    </div>
  );
}
