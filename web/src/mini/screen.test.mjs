import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ANSWER_MS, holdScreen, takesFullscreen, takesSwipes, TRIES } from './screen.ts';
import { versionAtLeast } from './bridge.ts';

function fakeApp({ version = '8.0', platform = 'ios', answer = 'changed', fullscreen = false, without = [] } = {}) {
  const calls = [];
  const handlers = new Map();
  const app = {
    calls,
    handlers,
    platform,
    isFullscreen: fullscreen,
    isVersionAtLeast: (wanted) => versionAtLeast(version, wanted),
    ready: () => calls.push('ready'),
    expand: () => calls.push('expand'),
    disableVerticalSwipes: () => calls.push('swipes'),
    requestFullscreen() {
      calls.push('fullscreen');
      if (answer === 'throw') {
        throw new Error('WebAppMethodUnsupported');
      }
      if (answer === 'changed') {
        app.isFullscreen = true;
        app.emit('fullscreenChanged');
      } else if (answer !== 'silent') {
        app.emit('fullscreenFailed', { error: answer });
      }
    },
    onEvent(name, cb) {
      handlers.set(name, [...(handlers.get(name) ?? []), cb]);
    },
    offEvent(name, cb) {
      handlers.set(
        name,
        (handlers.get(name) ?? []).filter((h) => h !== cb),
      );
    },
    emit(name, payload) {
      for (const cb of handlers.get(name) ?? []) {
        cb(payload);
      }
    },
    heard() {
      return [...handlers.values()].reduce((sum, list) => sum + list.length, 0);
    },
  };
  for (const name of without) {
    delete app[name];
  }
  return app;
}

const root = () => ({ dataset: {} });

test('на телефоне сначала готовность и раскрытие, потом запрет свайпа и полный экран', () => {
  for (const platform of ['ios', 'android', 'android_x']) {
    for (const version of ['8.0', '9.1', '10.0']) {
      const app = fakeApp({ platform, version });
      const page = root();
      const stop = holdScreen(app, page);
      assert.deepEqual(app.calls, ['ready', 'expand', 'swipes', 'fullscreen'], `${platform} ${version}`);
      assert.deepEqual(page.dataset, {});
      stop();
    }
  }
});

test('версии: полный экран с 8.0, запрет свайпа с 7.7, раньше только раскрытие и запасной способ', () => {
  const run = (version) => {
    const app = fakeApp({ version });
    const page = root();
    holdScreen(app, page)();
    return app.calls;
  };
  assert.deepEqual(run('8.0'), ['ready', 'expand', 'swipes', 'fullscreen']);
  assert.deepEqual(run('7.10'), ['ready', 'expand', 'swipes']);
  assert.deepEqual(run('7.7'), ['ready', 'expand', 'swipes']);
  assert.deepEqual(run('7.6'), ['ready', 'expand']);
  assert.deepEqual(run('6.0'), ['ready', 'expand']);

  const old = root();
  const stop = holdScreen(fakeApp({ version: '7.6' }), old);
  assert.equal(old.dataset.tgSwipes, 'css');
  stop();
  assert.deepEqual(old.dataset, {});
});

test('на компьютере и в Telegram Web полный экран не запрашивается, но окно раскрывается', () => {
  for (const platform of ['tdesktop', 'macos', 'weba', 'webk', 'web', 'unigram', 'unknown', undefined]) {
    const app = fakeApp({ platform });
    if (platform === undefined) {
      delete app.platform;
    }
    holdScreen(app, root())();
    assert.deepEqual(app.calls, ['ready', 'expand', 'swipes'], String(platform));
    assert.equal(takesFullscreen(app), false);
    assert.equal(takesSwipes(app), true);
  }
});

test('если полный экран уже включён в BotFather, повторный отказ ничего не ломает', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const app = fakeApp({ answer: 'ALREADY_FULLSCREEN', fullscreen: true });
  const stop = holdScreen(app, root());
  t.mock.timers.tick(ANSWER_MS * TRIES);
  assert.deepEqual(app.calls, ['ready', 'expand', 'swipes', 'fullscreen']);
  app.emit('activated');
  assert.deepEqual(app.calls, ['ready', 'expand', 'swipes', 'fullscreen', 'swipes']);
  stop();
});

test('клиент отказал в полном экране: больше не просим, остальное работает', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  for (const answer of ['UNSUPPORTED', undefined]) {
    const app = fakeApp({ answer });
    const stop = holdScreen(app, root());
    t.mock.timers.tick(ANSWER_MS * TRIES);
    app.emit('activated');
    t.mock.timers.tick(ANSWER_MS * TRIES);
    assert.deepEqual(app.calls, ['ready', 'expand', 'swipes', 'fullscreen', 'swipes'], String(answer));
    stop();
  }
});

test('исключение из скрипта Telegram не мешает работе и не повторяется', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const app = fakeApp({ answer: 'throw' });
  const stop = holdScreen(app, root());
  t.mock.timers.tick(ANSWER_MS * TRIES);
  app.emit('activated');
  assert.deepEqual(app.calls, ['ready', 'expand', 'swipes', 'fullscreen', 'swipes']);
  stop();

  const broken = {
    platform: 'ios',
    isVersionAtLeast: () => {
      throw new Error('no version');
    },
    ready: () => {
      throw new Error('not ready');
    },
    expand: () => {
      throw new Error('no expand');
    },
  };
  assert.doesNotThrow(() => holdScreen(broken, root())());
});

test('клиент промолчал: просим ещё раз, но не бесконечно', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const app = fakeApp({ answer: 'silent' });
  const stop = holdScreen(app, root());
  const asked = () => app.calls.filter((name) => name === 'fullscreen').length;
  assert.equal(asked(), 1);
  t.mock.timers.tick(ANSWER_MS);
  assert.equal(asked(), 2);
  t.mock.timers.tick(ANSWER_MS * 20);
  assert.equal(asked(), TRIES);
  stop();

  const answered = fakeApp({ answer: 'silent' });
  const stopAnswered = holdScreen(answered, root());
  answered.isFullscreen = true;
  answered.emit('fullscreenChanged');
  t.mock.timers.tick(ANSWER_MS * 20);
  assert.equal(answered.calls.filter((name) => name === 'fullscreen').length, 1);
  stopAnswered();
});

test('вернулись в свёрнутое приложение: режим не сбрасывается, а потерянный возвращается', () => {
  const kept = fakeApp();
  const stopKept = holdScreen(kept, root());
  kept.emit('activated');
  assert.deepEqual(kept.calls, ['ready', 'expand', 'swipes', 'fullscreen', 'swipes']);
  stopKept();

  const lost = fakeApp();
  const stopLost = holdScreen(lost, root());
  lost.isFullscreen = false;
  lost.emit('fullscreenChanged');
  assert.deepEqual(lost.calls, ['ready', 'expand', 'swipes', 'fullscreen']);
  lost.emit('activated');
  assert.deepEqual(lost.calls, ['ready', 'expand', 'swipes', 'fullscreen', 'swipes', 'fullscreen']);
  assert.equal(lost.isFullscreen, true);
  stopLost();
});

test('после остановки подписки сняты и таймер не срабатывает', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const app = fakeApp({ answer: 'silent' });
  const stop = holdScreen(app, root());
  assert.equal(app.heard(), 3);
  stop();
  assert.equal(app.heard(), 0);
  t.mock.timers.tick(ANSWER_MS * 20);
  assert.deepEqual(app.calls, ['ready', 'expand', 'swipes', 'fullscreen']);
});

test('клиент без части методов: работает то, что есть', () => {
  const app = fakeApp({ without: ['requestFullscreen', 'disableVerticalSwipes', 'onEvent', 'offEvent', 'expand'] });
  const page = root();
  const stop = holdScreen(app, page);
  assert.deepEqual(app.calls, ['ready']);
  assert.equal(page.dataset.tgSwipes, 'css');
  assert.doesNotThrow(stop);

  const bare = fakeApp({ without: ['isVersionAtLeast'] });
  holdScreen(bare, root())();
  assert.deepEqual(bare.calls, ['ready', 'expand']);
});
