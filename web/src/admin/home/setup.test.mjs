import { test } from 'node:test';
import assert from 'node:assert/strict';
import { setupTodos } from './setup.ts';

const owner = () => true;
const support = (permission) => ['clients_read', 'clients_write', 'support'].includes(permission);

test('на новой панели в делах шаги первого запуска', () => {
  const steps = setupTodos({ remnawave: false, bot: false, tariffs: false }, owner);
  assert.deepEqual(
    steps.map((s) => [s.title, s.to]),
    [
      ['Подключите Remnawave', '/admin/settings/remnawave'],
      ['Подключите бота', '/admin/settings/bot'],
      ['Создайте тариф', '/admin/tariffs'],
    ],
  );
  assert.deepEqual(setupTodos({ remnawave: true, bot: true, tariffs: true }, owner), []);
  assert.deepEqual(
    setupTodos({ remnawave: true, bot: false, tariffs: true }, owner).map((s) => s.key),
    ['setup-bot'],
  );
  assert.deepEqual(setupTodos(undefined, owner), []);
});

test('шаги видны только тем, кто может их сделать', () => {
  assert.deepEqual(setupTodos({ remnawave: false, bot: false, tariffs: false }, support), []);
  const tariffsOnly = (permission) => permission === 'tariffs';
  assert.deepEqual(
    setupTodos({ remnawave: false, bot: false, tariffs: false }, tariffsOnly).map((s) => s.key),
    ['setup-tariffs'],
  );
});
