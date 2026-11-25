import { test } from 'node:test';
import assert from 'node:assert/strict';
import { longParts } from './longWords.ts';

const join = (parts) => parts.map((part) => part.text).join('');

test('длинная ссылка в ячейке отделяется от обычных слов', () => {
  const text = 'Подробно: https://example.com/pay/0123456789abcdef и в поддержке';
  const parts = longParts(text);
  assert.equal(join(parts), text);
  assert.deepEqual(
    parts.filter((part) => part.long).map((part) => part.text),
    ['https://example.com/pay/0123456789abcdef'],
  );
});

test('обычные слова в таблице не рвутся', () => {
  const text = 'Требование законодательства, конфиденциальность, пользователя';
  assert.deepEqual(longParts(text), [{ text, long: false }]);
  assert.deepEqual(longParts(''), []);
});

test('несколько длинных строк подряд сохраняют текст и пробелы', () => {
  const long = 'a'.repeat(40);
  const text = `${long}  ${long}\n`;
  const parts = longParts(text);
  assert.equal(join(parts), text);
  assert.equal(parts.filter((part) => part.long).length, 2);
});
