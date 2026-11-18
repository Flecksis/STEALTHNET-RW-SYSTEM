import { useState } from 'react';
import { api, ApiError } from '../../lib/api';
import type { FieldValues } from '../SchemaFields';

type ModelOption = { id: string; free: boolean };

type Check = { ok: boolean; provider: string; model: string; model_found: boolean; models: ModelOption[]; error: string | null };

type Result = { provider: string; check: Check | null; failure: string | null };

function text(value: FieldValues[string] | undefined): string {
  return typeof value === 'string' ? value.trim() : '';
}

function verdict(check: Check, model: string): { text: string; bad: boolean } {
  if (!check.ok) {
    return { text: check.error ?? 'Провайдер не ответил', bad: true };
  }
  const current = model || check.model;
  const found = current === check.model ? check.model_found : check.models.some((m) => m.id === current);
  if (found) {
    return { text: `Ключ работает, модель ${current} есть у провайдера.`, bad: false };
  }
  if (!check.models.length) {
    return { text: 'Ключ работает, но провайдер не отдал список моделей.', bad: false };
  }
  return { text: `Ключ работает, но модели ${current} у провайдера нет. Выберите модель из списка.`, bad: true };
}

export function AiModels({ values, onPick, busy }: { values: FieldValues; onPick: (model: string) => void; busy: boolean }) {
  const provider = text(values.provider);
  const model = text(values.model);
  const [result, setResult] = useState<Result | null>(null);
  const [loading, setLoading] = useState(false);
  const shown = result && result.provider === provider ? result : null;

  const run = async () => {
    setLoading(true);
    try {
      const check = await api<Check>('/api/admin/m/ai/check', {
        method: 'POST',
        body: { provider, api_url: text(values.api_url), api_key: text(values.api_key), model },
      });
      setResult({ provider, check, failure: null });
    } catch (err) {
      setResult({ provider, check: null, failure: err instanceof ApiError ? err.message : 'Проверка не прошла' });
    } finally {
      setLoading(false);
    }
  };

  const models = shown?.check?.ok ? shown.check.models : [];
  const status = shown ? (shown.check ? verdict(shown.check, model) : { text: shown.failure ?? '', bad: true }) : null;

  return (
    <div className="ai-models">
      <div className="md-actions">
        <button type="button" className="b34 hov" onClick={() => void run()} disabled={busy || loading}>
          {loading ? 'Проверяем…' : 'Проверить ключ'}
        </button>
        {status ? (
          <span role="status" className={status.bad ? 'md-warn' : 'muted'}>
            {status.text}
          </span>
        ) : null}
      </div>
      {models.length ? (
        <div className="sf-field">
          <label htmlFor="ai-model-pick" className="sf-label">
            Модели провайдера
          </label>
          <select
            id="ai-model-pick"
            className="field"
            value={models.some((m) => m.id === model) ? model : ''}
            onChange={(e) => {
              if (e.target.value) {
                onPick(e.target.value);
              }
            }}
            aria-describedby="ai-model-pick-hint"
            disabled={busy}
          >
            <option value="">Выберите модель</option>
            {models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.free ? `${m.id} — бесплатная` : m.id}
              </option>
            ))}
          </select>
          <span id="ai-model-pick-hint" className="cap">
            Выбранная модель встанет в поле «Модель». Чтобы она заработала, сохраните настройки.
          </span>
        </div>
      ) : null}
    </div>
  );
}
