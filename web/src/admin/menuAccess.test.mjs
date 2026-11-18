import { test } from 'node:test';
import assert from 'node:assert/strict';
import { menuAllowed } from './menuAccess.ts';

const support = (permission) => ['clients_read', 'clients_write', 'support'].includes(permission);
const marketer = (permission) => ['marketing'].includes(permission);

test('поддержка не видит в меню разделы маркетинга и денег', () => {
  for (const module of ['analytics', 'referral', 'broadcast', 'promo', 'promolinks', 'gifts', 'contests', 'roulette']) {
    assert.equal(menuAllowed(module, support), false, module);
  }
});

test('маркетинг видит свои разделы, но не деньги', () => {
  assert.equal(menuAllowed('broadcast', marketer), true);
  assert.equal(menuAllowed('promo', marketer), true);
  assert.equal(menuAllowed('referral', marketer), false);
  assert.equal(menuAllowed('analytics', marketer), false);
});

test('раздел модуля без особого права виден всем', () => {
  assert.equal(menuAllowed('ext-something', support), true);
});
