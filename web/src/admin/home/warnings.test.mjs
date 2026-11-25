import { test } from 'node:test';
import assert from 'node:assert/strict';
import { warningText } from './warnings.ts';

const clock = (extra) => warningText({ kind: 'server_clock', seconds: 7380, span: '2 ч 3 мин', evidence: 'telegram', miniapp: 'closed', ...extra });

test('расхождение часов видно в заголовке вместе с величиной', () => {
  assert.equal(clock().title, 'Часы сервера спешат на 2 ч 3 мин');
  assert.equal(clock({ seconds: -300, span: '5 минут' }).title, 'Часы сервера отстают на 5 минут');
  assert.equal(clock({ seconds: -300, span: '5 минут', evidence: 'login' }).title, 'Часы сервера отстают не меньше чем на 5 минут');
  assert.equal(clock({ evidence: 'logins' }).title, 'Похоже, часы сервера спешат на 2 ч 3 мин');
});

test('сказано, пускает ли мини-приложение, и как исправить время', () => {
  assert.match(clock().text, /мини-приложение никого не пускает/);
  assert.match(clock({ seconds: 420, span: '7 минут', miniapp: 'open' }).text, /пока работает/);
  assert.doesNotMatch(clock({ miniapp: 'off' }).text, /мини-приложение никого не пускает|пока работает/i);
  for (const evidence of ['telegram', 'login', 'logins']) {
    assert.match(clock({ evidence }).text, /timedatectl set-ntp true/);
  }
});

test('прежнее предупреждение о прокси не изменилось, незнакомое не показывается', () => {
  const proxies = warningText({ kind: 'trusted_proxies_everyone', networks: ['0.0.0.0/0', '::/0'] });
  assert.equal(proxies.title, 'Любой посетитель может подменить свой адрес');
  assert.match(proxies.text, /стоит 0\.0\.0\.0\/0 и ::\/0:/);
  assert.equal(warningText({ kind: 'server_clock' }), null);
  assert.equal(warningText({ kind: 'something_new' }), null);
});
