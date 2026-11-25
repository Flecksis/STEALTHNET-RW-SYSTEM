import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fixedViewport, lockZoom } from './zoom.ts';

const html = readFileSync(new URL('../../index.html', import.meta.url), 'utf8');
const CONTENT = /<meta name="viewport" content="([^"]+)"/.exec(html)[1];
const FIXED = `${CONTENT}, minimum-scale=1, maximum-scale=1, user-scalable=no`;

function fakePage(content = CONTENT) {
  const handlers = new Map();
  const meta = {
    content,
    getAttribute: () => meta.content,
    setAttribute: (_, value) => {
      meta.content = value;
    },
  };
  return {
    meta,
    documentElement: { dataset: {} },
    querySelector: (selector) => (content !== null && selector === 'meta[name="viewport"]' ? meta : null),
    addEventListener(type, cb, options) {
      handlers.set(type, [...(handlers.get(type) ?? []), { cb, options }]);
    },
    removeEventListener(type, cb) {
      handlers.set(
        type,
        (handlers.get(type) ?? []).filter((h) => h.cb !== cb),
      );
    },
    heard(type) {
      return handlers.get(type) ?? [];
    },
    send(type, extra = {}) {
      let prevented = false;
      const event = {
        cancelable: true,
        preventDefault() {
          prevented = true;
        },
        ...extra,
      };
      for (const { cb } of handlers.get(type) ?? []) {
        cb(event);
      }
      return prevented;
    },
  };
}

test('масштаб в адресе страницы без Telegram не ограничен', () => {
  assert.ok(CONTENT.includes('viewport-fit=cover'));
  assert.ok(!/user-scalable|maximum-scale/.test(html));
});

test('к прежним параметрам страницы добавляется только запрет масштаба', () => {
  assert.equal(fixedViewport(CONTENT), FIXED);
  assert.equal(fixedViewport('width=device-width'), 'width=device-width, minimum-scale=1, maximum-scale=1, user-scalable=no');
  assert.equal(
    fixedViewport('width=device-width, maximum-scale=5, user-scalable=yes'),
    'width=device-width, maximum-scale=1, user-scalable=no, minimum-scale=1',
  );
  assert.equal(fixedViewport(''), 'minimum-scale=1, maximum-scale=1, user-scalable=no');
});

test('в Telegram масштаб закреплён, жест двумя пальцами отменяется, после остановки всё как было', () => {
  const page = fakePage();
  const stop = lockZoom(page, false);
  assert.equal(page.meta.content, FIXED);
  assert.equal(page.documentElement.dataset.tgZoom, 'fixed');
  for (const name of ['gesturestart', 'gesturechange', 'gestureend']) {
    assert.equal(page.send(name), true, name);
    assert.deepEqual(page.heard(name)[0].options, { passive: false });
  }
  assert.equal(page.send('wheel', { ctrlKey: true }), true);
  assert.equal(page.send('wheel', { ctrlKey: false }), false);
  assert.equal(page.send('gesturestart', { cancelable: false }), false);
  stop();
  assert.equal(page.meta.content, CONTENT);
  assert.deepEqual(page.documentElement.dataset, {});
  for (const name of ['gesturestart', 'gesturechange', 'gestureend', 'wheel']) {
    assert.equal(page.heard(name).length, 0, name);
  }
});

test('прокрутка пальцем и колесом не перехватывается', () => {
  const page = fakePage();
  lockZoom(page, false);
  for (const name of ['touchstart', 'touchmove', 'touchend', 'pointerdown', 'scroll', 'click', 'dblclick']) {
    assert.equal(page.heard(name).length, 0, name);
  }
});

test('в Telegram Web масштаб вкладки браузера остаётся за клиентом', () => {
  const page = fakePage();
  const stop = lockZoom(page, true);
  assert.equal(page.heard('wheel').length, 0);
  assert.equal(page.send('wheel', { ctrlKey: true }), false);
  assert.equal(page.documentElement.dataset.tgZoom, 'fixed');
  stop();
  assert.equal(page.meta.content, CONTENT);
});

test('страница без метатега масштаба не падает', () => {
  const page = fakePage(null);
  assert.doesNotThrow(() => lockZoom(page, false)());
});

test('поля мини-приложения не мельче 16 px, масштаб и свайпы в стилях включаются только внутри Telegram', () => {
  const css = readFileSync(new URL('./mini.css', import.meta.url), 'utf8') + readFileSync(new URL('./connect.css', import.meta.url), 'utf8');
  for (const tag of ['input', 'select', 'textarea']) {
    assert.ok(css.includes(`[data-surface='mini'] ${tag}`), tag);
  }
  for (const [, selector, body] of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (/\b(input|select|textarea)\b/.test(selector)) {
      const size = /font-size:\s*([^;]+);/.exec(body)?.[1];
      assert.ok(size === undefined || size === 'max(16px, 1em)' || Number.parseFloat(size) >= 16, `${selector.trim()}: ${size}`);
    }
    if (/touch-action|text-size-adjust/.test(body)) {
      assert.match(selector, /html\[data-tg-zoom='fixed'\]/);
    }
    if (/overscroll-behavior-y/.test(body)) {
      assert.match(selector, /html\[data-tg-swipes='css'\]/);
    }
  }
});
