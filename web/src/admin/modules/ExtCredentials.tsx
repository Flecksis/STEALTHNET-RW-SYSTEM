import { useState } from 'react';
import type { Credentials } from './ext';

function CopyLine({ label, value, hint }: { label: string; value: string; hint: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };
  return (
    <div className="row adm-row adm-issued">
      <span className="adm-who">
        <span className="ext-cred-label">{label}</span>
        <span className="mono ext-cred">{value}</span>
        <span className="cap">{hint}</span>
      </span>
      <span className="adm-acts">
        <button type="button" className="b34 on-surface hov" onClick={() => void copy()}>
          {copied ? 'Скопировано' : 'Скопировать'}
        </button>
      </span>
    </div>
  );
}

export function CredentialsBlock({ credentials, onDone }: { credentials: Credentials; onDone: () => void }) {
  return (
    <section className="sec" aria-labelledby="ext-cred-h">
      <div className="sec-head">
        <h2 id="ext-cred-h" className="section-title">
          Ключ и секрет модуля
        </h2>
        <span className="cap">Сохраните оба значения сейчас и передайте их модулю: больше панель их не покажет.</span>
      </div>
      <CopyLine
        label="Ключ API"
        value={credentials.api_key}
        hint="Модуль передаёт его в заголовке Authorization: Bearer … при запросах к панели."
      />
      <CopyLine
        label="Секрет подписи событий"
        value={credentials.signing_secret}
        hint="Им модуль проверяет, что событие пришло от панели, и проверяет токен своей страницы."
      />
      <div className="row">
        <button type="button" className="btn-line hov" onClick={onDone}>
          Готово, скрыть
        </button>
      </div>
    </section>
  );
}
