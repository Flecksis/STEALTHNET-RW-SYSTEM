import { test } from 'node:test';
import assert from 'node:assert/strict';
import { SETTLE_MS, settleLeft } from './settle.ts';

test('быстрое нажатие «Я не робот» ждёт, пока сервер примет ответ', () => {
  assert.ok(SETTLE_MS > 2000);
  assert.equal(settleLeft(1000, 1000), SETTLE_MS);
  assert.equal(settleLeft(1000, 1600), SETTLE_MS - 600);
  assert.equal(settleLeft(1000, 1000 + SETTLE_MS), 0);
  assert.equal(settleLeft(1000, 60_000), 0);
});

test('без отметки времени ожидания нет, а сбой часов не растягивает его', () => {
  assert.equal(settleLeft(undefined, 5000), 0);
  assert.equal(settleLeft(90_000, 1000), SETTLE_MS);
});
