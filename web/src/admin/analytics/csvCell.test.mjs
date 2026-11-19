import { test } from 'node:test';
import assert from 'node:assert/strict';
import { csvCell } from './csvCell.ts';

test('ячейка CSV с формулой выгружается как текст', () => {
  assert.equal(csvCell('=HYPERLINK("https://example.com";"Открыть")'), `"'=HYPERLINK(""https://example.com"";""Открыть"")"`);
  assert.equal(csvCell('+79001234567'), "'+79001234567");
  assert.equal(csvCell('@sum'), "'@sum");
  assert.equal(csvCell('-cmd'), "'-cmd");
  assert.equal(csvCell('\tнайди'), "'\tнайди");
});

test('числа, текст и пустые ячейки выгружаются как раньше', () => {
  assert.equal(csvCell(null), '');
  assert.equal(csvCell(42), '42');
  assert.equal(csvCell(-500), '-500');
  assert.equal(csvCell('-12,5'), '-12,5');
  assert.equal(csvCell('Алексей'), 'Алексей');
  assert.equal(csvCell('a;b'), '"a;b"');
  assert.equal(csvCell('скажи "да"'), '"скажи ""да"""');
});
