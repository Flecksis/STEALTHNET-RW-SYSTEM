import { test } from 'node:test';
import assert from 'node:assert/strict';
import { installedNote, noEventsNote, savedNote, stateNote } from './ext.ts';

test('после сохранения событие обещано, только если панель поставила его в очередь', () => {
  assert.equal(savedNote(true), 'Настройки сохранены, модуль получит событие об изменении');
  assert.equal(savedNote(false), 'Настройки сохранены');
});

test('карточка не обещает события модулю без адреса', () => {
  assert.equal(stateNote({ enabled: true, takes_events: true }), 'Получает события, ключ API работает.');
  assert.equal(stateNote({ enabled: true, takes_events: false }), 'Ключ API работает.');
  assert.equal(stateNote({ enabled: false, takes_events: true }), 'События не уходят, ключ API не принимается.');
  assert.equal(stateNote({ enabled: false, takes_events: false }), 'События не уходят, ключ API не принимается.');
});

test('после установки события обещаны только модулю с адресом', () => {
  assert.match(installedNote({ takes_events: true }), /^Он уже включён: события начнут уходить на его адрес\. Передайте модулю ключ и секрет/);
  assert.match(installedNote({ takes_events: false }), /^Он уже включён\. Передайте модулю ключ и секрет/);
});

test('архив без пути событий не обещает служебных событий', () => {
  assert.equal(noEventsNote({ events_path: '/events' }), 'Нет: только «Проверка связи» и «Настройки изменены».');
  assert.equal(noEventsNote({ events_path: null }), 'Нет: модуль не принимает события.');
});
