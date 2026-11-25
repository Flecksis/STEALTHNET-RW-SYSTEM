import { test } from 'node:test';
import assert from 'node:assert/strict';
import { supportLink, supportTickets, supportWay } from './support.ts';

const LINK = 'https://t.me/help_chat';
const SCREEN = '/app/support';

const site = (over) => ({ support_url: null, bot_username: 'vpn_bot', ...over });

test('обращения открываются своим экраном, а своя ссылка — ссылкой', () => {
  assert.deepEqual(supportWay(site({ support_tickets: true }), SCREEN), { to: SCREEN });
  assert.deepEqual(supportWay(site({ support_tickets: true, support_url: LINK }), SCREEN), { to: SCREEN });
  assert.deepEqual(supportWay(site({ support_tickets: false, support_url: LINK }), SCREEN), { url: LINK });
  assert.equal(supportWay(site({ support_tickets: false, support_url: ' ' }), SCREEN), null);
});

test('без ИИ-чата и без имени бота обращения остаются в своём экране', () => {
  for (const support_ai of [undefined, { cabinet: true, miniapp: false }, { cabinet: false, miniapp: true }]) {
    for (const bot_username of [null, 'vpn_bot']) {
      assert.deepEqual(supportWay(site({ support_tickets: true, support_ai, bot_username }), SCREEN), { to: SCREEN });
      assert.deepEqual(supportWay(site({ support_tickets: false, support_url: LINK, support_ai, bot_username }), SCREEN), { url: LINK });
    }
  }
});

test('старый ответ сервера без режима считается приёмом обращений', () => {
  assert.equal(supportTickets(site({})), true);
  assert.deepEqual(supportWay(site({}), SCREEN), { to: SCREEN });
  assert.equal(supportLink(site({ support_url: `  ${LINK} ` })), LINK);
});

test('ссылка tg:// на чат открывается и в мини-приложении', () => {
  const link = (support_url) => supportLink(site({ support_tickets: false, support_url }));
  assert.equal(link('tg://resolve?domain=help_chat'), 'https://t.me/help_chat');
  assert.equal(link('tg://resolve?domain=help_bot&start=vpn'), 'https://t.me/help_bot?start=vpn');
  assert.equal(link('TG://resolve?start=vpn&domain=help_bot'), 'https://t.me/help_bot?start=vpn');
  for (const url of ['tg://user?id=42', 'tg://resolve?phone=79990000000', 'tg://resolve?domain=../x', 'https://example.com/help?domain=x']) {
    assert.equal(link(url), url);
  }
});
