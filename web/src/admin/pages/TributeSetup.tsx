import { useCallback, useEffect, useMemo, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { money, periodLabel } from '../../lib/format';
import type { Currency } from '../../lib/types';
import type { Tariff, View } from '../tariffs/model';

export type TributeProduct = { id: number; name: string; amount: number; currency: Currency; link: string };
export type TributePlan = { period: number; subscription: number; name: string; tariff: number; tariff_period: number };
export type TributeValue = { products: TributeProduct[]; plans: TributePlan[] };

type CatalogPeriod = { id: number; period: string; price: number | null };
type CatalogSubscription = { id: number; name: string; currency: string; periods: CatalogPeriod[] };
type Catalog = { products: TributeProduct[]; subscriptions: CatalogSubscription[] };

const PERIODS: Record<string, string> = {
  weekly: 'каждую неделю',
  monthly: 'каждый месяц',
  quarterly: 'раз в 3 месяца',
  halfyearly: 'раз в полгода',
  yearly: 'раз в год',
  onetime: 'один раз',
  trial: 'пробный срок',
};

function record(item: unknown): Record<string, unknown> | null {
  return item && typeof item === 'object' ? (item as Record<string, unknown>) : null;
}

function isProduct(item: unknown): item is TributeProduct {
  const p = record(item);
  return !!p && typeof p.id === 'number' && typeof p.amount === 'number' && (p.currency === 'USD' || p.currency === 'RUB') && typeof p.link === 'string';
}

function isPlan(item: unknown): item is TributePlan {
  const p = record(item);
  return !!p && typeof p.period === 'number' && typeof p.tariff === 'number' && typeof p.tariff_period === 'number';
}

export function tributeValue(settings: Record<string, unknown>): TributeValue {
  const list = (key: string) => (Array.isArray(settings[key]) ? (settings[key] as unknown[]) : []);
  return {
    products: list('products')
      .filter(isProduct)
      .map((p) => ({ ...p, name: typeof p.name === 'string' ? p.name : '' })),
    plans: list('plans')
      .filter(isPlan)
      .map((p) => ({ ...p, subscription: typeof p.subscription === 'number' ? p.subscription : 0, name: typeof p.name === 'string' ? p.name : '' })),
  };
}

function periodText(period: string): string {
  return PERIODS[period] ?? period;
}

type Props = {
  value: TributeValue;
  onChange: (next: TributeValue) => void;
  busy: boolean;
  connected: boolean;
  currency: Currency;
};

export function TributeSetup({ value, onChange, busy, connected, currency }: Props) {
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [tariffs, setTariffs] = useState<Tariff[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    api<View>('/api/admin/tariffs')
      .then((view) => alive && setTariffs(view.tariffs))
      .catch(() => alive && setTariffs([]));
    return () => {
      alive = false;
    };
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setCatalog(await api<Catalog>('/api/admin/payments/providers/tribute/catalog'));
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Список из Tribute не загрузился');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (connected) {
      void load();
    }
  }, [connected, load]);

  const options = useMemo(
    () =>
      (tariffs ?? []).flatMap((t) =>
        t.periods.filter((p) => p.active).map((p) => ({ value: `${t.id}:${p.id}`, label: `${t.name} · ${periodLabel(p.days) ?? `${p.days} дн.`}` })),
      ),
    [tariffs],
  );
  const optionLabel = (plan: TributePlan) => options.find((o) => o.value === `${plan.tariff}:${plan.tariff_period}`)?.label ?? 'тариф удалён';

  const chosen = new Set(value.products.map((p) => p.id));
  const toggle = (product: TributeProduct) =>
    onChange({ ...value, products: chosen.has(product.id) ? value.products.filter((p) => p.id !== product.id) : [...value.products, product] });
  const dropProduct = (id: number) => onChange({ ...value, products: value.products.filter((p) => p.id !== id) });

  const bind = (sub: CatalogSubscription, period: CatalogPeriod, choice: string) => {
    const rest = value.plans.filter((p) => p.period !== period.id);
    const [tariff, tariffPeriod] = choice.split(':').map(Number);
    if (!choice || !tariff || !tariffPeriod) {
      onChange({ ...value, plans: rest });
      return;
    }
    onChange({ ...value, plans: [...rest, { period: period.id, subscription: sub.id, name: `${sub.name} · ${periodText(period.period)}`, tariff, tariff_period: tariffPeriod }] });
  };
  const dropPlan = (period: number) => onChange({ ...value, plans: value.plans.filter((p) => p.period !== period) });

  const staleProducts = catalog ? value.products.filter((p) => !catalog.products.some((c) => c.id === p.id)) : [];
  const knownPeriods = new Set((catalog?.subscriptions ?? []).flatMap((s) => s.periods.map((p) => p.id)));
  const stalePlans = catalog ? value.plans.filter((p) => !knownPeriods.has(p.period)) : [];

  return (
    <>
      <div className="sf-field" style={{ maxWidth: 'none' }} role="group" aria-labelledby="trb-products">
        <span className="sf-label" id="trb-products">
          Товары для пополнения
        </span>
        <span className="cap">
          Создайте в Tribute цифровые товары на суммы пополнения, например 300, 500 и 1000 ₽, и отметьте их здесь. Отмеченные суммы станут кнопками пополнения. При
          покупке тарифа клиент оплачивает самый дешёвый товар не меньше цены: заказ оплачивается с баланса, остаток остаётся на балансе.
        </span>
        {!connected ? <span className="cap">Сохраните API-ключ, и панель загрузит список товаров и подписок из Tribute.</span> : null}
        {catalog
          ? catalog.products.map((p) => {
              const other = p.currency !== currency;
              return (
                <div key={p.id} className="pay-mrow">
                  <label className="pay-check pay-mname">
                    <input type="checkbox" checked={chosen.has(p.id)} onChange={() => toggle(p)} disabled={busy || (other && !chosen.has(p.id))} />
                    {p.name || `Товар №${p.id}`}
                  </label>
                  <span className="pay-mhint muted">
                    {money({ minor: p.amount, currency: p.currency })}
                    {other ? ' · валюта не совпадает с валютой панели, клиентам не показывается' : ''}
                  </span>
                </div>
              );
            })
          : value.products.map((p) => (
              <div key={p.id} className="pay-mrow">
                <span className="pay-mname">{p.name || `Товар №${p.id}`}</span>
                <span className="pay-mhint muted">{money({ minor: p.amount, currency: p.currency })}</span>
              </div>
            ))}
        {catalog && catalog.products.length === 0 ? <span className="cap">В Tribute пока нет одобренных цифровых товаров.</span> : null}
        {staleProducts.map((p) => (
          <div key={p.id} className="pay-mrow">
            <span className="pay-mname">{p.name || `Товар №${p.id}`}</span>
            <span className="pay-mhint muted">{money({ minor: p.amount, currency: p.currency })} · Tribute больше не продаёт этот товар</span>
            <button type="button" className="b34 hov" onClick={() => dropProduct(p.id)} disabled={busy}>
              Убрать
            </button>
          </div>
        ))}
      </div>
      <div className="sf-field" style={{ maxWidth: 'none' }} role="group" aria-labelledby="trb-plans">
        <span className="sf-label" id="trb-plans">
          Подписки Tribute
        </span>
        <span className="cap">
          Привяжите срок подписки Tribute к тарифу: новая подписка выдаёт тариф на выбранный период, каждое продление — ещё на период, отмена подписки оплаченный срок не
          забирает. Деньги за непривязанный срок идут на баланс клиента.
        </span>
        {catalog
          ? catalog.subscriptions.flatMap((sub) =>
              sub.periods.map((period) => {
                const plan = value.plans.find((p) => p.period === period.id);
                const price = period.price !== null && (sub.currency === 'USD' || sub.currency === 'RUB') ? money({ minor: period.price, currency: sub.currency }) : null;
                return (
                  <div key={period.id} className="pay-mrow">
                    <span className="pay-mname">{sub.name || `Подписка №${sub.id}`}</span>
                    <span className="pay-mhint muted">
                      {periodText(period.period)}
                      {price ? ` · ${price}` : ''}
                    </span>
                    <select
                      className="field pay-mtitle"
                      value={plan ? `${plan.tariff}:${plan.tariff_period}` : ''}
                      onChange={(e) => bind(sub, period, e.target.value)}
                      disabled={busy || tariffs === null}
                      aria-label={`Что выдаёт «${sub.name}», ${periodText(period.period)}`}
                    >
                      <option value="">Деньги на баланс</option>
                      {options.map((o) => (
                        <option key={o.value} value={o.value}>
                          {o.label}
                        </option>
                      ))}
                    </select>
                  </div>
                );
              }),
            )
          : value.plans.map((p) => (
              <div key={p.period} className="pay-mrow">
                <span className="pay-mname">{p.name || `Срок №${p.period}`}</span>
                <span className="pay-mhint muted">{optionLabel(p)}</span>
              </div>
            ))}
        {catalog && catalog.subscriptions.length === 0 ? <span className="cap">В Tribute пока нет подписок.</span> : null}
        {stalePlans.map((p) => (
          <div key={p.period} className="pay-mrow">
            <span className="pay-mname">{p.name || `Срок №${p.period}`}</span>
            <span className="pay-mhint muted">{optionLabel(p)} · этого срока больше нет в Tribute</span>
            <button type="button" className="b34 hov" onClick={() => dropPlan(p.period)} disabled={busy}>
              Убрать
            </button>
          </div>
        ))}
        {connected ? (
          <span className="pay-lrow">
            <button type="button" className="btn-line hov" onClick={() => void load()} disabled={busy || loading} style={{ padding: '0 16px' }}>
              {loading ? 'Загружаем…' : 'Обновить список из Tribute'}
            </button>
          </span>
        ) : null}
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : null}
      </div>
    </>
  );
}
