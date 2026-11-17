import { useId, useMemo, useState } from 'react';
import { api } from '../../lib/api';
import { countryName } from '../../lib/geo';
import { inboundLabel } from './InfraNodes';
import {
  ALPNS,
  FINGERPRINTS,
  REMARK_MAX,
  SECURITY,
  draftBody,
  draftErrors,
  draftOf,
  failText,
  squadsText,
  type HostData,
  type HostDraft,
  type HostInbound,
  type HostRow,
  type HostsData,
} from './hosts';

type Props = {
  data: HostsData;
  row: HostRow | null;
  phone?: boolean;
  onCancel: () => void;
  onSaved: (host: HostData, created: boolean) => void;
  onDeleted?: (remark: string) => void;
};

function inboundTitle(inbound: HostInbound, data: HostsData, profiles: number): string {
  const names = inbound.nodes.map((uuid) => data.nodes.find((n) => n.uuid === uuid)?.name).filter(Boolean);
  const parts = [names.length ? names.join(', ') : null, inboundLabel(inbound), profiles > 1 && inbound.profile_name ? inbound.profile_name : null];
  const title = parts.filter(Boolean).join(' · ');
  return names.length ? title : `${title} · нет на нодах`;
}

export function HostForm({ data, row, phone = false, onCancel, onSaved, onDeleted }: Props) {
  const creating = row === null;
  const uid = useId();
  const inbounds = useMemo(() => [...data.inbounds].sort((a, b) => Number(b.nodes.length > 0) - Number(a.nodes.length > 0)), [data.inbounds]);
  const profiles = new Set(data.inbounds.map((i) => i.profile_uuid)).size;
  const [touched, setTouched] = useState<Set<keyof HostDraft>>(new Set());
  const [draft, setDraft] = useState<HostDraft>(() => {
    const base = draftOf(row);
    if (!creating) {
      return base;
    }
    const first = inbounds.find((i) => i.nodes.length) ?? inbounds[0];
    return first ? prefill(base, first, new Set()) : base;
  });
  const [tried, setTried] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [asking, setAsking] = useState(false);

  function prefill(base: HostDraft, inbound: HostInbound, typed: Set<keyof HostDraft>): HostDraft {
    const node = inbound.nodes.length === 1 ? data.nodes.find((n) => n.uuid === inbound.nodes[0]) : undefined;
    return {
      ...base,
      inbound: inbound.uuid,
      port: !typed.has('port') && inbound.port ? String(inbound.port) : base.port,
      address: !typed.has('address') && node ? node.address : base.address,
      remark: !typed.has('remark') && node ? countryName(node.country_code) || node.name : base.remark,
    };
  }

  const errors = draftErrors(draft, creating);
  const shown = tried ? errors : {};
  const blocked = Object.keys(errors).length > 0;
  const fingerprints = draft.fingerprint && !FINGERPRINTS.includes(draft.fingerprint) ? [draft.fingerprint, ...FINGERPRINTS] : FINGERPRINTS;
  const alpns = draft.alpn && !ALPNS.includes(draft.alpn) ? [draft.alpn, ...ALPNS] : ALPNS;
  const squads = row ? squadsText(row) : null;

  const set = (key: keyof HostDraft, value: string) => {
    setTouched((prev) => new Set(prev).add(key));
    setDraft((prev) => ({ ...prev, [key]: value }));
  };

  const pickInbound = (uuid: string) => {
    const inbound = data.inbounds.find((i) => i.uuid === uuid);
    setDraft((prev) => (inbound ? prefill(prev, inbound, touched) : { ...prev, inbound: uuid }));
  };

  const save = async () => {
    setTried(true);
    if (blocked) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const body = draftBody(draft, creating);
      const host = creating
        ? await api<HostData>('/api/admin/infra/hosts', { method: 'POST', body })
        : await api<HostData>(`/api/admin/infra/hosts/${row.host.uuid}`, { method: 'PUT', body });
      onSaved(host, creating);
    } catch (err) {
      setError(failText(err, creating ? 'Хост не создался' : 'Хост не сохранился'));
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!row) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api(`/api/admin/infra/hosts/${row.host.uuid}`, { method: 'DELETE' });
      onDeleted?.(row.host.remark);
    } catch (err) {
      setError(failText(err, 'Хост не удалился'));
      setBusy(false);
    }
  };

  const field = (key: keyof HostDraft, label: string, options: { mono?: boolean; hint?: string; placeholder?: string; max?: number; numeric?: boolean } = {}) => {
    const id = `${uid}-${key}`;
    const message = shown[key];
    return (
      <div className="tf-field">
        <label htmlFor={id}>{label}</label>
        <input
          id={id}
          className={options.mono ? 'field mono' : 'field'}
          value={draft[key]}
          maxLength={options.max ?? 253}
          inputMode={options.numeric ? 'numeric' : undefined}
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          placeholder={options.placeholder}
          onChange={(e) => set(key, e.target.value)}
          aria-invalid={message ? true : undefined}
          aria-describedby={message || options.hint ? `${id}-note` : undefined}
          disabled={busy}
        />
        {message ? (
          <span id={`${id}-note`} className="err">
            {message}
          </span>
        ) : options.hint ? (
          <span id={`${id}-note`} className="cap">
            {options.hint}
          </span>
        ) : null}
      </div>
    );
  };

  const select = (key: keyof HostDraft, label: string, items: Array<[string, string]>) => {
    const id = `${uid}-${key}`;
    return (
      <div className="tf-field">
        <label htmlFor={id}>{label}</label>
        <select id={id} className="field" value={draft[key]} onChange={(e) => set(key, e.target.value)} disabled={busy}>
          {items.map(([value, text]) => (
            <option key={value} value={value}>
              {text}
            </option>
          ))}
        </select>
      </div>
    );
  };

  return (
    <div className={phone ? 'hs-form phone' : 'hs-form'} role="group" aria-label={creating ? 'Новый хост' : `Изменить хост ${row.host.remark}`}>
      {squads ? <p className={squads.warn ? 'hs-squads sq-warn' : 'hs-squads'}>{squads.text}</p> : null}
      <div className="inf-grid hs-grid">
        {creating ? (
          <div className="tf-field hs-span">
            <label htmlFor={`${uid}-inbound`}>Вход</label>
            <select
              id={`${uid}-inbound`}
              className="field"
              value={draft.inbound}
              onChange={(e) => pickInbound(e.target.value)}
              aria-invalid={shown.inbound ? true : undefined}
              disabled={busy || inbounds.length === 0}
            >
              {inbounds.length === 0 ? <option value="">В Remnawave нет входов</option> : null}
              {inbounds.map((i) => (
                <option key={i.uuid} value={i.uuid}>
                  {inboundTitle(i, data, profiles)}
                </option>
              ))}
            </select>
            {shown.inbound ? <span className="err">{shown.inbound}</span> : <span className="cap">Хост покажут клиентам сквадов, в которые входит этот вход</span>}
          </div>
        ) : null}
        {field('remark', 'Название', { hint: 'Так сервер называется в приложении клиента', max: REMARK_MAX, placeholder: 'Например, Германия' })}
        {field('address', 'Адрес', { mono: true, hint: 'IP или домен, к которому подключается приложение', placeholder: 'de.example.com' })}
        {field('port', 'Порт', { mono: true, numeric: true, max: 5, placeholder: '443' })}
        {field('sni', 'SNI', { mono: true, placeholder: 'Можно оставить пустым' })}
        {field('host', 'Host', { mono: true, max: 512, placeholder: 'Можно оставить пустым' })}
        {field('path', 'Path', { mono: true, max: 512, placeholder: 'Можно оставить пустым' })}
        {select('fingerprint', 'Отпечаток', [['', 'По умолчанию'], ...fingerprints.map((f): [string, string] => [f, f])])}
        {select('alpn', 'ALPN', [['', 'По умолчанию'], ...alpns.map((a): [string, string] => [a, a])])}
        {select('security', 'Безопасность', SECURITY)}
      </div>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="sq-actions">
        <span className="cc-btns">
          <button type="button" className="btn-main btn" onClick={() => void save()} aria-disabled={tried && blocked ? true : undefined} disabled={busy}>
            {busy && !asking ? 'Сохраняем…' : creating ? 'Создать хост' : 'Сохранить'}
          </button>
          <button type="button" className="btn-line hov sq-cancel" onClick={onCancel} disabled={busy}>
            Отменить
          </button>
        </span>
        <span className="muted">{tried && blocked ? 'Исправьте поля, отмеченные красным' : 'Клиенты увидят изменения, когда приложение обновит подписку'}</span>
      </div>
      {row && onDeleted ? (
        <details className="inf-danger" open={asking ? true : undefined}>
          <summary>Опасные действия</summary>
          <div className="inf-col" style={{ gap: 10, paddingTop: 12 }}>
            {asking ? (
              <div role="status" className="cc-note">
                <span className="cc-note-text wide">
                  <strong>Удалить хост {row.host.remark}?</strong>{' '}
                  {row.squads.some((s) => s.shown) && !row.host.isDisabled
                    ? `Клиенты ${row.squads.filter((s) => s.shown).length === 1 ? 'сквада' : 'сквадов'} ${row.squads
                        .filter((s) => s.shown)
                        .map((s) => s.name)
                        .join(', ')} перестанут видеть этот сервер.`
                    : ''}{' '}
                  Отменить будет нельзя.
                </span>
                <span className="cc-btns">
                  <button type="button" className="b34 on-surface" style={{ color: 'var(--bad)', borderColor: 'var(--bad)' }} onClick={() => void remove()} disabled={busy}>
                    Удалить хост
                  </button>
                  <button type="button" className="b34 on-surface" onClick={() => setAsking(false)} disabled={busy}>
                    Отменить
                  </button>
                </span>
              </div>
            ) : (
              <button type="button" className="b36 bad hov" style={{ width: 'fit-content' }} onClick={() => setAsking(true)} disabled={busy}>
                Удалить хост
              </button>
            )}
          </div>
        </details>
      ) : null}
    </div>
  );
}
