import { test } from 'node:test';
import assert from 'node:assert/strict';
import { protectSections } from './sections.ts';

test('разделы защиты видны только у включённых модулей', () => {
  assert.deepEqual(protectSections(['promo', 'antibot'], true), { antibot: true, limits: false });
  assert.deepEqual(protectSections(['limits'], true), { antibot: false, limits: true });
  assert.deepEqual(protectSections(['antibot', 'limits'], true), { antibot: true, limits: true });
  assert.deepEqual(protectSections(undefined, true), { antibot: false, limits: false });
});

test('без права смотреть клиентов разделов защиты нет', () => {
  assert.deepEqual(protectSections(['antibot', 'limits'], false), { antibot: false, limits: false });
});
