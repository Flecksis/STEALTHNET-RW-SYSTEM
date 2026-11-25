import { registerHooks } from 'node:module';
import assert from 'node:assert/strict';
import { test } from 'node:test';

registerHooks({
  resolve(specifier, context, next) {
    if (specifier.startsWith('.') && !/\.[a-z]+$/.test(specifier)) {
      return next(`${specifier}.ts`, context);
    }
    return next(specifier, context);
  },
});

const { standardConfig } = await import('../../mini/config.ts');
const { MINI_PLACES, placeHint, placeMax, placeName, placeSample } = await import('./css.ts');
const { MINI_SNIPPETS } = await import('./cssSnippets.ts');
const { canonical, fromView, LIMITS } = await import('./model.ts');

test('у каждого места есть название, подсказка, пример и заготовки', () => {
  assert.equal(new Set(MINI_PLACES.map(placeName)).size, MINI_PLACES.length);
  for (const place of MINI_PLACES) {
    assert.ok(placeHint(place).startsWith('Действует'), place);
    assert.match(placeSample(place), /^[.a-z][\w-]* \{\n {2}[a-z-]+: [^;]+;\n\}$/, place);
    assert.ok(MINI_SNIPPETS[place].length >= 3, place);
    assert.equal(new Set(MINI_SNIPPETS[place].map((item) => item.name)).size, MINI_SNIPPETS[place].length, place);
    assert.equal(placeMax(place, LIMITS), place === 'page' ? 50_000 : 10_000);
  }
  assert.deepEqual(Object.keys(MINI_SNIPPETS).sort(), [...MINI_PLACES].sort());
});

test('CSS не попадает в вид блоков и не меняет их сравнение', () => {
  const view = fromView({ css: { page: '.mn {}' }, styles: { page: '.mn {}\n' } });
  assert.deepEqual(view, standardConfig());
  assert.equal('css' in canonical(view), false);
});
