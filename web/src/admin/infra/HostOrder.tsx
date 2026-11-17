import { Flag } from '../../lib/Flag';
import { Icon } from '../../lib/icons';
import { inboundText, moveIn, nodesOf, type HostsData } from './hosts';

type OrderProps = {
  data: HostsData;
  order: string[];
  setOrder: (order: string[]) => void;
  busy: boolean;
  onSave: () => void;
  onCancel: () => void;
  phone?: boolean;
};

export function OrderList({ data, order, setOrder, busy, onSave, onCancel, phone = false }: OrderProps) {
  const changed = order.some((uuid, i) => data.hosts[i]?.host.uuid !== uuid);
  return (
    <div className={phone ? 'hs-ordering phone' : 'hs-ordering'}>
      <div className="hs-bar">
        <span className="inf-col">
          <strong>Порядок в приложении</strong>
          <span className="muted">Так серверы идут в списке у клиента, сверху вниз</span>
        </span>
        {phone ? null : (
          <span className="cc-btns" style={{ gap: 10 }}>
            <button type="button" className="btn-main btn" onClick={onSave} disabled={busy || !changed}>
              {busy ? 'Сохраняем…' : 'Сохранить порядок'}
            </button>
            <button type="button" className="btn-line hov sq-cancel" onClick={onCancel} disabled={busy}>
              Отменить
            </button>
          </span>
        )}
      </div>
      <ol className="hs-order" aria-label="Порядок хостов">
        {order.map((uuid, index) => {
          const row = data.hosts.find((r) => r.host.uuid === uuid);
          if (!row) {
            return null;
          }
          const nodes = nodesOf(row, data);
          return (
            <li key={uuid} className={row.host.isDisabled ? 'hs-orow hs-off' : 'hs-orow'}>
              <span className="hs-num num" aria-hidden="true">
                {index + 1}
              </span>
              <span className="hs-oname">
                <span className="hs-otitle">
                  {nodes.map((n) => (
                    <Flag key={n.uuid} code={n.country_code} />
                  ))}
                  <strong>{row.host.remark}</strong>
                </span>
                <span className="inf-sub">
                  {[nodes.map((n) => n.name).join(', '), inboundText(row, data), row.host.isDisabled ? 'выключен' : row.host.isHidden ? 'скрыт' : null].filter(Boolean).join(' · ')}
                </span>
              </span>
              <span className="hs-arrows">
                <button type="button" className="hs-arrow hov press" aria-label={`Выше: ${row.host.remark}`} onClick={() => setOrder(moveIn(order, index, -1))} disabled={busy || index === 0}>
                  <Icon name="arrow-up" size={18} stroke={2} />
                </button>
                <button
                  type="button"
                  className="hs-arrow hov press"
                  aria-label={`Ниже: ${row.host.remark}`}
                  onClick={() => setOrder(moveIn(order, index, 1))}
                  disabled={busy || index === order.length - 1}
                >
                  <Icon name="arrow-down" size={18} stroke={2} />
                </button>
              </span>
            </li>
          );
        })}
      </ol>
      {phone ? (
        <div className="hs-pbtns">
          <button type="button" className="np-ok press" onClick={onSave} disabled={busy || !changed}>
            {busy ? 'Сохраняем…' : 'Сохранить порядок'}
          </button>
          <button type="button" className="np-cancel press" onClick={onCancel} disabled={busy}>
            Отменить
          </button>
        </div>
      ) : null}
    </div>
  );
}
