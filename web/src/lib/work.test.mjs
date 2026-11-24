import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { sha256, solveWork, workDone, zeroBits } from './work.ts';

function hex(words) {
  return Array.from(words, (w) => w.toString(16).padStart(8, '0')).join('');
}

test('sha256 совпадает с node:crypto на коротких и длинных строках', () => {
  for (const text of ['', 'abc', 'a'.repeat(55), 'b'.repeat(56), 'c'.repeat(64), 'д'.repeat(100)]) {
    const mine = hex(sha256(new TextEncoder().encode(text)));
    assert.equal(mine, createHash('sha256').update(text).digest('hex'), text.slice(0, 8));
  }
});

test('найденное доказательство проходит проверку, как на сервере', async () => {
  const seed = '0f1e2d3c4b5a69788796a5b4c3d2e1f0';
  const proof = await solveWork(seed, 12);
  assert.match(proof, /^\d+$/);
  const digest = createHash('sha256').update(`${seed}:${proof}`).digest();
  assert.equal(digest[0], 0);
  assert.equal(digest[1] >> 4, 0);
  assert.equal(workDone(seed, proof, 12), true);
  assert.equal(workDone(seed, `${proof}x`, 12), false);
});

test('ведущие нулевые биты считаются по первому слову', () => {
  assert.equal(zeroBits(0x0000ffff, 16), true);
  assert.equal(zeroBits(0x0001ffff, 16), false);
  assert.equal(zeroBits(0, 32), true);
  assert.equal(zeroBits(1, 32), false);
  assert.equal(zeroBits(0xffffffff, 0), true);
});

test('проверку можно прервать', async () => {
  const stop = new AbortController();
  stop.abort();
  await assert.rejects(solveWork('ffffffffffffffffffffffffffffffff', 31, stop.signal));
});
