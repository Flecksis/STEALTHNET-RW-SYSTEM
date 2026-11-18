import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ageDays, ageText, early, funnel } from './funnel.ts';

const rows = (steps) => steps.map((s) => [s.title, s.value, s.share]);

test('доли шагов считаются от пришедших, а не от прошлого шага', () => {
  assert.deepEqual(rows(funnel({ registrations: 20, trials: 10, connected: 12, paid: 5 }, true)), [
    ['Пришли', 20, null],
    ['Попробовали', 10, 50],
    ['Подключились', 12, 60],
    ['Оплатили', 5, 25],
  ]);
});

test('без пробного периода шаг «попробовали» не показывается', () => {
  assert.deepEqual(
    funnel({ registrations: 4, trials: 0, connected: 3, paid: 1 }, false).map((s) => s.key),
    ['arrived', 'connected', 'paid'],
  );
});

test('без пришедших доли пустые, а не деление на ноль', () => {
  assert.deepEqual(rows(funnel({ registrations: 0, trials: 0, connected: 0, paid: 0 }, true)).map((r) => r[2]), [null, null, null, null]);
});

test('метке меньше 30 дней — рано судить, дни склоняются', () => {
  const now = Date.parse('2026-11-18T12:00:00Z');
  assert.equal(ageDays('2026-11-06T10:00:00Z', now), 12);
  assert.equal(ageDays('2026-11-19T10:00:00Z', now), 0);
  assert.equal(ageDays(null, now), null);
  assert.equal(early(12), true);
  assert.equal(early(30), false);
  assert.equal(early(null), false);
  assert.equal(ageText(1), '1 день');
  assert.equal(ageText(22), '22 дня');
  assert.equal(ageText(11), '11 дней');
  assert.equal(ageText(45), '45 дней');
});
