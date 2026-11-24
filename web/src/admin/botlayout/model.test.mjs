import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  applyMove,
  arrange,
  dropTarget,
  gapTarget,
  placeText,
  project,
  fillTemplate,
  keyboardFor,
  kindIn,
  messageFor,
  opensText,
  pack,
  place,
  sameRows,
  spread,
  webOn,
} from './model.ts';

const slot = (key, shown, extra = {}) => ({
  key,
  label: key,
  text: key,
  text_en: key,
  audience: 'all',
  when: '',
  off: null,
  hidden: false,
  hideable: true,
  style: null,
  emoji: false,
  kind: 'callback',
  web: null,
  web_note: null,
  shown,
  ...extra,
});

test('раскладка ставит кнопки по рядам, новая кнопка встаёт в конец, удалённая пропускается', () => {
  const rows = [['gone', 'b'], ['a', 'c']];
  const present = [['a', 1], ['b', 2], ['c', 3], ['new', 4]];
  assert.deepEqual(arrange(rows, present), [[2], [1, 3], [4]]);
});

test('в ряду при показе не больше трёх кнопок', () => {
  const present = ['a', 'b', 'c', 'd'].map((k, i) => [k, i]);
  assert.deepEqual(arrange([['a', 'b', 'c', 'd']], present), [[0, 1, 2], [3]]);
  assert.deepEqual(pack(present, 2), [[0, 1], [2, 3]]);
  assert.deepEqual(pack(present, 9), [[0, 1, 2], [3]]);
});

test('без своей раскладки меню идёт подряд по колонкам, со своей — по рядам', () => {
  const screen = {
    key: 'menu',
    packed: 2,
    slots: [slot('a', ['new']), slot('b', ['active']), slot('c', ['new', 'active']), slot('d', ['new'])],
  };
  const rows = [['a', 'b'], ['c', 'd']];
  assert.deepEqual(
    keyboardFor(screen, { rows, custom: false }, 'new', {}).map((r) => r.map((s) => s.key)),
    [['a', 'c'], ['d']],
  );
  assert.deepEqual(
    keyboardFor(screen, { rows, custom: true }, 'new', {}).map((r) => r.map((s) => s.key)),
    [['a'], ['c', 'd']],
  );
  assert.deepEqual(
    keyboardFor(screen, { rows, custom: true }, 'new', { c: true }).map((r) => r.map((s) => s.key)),
    [['a'], ['d']],
  );
});

test('перетаскивание ставит кнопку в ряд или между рядами и не переполняет ряд', () => {
  const rows = [['a'], ['b', 'c'], ['d', 'e', 'f']];
  assert.deepEqual(place(rows, 'a', { row: 1, at: 1 }), [['b', 'a', 'c'], ['d', 'e', 'f']]);
  assert.deepEqual(place(rows, 'b', { row: 1, at: 2 }), [['a'], ['c', 'b'], ['d', 'e', 'f']]);
  assert.deepEqual(place(rows, 'a', { row: 2, at: 0 }), rows);
  assert.deepEqual(place(rows, 'e', { gap: 0 }), [['e'], ['a'], ['b', 'c'], ['d', 'f']]);
  assert.deepEqual(place(rows, 'a', { gap: 3 }), [['b', 'c'], ['d', 'e', 'f'], ['a']]);
});

test('раскладка «по две в ряд» и сравнение рядов', () => {
  const rows = [['a'], ['b', 'c'], ['d']];
  assert.deepEqual(spread(rows, 2), [['a', 'b'], ['c', 'd']]);
  assert.deepEqual(spread(rows, 5), [['a', 'b', 'c'], ['d']]);
  assert.equal(sameRows(rows, [['a'], ['b', 'c'], ['d']]), true);
  assert.equal(sameRows(rows, [['a'], ['c', 'b'], ['d']]), false);
});

test('предпросмотр сообщения собирается из текстов бота с примерами', () => {
  assert.equal(fillTemplate('<b>{brand}</b> {x}', { brand: 'A&B' }), '<b>A&amp;B</b> {x}');
  const view = {
    brand: 'STEALTHNET',
    balance: true,
    texts: {
      'menu.title': '<b>{brand}</b>',
      'menu.greeting': '',
      'menu.no_subscription': 'Подписки пока нет.',
      'menu.single': '\n<b>{tariff}</b> · {status}',
      'menu.single_until': 'До {date}, осталось {days}',
      'menu.single_ended': 'Закончилась {date}',
      'menu.single_traffic': 'Трафик: {used} из {limit}',
      'menu.single_devices': 'Устройства: {count} из {limit}',
      'menu.balance': 'Баланс: {balance}',
      'status.active': 'активна',
      'status.expired': 'истекла',
    },
    texts_en: {},
  };
  assert.equal(messageFor(view, 'menu', 'new', 'ru'), '<b>STEALTHNET</b>\nПодписки пока нет.');
  const ended = messageFor(view, 'menu', 'ended', 'ru');
  assert.ok(ended.includes('<b>Стандарт</b> · истекла'), ended);
  assert.ok(ended.includes('Закончилась 2 ноября 2026'), ended);
  assert.ok(ended.endsWith('Баланс: 150 ₽'), ended);
});

test('стрелки для выбранного клиента двигают кнопку относительно видимых соседей', () => {
  const rows = [['a', 'x', 'b'], ['c'], ['y'], ['d', 'e', 'f']];
  const visible = (key) => !['x', 'y'].includes(key);
  const shown = project(rows, visible);
  assert.deepEqual(shown.rows, [['a', 'b'], ['c'], ['d', 'e', 'f']]);
  assert.deepEqual(shown.full, [0, 1, 3]);
  assert.deepEqual(applyMove(rows, shown, 'b', 'left'), [['b', 'a', 'x'], ['c'], ['y'], ['d', 'e', 'f']]);
  assert.deepEqual(applyMove(rows, shown, 'a', 'right'), [['x', 'b', 'a'], ['c'], ['y'], ['d', 'e', 'f']]);
  assert.deepEqual(applyMove(rows, shown, 'c', 'up'), [['c'], ['a', 'x', 'b'], ['y'], ['d', 'e', 'f']]);
  assert.deepEqual(applyMove(rows, shown, 'c', 'down'), [['a', 'x', 'b'], ['y'], ['d', 'e', 'f'], ['c']]);
  assert.deepEqual(applyMove(rows, shown, 'e', 'own'), [['a', 'x', 'b'], ['c'], ['y'], ['d', 'f'], ['e']]);
  assert.deepEqual(applyMove(rows, shown, 'c', 'own'), rows);
  assert.deepEqual(applyMove(rows, shown, 'a', 'up'), [['a'], ['x', 'b'], ['c'], ['y'], ['d', 'e', 'f']]);
  assert.deepEqual(applyMove(rows, shown, 'f', 'down'), [['a', 'x', 'b'], ['c'], ['y'], ['d', 'e'], ['f']]);
});

test('перетаскивание по видимым кнопкам и между рядами', () => {
  const rows = [['a', 'x'], ['b'], ['c', 'd', 'e']];
  assert.deepEqual(dropTarget(rows, 'b', 'a', false), { row: 0, at: 0 });
  assert.deepEqual(dropTarget(rows, 'b', 'x', true), { row: 0, at: 2 });
  assert.equal(dropTarget(rows, 'b', 'c', false), null);
  assert.deepEqual(dropTarget(rows, 'd', 'c', false), { row: 2, at: 0 });
  assert.equal(dropTarget(rows, 'a', 'a', true), null);
  const shown = [['a'], ['b'], ['c', 'd', 'e']];
  assert.deepEqual(gapTarget(rows, shown, 0), { gap: 0 });
  assert.deepEqual(gapTarget(rows, shown, 2), { gap: 2 });
  assert.deepEqual(gapTarget(rows, shown, 3), { gap: 3 });
});

test('место кнопки подписано словами', () => {
  const shown = [['a'], ['b', 'c'], ['d', 'e', 'f']];
  assert.equal(placeText(shown, 'a'), 'Ряд 1, одна в ряду');
  assert.equal(placeText(shown, 'c'), 'Ряд 2, вторая из двух');
  assert.equal(placeText(shown, 'f'), 'Ряд 3, третья из трёх');
  assert.equal(placeText(shown, 'z'), null);
});

test('в веб-режиме кнопки с разделом мини-приложения открывают его, остальные остаются в чате', () => {
  const buy = slot('menu.btn.buy', ['new'], { web: 'Тарифы' });
  const docs = slot('menu.btn.documents', ['new'], { web_note: 'Документы открываются в боте: в мини-приложении их нет' });
  const support = slot('menu.btn.support', ['new'], { kind: 'link' });
  const app = slot('menu.btn.app', ['new'], { kind: 'web_app', web: 'Главная' });
  const ready = { web_off: null };
  const off = { web_off: 'Мини-приложение выключено' };
  assert.equal(webOn(ready, 'web'), true);
  assert.equal(webOn(ready, 'classic'), false);
  assert.equal(webOn(off, 'web'), false);
  assert.equal(kindIn(buy, true), 'web_app');
  assert.equal(kindIn(buy, false), 'callback');
  assert.equal(kindIn(docs, true), 'callback');
  assert.equal(kindIn(support, true), 'link');
  assert.equal(kindIn(app, false), 'web_app');
  assert.equal(opensText(buy, true), 'Мини-приложение, раздел «Тарифы»');
  assert.equal(opensText(buy, false), 'Экран бота в чате');
  assert.equal(opensText(docs, true), 'Экран бота в чате');
  assert.equal(opensText(support, true), 'Ссылку');
  assert.equal(opensText(app, false), 'Мини-приложение, раздел «Главная»');
});
