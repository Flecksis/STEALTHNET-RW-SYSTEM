import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { appScheme, forbiddenScheme } from './appScheme.ts';

test('схемой приложения считается только чужая для браузера схема', () => {
  for (const [link, scheme] of [
    ['happ://add/https://sub.example.com/abc', 'happ'],
    ['Hiddify://import/{url}', 'hiddify'],
    ['v2rayng://install-config?url=https%3A%2F%2Fsub.example.com', 'v2rayng'],
    ['sing-box://import-remote-profile?url=x', 'sing-box'],
    ['intent://add/x#Intent;scheme=happ;end', 'intent'],
  ]) {
    assert.equal(appScheme(link), scheme, link);
  }
  for (const link of [
    'https://open.example/add?sub=x',
    'HTTPS://open.example',
    'http://open.example',
    'javascript:alert(1)',
    'JAVASCRIPT:alert(1)',
    'java\nscript:alert(1)',
    ' javascript:alert(1)',
    '\tjavascript:alert(1)',
    'data:text/html,<b>x</b>',
    'vbscript:x',
    'file:///etc/passwd',
    'blob:https://panel.example/1',
    'about:blank',
    'view-source:https://panel.example',
    'mailto:a@example.com',
    '//evil.example',
    '/cabinet',
    'happ:',
    'happ',
    '',
  ]) {
    assert.equal(appScheme(link), null, JSON.stringify(link));
  }
});

test('запрещённые схемы в вебе и на сервере одни и те же', () => {
  const rust = readFileSync(new URL('../../../crates/sn-domain/src/connect.rs', import.meta.url), 'utf8');
  const list = /const FORBIDDEN_SCHEMES: \[&str; (\d+)\] = \[([^\]]*)\]/.exec(rust);
  const server = [...list[2].matchAll(/"([^"]+)"/g)].map((found) => found[1]);
  assert.equal(server.length, Number(list[1]));
  for (const scheme of server) {
    assert.equal(forbiddenScheme(scheme), true, scheme);
    assert.equal(appScheme(`${scheme}://x`), null, scheme);
  }
  const web = readFileSync(new URL('./appScheme.ts', import.meta.url), 'utf8');
  const own = [...(/new Set\(\[([^\]]*)\]\)/.exec(web)[1]).matchAll(/'([^']+)'/g)].map((found) => found[1]);
  assert.deepEqual(own.sort(), server.sort());
  for (const scheme of ['happ', 'hiddify', 'https']) {
    assert.equal(forbiddenScheme(scheme), false, scheme);
  }
});
