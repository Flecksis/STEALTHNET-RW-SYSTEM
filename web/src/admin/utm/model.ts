import { useEffect, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { useSession } from '../session';

export type Stat = {
  source_type: string;
  source_key: string;
  medium: string;
  campaign: string;
  registrations: number;
  trials: number;
  paid: number;
  revenue: number;
  revenue_all: number;
  kept_base: number;
  kept: number;
  clicks: number;
};

export type Link = {
  id: number;
  title: string;
  source: string;
  medium: string | null;
  campaign: string | null;
  content: string | null;
  term: string | null;
  created_at: string;
};

export type LinkRow = { link: Link; param: string; stats: Stat };

export type LinksPage = {
  from: string;
  to: string;
  bot: string | null;
  site: string | null;
  preparing: boolean;
  links: LinkRow[];
  stray: Stat[];
};

export type Draft = { title: string; source: string; medium: string; campaign: string; content: string; term: string };

export const EMPTY: Draft = { title: '', source: '', medium: '', campaign: '', content: '', term: '' };

export const FIELDS: Array<[keyof Draft, string, string, string]> = [
  ['source', 'Источник', 'utm_source — площадка: tiktok, telegram_ads, vk', 'например, tiktok'],
  ['medium', 'Канал', 'utm_medium — вид размещения: video, cpc, post', 'необязательно'],
  ['campaign', 'Кампания', 'utm_campaign — название кампании', 'например, osen-shorts'],
  ['content', 'Объявление', 'utm_content — необязательно', 'необязательно'],
  ['term', 'Ключевое слово', 'utm_term — необязательно', 'необязательно'],
];

const LATIN: Record<string, string> = {
  а: 'a', б: 'b', в: 'v', г: 'g', д: 'd', е: 'e', ё: 'e', ж: 'zh', з: 'z', и: 'i', й: 'y', к: 'k', л: 'l', м: 'm', н: 'n',
  о: 'o', п: 'p', р: 'r', с: 's', т: 't', у: 'u', ф: 'f', х: 'h', ц: 'ts', ч: 'ch', ш: 'sh', щ: 'sch', ъ: '', ы: 'y', ь: '',
  э: 'e', ю: 'yu', я: 'ya',
};

export function tag(value: string): string {
  const latin = [...value.trim()]
    .map((ch) => {
      const low = ch.toLowerCase();
      const t = LATIN[low];
      if (t === undefined) {
        return ch;
      }
      return ch === low ? t : t.charAt(0).toUpperCase() + t.slice(1);
    })
    .join('');
  return latin
    .replace(/\s+/g, '-')
    .replace(/[^A-Za-z0-9_-]/g, '')
    .replace(/_{2,}/g, '_')
    .replace(/^_+|_+$/g, '')
    .slice(0, 64);
}

export function param(d: Draft): string {
  const parts: Array<[string, string]> = [
    ['s_', tag(d.source)],
    ['m_', tag(d.medium)],
    ['k_', tag(d.campaign)],
    ['n_', tag(d.content)],
    ['t_', tag(d.term)],
  ];
  return parts
    .filter(([, v]) => v)
    .map(([p, v]) => `${p}${v}`)
    .join('__');
}

export function siteLink(site: string | null, d: Draft): string | null {
  if (!site) {
    return null;
  }
  const q = new URLSearchParams();
  const add = (k: string, v: string) => {
    const t = tag(v);
    if (t) {
      q.set(k, t);
    }
  };
  add('utm_source', d.source);
  add('utm_medium', d.medium);
  add('utm_campaign', d.campaign);
  add('utm_content', d.content);
  add('utm_term', d.term);
  const base = /^https?:\/\//.test(site) ? site : `https://${site}`;
  return `${base.replace(/\/+$/, '')}/?${q.toString()}`;
}

export type Built = { name: string; url: string | null; hint?: string };

export function links(page: LinksPage | null, d: Draft): Built[] {
  const p = param(d);
  const bot = page?.bot ?? null;
  return [
    { name: 'Бот', url: bot && p ? `https://t.me/${bot}?start=${p}` : null, hint: bot ? undefined : 'Бот ещё не подключён в настройках' },
    {
      name: 'Мини-приложение',
      url: bot && p ? `https://t.me/${bot}?startapp=${p}` : null,
      hint: 'Откроется, если в BotFather у бота включено главное мини-приложение',
    },
    { name: 'Сайт', url: siteLink(page?.site ?? null, d), hint: page?.site ? undefined : 'Адрес сайта не указан в настройках' },
  ];
}

export function draftOf(l: Link): Draft {
  return {
    title: l.title,
    source: l.source,
    medium: l.medium ?? '',
    campaign: l.campaign ?? '',
    content: l.content ?? '',
    term: l.term ?? '',
  };
}

export function draftOfStat(s: Stat): Draft {
  return { ...EMPTY, source: s.source_key, medium: s.medium, campaign: s.campaign };
}

export function tags(parts: Array<string | null | undefined>): string {
  return parts.filter((p) => p).join(' · ');
}

export type Period = 'month' | 'last_month' | '30' | '90' | 'year' | 'all';

export const PERIODS: Array<[Period, string]> = [
  ['month', 'Этот месяц'],
  ['last_month', 'Прошлый месяц'],
  ['30', '30 дней'],
  ['90', '90 дней'],
  ['year', '12 месяцев'],
  ['all', 'Всё время'],
];

function iso(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

export function dates(p: Period): { from: string; to: string } {
  const t = new Date();
  const today = iso(t);
  switch (p) {
    case 'last_month':
      return { from: iso(new Date(t.getFullYear(), t.getMonth() - 1, 1)), to: iso(new Date(t.getFullYear(), t.getMonth(), 0)) };
    case '30':
      return { from: iso(new Date(t.getFullYear(), t.getMonth(), t.getDate() - 29)), to: today };
    case '90':
      return { from: iso(new Date(t.getFullYear(), t.getMonth(), t.getDate() - 89)), to: today };
    case 'year':
      return { from: iso(new Date(t.getFullYear() - 1, t.getMonth() + 1, 1)), to: today };
    case 'all':
      return { from: '2015-01-01', to: today };
    default:
      return { from: iso(new Date(t.getFullYear(), t.getMonth(), 1)), to: today };
  }
}

export type Data = {
  allowed: boolean;
  page: LinksPage | null;
  loading: boolean;
  error: string | null;
  off: boolean;
  reload: () => void;
};

export function useLinks(period: Period): Data {
  const { can } = useSession();
  const allowed = can('marketing');
  const [page, setPage] = useState<LinksPage | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [off, setOff] = useState(false);
  const [round, setRound] = useState(0);
  useEffect(() => {
    if (!allowed) {
      return;
    }
    let alive = true;
    let timer: number | undefined;
    const { from, to } = dates(period);
    const load = () => {
      setLoading(true);
      api<LinksPage>(`/api/admin/m/utm/links?from=${from}&to=${to}`)
        .then((data) => {
          if (!alive) {
            return;
          }
          setPage(data);
          setOff(false);
          setError(null);
          if (data.preparing) {
            timer = window.setTimeout(load, 3000);
          }
        })
        .catch((err: unknown) => {
          if (!alive) {
            return;
          }
          if (err instanceof ApiError && err.code === 'module.disabled') {
            setOff(true);
            return;
          }
          setError(err instanceof ApiError ? err.message : 'Не получилось загрузить метки');
        })
        .finally(() => {
          if (alive) {
            setLoading(false);
          }
        });
    };
    load();
    return () => {
      alive = false;
      if (timer) {
        window.clearTimeout(timer);
      }
    };
  }, [allowed, period, round]);
  return { allowed, page, loading, error, off, reload: () => setRound((n) => n + 1) };
}

export async function save(draft: Draft, id: number | null): Promise<Link> {
  const body = {
    title: draft.title.trim(),
    source: tag(draft.source),
    medium: tag(draft.medium) || null,
    campaign: tag(draft.campaign) || null,
    content: tag(draft.content) || null,
    term: tag(draft.term) || null,
  };
  return id === null
    ? api<Link>('/api/admin/m/utm/links', { method: 'POST', body })
    : api<Link>(`/api/admin/m/utm/links/${id}`, { method: 'PUT', body });
}

export async function archive(id: number): Promise<void> {
  await api(`/api/admin/m/utm/links/${id}/archive`, { method: 'POST', body: {} });
}
