import { useCallback, useEffect, useRef, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { mainApp, menuNote, type ButtonsView } from './chatButtons';

type State = { kind: 'loading' } | { kind: 'ready'; view: ButtonsView } | { kind: 'no_bot' } | { kind: 'failed'; text: string };

const PATH = '/api/admin/settings/bot/buttons';
const APPLY_WAIT = [1200, 2500];
const RECHECK_AFTER = 3000;

function failText(err: unknown): string {
  return err instanceof ApiError ? err.message : 'нет связи с панелью';
}

function stateOf(err: unknown): State {
  if (err instanceof ApiError && err.code === 'bot.not_connected') {
    return { kind: 'no_bot' };
  }
  return { kind: 'failed', text: failText(err) };
}

const pause = (ms: number) => new Promise((resolve) => window.setTimeout(resolve, ms));

function useButtons(turn: number) {
  const [state, setState] = useState<State>({ kind: 'loading' });
  const [putting, setPutting] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  const request = useRef(0);
  const checked = useRef(0);

  const ask = useCallback(async (waits: number[]) => {
    request.current += 1;
    const mine = request.current;
    const fresh = () => mine === request.current;
    for (let step = 0; step <= waits.length; step += 1) {
      const wait = waits[step - 1];
      if (wait !== undefined) {
        await pause(wait);
      }
      if (!fresh()) {
        return;
      }
      if (waits.length > 0 && step === 0) {
        continue;
      }
      let next: State;
      try {
        next = { kind: 'ready', view: await api<ButtonsView>(PATH) };
      } catch (err) {
        next = stateOf(err);
      }
      if (!fresh()) {
        return;
      }
      checked.current = Date.now();
      if (next.kind !== 'ready' || next.view.applied || step === waits.length) {
        setState(next);
        setRefusal(null);
        return;
      }
    }
  }, []);

  useEffect(() => {
    void ask(turn === 0 ? [] : APPLY_WAIT);
    return () => {
      request.current += 1;
    };
  }, [ask, turn]);

  useEffect(() => {
    const onShow = () => {
      if (document.visibilityState === 'visible' && Date.now() - checked.current > RECHECK_AFTER) {
        void ask([]);
      }
    };
    document.addEventListener('visibilitychange', onShow);
    window.addEventListener('focus', onShow);
    return () => {
      document.removeEventListener('visibilitychange', onShow);
      window.removeEventListener('focus', onShow);
    };
  }, [ask]);

  const put = async () => {
    if (putting) {
      return;
    }
    request.current += 1;
    setPutting(true);
    setRefusal(null);
    try {
      const view = await api<ButtonsView>(`${PATH}/menu`, { method: 'POST' });
      setState({ kind: 'ready', view });
    } catch (err) {
      setRefusal(failText(err));
    } finally {
      checked.current = Date.now();
      setPutting(false);
    }
  };

  const again = () => {
    setState({ kind: 'loading' });
    void ask([]);
  };

  return { state, putting, refusal, put, again };
}

export function TelegramButtons({ turn, miniapp, appUrl }: { turn: number; miniapp: boolean; appUrl: string | null }) {
  const { state, putting, refusal, put, again } = useButtons(turn);
  const [copied, setCopied] = useState(false);

  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setCopied(false);
    }
  };

  if (state.kind === 'loading') {
    return (
      <div className="row">
        <span className="cap" role="status">
          Спрашиваем у Telegram, какие кнопки стоят…
        </span>
      </div>
    );
  }
  if (state.kind === 'no_bot') {
    return (
      <div className="row">
        <span className="cap">
          Бот не подключён: кнопки появятся, когда он заработает.{' '}
          <Link className="link" to="/admin/settings/bot">
            Подключить
          </Link>
        </span>
      </div>
    );
  }
  if (state.kind === 'failed') {
    return (
      <div className="row adm-row">
        <span className="adm-who">
          <span className="adm-status warn" role="status">
            Какие кнопки стоят в Telegram, узнать не удалось: {state.text}
          </span>
        </span>
        <span className="adm-acts">
          <button type="button" className="b34 hov" onClick={again}>
            Проверить ещё раз
          </button>
        </span>
      </div>
    );
  }
  const note = menuNote(state.view);
  const main = mainApp(state.view, miniapp, appUrl);
  return (
    <>
      {miniapp || note.action ? (
        <div className="row adm-row">
          <span className="adm-who">
            <span className={note.warn ? 'adm-status warn' : 'adm-status'} role="status">
              {note.text}
            </span>
            {refusal ? (
              <span className="err" role="alert">
                Не получилось: {refusal}
              </span>
            ) : null}
          </span>
          {note.action ? (
            <span className="adm-acts">
              <button type="button" className="b34 hov" onClick={() => void put()} disabled={putting}>
                {putting ? 'Отправляем…' : note.action}
              </button>
            </span>
          ) : null}
        </div>
      ) : null}
      {main ? (
        <div className="row adm-row">
          <span className="adm-who">
            <span className="adm-email">Кнопка «Открыть» в списке чатов</span>
            <span className={main.warn ? 'adm-status warn' : 'adm-status'}>{main.text}</span>
            <span className="adm-under">
              {miniapp && appUrl ? (
                <button type="button" className="b34 hov" onClick={() => void copy(appUrl)}>
                  {copied ? 'Скопировано' : 'Скопировать адрес'}
                </button>
              ) : null}
              <a className="b34 hov" href="https://t.me/BotFather" target="_blank" rel="noreferrer">
                Открыть @BotFather
              </a>
            </span>
          </span>
        </div>
      ) : null}
    </>
  );
}
