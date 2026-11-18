import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../../lib/api';

type Core = { settings?: { public_url?: string | null } };

export function OauthSetup() {
  const [publicUrl, setPublicUrl] = useState<string | null | undefined>(undefined);
  const [copied, setCopied] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api<Core>('/api/admin/settings/core').then(
      (core) => setPublicUrl(core.settings?.public_url ?? null),
      () => setPublicUrl(window.location.origin),
    );
  }, []);

  if (publicUrl === undefined) {
    return null;
  }

  const base = (publicUrl ?? '').trim().replace(/\/+$/, '');
  const https = base.startsWith('https://');
  const host = https ? (base.slice('https://'.length).split('/')[0] ?? '') : '';

  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(text);
      setError(null);
      window.setTimeout(() => setCopied(null), 2000);
    } catch {
      setError('Браузер не дал скопировать: выделите текст вручную');
    }
  };

  const value = (text: string) => (
    <span className="md-copy">
      <code className="mono">{text}</code>
      <button type="button" className="b34 hov" onClick={() => void copy(text)}>
        {copied === text ? 'Скопировано' : 'Скопировать'}
      </button>
    </span>
  );

  return (
    <section className="card md-form md-oauth" aria-labelledby="oauth-h">
      <h2 id="oauth-h" className="section-title">
        Как подключить
      </h2>
      {https ? null : (
        <p className="md-oauth-warn">
          Нужен адрес сайта с https: Google и Apple возвращают клиента только на него. Адрес задаётся на сервере командой{' '}
          <code className="mono">stealthnet domain</code>, текущий виден в{' '}
          <Link className="link" to="/admin/settings/general">
            «Общих»
          </Link>
          .
        </p>
      )}
      <div className="md-oauth-part">
        <h3>Google</h3>
        <ol>
          <li>В Google Cloud Console откройте Google Auth Platform → Clients и создайте клиента с типом Web application.</li>
          <li>
            В Authorized redirect URIs добавьте адрес:
            {https ? value(`${base}/api/client/auth/oauth/google/callback`) : null}
          </li>
          <li>Скопируйте Client ID и Client secret в поля ниже.</li>
          <li>В Audience нажмите Publish app: пока приложение в режиме Testing, войти смогут только тестовые пользователи.</li>
        </ol>
      </div>
      <div className="md-oauth-part">
        <h3>Apple</h3>
        <ol>
          <li>На developer.apple.com в Identifiers создайте App ID или откройте существующий и включите у него Sign in with Apple. Затем там же создайте Services ID — его впишите ниже.</li>
          <li>
            У Services ID откройте Sign in with Apple → Configure. В Domains and Subdomains впишите:
            {https ? value(host) : null}
            В Return URLs:
            {https ? value(`${base}/api/client/auth/oauth/apple/callback`) : null}
          </li>
          <li>В Keys создайте ключ с Sign in with Apple и скачайте файл .p8 — его дают один раз. Key ID виден у ключа, Team ID — справа вверху страницы.</li>
          <li>Клиент может скрыть почту. Чтобы письма доходили, в Services → Sign in with Apple for Email Communication добавьте адрес отправителя из модуля «Почта».</li>
        </ol>
      </div>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
    </section>
  );
}
