import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { ApiError } from '../../lib/api';
import { dateTime, groupDigits } from '../../lib/format';
import { Icon } from '../../lib/icons';
import type { Currency } from '../../lib/types';
import { downloadRows } from './csv';
import { amountText, loadList, type ListItem, type ListRef, type Query } from './model';

export type Opened = { ref: ListRef; title: string };

type Props = {
  opened: Opened | null;
  q: Query;
  currency: Currency;
  onClose: () => void;
  phone: boolean;
};

export function ListDrawer({ opened, q, currency, onClose, phone }: Props) {
  const [items, setItems] = useState<ListItem[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(0);
  const [more, setMore] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const key = opened ? JSON.stringify(opened.ref) : '';

  useEffect(() => {
    if (!opened) {
      return;
    }
    let alive = true;
    setBusy(true);
    setError(null);
    setItems([]);
    setPage(0);
    loadList(opened.ref, q, 0)
      .then((p) => {
        if (alive) {
          setItems(p.items);
          setTotal(p.total);
          setMore(p.more);
        }
      })
      .catch((err: unknown) => {
        if (alive) {
          setError(err instanceof ApiError ? err.message : 'Не получилось загрузить список');
        }
      })
      .finally(() => {
        if (alive) {
          setBusy(false);
        }
      });
    return () => {
      alive = false;
    };
  }, [key]);

  useEffect(() => {
    if (!opened) {
      return;
    }
    const esc = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
      }
    };
    document.addEventListener('keydown', esc);
    return () => document.removeEventListener('keydown', esc);
  }, [opened, onClose]);

  if (!opened) {
    return null;
  }

  const next = () => {
    setBusy(true);
    loadList(opened.ref, q, page + 1)
      .then((p) => {
        setItems((cur) => [...cur, ...p.items]);
        setPage(p.page);
        setMore(p.more);
      })
      .catch(() => setError('Не получилось загрузить ещё'))
      .finally(() => setBusy(false));
  };

  const csv = () =>
    downloadRows(
      opened.title,
      [
        ['Клиент', 'ID', 'Что', 'Сумма', 'Когда'],
        ...items.map((i) => [i.name, String(i.client_id), i.what, amountText(i, currency), i.at ? dateTime(i.at) : '']),
      ],
    );

  const list = (
    <div className="dr-list">
      {error ? <p className="dr-say" role="alert">{error}</p> : null}
      {items.map((i, n) => (
        <Link key={`${i.client_id}-${n}`} to={`/admin/clients/${i.client_id}`} className="dr-row hov">
          <span className="dr-who">{i.name}</span>
          <span className="dr-sum num">{amountText(i, currency)}</span>
          <span className="dr-what">{i.what}</span>
          <span className="dr-when">{i.at ? dateTime(i.at) : ''}</span>
        </Link>
      ))}
      {!busy && !error && items.length === 0 ? <p className="dr-say muted">Здесь пусто</p> : null}
      {more ? (
        <button type="button" className="dr-more hov" onClick={next} disabled={busy}>
          {busy ? 'Загружаем…' : 'Показать ещё'}
        </button>
      ) : null}
    </div>
  );

  const head = (
    <>
      <h2 className="dr-title">{opened.title}</h2>
      <p className="dr-say">{busy && !items.length ? 'Загружаем…' : `Всего ${groupDigits(total)}`}</p>
    </>
  );

  if (phone) {
    return (
      <>
        <div className="sheet-back" onClick={onClose} />
        <div className="sheet" role="dialog" aria-label={opened.title}>
          <div className="sheet-grip" />
          <div className="sheet-head">{head}</div>
          {list}
          <div className="sheet-foot">
            <button type="button" onClick={onClose}>
              Закрыть
            </button>
            <button type="button" style={{ flex: '0 0 88px' }} onClick={csv} disabled={!items.length}>
              CSV
            </button>
          </div>
        </div>
      </>
    );
  }

  return (
    <>
      <div className="dr-back" onClick={onClose} />
      <aside className="dr" role="dialog" aria-label={opened.title}>
        <div className="dr-head">
          <div className="dr-top">
            <div>{head}</div>
            <button type="button" className="dr-x hov" aria-label="Закрыть" onClick={onClose}>
              <Icon name="close" size={20} stroke={2} />
            </button>
          </div>
        </div>
        {list}
        <div className="dr-foot">
          <span className="dr-back-note">{items.length < total ? `Показаны ${groupDigits(items.length)} из ${groupDigits(total)}` : ''}</span>
          <button type="button" className="b36 hov" onClick={csv} disabled={!items.length}>
            CSV
          </button>
        </div>
      </aside>
    </>
  );
}
