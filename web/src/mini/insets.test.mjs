import { test } from 'node:test';
import assert from 'node:assert/strict';
import { insetVars, watchInsets } from './insets.ts';
import { paintTelegram } from './telegram.ts';

const ZERO = { top: 0, right: 0, bottom: 0, left: 0 };

function fakeApp(safe, content) {
  const handlers = new Map();
  return {
    safeAreaInset: safe,
    contentSafeAreaInset: content,
    onEvent(name, cb) {
      handlers.set(name, [...(handlers.get(name) ?? []), cb]);
    },
    offEvent(name, cb) {
      handlers.set(
        name,
        (handlers.get(name) ?? []).filter((h) => h !== cb),
      );
    },
    emit(name) {
      for (const cb of handlers.get(name) ?? []) {
        cb();
      }
    },
    count(name) {
      return (handlers.get(name) ?? []).length;
    },
  };
}

function fakeRoot() {
  const vars = new Map();
  return {
    vars,
    style: {
      setProperty(name, value) {
        vars.set(name, value);
      },
      removeProperty(name) {
        const old = vars.get(name) ?? '';
        vars.delete(name);
        return old;
      },
    },
  };
}

test('вне Telegram и в клиентах без отступов переменные не задаются, остаётся env()', () => {
  assert.deepEqual(insetVars(null), {});
  assert.deepEqual(insetVars(undefined), {});
  assert.deepEqual(insetVars({ initData: '' }), {});
  assert.deepEqual(insetVars({ safeAreaInset: null, contentSafeAreaInset: null }), {});
});

test('полный экран с вырезом: системный отступ и отступ под кнопки Telegram передаются отдельно', () => {
  const vars = insetVars({ safeAreaInset: { top: 59, right: 0, bottom: 34, left: 0 }, contentSafeAreaInset: { top: 46, right: 0, bottom: 0, left: 0 } });
  assert.deepEqual(vars, {
    '--mn-tg-safe-top': '59px',
    '--mn-tg-safe-right': '0px',
    '--mn-tg-safe-bottom': '34px',
    '--mn-tg-safe-left': '0px',
    '--mn-tg-content-top': '46px',
    '--mn-tg-content-right': '0px',
    '--mn-tg-content-bottom': '0px',
    '--mn-tg-content-left': '0px',
  });
});

test('обычный режим: сверху ноль, снизу место под полоску жестов', () => {
  const vars = insetVars({ safeAreaInset: { top: 0, right: 0, bottom: 34, left: 0 }, contentSafeAreaInset: ZERO });
  assert.equal(vars['--mn-tg-safe-top'], '0px');
  assert.equal(vars['--mn-tg-content-top'], '0px');
  assert.equal(vars['--mn-tg-safe-bottom'], '34px');
});

test('телефон без выреза в полном экране: сверху только строка состояния и кнопки', () => {
  const vars = insetVars({ safeAreaInset: { top: 20, right: 0, bottom: 0, left: 0 }, contentSafeAreaInset: { top: 46, right: 0, bottom: 0, left: 0 } });
  assert.equal(vars['--mn-tg-safe-top'], '20px');
  assert.equal(vars['--mn-tg-content-top'], '46px');
  assert.equal(vars['--mn-tg-safe-bottom'], '0px');
});

test('пропущенные, отрицательные и нечисловые значения считаются нулём, дробные округляются до сотых', () => {
  const vars = insetVars({ safeAreaInset: { top: -5, right: Number.NaN, bottom: '34', left: Number.POSITIVE_INFINITY }, contentSafeAreaInset: { top: 46.666, bottom: null } });
  assert.equal(vars['--mn-tg-safe-top'], '0px');
  assert.equal(vars['--mn-tg-safe-right'], '0px');
  assert.equal(vars['--mn-tg-safe-bottom'], '34px');
  assert.equal(vars['--mn-tg-safe-left'], '0px');
  assert.equal(vars['--mn-tg-content-top'], '46.67px');
  assert.equal(vars['--mn-tg-content-right'], '0px');
  assert.equal(vars['--mn-tg-content-bottom'], '0px');
  assert.equal(vars['--mn-tg-content-left'], '0px');
});

test('если клиент прислал только системный отступ, отступ под кнопки берётся из CSS-переменных Telegram', () => {
  const vars = insetVars({ safeAreaInset: { top: 47, bottom: 34 } });
  assert.equal(vars['--mn-tg-safe-top'], '47px');
  assert.equal('--mn-tg-content-top' in vars, false);
});

test('отступы пересчитываются по событиям Telegram, в том числе при выходе из полного экрана', () => {
  const app = fakeApp({ top: 0, right: 0, bottom: 34, left: 0 }, ZERO);
  const root = fakeRoot();
  const stop = watchInsets(app, root);
  assert.equal(root.vars.get('--mn-tg-safe-top'), '0px');
  assert.equal(root.vars.get('--mn-tg-safe-bottom'), '34px');

  app.safeAreaInset = { top: 47, right: 0, bottom: 34, left: 0 };
  app.emit('safeAreaChanged');
  assert.equal(root.vars.get('--mn-tg-safe-top'), '47px');

  app.contentSafeAreaInset = { top: 46, right: 0, bottom: 0, left: 0 };
  app.emit('contentSafeAreaChanged');
  assert.equal(root.vars.get('--mn-tg-content-top'), '46px');

  app.safeAreaInset = { top: 0, right: 0, bottom: 34, left: 0 };
  app.contentSafeAreaInset = ZERO;
  app.emit('fullscreenChanged');
  assert.equal(root.vars.get('--mn-tg-safe-top'), '0px');
  assert.equal(root.vars.get('--mn-tg-content-top'), '0px');

  app.safeAreaInset = { top: 0, right: 44, bottom: 21, left: 44 };
  app.emit('safeAreaChanged');
  assert.equal(root.vars.get('--mn-tg-safe-left'), '44px');
  assert.equal(root.vars.get('--mn-tg-safe-right'), '44px');
  assert.equal(root.vars.get('--mn-tg-safe-bottom'), '21px');
  stop();
});

test('после остановки подписки снимаются, а переменные убираются', () => {
  const app = fakeApp({ top: 59, right: 0, bottom: 34, left: 0 }, { top: 46, right: 0, bottom: 0, left: 0 });
  const root = fakeRoot();
  const stop = watchInsets(app, root);
  assert.equal(root.vars.size, 8);
  for (const name of ['safeAreaChanged', 'contentSafeAreaChanged', 'fullscreenChanged']) {
    assert.equal(app.count(name), 1);
  }
  stop();
  assert.equal(root.vars.size, 0);
  for (const name of ['safeAreaChanged', 'contentSafeAreaChanged', 'fullscreenChanged']) {
    assert.equal(app.count(name), 0);
  }
});

test('объект с отступами пропал после события: его переменные убираются', () => {
  const app = fakeApp({ top: 59, right: 0, bottom: 34, left: 0 }, { top: 46, right: 0, bottom: 0, left: 0 });
  const root = fakeRoot();
  const stop = watchInsets(app, root);
  app.contentSafeAreaInset = undefined;
  app.emit('contentSafeAreaChanged');
  assert.equal(root.vars.has('--mn-tg-content-top'), false);
  assert.equal(root.vars.get('--mn-tg-safe-top'), '59px');
  stop();
});

test('без подписки на события отступы ставятся один раз и без ошибок', () => {
  const root = fakeRoot();
  const stop = watchInsets({ safeAreaInset: { top: 47, right: 0, bottom: 34, left: 0 } }, root);
  assert.equal(root.vars.get('--mn-tg-safe-top'), '47px');
  stop();
  assert.equal(root.vars.size, 0);
});

test('цвет шапки Telegram совпадает с фоном темы, старым клиентам его не задаём', () => {
  const calls = [];
  const app = (version) => ({
    isVersionAtLeast: (v) => Number.parseFloat(version) >= Number.parseFloat(v),
    setHeaderColor: (c) => calls.push(['header', c]),
    setBackgroundColor: (c) => calls.push(['background', c]),
  });
  paintTelegram(app('8.0'), '#F2F3F6');
  assert.deepEqual(calls, [
    ['header', '#F2F3F6'],
    ['background', '#F2F3F6'],
  ]);
  calls.length = 0;
  paintTelegram(app('6.1'), '#0B0C0F');
  paintTelegram(app('8.0'), 'var(--bg)');
  paintTelegram(app('8.0'), '');
  paintTelegram({ setHeaderColor: (c) => calls.push(['header', c]) }, '#0B0C0F');
  assert.deepEqual(calls, []);
  assert.doesNotThrow(() =>
    paintTelegram(
      {
        isVersionAtLeast: () => true,
        setHeaderColor: () => {
          throw new Error('WebAppHeaderColorInvalid');
        },
      },
      '#0B0C0F',
    ),
  );
});
