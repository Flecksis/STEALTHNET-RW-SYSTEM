import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { dateTime, groupDigits } from '../../lib/format';

type Usage = { answers: number; handoffs: number; tokens: number; clients: number };

type Overview = {
  today: Usage;
  daily_tokens: number;
  client_daily: number;
  last_error: { at: string; text: string } | null;
};

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function AiUsage({ version }: { version: number }) {
  const [data, setData] = useState<Overview | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await api<Overview>('/api/admin/m/ai/overview'));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Расход не загрузился'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, version]);

  if (!data) {
    return error ? (
      <p className="err" role="alert">
        {error}
      </p>
    ) : null;
  }

  const share = data.daily_tokens > 0 ? Math.min(data.today.tokens / data.daily_tokens, 1) : 0;
  const spent = data.today.tokens >= data.daily_tokens;

  return (
    <section className="card md-form" aria-labelledby="ai-today-h">
      <h2 id="ai-today-h" className="section-title">
        Сегодня
      </h2>
      <p className="ai-line">
        {data.today.answers || data.today.handoffs
          ? `Ответов ИИ: ${groupDigits(data.today.answers)}, клиентов: ${groupDigits(data.today.clients)}, передано оператору: ${groupDigits(data.today.handoffs)}.`
          : 'ИИ сегодня ещё не отвечал.'}
      </p>
      <div className="ai-budget">
        <span className="ai-line">
          Токены: {groupDigits(data.today.tokens)} из {groupDigits(data.daily_tokens)}
          {spent ? ' — бюджет на сегодня закончился, отвечают операторы' : ''}
        </span>
        <span className="ai-bar" aria-hidden="true">
          <span style={{ width: `${Math.round(share * 100)}%`, background: spent ? 'var(--warn)' : undefined }} />
        </span>
      </div>
      {data.last_error ? (
        <p className="ai-line muted">
          Последняя ошибка ({dateTime(data.last_error.at).toLowerCase()}): {data.last_error.text}
        </p>
      ) : null}
    </section>
  );
}
