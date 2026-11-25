import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative, sep } from 'node:path';
import { servedBrand, tabTitle } from './tabTitle.ts';

const src = new URL('..', import.meta.url).pathname;

const spare = { brand_name: 'Тихий VPN' };
const loaded = { brand_name: 'Тихий VPN', title: 'Тихий VPN, доступ без блокировок' };
const landing = (site) => site.title ?? site.brand_name;

function page(content) {
  return {
    querySelector: (selectors) => (selectors === 'meta[name="application-name"]' && content !== null ? { getAttribute: (name) => (name === 'content' ? content : null) } : null),
  };
}

function sources(dir = src) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      return sources(path);
    }
    return /\.tsx?$/.test(entry.name) ? [relative(src, path).split(sep).join('/')] : [];
  });
}

const visitor = sources().filter((path) => !path.startsWith('admin/'));
const read = (path) => readFileSync(join(src, path), 'utf8');

test('пока ответа о сайте нет, заголовок вкладки не трогается', () => {
  assert.equal(tabTitle(null, spare, landing), null);
});

test('после сбоя загрузки заголовок вкладки не трогается', () => {
  assert.equal(tabTitle(spare, spare, landing), null);
});

test('настоящие данные сайта меняют заголовок', () => {
  assert.equal(tabTitle(loaded, spare, landing), 'Тихий VPN, доступ без блокировок');
  assert.equal(tabTitle({ ...spare }, spare, landing), 'Тихий VPN');
  assert.equal(tabTitle({ brand_name: '' }, spare, landing), null);
});

test('название до ответа сервера берётся из самой страницы', () => {
  assert.equal(servedBrand(page(' Тихий VPN ')), 'Тихий VPN');
  assert.equal(servedBrand(page(null)), '');
  assert.equal(servedBrand(null), '');
});

test('на страницах для клиентов нет чужого названия', () => {
  assert.ok(visitor.length > 40);
  assert.deepEqual(
    visitor.filter((path) => /stealthnet/i.test(read(path))),
    [],
  );
  assert.match(readFileSync(join(src, '..', 'index.html'), 'utf8'), /<title><\/title>/);
});

test('заголовок вкладки меняется только через общую функцию', () => {
  assert.deepEqual(
    visitor.filter((path) => /document\.title\s*=[^=]/.test(read(path))),
    ['lib/site.ts'],
  );
});
