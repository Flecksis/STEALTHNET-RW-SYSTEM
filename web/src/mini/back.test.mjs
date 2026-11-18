import { test } from 'node:test';
import assert from 'node:assert/strict';
import { backTarget } from './back.ts';

test('на главном экране мини-приложения кнопки «Назад» нет', () => {
  assert.equal(backTarget('/app'), null);
  assert.equal(backTarget('/app/'), null);
  assert.equal(backTarget('/cabinet'), null);
});

test('с вложенного экрана назад ведёт на родительский', () => {
  assert.equal(backTarget('/app/plans'), '/app');
  assert.equal(backTarget('/app/profile/email'), '/app/profile');
  assert.equal(backTarget('/app/support/12'), '/app/support');
  assert.equal(backTarget('/app/support/12/'), '/app/support');
});
