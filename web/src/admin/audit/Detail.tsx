import { Link } from 'react-router';
import { avatarColor, dateTime, initials } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { EMPTY, actorKey, actorName, actorNote, valueText, type Actor, type AuditItem, type Filters } from './model';

const HIDDEN = 'скрыто';

export function ActorMark({ actor, size = 34 }: { actor: Actor; size?: number }) {
  const style = { width: size, height: size };
  if (actor.kind === 'admin') {
    return (
      <span className="au-mark" style={{ ...style, background: avatarColor(actor.id) }} aria-hidden="true">
        {(actor.email ?? 'А').charAt(0).toUpperCase()}
      </span>
    );
  }
  if (actor.kind === 'client') {
    return (
      <span className="au-mark" style={{ ...style, background: avatarColor(actor.id) }} aria-hidden="true">
        {initials(actor.name.replace(/^@/, ''))}
      </span>
    );
  }
  const icon = actor.kind === 'system' ? 'refresh' : actor.kind === 'module' ? 'blocks' : 'lock';
  return (
    <span className="au-mark quiet" style={style} aria-hidden="true">
      <Icon name={icon} size={Math.round(size * 0.5)} />
    </span>
  );
}

function whenFull(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime())
    ? ''
    : date.toLocaleString('ru-RU', { day: 'numeric', month: 'long', year: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

function actorButton(actor: Actor): string {
  switch (actor.kind) {
    case 'system':
      return 'Все автоматические действия';
    case 'client':
      return 'Действия клиента из кабинета и бота';
    case 'api_key':
      return 'Все действия ключа';
    case 'module':
      return 'Все действия модуля';
    default:
      return 'Все действия администратора';
  }
}

type Props = {
  item: AuditItem;
  phone?: boolean;
  onFilter: (patch: Partial<Filters>) => void;
};

export function Detail({ item, phone, onFilter }: Props) {
  const target = item.target;
  const clientId = target?.client_id ?? null;
  return (
    <div className={phone ? 'au-panel phone' : 'au-panel'}>
      <div className="au-panel-head">
        <p className="au-panel-when" title={whenFull(item.at)}>
          {dateTime(item.at)}
        </p>
        <h2 className="au-panel-title">{item.title}</h2>
        {target ? (
          <p className="au-panel-target">
            {target.link ? <Link to={target.link}>{target.label}</Link> : <span>{target.label}</span>}
            {target.note ? <span className="muted"> · {target.note}</span> : null}
          </p>
        ) : null}
      </div>
      <div className="au-panel-who">
        <ActorMark actor={item.actor} />
        <span className="au-who-text">
          <span className="au-who-name">{actorName(item.actor)}</span>
          <span className="au-who-note">{actorNote(item.actor)}</span>
        </span>
      </div>
      {item.via_unknown ? (
        <p className="au-legacy">
          Запись сделана до обновления, с которого журнал отличает API-ключи и внешние модули. Если действие шло через ключ, здесь указан администратор, который его
          выпустил.
        </p>
      ) : null}
      {item.changes.length ? (
        <section className="au-block" aria-label="Что изменилось">
          <h3 className="au-block-title">Что изменилось</h3>
          {phone ? (
            <dl className="au-diff">
              {item.changes.map((change, index) => (
                <div key={`${change.label}-${index}`} className="au-diff-row">
                  <dt>{change.label}</dt>
                  <dd>
                    <span className="au-was">{valueText(change.before)}</span>
                    <span className="au-arrow" aria-hidden="true">
                      →
                    </span>
                    <span className="sr-only">стало</span>
                    <span className="au-now">{valueText(change.after)}</span>
                  </dd>
                </div>
              ))}
            </dl>
          ) : (
            <div className="au-table" role="table" aria-label="Что изменилось">
              <div role="row" className="au-trow au-thead">
                <span role="columnheader">Поле</span>
                <span role="columnheader">Было</span>
                <span role="columnheader">Стало</span>
              </div>
              {item.changes.map((change, index) => (
                <div key={`${change.label}-${index}`} role="row" className="au-trow">
                  <span role="cell">{change.label}</span>
                  <span role="cell" className="au-was">
                    {valueText(change.before)}
                  </span>
                  <span role="cell" className="au-now">
                    {valueText(change.after)}
                  </span>
                </div>
              ))}
            </div>
          )}
        </section>
      ) : null}
      {item.fields.length || item.ip ? (
        <section className="au-block" aria-label="Подробности">
          <h3 className="au-block-title">Подробности</h3>
          <dl className="au-facts">
            {item.fields.map((field) => (
              <div key={field.key}>
                <dt>{field.label ?? <span className="mono">{field.key}</span>}</dt>
                <dd className={field.value === HIDDEN ? 'muted' : undefined}>{valueText(field.value)}</dd>
              </div>
            ))}
            {item.ip ? (
              <div>
                <dt>Адрес</dt>
                <dd className="num">{item.ip}</dd>
              </div>
            ) : null}
          </dl>
        </section>
      ) : null}
      <div className="au-panel-foot">
        {clientId !== null ? (
          <button type="button" className="b36 hov" onClick={() => onFilter({ ...EMPTY, client: String(clientId) })}>
            Все действия с этим клиентом
          </button>
        ) : null}
        <button type="button" className="b36 hov" onClick={() => onFilter({ ...EMPTY, actor: actorKey(item.actor) })}>
          {actorButton(item.actor)}
        </button>
      </div>
      <p className="au-code">
        Запись №{item.id} · <span className="mono">{item.action}</span>
      </p>
    </div>
  );
}
