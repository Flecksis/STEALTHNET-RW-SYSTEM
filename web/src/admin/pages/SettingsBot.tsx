import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import '../form.css';
import { SettingsLayout } from './SettingsLayout';

type Bot = { token_set: boolean; token_from_env: boolean; username: string | null };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function SettingsBot() {
  const [bot, setBot] = useState<Bot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [token, setToken] = useState('');
  const [replacing, setReplacing] = useState(false);
  const [username, setUsername] = useState('');
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const data = await api<Bot>('/api/admin/settings/bot');
      setBot(data);
      setUsername(data.username ?? '');
      setToken('');
      setReplacing(false);
      setError(null);
    } catch (err) {
      setError(failText(err, 'Настройки бота не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  if (!bot) {
    return (
      <SettingsLayout>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем настройки бота…</p>
        )}
      </SettingsLayout>
    );
  }

  const clean = username.trim().replace(/^@/, '');
  const nameBad = clean !== '' && !/^[A-Za-z0-9_]{5,32}$/.test(clean);
  const tokenBad = token.trim() !== '' && !/^\d+:[A-Za-z0-9_-]{20,}$/.test(token.trim());
  const dirty = token.trim() !== '' || clean !== (bot.username ?? '');
  const editingToken = !bot.token_from_env && (!bot.token_set || replacing);

  const save = async () => {
    if (!dirty || busy) {
      return;
    }
    if (nameBad || tokenBad) {
      setStatus({ text: 'Исправьте поля, отмеченные красным', bad: true });
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      const body: Record<string, string> = {};
      if (token.trim()) {
        body.token = token.trim();
      }
      if (clean !== (bot.username ?? '')) {
        body.username = clean;
      }
      await api('/api/admin/settings/bot', { method: 'PUT', body });
      await load();
      setStatus({ text: 'Сохранено', bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Настройки бота не сохранились'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsLayout>
      <section className="sec" aria-labelledby="bot-h">
        <div className="sec-head">
          <h2 id="bot-h" className="section-title">
            Telegram-бот
          </h2>
          <span className="muted">Через бота клиенты покупают и продлевают подписку, платят звёздами и пишут в поддержку</span>
        </div>
        <div className="row">
          <label htmlFor="bot-token" className="rl">
            Токен бота
          </label>
          <div className="rc">
            {bot.token_from_env ? (
              <span className="bot-state">
                <strong>Задан на сервере</strong>
                <span className="muted">в файле .env, поменять можно там</span>
              </span>
            ) : editingToken ? (
              <input
                id="bot-token"
                className="field mono"
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={token}
                placeholder={bot.token_set ? 'Новый токен' : '123456789:AA…'}
                aria-invalid={tokenBad}
                aria-describedby="bot-token-hint"
                onChange={(e) => {
                  setToken(e.target.value);
                  setStatus(null);
                }}
                disabled={busy}
              />
            ) : (
              <span className="bot-state">
                <strong>Сохранён</strong>
                <button type="button" className="b34 hov" onClick={() => setReplacing(true)} disabled={busy}>
                  Заменить
                </button>
              </span>
            )}
            {editingToken ? (
              <span id="bot-token-hint" className={tokenBad ? 'err' : 'cap'}>
                {tokenBad ? 'Токен выглядит как 123456789:AAH… — скопируйте его из @BotFather целиком' : 'Выдаёт @BotFather командой /newbot или /token'}
              </span>
            ) : null}
          </div>
        </div>
        <div className="row">
          <label htmlFor="bot-name" className="rl">
            Имя бота
          </label>
          <div className="rc">
            <input
              id="bot-name"
              className="field"
              value={username}
              placeholder="@stealthnet_bot"
              maxLength={33}
              aria-invalid={nameBad}
              aria-describedby="bot-name-hint"
              onChange={(e) => {
                setUsername(e.target.value);
                setStatus(null);
              }}
              disabled={busy}
              style={{ maxWidth: 320 }}
            />
            <span id="bot-name-hint" className={nameBad ? 'err' : 'cap'}>
              {nameBad ? 'Латиница, цифры и _, от 5 до 32 символов' : 'Нужно для ссылок на бота в кабинете, на сайте и в приглашениях'}
            </span>
          </div>
        </div>
      </section>

      <div className="save-row">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button
          type="button"
          className="btn-line hov"
          disabled={busy}
          onClick={() => {
            setToken('');
            setReplacing(false);
            setUsername(bot.username ?? '');
            setStatus(null);
          }}
        >
          Отменить
        </button>
        <span role="status" className={status?.bad ? 'bad' : undefined}>
          {status?.text ?? (dirty ? 'Есть несохранённые изменения' : '')}
        </span>
      </div>
    </SettingsLayout>
  );
}
