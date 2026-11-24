import { useId, useState, type ReactNode } from 'react';
import { api } from '../../lib/api';
import { count } from '../../lib/format';
import type { Currency } from '../../lib/types';
import {
  AUDIENCES,
  BASE,
  buildBody,
  englishCount,
  failText,
  messageKey,
  placesLabel,
  PLACEHOLDERS,
  PRIZE_KINDS,
  prizeRow,
  type Audience,
  type Contest,
  type Draft,
  type DraftErrors,
  type MessageDraft,
  type MessageLang,
  type Options,
  type PrizeFields,
  type PrizeRow,
  rowPlaces,
} from './model';

type Props = {
  phone: boolean;
  currency: Currency;
  options: Options | null;
  initial: Draft;
  contest: Contest | null;
  onCancel: () => void;
  onSaved: (contest: Contest) => void;
};

type FieldProps = {
  id: string;
  label: string;
  hint?: string;
  error?: string;
  className?: string;
  children: ReactNode;
};

function Field({ id, label, hint, error, className, children }: FieldProps) {
  return (
    <div className={className ? `pf-field ${className}` : 'pf-field'}>
      <label htmlFor={id} className="pf-label">
        {label}
      </label>
      {children}
      {error ? (
        <span id={`${id}-err`} className="err">
          {error}
        </span>
      ) : hint ? (
        <span id={`${id}-hint`} className="cap">
          {hint}
        </span>
      ) : null}
    </div>
  );
}

function described(id: string, error?: string): string {
  return error ? `${id}-err` : `${id}-hint`;
}

type PrizeInputsProps = {
  id: string;
  fields: PrizeFields;
  onChange: (patch: Partial<PrizeFields>) => void;
  currency: Currency;
  phone: boolean;
  promoOff: boolean;
  error?: string;
  label: string;
  extra?: ReactNode;
};

function PrizeInputs({ id, fields, onChange, currency, phone, promoOff, error, label, extra }: PrizeInputsProps) {
  const unit = currency === 'RUB' ? '₽' : '$';
  const value = `${id}-value`;
  return (
    <>
      <div className="pf-field ct-kind">
        <span className="pf-label" id={`${id}-kind`}>
          {label}
        </span>
        <div role="group" aria-labelledby={`${id}-kind`} className={phone ? 'pf-psegs ct-kinds' : 'segs'}>
          {PRIZE_KINDS.map(([key, title]) => (
            <button key={key} type="button" aria-pressed={fields.kind === key} onClick={() => onChange({ kind: key })}>
              {title}
            </button>
          ))}
        </div>
        {fields.kind === 'promo' && promoOff ? <span className="err">Промокод сработает при оплате, только если включён модуль «Промокоды».</span> : null}
      </div>
      {fields.kind === 'custom' ? (
        <div className="ct-custom">
          <div className="pf-grid pf-three">
            <Field id={value} label="Название приза" error={error} hint="Например, Telegram Premium на год" className="ct-wide2">
              <input
                id={value}
                className="field"
                value={fields.customTitle}
                onChange={(e) => onChange({ customTitle: e.target.value })}
                maxLength={100}
                aria-invalid={error ? true : undefined}
                aria-describedby={described(value, error)}
              />
            </Field>
            {extra}
          </div>
          <Field id={`${id}-how`} label="Как получить приз" hint="Победитель получит это в боте вместе с итогами. Пусто — «Чтобы получить приз, напишите в поддержку»">
            <textarea
              id={`${id}-how`}
              className="field ct-area"
              rows={phone ? 3 : 2}
              value={fields.instruction}
              onChange={(e) => onChange({ instruction: e.target.value })}
              maxLength={500}
              placeholder="Напишите в поддержку кодовое слово ПРИЗ, пришлём подарок в течение трёх дней"
              aria-describedby={`${id}-how-hint`}
            />
          </Field>
        </div>
      ) : (
        <div className="pf-grid pf-three">
          {fields.kind === 'days' ? (
            <Field id={value} label="Сколько дней" error={error} hint="Добавятся к действующей подписке победителя">
              <span className="pf-unit">
                <input
                  id={value}
                  className="field num"
                  inputMode="numeric"
                  value={fields.days}
                  onChange={(e) => onChange({ days: e.target.value })}
                  maxLength={3}
                  aria-invalid={error ? true : undefined}
                  aria-describedby={described(value, error)}
                />
                <span className="muted">дней</span>
              </span>
            </Field>
          ) : null}
          {fields.kind === 'balance' ? (
            <Field id={value} label="Сумма на баланс" error={error} hint="Зачислится победителю автоматически">
              <span className="pf-unit">
                {currency === 'USD' ? <span className="muted">{unit}</span> : null}
                <input
                  id={value}
                  className="field num"
                  inputMode={currency === 'USD' ? 'decimal' : 'numeric'}
                  value={fields.amount}
                  onChange={(e) => onChange({ amount: e.target.value })}
                  maxLength={10}
                  aria-invalid={error ? true : undefined}
                  aria-describedby={described(value, error)}
                />
                {currency === 'RUB' ? <span className="muted">{unit}</span> : null}
              </span>
            </Field>
          ) : null}
          {fields.kind === 'promo' ? (
            <>
              <Field id={value} label="Скидка" error={error} hint="Личный одноразовый код для каждого победителя">
                <span className="pf-unit">
                  <input
                    id={value}
                    className="field num"
                    inputMode="numeric"
                    value={fields.percent}
                    onChange={(e) => onChange({ percent: e.target.value })}
                    maxLength={3}
                    aria-invalid={error ? true : undefined}
                    aria-describedby={described(value, error)}
                  />
                  <span className="muted">%</span>
                </span>
              </Field>
              <Field id={`${id}-valid`} label="Код действует" hint="Дней с момента выдачи">
                <span className="pf-unit">
                  <input id={`${id}-valid`} className="field num" inputMode="numeric" value={fields.validDays} onChange={(e) => onChange({ validDays: e.target.value })} maxLength={3} />
                  <span className="muted">дней</span>
                </span>
              </Field>
            </>
          ) : null}
          {extra}
        </div>
      )}
    </>
  );
}


export function ContestForm({ phone, currency, options, initial, contest, onCancel, onSaved }: Props) {
  const base = useId();
  const [draft, setDraft] = useState<Draft>(initial);
  const [errors, setErrors] = useState<DraftErrors>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [lang, setLang] = useState<MessageLang>('ru');
  const locked = contest !== null && !contest.editable.all;

  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => {
    setDraft((d) => ({ ...d, [key]: value }));
    setErrors((e) => ({ ...e, [key]: undefined }));
  };

  const setRow = (key: string, patch: Partial<PrizeRow>) => {
    setDraft((d) => ({ ...d, rows: d.rows.map((r) => (r.key === key ? { ...r, ...patch } : r)) }));
    setErrors((e) => ({ ...e, value: undefined, winners: undefined, prizes: undefined }));
  };

  const addRow = () => {
    setDraft((d) => ({ ...d, rows: [...d.rows, prizeRow(1)] }));
    setErrors((e) => ({ ...e, prizes: undefined }));
  };

  const removeRow = (key: string) => {
    setDraft((d) => ({ ...d, rows: d.rows.filter((r) => r.key !== key) }));
    setErrors((e) => ({ ...e, prizes: undefined }));
  };

  const setMessage = (key: string, patch: Partial<MessageDraft>) => {
    setDraft((d) => ({ ...d, messages: (d.messages ?? []).map((m) => (m.key === key ? { ...m, ...patch } : m)) }));
    setErrors((e) => ({ ...e, messages: undefined }));
  };

  const addMessage = () => {
    setDraft((d) => ({
      ...d,
      messages: [...(d.messages ?? []), { key: messageKey(), id: null, audience: 'participants', onlyEligible: false, sendAt: '', text: '', textEn: '', locked: false }],
    }));
  };

  const submit = async () => {
    const built = buildBody(draft, currency, options);
    const found = Object.values(built.errors).some(Boolean);
    setErrors(built.errors);
    setFailure(found ? 'Исправьте отмеченные поля' : null);
    if (found) {
      return;
    }
    setBusy(true);
    try {
      const saved = contest
        ? await api<Contest>(`${BASE}/contests/${contest.id}`, { method: 'PUT', body: built.body })
        : await api<Contest>(`${BASE}/contests`, { method: 'POST', body: built.body });
      onSaved(saved);
    } catch (err) {
      setFailure(failText(err, contest ? 'Изменения не сохранились' : 'Конкурс не создался'));
    } finally {
      setBusy(false);
    }
  };

  const ids = {
    title: `${base}-title`,
    description: `${base}-desc`,
    winners: `${base}-winners`,
    starts: `${base}-starts`,
    ends: `${base}-ends`,
    min: `${base}-min`,
    friends: `${base}-friends`,
  };

  const unit = currency === 'RUB' ? '₽' : '$';
  const places = rowPlaces(draft.rows);
  const many = draft.rows.length > 1;
  const messages = draft.messages;
  const canAdd = messages !== null && messages.length < (options?.messages_max ?? 6);
  const hasResults = (messages ?? []).some((m) => m.audience === 'results');
  const english = lang === 'en';
  const total = messages?.length ?? 0;
  const translated = englishCount(messages ?? []);
  const langNote = english
    ? options && !options.english
      ? 'Английский выключен в настройках «Языки»: тексты сохранятся и заработают, когда его включат.'
      : 'Эти тексты получат клиенты, выбравшие английский. Где английского текста нет, уйдёт русский. Кому и когда — общие для обоих языков.'
    : translated === 0
      ? 'Клиенты, выбравшие английский, получат эти тексты: английских пока нет.'
      : translated === total
        ? 'Клиенты, выбравшие английский, получат английские тексты.'
        : `Английский текст есть у ${count(translated, 'сообщения', 'сообщений', 'сообщений')} из ${total}, по остальным клиенты с английским получат русский.`;

  return (
    <form
      className={phone ? 'pf pf-phone ct-form' : 'pf ct-form'}
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <fieldset className="ct-block" disabled={busy}>
        <legend className="ct-legend">Конкурс</legend>
        <div className="pf-grid">
          <Field id={ids.title} label="Название" error={errors.title} hint="Видят клиенты в боте и в сообщениях">
            <input
              id={ids.title}
              className="field"
              value={draft.title}
              onChange={(e) => set('title', e.target.value)}
              maxLength={options?.title_max ?? 100}
              placeholder="Например, Новогодний розыгрыш"
              aria-invalid={errors.title ? true : undefined}
              aria-describedby={described(ids.title, errors.title)}
            />
          </Field>
          <Field id={ids.description} label="Описание" error={errors.description} hint="Необязательно. Пара строк о конкурсе для клиента">
            <textarea
              id={ids.description}
              className="field ct-area"
              rows={2}
              value={draft.description}
              onChange={(e) => set('description', e.target.value)}
              maxLength={options?.description_max ?? 1000}
              aria-describedby={described(ids.description, errors.description)}
            />
          </Field>
        </div>
      </fieldset>

      <fieldset className="ct-block" disabled={busy || locked}>
        <legend className="ct-legend">{many ? 'Призы' : 'Приз'}</legend>
        {locked ? <p className="cap ct-lock">Конкурс уже идёт: призы, победителей и условия менять нельзя — так честнее для участников.</p> : null}
        {draft.rows.map((row, index) => {
          const [from, to] = places[index] ?? [1, 1];
          const countId = `${base}-${row.key}-count`;
          const countError = many ? undefined : errors.winners;
          return (
            <div key={row.key} className={many ? 'ct-tier' : 'ct-one'}>
              {many ? (
                <div className="ct-tier-top">
                  <b className="ct-tier-title">{placesLabel(from, to)}</b>
                  <button type="button" className="ct-remove hov" onClick={() => removeRow(row.key)} aria-label={`Удалить приз за ${placesLabel(from, to)}`}>
                    Удалить
                  </button>
                </div>
              ) : null}
              <PrizeInputs
                id={`${base}-${row.key}`}
                fields={row}
                onChange={(patch) => setRow(row.key, patch)}
                currency={currency}
                phone={phone}
                promoOff={!!options && !options.promo_enabled}
                error={many ? undefined : errors.value}
                label={many ? 'Приз' : 'Что получат победители'}
                extra={
                  <Field id={countId} label={many ? 'Сколько победителей' : 'Победителей'} error={countError} hint={many ? undefined : `От 1 до ${options?.winners_max ?? 100}`}>
                    <input
                      id={countId}
                      className="field num"
                      inputMode="numeric"
                      value={row.count}
                      onChange={(e) => setRow(row.key, { count: e.target.value })}
                      maxLength={3}
                      aria-invalid={countError ? true : undefined}
                      aria-describedby={described(countId, countError)}
                    />
                  </Field>
                }
              />
            </div>
          );
        })}
        {errors.prizes ? <span className="err">{errors.prizes}</span> : null}
        <div className="ct-tier-foot">
          <button type="button" className={phone ? 'pf-quiet press' : 'b40 hov'} onClick={addRow}>
            {many ? 'Добавить приз для следующих мест' : 'Другой приз для следующих мест'}
          </button>
          <span className="cap">
            {many
              ? `Всего победителей: ${places.reduce((m, [, to]) => Math.max(m, to), 0)}. Места раздаются по жеребьёвке, первый приз — первым местам.`
              : 'Например, первому месту — Telegram Premium, следующим — дни подписки.'}
          </span>
        </div>
      </fieldset>

      <fieldset className="ct-block" disabled={busy}>
        <legend className="ct-legend">Сроки</legend>
        <div className="pf-grid pf-three">
          {contest ? (
            <div className="pf-field">
              <span className="pf-label">Начало</span>
              <input className="field" type="datetime-local" value={draft.startsAt} disabled={locked} onChange={(e) => set('startsAt', e.target.value)} aria-label="Начало" />
              <span className="cap">{locked ? 'Конкурс уже начался' : 'Когда начнётся приём участников'}</span>
            </div>
          ) : (
            <Field id={ids.starts} label="Начало" error={errors.startsAt} hint={draft.startNow ? 'Приём участников начнётся сразу' : 'Когда начнётся приём участников'}>
              <span className="ct-start">
                <label className="ct-inline-check">
                  <input type="checkbox" checked={draft.startNow} onChange={(e) => set('startNow', e.target.checked)} />
                  <span>Сразу</span>
                </label>
                {draft.startNow ? null : (
                  <input
                    id={ids.starts}
                    className="field"
                    type="datetime-local"
                    value={draft.startsAt}
                    onChange={(e) => set('startsAt', e.target.value)}
                    aria-invalid={errors.startsAt ? true : undefined}
                    aria-describedby={described(ids.starts, errors.startsAt)}
                  />
                )}
              </span>
            </Field>
          )}
          <Field id={ids.ends} label="Итоги" error={errors.endsAt} hint={locked ? 'Итоги можно только отложить' : 'В это время бот выберет победителей и выдаст призы'}>
            <input
              id={ids.ends}
              className="field"
              type="datetime-local"
              value={draft.endsAt}
              onChange={(e) => set('endsAt', e.target.value)}
              aria-invalid={errors.endsAt ? true : undefined}
              aria-describedby={described(ids.ends, errors.endsAt)}
            />
          </Field>
        </div>
      </fieldset>

      <fieldset className="ct-block" disabled={busy || locked}>
        <legend className="ct-legend">Условия участия</legend>
        <p className="cap ct-lead">Проверяются по данным проекта — подпискам, оплатам и приглашениям, тариф не важен. Без условий участвовать может любой клиент бота. Перед жеребьёвкой условия проверяются ещё раз.</p>
        <label className="pf-check">
          <input type="checkbox" checked={draft.subscription} onChange={(e) => set('subscription', e.target.checked)} />
          <span>
            Действующая подписка
            <span className="cap">Подписка должна действовать и при вступлении, и в момент итогов</span>
          </span>
        </label>
        {draft.subscription ? (
          <label className="pf-check ct-indent">
            <input type="checkbox" checked={draft.paidOnly} onChange={(e) => set('paidOnly', e.target.checked)} />
            <span>
              Только купленная или подаренная
              <span className="cap">Пробная, по промо-ссылке и выданная вручную не считаются</span>
            </span>
          </label>
        ) : null}
        <label className="pf-check">
          <input type="checkbox" checked={draft.purchase} onChange={(e) => set('purchase', e.target.checked)} />
          <span>
            Оплата во время конкурса
            <span className="cap">Подписка, продление или доп. опции, оплаченные между началом и итогами. Возвращённые оплаты не считаются</span>
          </span>
        </label>
        {draft.purchase ? (
          <div className="ct-indent ct-row">
            <Field id={ids.min} label="Сумма оплаты от" error={errors.purchaseMin} hint="Пусто — любая сумма">
              <span className="pf-unit">
                {currency === 'USD' ? <span className="muted">{unit}</span> : null}
                <input
                  id={ids.min}
                  className="field num"
                  inputMode={currency === 'USD' ? 'decimal' : 'numeric'}
                  value={draft.purchaseMin}
                  onChange={(e) => set('purchaseMin', e.target.value)}
                  maxLength={10}
                  aria-invalid={errors.purchaseMin ? true : undefined}
                  aria-describedby={described(ids.min, errors.purchaseMin)}
                />
                {currency === 'RUB' ? <span className="muted">{unit}</span> : null}
              </span>
            </Field>
          </div>
        ) : null}
        <div className="ct-row">
          <Field
            id={ids.friends}
            label="Пригласить друзей"
            error={errors.friends}
            hint={options && !options.referral_enabled ? 'Модуль «Рефералы» выключен — приглашать будет нечем' : 'Сколько друзей пригласить за время конкурса. Пусто — не нужно'}
          >
            <input
              id={ids.friends}
              className="field num ct-short"
              inputMode="numeric"
              value={draft.friends}
              onChange={(e) => set('friends', e.target.value)}
              maxLength={3}
              placeholder="0"
              aria-invalid={errors.friends ? true : undefined}
              aria-describedby={described(ids.friends, errors.friends)}
            />
          </Field>
          {draft.friends.trim() && draft.friends.trim() !== '0' ? (
            <label className="pf-check ct-friend-paid">
              <input type="checkbox" checked={draft.friendsPaid} onChange={(e) => set('friendsPaid', e.target.checked)} />
              <span>
                Друг должен оплатить подписку
                <span className="cap">Защищает от накрутки пустыми аккаунтами</span>
              </span>
            </label>
          ) : null}
        </div>
      </fieldset>

      {messages !== null ? (
        <fieldset className="ct-block" disabled={busy}>
          <legend className="ct-legend">Сообщения</legend>
          <p className="cap ct-lead">
            Уходят через рассылки в бот, с кнопкой конкурса. Подстановки:{' '}
            {PLACEHOLDERS.map(([code, hint], i) => (
              <span key={code}>
                <code>{code}</code> — {hint}
                {i < PLACEHOLDERS.length - 1 ? ', ' : ''}
              </span>
            ))}
            . Победители получают отдельное сообщение со своим призом.
          </p>
          <div className="pf-field ct-langs">
            <span className="pf-label" id={`${base}-lang`}>
              Язык текстов
            </span>
            <div role="group" aria-labelledby={`${base}-lang`} className={phone ? 'pf-psegs' : 'segs'}>
              {(['ru', 'en'] as const).map((value) => (
                <button key={value} type="button" aria-pressed={lang === value} onClick={() => setLang(value)}>
                  {value === 'ru' ? 'Русский' : 'English'}
                </button>
              ))}
            </div>
            <span className="cap">{langNote}</span>
          </div>
          {messages.map((m, index) => (
            <div key={m.key} className={m.locked ? 'ct-msg ct-msg-locked' : 'ct-msg'}>
              <div className="ct-msg-top">
                <span className="ct-msg-n">Сообщение {index + 1}</span>
                {m.locked ? <span className="cap">Уже отправлено, не меняется</span> : null}
                {!m.locked ? (
                  <button
                    type="button"
                    className="ct-remove hov"
                    onClick={() => setDraft((d) => ({ ...d, messages: (d.messages ?? []).filter((x) => x.key !== m.key) }))}
                    aria-label={`Удалить сообщение ${index + 1}`}
                  >
                    Удалить
                  </button>
                ) : null}
              </div>
              <div className="ct-msg-grid">
                <div className="pf-field">
                  <span className="pf-label">Кому</span>
                  <select
                    className="field"
                    value={m.audience}
                    disabled={m.locked}
                    onChange={(e) => setMessage(m.key, { audience: e.target.value as Audience })}
                    aria-label={`Кому сообщение ${index + 1}`}
                  >
                    {AUDIENCES.filter(([key]) => key !== 'results' || m.audience === 'results' || !hasResults).map(([key, label]) => (
                      <option key={key} value={key}>
                        {label}
                      </option>
                    ))}
                  </select>
                  {m.audience === 'outsiders' ? (
                    <label className="ct-inline-check">
                      <input type="checkbox" checked={m.onlyEligible} disabled={m.locked} onChange={(e) => setMessage(m.key, { onlyEligible: e.target.checked })} />
                      <span>Только тем, кто уже подходит по условиям</span>
                    </label>
                  ) : null}
                </div>
                <div className="pf-field">
                  <span className="pf-label">Когда</span>
                  {m.audience === 'results' ? (
                    <span className="ct-when">Сразу после жеребьёвки</span>
                  ) : (
                    <input
                      className="field"
                      type="datetime-local"
                      value={m.sendAt}
                      disabled={m.locked}
                      onChange={(e) => setMessage(m.key, { sendAt: e.target.value })}
                      aria-label={`Когда отправить сообщение ${index + 1}`}
                    />
                  )}
                </div>
              </div>
              {english ? (
                <textarea
                  key="en"
                  className="field ct-area"
                  rows={phone ? 4 : 3}
                  lang="en"
                  value={m.textEn}
                  disabled={m.locked}
                  onChange={(e) => setMessage(m.key, { textEn: e.target.value })}
                  placeholder="Пусто — клиенты с английским получат русский текст"
                  aria-label={`Текст сообщения ${index + 1} на английском`}
                />
              ) : (
                <textarea
                  key="ru"
                  className="field ct-area"
                  rows={phone ? 4 : 3}
                  value={m.text}
                  disabled={m.locked}
                  onChange={(e) => setMessage(m.key, { text: e.target.value })}
                  aria-label={`Текст сообщения ${index + 1}`}
                />
              )}
            </div>
          ))}
          {errors.messages ? <span className="err">{errors.messages}</span> : null}
          {canAdd ? (
            <button type="button" className={phone ? 'pf-quiet press' : 'b40 hov'} onClick={addMessage}>
              Добавить сообщение
            </button>
          ) : null}
        </fieldset>
      ) : (
        <p className="cap ct-lead">
          После создания появятся сообщения на русском и английском: объявление на старте, напоминание за день до итогов тем, кто не участвует, напоминание участникам за три часа и итоги. Их можно поправить на странице конкурса.
        </p>
      )}

      <div className="pf-actions">
        <button type="submit" className={phone ? 'np-ok press' : 'btn-main btn'} disabled={busy}>
          {busy ? 'Сохраняем…' : contest ? 'Сохранить' : 'Создать конкурс'}
        </button>
        <button type="button" className={phone ? 'np-cancel press' : 'btn-line hov'} onClick={onCancel} disabled={busy}>
          Отменить
        </button>
        {failure ? (
          <span className="err pf-fail" role="alert">
            {failure}
          </span>
        ) : null}
      </div>
    </form>
  );
}
