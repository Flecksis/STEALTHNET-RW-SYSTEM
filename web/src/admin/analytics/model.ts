import { useEffect, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { count, money, NBSP, plural } from '../../lib/format';
import type { Currency } from '../../lib/types';
import { useSession } from '../session';

export type MonthRow = {
  month: string;
  revenue: number;
  stars: number;
  refunds: number;
  payments: number;
  mrr: number;
  new: number;
  renewed: number;
  returned: number;
  churned: number;
  paying: number;
  trials: number;
  trials_bought: number;
};

export type Totals = {
  revenue: number;
  stars: number;
  refunds: number;
  payments: number;
  new: number;
  renewed: number;
  returned: number;
  churned: number;
  trials: number;
  trials_bought: number;
  mrr: number;
  paying: number;
  ltv: number;
  ltv_clients: number;
};

export type Cohort = {
  month: string;
  size: number;
  revenue: number;
  retained: Array<number | null>;
};

export type MethodRow = {
  code: string | null;
  title: string;
  revenue: number;
  stars: number;
  payments: number;
  invoices: number;
  failed: number;
  refunds: number;
  refunded: number;
};

export type ProviderRow = MethodRow & {
  provider: string;
  methods: MethodRow[];
};

export type SourceRow = {
  source: string | null;
  clients: number;
  paying: number;
  revenue: number;
  stars: number;
};

export type Report = {
  currency: Currency;
  from: string;
  to: string;
  current: string;
  months: MonthRow[];
  totals: Totals;
  cohorts: Cohort[];
  providers: ProviderRow[];
  sources: SourceRow[];
  source_options: string[];
};

export type Period = '3' | '6' | '12' | 'all';

export const PERIODS: Array<[Period, string]> = [
  ['3', '3 мес'],
  ['6', '6 мес'],
  ['12', '12 мес'],
  ['all', 'Всё время'],
];

export type Tab = 'money' | 'clients' | 'cohorts' | 'pay' | 'sources';

export const TABS: Array<[Tab, string]> = [
  ['money', 'Деньги'],
  ['clients', 'Клиенты'],
  ['cohorts', 'Удержание'],
  ['pay', 'Платёжки'],
  ['sources', 'Источники'],
];

const MONTHS = ['январь', 'февраль', 'март', 'апрель', 'май', 'июнь', 'июль', 'август', 'сентябрь', 'октябрь', 'ноябрь', 'декабрь'];
const MONTHS_SHORT = ['янв', 'фев', 'мар', 'апр', 'май', 'июн', 'июл', 'авг', 'сен', 'окт', 'ноя', 'дек'];

function parts(month: string): [number, number] {
  const [y = 1970, m = 1] = month.split('-').map(Number);
  return [y, m - 1];
}

export const EMPTY_MONTH: MonthRow = {
  month: '1970-01',
  revenue: 0,
  stars: 0,
  refunds: 0,
  payments: 0,
  mrr: 0,
  new: 0,
  renewed: 0,
  returned: 0,
  churned: 0,
  paying: 0,
  trials: 0,
  trials_bought: 0,
};

export function monthAt(months: MonthRow[], index: number): MonthRow {
  return months[index] ?? months[months.length - 1] ?? EMPTY_MONTH;
}

export function monthName(month: string, withYear = false): string {
  const [y, m] = parts(month);
  const name = MONTHS[m] ?? month;
  const cap = name.charAt(0).toUpperCase() + name.slice(1);
  return withYear ? `${cap} ${y}` : cap;
}

export function monthShort(month: string, withYear = false): string {
  const [y, m] = parts(month);
  const name = MONTHS_SHORT[m] ?? month;
  return withYear ? `${name} ${String(y).slice(2)}` : name;
}

export function sum(value: number, currency: Currency): string {
  return money({ minor: value, currency }, true);
}

export function stars(value: number): string {
  return count(value, 'звезда', 'звезды', 'звёзд');
}

export function compact(value: number, currency: Currency): string {
  const major = value / 100;
  const abs = Math.abs(major);
  const tail = currency === 'RUB' ? `${NBSP}₽` : '';
  const head = currency === 'USD' ? '$' : '';
  if (abs >= 1_000_000) {
    return `${head}${(major / 1_000_000).toFixed(abs >= 10_000_000 ? 0 : 1).replace('.', ',')}${NBSP}млн${tail}`;
  }
  if (abs >= 1_000) {
    return `${head}${(major / 1_000).toFixed(abs >= 10_000 ? 0 : 1).replace('.', ',')}${NBSP}тыс${tail}`;
  }
  return `${head}${Math.round(major)}${tail}`;
}

export function percent(part: number, whole: number): string {
  if (!whole) {
    return '—';
  }
  const value = (part / whole) * 100;
  if (value > 0 && value < 0.1) {
    return `<0,1${NBSP}%`;
  }
  if (value > 0 && value < 1) {
    return `${value.toFixed(1).replace('.', ',')}${NBSP}%`;
  }
  return `${Math.round(value)}${NBSP}%`;
}

export function share(part: number, whole: number): number {
  return whole > 0 ? Math.max(0, Math.min(1, part / whole)) : 0;
}

export function avgCheck(revenue: number, payments: number): number {
  return payments > 0 ? Math.round(revenue / payments) : 0;
}

export function people(n: number): string {
  return count(n, 'клиент', 'клиента', 'клиентов');
}

export function sourceName(source: string | null): string {
  return source ?? 'Без метки';
}

export function moneyLine(revenue: number, starsValue: number, currency: Currency): string {
  if (revenue === 0 && starsValue > 0) {
    return stars(starsValue);
  }
  return starsValue > 0 ? `${sum(revenue, currency)} и${NBSP}${stars(starsValue)}` : sum(revenue, currency);
}

export function periodText(period: Period, months: number): string {
  if (period === 'all') {
    return 'всё время';
  }
  return `последние ${months}${NBSP}${plural(months, 'месяц', 'месяца', 'месяцев')}`;
}

export function periodQuery(period: Period, source: string): string {
  const params = new URLSearchParams();
  if (period === 'all') {
    params.set('all', 'true');
  } else {
    params.set('months', period);
  }
  if (source === 'none') {
    params.set('source', '');
  } else if (source !== 'all') {
    params.set('source', source);
  }
  return params.toString();
}

export type Analytics = {
  allowed: boolean;
  report: Report | null;
  loading: boolean;
  error: string | null;
  off: boolean;
  period: Period;
  setPeriod: (period: Period) => void;
  source: string;
  setSource: (source: string) => void;
  reload: () => void;
};

export function useAnalytics(): Analytics {
  const { can } = useSession();
  const allowed = can('money');
  const [period, setPeriod] = useState<Period>('12');
  const [source, setSource] = useState('all');
  const [report, setReport] = useState<Report | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [off, setOff] = useState(false);
  const [round, setRound] = useState(0);

  useEffect(() => {
    if (!allowed) {
      return;
    }
    const abort = new AbortController();
    setLoading(true);
    setError(null);
    api<Report>(`/api/admin/m/analytics/report?${periodQuery(period, source)}`, { signal: abort.signal })
      .then((data) => {
        setReport(data);
        setOff(false);
      })
      .catch((err: unknown) => {
        if (abort.signal.aborted) {
          return;
        }
        if (err instanceof ApiError && err.code === 'module.disabled') {
          setOff(true);
          return;
        }
        setError(err instanceof ApiError ? err.message : 'Не получилось загрузить аналитику');
      })
      .finally(() => {
        if (!abort.signal.aborted) {
          setLoading(false);
        }
      });
    return () => abort.abort();
  }, [allowed, period, source, round]);

  return {
    allowed,
    report,
    loading,
    error,
    off,
    period,
    setPeriod,
    source,
    setSource,
    reload: () => setRound((n) => n + 1),
  };
}

export function useActive(report: Report | null): [number, (index: number) => void] {
  const size = report?.months.length ?? 0;
  const key = report ? `${report.from}:${report.to}` : '';
  const [state, setState] = useState<{ key: string; index: number }>({ key: '', index: 0 });
  const index = state.key === key ? Math.min(state.index, Math.max(0, size - 1)) : Math.max(0, size - 1);
  return [index, (next: number) => setState({ key, index: next })];
}
