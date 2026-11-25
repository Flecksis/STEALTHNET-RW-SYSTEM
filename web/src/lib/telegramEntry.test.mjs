import { test } from 'node:test';
import assert from 'node:assert/strict';
import { servedApp, telegramEntry } from './telegramEntry.ts';

const HASH = '#tgWebAppData=user%3D%257B%2522id%2522%253A1%257D%26hash%3Dabc&tgWebAppVersion=8.0&tgWebAppPlatform=ios';

const at = (pathname, over = {}) => ({ pathname, search: '', hash: HASH, ...over });

function page(content) {
  return {
    querySelector: (selectors) => (selectors === 'meta[name="telegram-app"]' && content !== null ? { getAttribute: (name) => (name === 'content' ? content : null) } : null),
  };
}

test('главная и кабинет, открытые из Telegram, ведут в приложение с теми же данными', () => {
  for (const path of ['/', '/index.html', '/cabinet', '/cabinet/', '/cabinet/support/12']) {
    assert.equal(telegramEntry(at(path), '/app'), `/app${HASH}`, path);
  }
});

test('метки и приглашение из адреса сохраняются', () => {
  assert.equal(telegramEntry(at('/', { search: '?ref=ABCD1234&utm_source=tg' }), '/app'), `/app?ref=ABCD1234&utm_source=tg${HASH}`);
});

test('свой якорь в адресе кнопки не мешает', () => {
  const hash = `#pricing?${HASH.slice(1)}`;
  assert.equal(telegramEntry(at('/', { hash }), '/app'), `/app${hash}`);
});

test('обычный браузер остаётся на сайте и в кабинете', () => {
  for (const hash of ['', '#', '#pricing', '#tgWebAppData=', '#tgWebAppVersion=8.0', '#a?b=tgWebAppData']) {
    assert.equal(telegramEntry(at('/', { hash }), '/app'), null, hash);
    assert.equal(telegramEntry(at('/cabinet', { hash }), '/app'), null, hash);
  }
});

test('остальные страницы не переходят', () => {
  for (const path of ['/app', '/app/plans', '/admin', '/admin/settings', '/pay/5', '/verify/abc', '/open-app', '/privacy', '/cabinets', '/cabinet-old']) {
    assert.equal(telegramEntry(at(path), '/app'), null, path);
  }
});

test('с выключенным приложением переходить некуда', () => {
  assert.equal(telegramEntry(at('/'), null), null);
  assert.equal(servedApp(page(null)), null);
  assert.equal(servedApp(null), null);
});

test('адрес приложения берётся из страницы и бывает только внутри сайта', () => {
  assert.equal(servedApp(page(' /app ')), '/app');
  for (const bad of ['', '//evil.example.com', 'https://evil.example.com', 'javascript:alert(1)', '/']) {
    assert.equal(servedApp(page(bad)), null, bad);
  }
});
