import { test } from 'node:test';
import assert from 'node:assert/strict';
import { leavesPath, pathPart } from './apiPath.ts';

const BASE = 'https://panel.test';

test('путь запроса с переходом на уровень выше отклоняется', () => {
  for (const path of [
    '/api/admin/ext-modules/../subscriptions/7/actions/extend?/page-token',
    '/api/admin/ext-modules/..\\subscriptions\\7\\actions\\extend',
    '/api/admin/ext-modules/%2e%2e/clients/5/unblock',
    '/api/admin/ext-modules/.%2E/clients/5/unblock',
    '/api/admin/ext-modules/%2E./clients/5/unblock',
    '/api/admin/modules/./config',
    '/api/client/payments/..',
  ]) {
    assert.equal(leavesPath(path), true, path);
  }
  for (const path of [
    '/api/admin/ext-modules/demo/page-token',
    '/api/admin/clients?q=../x',
    '/api/admin/backups/files/backup-2026.11.19.snb',
    '/api/public/logo/a1b2.png',
    '/api/admin/ext-modules/..%2Fsubscriptions%2F7/page-token',
  ]) {
    assert.equal(leavesPath(path), false, path);
  }
});

test('номер из адреса страницы остаётся одной частью пути', () => {
  const hostile = '../subscriptions/7/actions/extend?';
  const path = `/api/admin/ext-modules/${pathPart(hostile)}/page-token`;
  const url = new URL(path, BASE);
  assert.equal(url.pathname.startsWith('/api/admin/ext-modules/'), true, url.pathname);
  assert.equal(url.pathname.endsWith('/page-token'), true, url.pathname);
  assert.equal(url.search, '');
  assert.equal(leavesPath(path), false);
  assert.equal(new URL(`/api/admin/ext-modules/${hostile}/page-token`, BASE).pathname, '/api/admin/subscriptions/7/actions/extend');
  assert.equal(pathPart('demo-module'), 'demo-module');
  assert.equal(pathPart(42), '42');
  assert.equal(leavesPath(`/api/admin/ext-modules/${pathPart('..')}/page-token`), true);
});
