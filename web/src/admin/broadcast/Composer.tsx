import { useEffect, useMemo, useState } from 'react';
import { api } from '../../lib/api';
import { useSession } from '../session';
import { MessageFields, TestFields, checkMessage, messageFrom, subjectOf, toDraft, toSpec, type MessageValue, type Note } from './Message';
import { ChannelPreview } from './Preview';
import {
  AUDIENCES,
  BASE,
  addresses,
  at,
  audienceOf,
  clients,
  count,
  hasBot,
  hasEmail,
  reachText,
  reachTotal,
  failText,
  imageUrl,
  localInput,
  zoneLabel,
  type Audience,
  type AudienceKind,
  type Broadcast,
  type Options,
  type Reach,
} from './model';
import '../form.css';

function nextHour(): Date {
  const date = new Date(Date.now() + 3_600_000);
  date.setMinutes(0, 0, 0);
  return date;
}

type Props = {
  options: Options;
  initial: Broadcast | null;
  onSaved: (broadcast: Broadcast) => void;
  onDeleted: () => void;
};

export function Composer({ options, initial, onSaved, onDeleted }: Props) {
  const { me } = useSession();
  const limits = options.limits;
  const start: Audience = initial?.audience ?? { kind: 'all' };
  const [id, setId] = useState<number | null>(initial?.id ?? null);
  const [message, setMessage] = useState<MessageValue>(() => messageFrom(initial));
  const [kind, setKind] = useState<AudienceKind>(start.kind);
  const [days, setDays] = useState(start.kind === 'expiring' ? String(start.days) : '3');
  const [tariff, setTariff] = useState<number | null>(start.kind === 'tariff' ? start.tariff_id : (options.tariffs[0]?.id ?? null));
  const [reach, setReach] = useState<Reach | null>(null);
  const [reachError, setReachError] = useState<string | null>(null);
  const [when, setWhen] = useState<'now' | 'later'>('now');
  const [later, setLater] = useState(() => localInput(nextHour()));
  const [confirm, setConfirm] = useState<'send' | 'delete' | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<Note | null>(null);
  const [tried, setTried] = useState(false);

  const dayCount = Number.parseInt(days, 10);
  const daysOk = /^\d+$/.test(days.trim()) && dayCount >= 1 && dayCount <= limits.days;
  const audience = kind === 'expiring' ? (daysOk ? audienceOf(kind, dayCount, tariff) : null) : audienceOf(kind, dayCount, tariff);
  const audienceKey = JSON.stringify(audience);

  const { channel, subject, text, image, buttons } = message;
  const change = (patch: Partial<MessageValue>) => setMessage((current) => ({ ...current, ...patch }));
  const subjectValue = subjectOf(message);
  const body = useMemo(
    () => ({ channel, subject: subjectValue, text, image_id: image?.id ?? null, buttons: buttons.map(toSpec), audience }),
    [channel, subjectValue, text, image, buttons, audience],
  );
  const bodyKey = JSON.stringify(body);
  const [savedKey, setSavedKey] = useState(() => (initial ? bodyKey : ''));
  const dirty = bodyKey !== savedKey;

  useEffect(() => {
    const wanted = JSON.parse(audienceKey) as Audience | null;
    if (!wanted) {
      setReach(null);
      return;
    }
    let alive = true;
    const timer = window.setTimeout(() => {
      api<Reach>(`${BASE}/audience`, { method: 'POST', body: { audience: wanted } })
        .then((value) => {
          if (alive) {
            setReach(value);
            setReachError(null);
          }
        })
        .catch((err) => {
          if (alive) {
            setReach(null);
            setReachError(failText(err, 'Не получилось посчитать получателей'));
          }
        });
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [audienceKey]);

  const check = checkMessage(message, options, tried);
  const { contentOk, mailOff } = check;
  const laterDate = new Date(later);
  const laterOk =
    when === 'now' ||
    (!Number.isNaN(laterDate.getTime()) &&
      laterDate.getTime() > Date.now() + 60_000 &&
      laterDate.getTime() < Date.now() + limits.schedule_days * 86_400_000);
  const total = reach ? reachTotal(reach, channel) : null;
  const nobody = total !== null && total === 0;
  const canSend = contentOk && audience !== null && laterOk && !mailOff && !(when === 'now' && nobody) && busy === null;

  const persist = async (): Promise<Broadcast> => {
    const saved =
      id === null
        ? await api<Broadcast>(`${BASE}/broadcasts`, { method: 'POST', body })
        : await api<Broadcast>(`${BASE}/broadcasts/${id}`, { method: 'PUT', body });
    const normalized = {
      channel: saved.channel,
      subject: saved.subject,
      text: saved.text,
      image_id: saved.image?.id ?? null,
      buttons: saved.buttons.map((b) => toSpec(toDraft(b))),
      audience: saved.audience,
    };
    setId(saved.id);
    setMessage((current) => ({ ...current, subject: saved.subject ?? '', text: saved.text, buttons: saved.buttons.map(toDraft) }));
    setSavedKey(JSON.stringify(normalized));
    onSaved(saved);
    return saved;
  };

  const save = async () => {
    setTried(true);
    if (!text.trim()) {
      setNote({ text: 'Напишите текст сообщения', tone: 'bad' });
      return;
    }
    if (!audience) {
      setNote({ text: 'Выберите, кому отправить', tone: 'bad' });
      return;
    }
    setBusy('save');
    setNote(null);
    try {
      await persist();
      setNote({ text: 'Черновик сохранён', tone: 'ok' });
    } catch (err) {
      setNote({ text: failText(err, 'Черновик не сохранился'), tone: 'bad' });
    } finally {
      setBusy(null);
    }
  };

  const send = async () => {
    setBusy('send');
    setNote(null);
    try {
      const saved = await persist();
      const atIso = when === 'later' ? laterDate.toISOString() : null;
      const started = await api<Broadcast>(`${BASE}/broadcasts/${saved.id}/send`, { method: 'POST', body: { at: atIso } });
      setConfirm(null);
      onSaved(started);
    } catch (err) {
      setConfirm(null);
      setNote({ text: failText(err, 'Рассылка не отправилась'), tone: 'bad' });
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    if (id === null) {
      onDeleted();
      return;
    }
    setBusy('delete');
    try {
      await api(`${BASE}/broadcasts/${id}`, { method: 'DELETE' });
      onDeleted();
    } catch (err) {
      setConfirm(null);
      setNote({ text: failText(err, 'Черновик не удалился'), tone: 'bad' });
      setBusy(null);
    }
  };

  const imageSrc = image ? imageUrl(image.id) : null;
  const preview = (
    <ChannelPreview channel={channel} subject={subject} text={text} image={imageSrc} buttons={buttons.map(toSpec)} brand={me.brand_name} />
  );
  const recipients = reach === null ? null : reachText(reach, channel);
  const sendLabel =
    when === 'later'
      ? laterOk
        ? `Запланировать на ${at(laterDate.toISOString())}`
        : 'Запланировать'
      : total && total > 0
        ? channel === 'bot' && reach
          ? `Отправить ${clients(reach.recipients)}`
          : channel === 'email' && reach
            ? `Отправить на${'\u00a0'}${addresses(reach.emails)}`
            : reach
              ? `Отправить: ${count(reach.recipients)} в${'\u00a0'}бот, ${count(reach.emails)} на${'\u00a0'}почту`
              : `Отправить: ${count(total)}`
        : 'Отправить';

  return (
    <div className="bc-grid">
      <div className="bc-col">
        <MessageFields
          options={options}
          value={message}
          onChange={change}
          check={check}
          tried={tried}
          busy={busy}
          setBusy={setBusy}
          onNote={setNote}
          preview={preview}
          placeholder="Например: Подключили новые серверы в Нидерландах — скорость выше, пинг ниже. Обновите подписку в приложении."
          subjectPlaceholder="Например: Новые серверы в Нидерландах"
        />

        <section className="sec" aria-labelledby="bc-who-h">
          <div className="sec-head">
            <h2 id="bc-who-h" className="section-title">
              Кому
            </h2>
            <span className="muted">
              Список получателей соберётся в момент отправки.
              {channel === 'bot'
                ? ' Тем, кто остановил бота, сообщение не уходит.'
                : channel === 'email'
                  ? ' Тем, кто отписался от рассылок, письмо не уходит.'
                  : ' Кто остановил бота, не получит сообщение в бот, кто отписался от рассылок — письмо.'}
            </span>
          </div>
          <div className="row">
            <div className="rc bc-opts" role="radiogroup" aria-label="Кому отправить">
              {AUDIENCES.map((option) => (
                <div key={option.kind}>
                  <label className="bc-opt">
                    <input type="radio" name="bc-audience" checked={kind === option.kind} onChange={() => setKind(option.kind)} />
                    <span className="bc-opt-text">
                      <span className="bc-opt-title">{option.title}</span>
                      <span className="cap">{option.hint}</span>
                    </span>
                  </label>
                  {option.kind === 'expiring' && kind === 'expiring' ? (
                    <span className="bc-extra">
                      <span>Не больше</span>
                      <input
                        className="fld"
                        value={days}
                        inputMode="numeric"
                        aria-label="Сколько дней осталось"
                        aria-invalid={!daysOk}
                        onChange={(e) => setDays(e.target.value.replace(/[^\d]/g, '').slice(0, 3))}
                      />
                      <span>дней до конца подписки</span>
                      {!daysOk ? <span className="err">от 1 до {limits.days}</span> : null}
                    </span>
                  ) : null}
                  {option.kind === 'tariff' && kind === 'tariff' ? (
                    <span className="bc-extra">
                      {options.tariffs.length ? (
                        <select
                          className="field"
                          value={tariff ?? ''}
                          aria-label="Тариф"
                          onChange={(e) => setTariff(e.target.value ? Number(e.target.value) : null)}
                        >
                          {options.tariffs.map((t) => (
                            <option key={t.id} value={t.id}>
                              {t.name}
                            </option>
                          ))}
                        </select>
                      ) : (
                        <span className="err">Тарифов пока нет</span>
                      )}
                    </span>
                  ) : null}
                </div>
              ))}
            </div>
          </div>
          <div className="bc-reach" aria-live="polite">
            {reachError ? (
              <span className="err">{reachError}</span>
            ) : !audience ? (
              <span className="muted">Уточните аудиторию</span>
            ) : reach === null ? (
              <span className="muted">Считаем получателей…</span>
            ) : (
              <>
                {hasBot(channel) ? (
                  <span className="bc-reach-line">
                    <span>
                      В бот: <strong className="num">{count(reach.recipients)}</strong>
                    </span>
                    {reach.stopped > 0 ? <span className="muted">ещё у {count(reach.stopped)} бот остановлен — им не отправим</span> : null}
                  </span>
                ) : null}
                {hasEmail(channel) ? (
                  <span className="bc-reach-line">
                    <span>
                      На почту: <strong className="num">{count(reach.emails)}</strong>
                    </span>
                    {reach.unsubscribed > 0 ? <span className="muted">ещё {addresses(reach.unsubscribed)} отписались — им не отправим</span> : null}
                  </span>
                ) : null}
              </>
            )}
          </div>
        </section>

        <TestFields
          options={options}
          value={message}
          check={check}
          busy={busy}
          setBusy={setBusy}
          onTried={() => setTried(true)}
          lead="Отправьте рассылку себе и посмотрите, как она выглядит у клиента."
        />

        <section className="sec" aria-labelledby="bc-send-h">
          <div className="sec-head">
            <h2 id="bc-send-h" className="section-title">
              Отправка
            </h2>
            <span className="muted">
              {hasBot(channel) ? 'Telegram принимает около 25 сообщений в секунду: тысяча получателей — примерно минута. ' : ''}
              {hasEmail(channel) ? 'Письма уходят по два в секунду, чтобы почтовый сервис не начал отказывать: тысяча писем — около восьми минут.' : ''}
            </span>
          </div>
          <div className="row">
            <span className="rl">Когда</span>
            <div className="rc">
              <span className="segs" role="group" aria-label="Когда отправить">
                <button type="button" aria-pressed={when === 'now'} onClick={() => setWhen('now')}>
                  Сейчас
                </button>
                <button type="button" aria-pressed={when === 'later'} onClick={() => setWhen('later')}>
                  Запланировать
                </button>
              </span>
              {when === 'later' ? (
                <>
                  <input
                    className="field bc-when"
                    type="datetime-local"
                    value={later}
                    min={localInput(new Date(Date.now() + 120_000))}
                    max={localInput(new Date(Date.now() + limits.schedule_days * 86_400_000))}
                    aria-label="Дата и время отправки"
                    aria-invalid={!laterOk}
                    onChange={(e) => setLater(e.target.value)}
                  />
                  <span className={laterOk ? 'cap' : 'err'}>
                    {laterOk ? `Время этого устройства, ${zoneLabel()}` : `Выберите время позже текущего, не дальше ${limits.schedule_days} дней`}
                  </span>
                </>
              ) : null}
            </div>
          </div>
          {confirm === 'send' ? (
            <div className="bc-ask" role="group" aria-label="Подтверждение отправки">
              {when === 'now' ? (
                <p>
                  Сообщение уйдёт <strong>{recipients ?? 'получателям'}</strong> сразу после подтверждения.{' '}
                  <span className="muted">Отправку можно остановить, пока она идёт: у тех, кому уже ушло, сообщение останется.</span>
                </p>
              ) : (
                <p>
                  Сообщение уйдёт <strong>{at(laterDate.toISOString())}</strong>. Получатели соберутся в этот момент
                  {recipients ? `, сейчас оно ушло бы ${recipients}` : ''}. <span className="muted">До этого времени отправку можно отменить.</span>
                </p>
              )}
              <div className="bc-ask-btns">
                <button type="button" className="btn-main btn" onClick={() => void send()} disabled={busy !== null}>
                  {busy === 'send' ? 'Отправляем…' : when === 'now' ? 'Отправить' : 'Запланировать'}
                </button>
                <button type="button" className="btn-line hov" onClick={() => setConfirm(null)} disabled={busy !== null}>
                  Не отправлять
                </button>
              </div>
            </div>
          ) : confirm === 'delete' ? (
            <div className="bc-ask" role="group" aria-label="Подтверждение удаления">
              <p>Удалить черновик? Вернуть его не получится.</p>
              <div className="bc-ask-btns">
                <button type="button" className="btn-bad" onClick={() => void remove()} disabled={busy !== null}>
                  {busy === 'delete' ? 'Удаляем…' : 'Удалить'}
                </button>
                <button type="button" className="btn-line hov" onClick={() => setConfirm(null)} disabled={busy !== null}>
                  Оставить
                </button>
              </div>
            </div>
          ) : (
            <div className="bc-actions">
              <button
                type="button"
                className="btn-main btn"
                onClick={() => {
                  setTried(true);
                  if (canSend) {
                    setConfirm('send');
                  }
                }}
                disabled={busy !== null || (tried && !canSend)}
              >
                {sendLabel}
              </button>
              <button type="button" className="btn-line hov" onClick={() => void save()} disabled={busy !== null || (id !== null && !dirty)}>
                {busy === 'save' ? 'Сохраняем…' : 'Сохранить черновик'}
              </button>
              {id !== null ? (
                <button type="button" className="bc-link bad" onClick={() => setConfirm('delete')} disabled={busy !== null}>
                  Удалить черновик
                </button>
              ) : null}
              <span role="status" className={note?.tone === 'bad' ? 'bad' : undefined}>
                {note?.text ?? (when === 'now' && nobody ? 'В этой аудитории некому отправить' : id !== null && dirty ? 'Есть несохранённые изменения' : '')}
              </span>
            </div>
          )}
        </section>
      </div>
      <aside className="bc-side">{preview}</aside>
    </div>
  );
}
