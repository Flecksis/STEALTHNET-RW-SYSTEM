import assert from 'node:assert/strict';
import test from 'node:test';
import { mainApp, menuNote, opensApp, placeOf } from './chatButtons.ts';

const APP = 'https://vpn.example.com/app';
const ours = { type: 'web_app', text: 'Приложение', url: APP };
const foreign = { type: 'web_app', text: 'Кабинет', url: 'https://old.example.com/cabinet' };
const cabinet = { type: 'web_app', text: 'Кабинет', url: 'https://vpn.example.com/cabinet' };
const commands = { type: 'commands' };

const view = (over = {}) => ({ username: 'vpn_example_bot', wanted: ours, menu: ours, same: true, applied: true, main_app: false, ...over });

test('кнопка описывается подписью и адресом', () => {
  assert.equal(placeOf(ours), `кнопка «Приложение», ведёт на ${APP}`);
  assert.equal(placeOf({ type: 'web_app', text: ' ', url: '' }), 'кнопка, ведёт на неизвестный адрес');
  assert.equal(placeOf(commands), 'обычное «Меню» с командами');
  assert.equal(placeOf({ type: 'default' }), 'обычное «Меню» с командами');
});

test('та же кнопка показывается без предупреждения и без кнопки', () => {
  assert.deepEqual(menuNote(view()), { warn: false, text: `Сейчас в Telegram: кнопка «Приложение», ведёт на ${APP}.`, action: null });
  assert.equal(menuNote(view({ wanted: commands, menu: { type: 'default' } })).action, null);
});

test('чужую кнопку предлагают поставить заново и называют рабочий адрес', () => {
  const note = menuNote(view({ menu: foreign, same: false }));
  assert.equal(note.warn, true);
  assert.equal(note.action, 'Поставить заново');
  assert.ok(note.text.includes('https://old.example.com/cabinet'));
  assert.ok(note.text.includes('через @BotFather или прежним ботом'));
  assert.ok(note.text.endsWith(APP));
});

test('кнопка на сайт или в кабинет этой же панели не пугает: приложение откроется', () => {
  const note = menuNote(view({ menu: cabinet, same: false }));
  assert.equal(note.warn, false);
  assert.equal(note.action, 'Поставить заново');
  assert.ok(note.text.endsWith('Приложение откроется и так.'));
});

test('приложение открывают только свои страницы', () => {
  for (const path of ['', '/', '/index.html', '/cabinet', '/cabinet/support', '/app', '/app/plans', '/?utm_source=tg']) {
    assert.equal(opensApp(`https://vpn.example.com${path}`, APP), true, path);
  }
  for (const path of ['/privacy', '/cabinets', '/application', '/pay/5']) {
    assert.equal(opensApp(`https://vpn.example.com${path}`, APP), false, path);
  }
  for (const url of ['https://old.example.com/', 'http://vpn.example.com/', 'https://vpn.example.com:8443/', 'мусор', '']) {
    assert.equal(opensApp(url, APP), false, url);
  }
});

test('пока бот не применил настройку, об этом так и сказано', () => {
  const put = menuNote(view({ menu: commands, same: false, applied: false }));
  assert.equal(put.action, 'Поставить сейчас');
  assert.ok(put.text.includes('Бот поставит новую сам'));
  const drop = menuNote(view({ wanted: commands, menu: ours, same: false, applied: false }));
  assert.equal(drop.action, 'Убрать сейчас');
  assert.ok(drop.text.includes('Бот уберёт её сам'));
});

test('лишнюю кнопку при выключенном приложении предлагают убрать', () => {
  const note = menuNote(view({ wanted: { type: 'default' }, menu: foreign, same: false }));
  assert.equal(note.action, 'Убрать');
  assert.ok(!note.text.includes('Вход без пароля'));
});

test('кнопка в списке чатов: состояние и где её менять', () => {
  const off = mainApp(view(), true, APP);
  assert.ok(off.text.startsWith('Выключена. '));
  assert.ok(off.text.includes('/mybots → @vpn_example_bot → Bot Settings → Configure Mini App'));
  assert.ok(off.text.endsWith(`Адрес для включения: ${APP}`));
  const on = mainApp(view({ main_app: true }), true, APP);
  assert.ok(on.text.startsWith('Включена. '));
  assert.equal(on.warn, false);
  assert.ok(on.text.endsWith(`Адрес там должен быть ${APP}`));
});

test('кнопка в списке чатов не показывается, когда ей нечего открывать, и предупреждает, если осталась', () => {
  assert.equal(mainApp(view(), false, APP), null);
  assert.equal(mainApp(view(), true, null), null);
  for (const [miniapp, address] of [
    [false, APP],
    [true, null],
  ]) {
    const left = mainApp(view({ main_app: true, username: null }), miniapp, address);
    assert.equal(left.warn, true);
    assert.ok(left.text.startsWith('Включена, хотя приложение не работает.'));
    assert.ok(left.text.includes('ваш бот'));
  }
});
