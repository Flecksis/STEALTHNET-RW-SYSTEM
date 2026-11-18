import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../../lib/api';
import { dateLong } from '../../lib/format';
import './utm.css';

type ClientUtm = {
  utm: { source: string | null; medium: string | null; campaign: string | null; content: string | null; term: string | null };
  link?: { id: number; title: string };
};

const WHERE: Record<string, string> = {
  bot: 'через бота',
  miniapp: 'через мини-приложение',
  web: 'на сайте',
  admin: 'в админке',
  import: 'перенос из старой панели',
  migration: 'перенос из старой панели',
};

export function registrationText(source: string, created: string): string {
  return `${WHERE[source] ?? source}, ${dateLong(created, true)}`;
}

export function useClientUtm(clientId: number): ClientUtm | null {
  const [found, setFound] = useState<ClientUtm | null>(null);
  useEffect(() => {
    let alive = true;
    api<ClientUtm | null>(`/api/admin/m/utm/clients/${clientId}`)
      .then((data) => {
        if (alive) {
          setFound(data);
        }
      })
      .catch(() => {
        if (alive) {
          setFound(null);
        }
      });
    return () => {
      alive = false;
    };
  }, [clientId]);
  return found;
}

export function UtmValue({ found }: { found: ClientUtm }) {
  const u = found.utm;
  const tags = [u.source, u.medium, u.campaign, u.content, u.term].filter(Boolean).join(' · ');
  return (
    <span className="ut-cc">
      {found.link ? <Link to="/admin/m/utm">{found.link.title}</Link> : null}
      <span className="mono">{tags}</span>
    </span>
  );
}
