import assert from 'node:assert/strict';
import { test } from 'node:test';
import { previewReason } from './previewReason.ts';

const PATH = '/admin/mini-preview';
const ORIGIN = 'https://panel.example.com';
const OWN = "default-src 'self'; script-src 'self'; frame-ancestors 'self'; report-uri /api/public/csp-report";
const reason = (patch) => previewReason({ status: 200, redirected: false, frame: 'SAMEORIGIN', policy: OWN, ...patch }, PATH, ORIGIN);
const FINE = 'Браузер не открыл превью, обновите страницу.';
const frame = (value) => `Перед панелью стоит прокси, который добавляет X-Frame-Options: ${value}, уберите его для ${PATH}.`;
const policy = (value) => `Перед панелью стоит прокси, который добавляет Content-Security-Policy: ${value}, уберите его для ${PATH}.`;

test('свои заголовки превью разрешают, тогда дело в браузере', () => {
  assert.equal(reason({}), FINE);
  assert.equal(reason({ frame: 'SAMEORIGIN, sameorigin' }), FINE);
  assert.equal(reason({ frame: null, policy: null }), FINE);
  assert.equal(reason({ frame: 'ALLOW-FROM https://example.com' }), FINE);
});

test('DENY и разные значения X-Frame-Options называются вместе со страницей', () => {
  assert.equal(reason({ frame: 'DENY' }), frame('DENY'));
  assert.equal(reason({ frame: 'SAMEORIGIN, DENY' }), frame('DENY'));
  assert.equal(reason({ frame: 'deny, SAMEORIGIN' }), frame('DENY'));
  assert.equal(reason({ frame: 'SAMEORIGIN, ALLOW-FROM https://a.example' }), frame('SAMEORIGIN, ALLOW-FROM https://a.example'));
});

test('лишний frame-ancestors называется, а разрешающий сайт — нет', () => {
  assert.equal(reason({ policy: `${OWN}, frame-ancestors 'none'` }), policy("frame-ancestors 'none'"));
  assert.equal(reason({ policy: 'frame-ancestors https://t.me' }), policy('frame-ancestors https://t.me'));
  assert.equal(reason({ policy: 'frame-ancestors' }), policy("frame-ancestors 'none'"));
  assert.equal(reason({ policy: `${OWN}, upgrade-insecure-requests` }), FINE);
  for (const source of ['*', 'https:', ORIGIN, 'panel.example.com', '*.example.com', 'https://*.example.com']) {
    assert.equal(reason({ policy: `frame-ancestors ${source}` }), FINE, source);
  }
  assert.equal(reason({ policy: 'frame-ancestors *.other.com' }), policy('frame-ancestors *.other.com'));
});

test('ошибка сервера, отказ прокси и перенаправление различаются', () => {
  assert.equal(reason({ status: 502 }), 'Страница превью ответила ошибкой 502. Проверьте, что панель запущена, и повторите.');
  assert.equal(reason({ status: 403, frame: 'DENY' }), `Страница превью ответила ошибкой 403. Проверьте в прокси перед панелью правила для ${PATH}.`);
  assert.equal(reason({ status: 0, redirected: true }), `Перед панелью стоит прокси, который перенаправляет ${PATH}, уберите перенаправление для этого адреса.`);
});
