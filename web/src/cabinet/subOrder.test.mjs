import { test } from 'node:test';
import assert from 'node:assert/strict';
import { betterStart, byTimeLeft, numberOf } from './subOrder.ts';

const sub = (id, status, expire_at) => ({ status, subscription: { id, mirror: { expire_at } } });
const ids = (list) => list.map((v) => v.subscription.id);

const old = sub(1, 'expired', '2026-10-01T00:00:00Z');
const month = sub(2, 'active', '2026-12-20T00:00:00Z');
const half = sub(3, 'active', '2027-04-24T00:00:00Z');
const soon = sub(4, 'expiring', '2026-11-27T00:00:00Z');

test('действующие подписки идут первыми, самая долгая — сверху', () => {
  const list = [old, month, half, soon];
  assert.deepEqual(ids(byTimeLeft(list)), [3, 2, 4, 1]);
  assert.deepEqual(ids(list), [1, 2, 3, 4]);
});

test('подписки без трафика, заблокированные и закончившиеся ниже рабочих', () => {
  const limited = sub(5, 'limited', '2028-01-01T00:00:00Z');
  const blocked = sub(6, 'blocked', '2029-01-01T00:00:00Z');
  assert.deepEqual(ids(byTimeLeft([old, blocked, limited, soon])), [4, 5, 6, 1]);
});

test('только что купленная подписка без срока стоит первой, равные сроки — по порядку покупки', () => {
  const fresh = sub(9, 'active', null);
  const twin = sub(8, 'active', null);
  assert.deepEqual(ids(byTimeLeft([half, fresh, twin])), [8, 9, 3]);
  assert.deepEqual(ids(byTimeLeft([sub(12, 'active', half.subscription.mirror.expire_at), half])), [3, 12]);
  assert.deepEqual(byTimeLeft([]), []);
});

test('номер подписки в названии не зависит от порядка на экране', () => {
  const list = byTimeLeft([old, month, half]);
  assert.deepEqual([1, 2, 3].map((id) => numberOf(id, list)), [1, 2, 3]);
});

test('закончившаяся подписка не открывается, если есть действующая', () => {
  const list = byTimeLeft([old, month, half]);
  assert.equal(betterStart(list, 1), 3);
  assert.equal(betterStart(list, 2), null);
  assert.equal(betterStart(list, null), null);
  assert.equal(betterStart(list, 'newest'), null);
  assert.equal(betterStart(list, 77), null);
  assert.equal(betterStart([old], 1), null);
  assert.equal(betterStart(byTimeLeft([old, sub(5, 'expired', '2026-11-01T00:00:00Z')]), 1), null);
  assert.equal(betterStart([], 1), null);
});
