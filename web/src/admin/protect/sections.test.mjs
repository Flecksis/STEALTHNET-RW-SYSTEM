import { test } from 'node:test';
import assert from 'node:assert/strict';
import { clientFrom, protectSections } from './sections.ts';

test('разделы защиты видны только у включённых модулей', () => {
  assert.deepEqual(protectSections(['promo', 'antibot'], true), { antibot: true, limits: false });
  assert.deepEqual(protectSections(['limits'], true), { antibot: false, limits: true });
  assert.deepEqual(protectSections(['antibot', 'limits'], true), { antibot: true, limits: true });
  assert.deepEqual(protectSections(undefined, true), { antibot: false, limits: false });
});

test('без права смотреть клиентов разделов защиты нет', () => {
  assert.deepEqual(protectSections(['antibot', 'limits'], false), { antibot: false, limits: false });
});

test('журнал одного клиента открывается по номеру из адреса', () => {
  assert.equal(clientFrom('4152'), 4152);
  for (const raw of [null, '', '0', '-3', '12abc', '1.5', '../4']) {
    assert.equal(clientFrom(raw), null, String(raw));
  }
});
