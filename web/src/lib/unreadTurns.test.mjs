import { test } from 'node:test';
import assert from 'node:assert/strict';
import { unreadTurns } from './unreadTurns.ts';

const held = () => {
  let answer;
  const request = new Promise((resolve, reject) => {
    answer = { resolve, reject };
  });
  return { request: () => request, ...answer };
};

test('счётчик показывает ответ сервера', async () => {
  const shown = [];
  const turns = unreadTurns((n) => shown.push(n));
  await turns.ask(async () => 2);
  await turns.ask(async () => 0);
  assert.deepEqual(shown, [2, 0]);
});

test('ответ, запрошенный до открытия переписки, не возвращает точку', async () => {
  const shown = [];
  const turns = unreadTurns((n) => shown.push(n));
  const late = held();
  const asked = turns.ask(late.request);
  turns.put(0);
  late.resolve(1);
  await asked;
  assert.deepEqual(shown, [0]);
  await turns.ask(async () => 1);
  assert.deepEqual(shown, [0, 1]);
});

test('после выхода ответ на прежний запрос не показывается', async () => {
  const shown = [];
  const turns = unreadTurns((n) => shown.push(n));
  const late = held();
  const asked = turns.ask(late.request);
  turns.drop();
  late.resolve(3);
  await asked;
  assert.deepEqual(shown, []);
});

test('сбой запроса не меняет счётчик', async () => {
  const shown = [];
  const turns = unreadTurns((n) => shown.push(n));
  turns.put(2);
  await turns.ask(async () => {
    throw new Error('сеть');
  });
  assert.deepEqual(shown, [2]);
});
