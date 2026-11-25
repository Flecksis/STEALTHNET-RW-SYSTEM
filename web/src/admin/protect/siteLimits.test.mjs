import { test } from 'node:test';
import assert from 'node:assert/strict';
import { dayLimited, hiddenShort, hiddenText, PACE_DEFAULTS, PACE_NUMBERS, paceError, pacesOf, siteLine, SITE_NUMBERS } from './siteLimits.ts';

const OFF = { site_address_day: 0, site_network_day: 0, site_minute: 0 };
const QUIET = { created: 0, address: 0, network: 0, minute: 0, asked: 0 };

test('пока пределы выключены и отказов нет, строки о пределах нет', () => {
  assert.equal(siteLine(OFF, { ...QUIET, created: 12 }), null);
  assert.equal(dayLimited(OFF), false);
});

test('включённый предел виден с нулём отказов, выключенный — только если отказы были', () => {
  assert.equal(
    siteLine({ ...OFF, site_address_day: 10 }, { ...QUIET, created: 3 }),
    'Новых аккаунтов на сайте за сутки: 3. Отказов сверх предела: с одного адреса — 0.',
  );
  assert.equal(
    siteLine({ ...OFF, site_minute: 60 }, { created: 60, address: 4, network: 0, minute: 7, asked: 2 }),
    'Новых аккаунтов на сайте за сутки: 60. Отказов сверх предела: с одного адреса — 4, в минуту — 7. Капчу сверх минутного предела попросили 2 раза.',
  );
  assert.ok(siteLine({ ...OFF, site_network_day: 1 }, { ...QUIET, asked: 11 }).endsWith('попросили 11 раз.'));
  assert.equal(dayLimited({ ...OFF, site_network_day: 1 }), true);
  assert.equal(dayLimited({ ...OFF, site_minute: 1 }), false);
});

test('предупреждение говорит, чей адрес видит панель и что вписать', () => {
  const edge = hiddenText({ kind: 'cloudflare', address: '172.68.10.20', at: '' }, 'сегодня, 12:05');
  assert.match(edge, /Cloudflare/);
  assert.match(edge, /172\.68\.10\.20, сегодня, 12:05/);
  assert.match(edge, /deploy\/\.env/);
  const inner = hiddenText({ kind: 'internal', address: '172.18.0.5', at: '' }, 'сегодня, 12:05');
  assert.match(inner, /SN_TRUSTED_PROXIES/);
  assert.doesNotMatch(inner, /Cloudflare/);
  assert.match(hiddenShort({ kind: 'cloudflare', address: '172.68.10.20', at: '' }), /Cloudflare.*«Настроить»/);
  assert.doesNotMatch(hiddenShort({ kind: 'internal', address: '172.18.0.5', at: '' }), /Cloudflare/);
});

test('подписи пределов говорят об отказе, а у адреса есть разумное значение', () => {
  for (const [, label, hint] of SITE_NUMBERS) {
    assert.match(label, /^Новых аккаунтов/);
    assert.match(hint, /отказ|Отказ/);
    assert.match(hint, /0 — без предела/);
  }
  assert.match(SITE_NUMBERS[0][2], /10 и больше/);
});

test('пороги с адреса по умолчанию прежние и принимают числа от 1 до 1000', () => {
  assert.deepEqual(pacesOf({}), { email_ip: 5, telegram_ip: 10, verify_ip: 60 });
  assert.deepEqual(pacesOf({ email_ip: 7 }), { email_ip: 7, telegram_ip: 10, verify_ip: 60 });
  for (const good of ['1', '5', '1000']) {
    assert.equal(paceError('Входов', good), null);
  }
  for (const bad of ['', '0', '1001', '-1', '2.5', 'пять', '10000']) {
    assert.equal(paceError('Входов', bad), 'Входов: целое число от 1 до 1000');
  }
  for (const [key, , hint] of PACE_NUMBERS) {
    assert.match(hint, new RegExp(`Обычно ${PACE_DEFAULTS[key]}\\.`));
  }
});
