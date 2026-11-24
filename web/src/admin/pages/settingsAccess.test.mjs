import { test } from 'node:test';
import assert from 'node:assert/strict';
import { firstSection, pageNeed, SECTIONS } from './settingsAccess.ts';

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

test('настройки открываются на первом разделе, который администратору доступен', () => {
  const everything = () => true;
  const admin = can(['clients_read', 'clients_write', 'money', 'refunds', 'tariffs', 'infrastructure', 'support', 'marketing', 'modules', 'settings', 'clients_login']);
  assert.equal(firstSection(everything)?.label, 'Общие');
  assert.equal(firstSection(everything)?.to, '/admin/settings/general');
  assert.equal(firstSection(admin)?.label, 'Общие');
  assert.equal(firstSection(can([...support, 'backups']))?.label, 'Резервные копии');
  assert.equal(firstSection(can([...support, 'backups']))?.to, '/admin/settings/backups');
  assert.equal(firstSection(can(['admins']))?.label, 'Администраторы');
  assert.equal(firstSection(can(support)), undefined);
});
