import { test } from 'node:test';
import assert from 'node:assert/strict';
import { langOfTag, wishedLang } from './wish.ts';

test('язык до входа: выбор в адресе и сохранённый выбор важнее языка Telegram и браузера', () => {
  assert.equal(wishedLang({ query: 'en', stored: 'ru', hint: 'ru', browser: ['ru'] }), 'en');
  assert.equal(wishedLang({ query: null, stored: 'ru', hint: 'en', browser: ['en'] }), 'ru');
  assert.equal(wishedLang({ query: 'de', stored: 'xx', hint: 'en', browser: ['ru'] }), 'en');
});

test('без выбора берётся язык Telegram, без него — первый понятный язык браузера', () => {
  assert.equal(wishedLang({ hint: 'en', browser: ['ru-RU'] }), 'en');
  assert.equal(wishedLang({ hint: 'uk', browser: ['en-US'] }), 'ru');
  assert.equal(wishedLang({ hint: 'de-AT' }), 'en');
  assert.equal(wishedLang({ hint: '???', browser: ['en'] }), null);
  assert.equal(wishedLang({ browser: [undefined, '', 'be-BY', 'en-US'] }), 'ru');
  assert.equal(wishedLang({ browser: ['en-US', 'ru-RU'] }), 'en');
  assert.equal(wishedLang({}), null);
});

test('русский показывается тем, у кого язык близок к русскому, остальным английский', () => {
  for (const tag of ['ru', 'uk-UA', 'be', 'kk_KZ', ' RU ']) {
    assert.equal(langOfTag(tag), 'ru', tag);
  }
  for (const tag of ['en', 'de', 'tr-TR', 'zh_CN']) {
    assert.equal(langOfTag(tag), 'en', tag);
  }
  for (const tag of ['', null, undefined, '1', 'русский']) {
    assert.equal(langOfTag(tag), null, String(tag));
  }
});
