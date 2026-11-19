import { test } from 'node:test';
import assert from 'node:assert/strict';
import { RELOAD_PAUSE_MS, shouldReload } from './chunks.ts';

function memory(initial = null, writable = true) {
  let value = initial;
  return {
    read: () => value,
    write: (next) => {
      if (!writable) {
        return false;
      }
      value = next;
      return true;
    },
  };
}

test('после обновления панели вкладка перезагружается один раз, а не по кругу', () => {
  const m = memory();
  assert.equal(shouldReload(1_000_000, m), true);
  assert.equal(shouldReload(1_000_000 + 1_000, m), false);
  assert.equal(shouldReload(1_000_000 + RELOAD_PAUSE_MS + 1, m), true);
});

test('без хранилища вкладки страница не перезагружается', () => {
  assert.equal(shouldReload(1_000_000, memory(null, false)), false);
});
