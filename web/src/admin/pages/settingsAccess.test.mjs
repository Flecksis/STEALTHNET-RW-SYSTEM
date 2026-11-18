import { test } from 'node:test';
import assert from 'node:assert/strict';
import { pageNeed, SECTIONS } from './settingsAccess.ts';

const support = ['clients_read', 'clients_write', 'support'];
const can = (granted) => (permission) => granted.includes(permission);

test('поддержке не открыт ни один раздел настроек', () => {
  assert.deepEqual(
    SECTIONS.filter((s) => s.need.every(can(support))).map((s) => s.label),
    [],
  );
  assert.deepEqual(pageNeed('/admin/settings/general'), ['settings']);
  assert.deepEqual(pageNeed('/admin/settings/bot/texts'), ['settings']);
  assert.deepEqual(pageNeed('/admin/settings/unknown'), ['settings']);
});

test('отдельные права открывают только свои разделы', () => {
  const backups = can([...support, 'backups']);
  assert.deepEqual(
    SECTIONS.filter((s) => s.need.every(backups)).map((s) => s.label),
    ['Резервные копии'],
  );
  assert.deepEqual(pageNeed('/admin/settings/backups'), ['backups']);
  assert.deepEqual(pageNeed('/admin/settings/remnawave/sync'), ['remna_sync']);
  assert.deepEqual(pageNeed('/admin/settings/remnawave'), ['settings']);
  assert.deepEqual(pageNeed('/admin/settings/import'), ['settings', 'money']);
  const settingsOnly = can(['settings']);
  assert.ok(!SECTIONS.find((s) => s.label === 'Перенос из другого бота').need.every(settingsOnly));
});
