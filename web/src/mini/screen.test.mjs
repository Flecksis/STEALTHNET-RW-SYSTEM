import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ANSWER_MS, fullscreenOn, holdFullscreen, holdScreen, takesFullscreen, takesSwipes, TRIES } from './screen.ts';
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

function run(app, page, on) {
  const stopScreen = holdScreen(app, page);
  const stopFull = on ? holdFullscreen(app) : () => undefined;
  return () => {
    stopFull();
    stopScreen();
  };
}

const OPEN = ['ready', 'expand', 'swipes'];
const FULL = [...OPEN, 'fullscreen'];

test('полный экран включён, только если так решил оператор', () => {
  assert.equal(fullscreenOn({ client_access: { miniapp: true, miniapp_fullscreen: true, site_login: true, devices: true, reissue: true } }), true);
  for (const site of [
    null,
    undefined,
    {},
    { client_access: null },
    { client_access: {} },
    { client_access: { miniapp: true, site_login: true, devices: true, reissue: true } },
    { client_access: { miniapp_fullscreen: false } },
    { client_access: { miniapp_fullscreen: 'true' } },
    { client_access: { miniapp_fullscreen: 1 } },
  ]) {
    assert.equal(fullscreenOn(site), false, JSON.stringify(site));
  }
});

test('выключено: полный экран не просится и не закрывается, остальное работает', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  for (const platform of ['ios', 'android', 'android_x']) {
    const app = fakeApp({ platform, fullscreen: platform === 'ios' });
    app.exitFullscreen = () => app.calls.push('exit');
    const stop = run(app, root(), false);
    t.mock.timers.tick(ANSWER_MS * 20);
    app.emit('activated');
    app.emit('fullscreenChanged');
    t.mock.timers.tick(ANSWER_MS * 20);
    assert.deepEqual(app.calls, [...OPEN, 'swipes'], platform);
    assert.equal(app.isFullscreen, platform === 'ios');
    stop();
    assert.deepEqual(app.calls, [...OPEN, 'swipes'], platform);
  }
});

test('включено: сначала готовность, раскрытие и запрет свайпа, потом полный экран', () => {
  for (const platform of ['ios', 'android', 'android_x']) {
    for (const version of ['8.0', '9.1', '10.0']) {
      const app = fakeApp({ platform, version });
      const page = root();
      const stop = run(app, page, true);
      assert.deepEqual(app.calls, FULL, `${platform} ${version}`);
      assert.deepEqual(page.dataset, {});
      stop();
    }
  }
});

test('настройка пришла позже: остальное уже работает, полный экран просится после неё', () => {
  const app = fakeApp();
  const stopScreen = holdScreen(app, root());
  assert.deepEqual(app.calls, OPEN);
  const stopFull = holdFullscreen(app);
  assert.deepEqual(app.calls, FULL);
  stopFull();
  app.emit('activated');
  assert.deepEqual(app.calls, [...FULL, 'swipes']);
  stopScreen();
});

test('версии: полный экран с 8.0, запрет свайпа с 7.7, раньше только раскрытие и запасной способ', () => {
  const calls = (version) => {
    const app = fakeApp({ version });
    run(app, root(), true)();
    return app.calls;
  };
  assert.deepEqual(calls('8.0'), FULL);
  assert.deepEqual(calls('7.10'), OPEN);
  assert.deepEqual(calls('7.7'), OPEN);
  assert.deepEqual(calls('7.6'), ['ready', 'expand']);
  assert.deepEqual(calls('6.0'), ['ready', 'expand']);

  const old = root();
  const stop = run(fakeApp({ version: '7.6' }), old, true);
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
    run(app, root(), true)();
    assert.deepEqual(app.calls, OPEN, String(platform));
    assert.equal(takesFullscreen(app), false);
    assert.equal(takesSwipes(app), true);
  }
});

test('если полный экран уже включён в BotFather, повторный отказ ничего не ломает', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const app = fakeApp({ answer: 'ALREADY_FULLSCREEN', fullscreen: true });
  const stop = run(app, root(), true);
  t.mock.timers.tick(ANSWER_MS * TRIES);
  assert.deepEqual(app.calls, FULL);
  app.emit('activated');
  assert.deepEqual(app.calls, [...FULL, 'swipes']);
  stop();
});

test('клиент отказал в полном экране: больше не просим, остальное работает', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  for (const answer of ['UNSUPPORTED', undefined]) {
    const app = fakeApp({ answer });
    const stop = run(app, root(), true);
    t.mock.timers.tick(ANSWER_MS * TRIES);
    app.emit('activated');
    t.mock.timers.tick(ANSWER_MS * TRIES);
    assert.deepEqual(app.calls, [...FULL, 'swipes'], String(answer));
    stop();
  }
});

test('исключение из скрипта Telegram не мешает работе и не повторяется', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const app = fakeApp({ answer: 'throw' });
  const stop = run(app, root(), true);
  t.mock.timers.tick(ANSWER_MS * TRIES);
  app.emit('activated');
  assert.deepEqual(app.calls, [...FULL, 'swipes']);
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
    requestFullscreen: () => {
      throw new Error('no fullscreen');
    },
  };
  assert.doesNotThrow(() => run(broken, root(), true)());
});

test('клиент промолчал: просим ещё раз, но не бесконечно', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const asked = (app) => app.calls.filter((name) => name === 'fullscreen').length;
  const app = fakeApp({ answer: 'silent' });
  const stop = run(app, root(), true);
  assert.equal(asked(app), 1);
  t.mock.timers.tick(ANSWER_MS);
  assert.equal(asked(app), 2);
  t.mock.timers.tick(ANSWER_MS * 20);
  assert.equal(asked(app), TRIES);
  stop();

  const answered = fakeApp({ answer: 'silent' });
  const stopAnswered = run(answered, root(), true);
  answered.isFullscreen = true;
  answered.emit('fullscreenChanged');
  t.mock.timers.tick(ANSWER_MS * 20);
  assert.equal(asked(answered), 1);
  stopAnswered();
});

test('вернулись в свёрнутое приложение: режим не сбрасывается, а потерянный возвращается', () => {
  const kept = fakeApp();
  const stopKept = run(kept, root(), true);
  kept.emit('activated');
  assert.deepEqual(kept.calls, [...FULL, 'swipes']);
  stopKept();

  const lost = fakeApp();
  const stopLost = run(lost, root(), true);
  lost.isFullscreen = false;
  lost.emit('fullscreenChanged');
  assert.deepEqual(lost.calls, FULL);
  lost.emit('activated');
  assert.deepEqual(lost.calls, [...FULL, 'swipes', 'fullscreen']);
  assert.equal(lost.isFullscreen, true);
  stopLost();
});

test('после остановки подписки сняты и таймер не срабатывает', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const app = fakeApp({ answer: 'silent' });
  const stop = run(app, root(), true);
  assert.equal(app.heard(), 4);
  stop();
  assert.equal(app.heard(), 0);
  t.mock.timers.tick(ANSWER_MS * 20);
  assert.deepEqual(app.calls, FULL);
});

test('клиент без части методов: работает то, что есть', () => {
  const app = fakeApp({ without: ['requestFullscreen', 'disableVerticalSwipes', 'onEvent', 'offEvent', 'expand'] });
  const page = root();
  const stop = run(app, page, true);
  assert.deepEqual(app.calls, ['ready']);
  assert.equal(page.dataset.tgSwipes, 'css');
  assert.doesNotThrow(stop);

  const bare = fakeApp({ without: ['isVersionAtLeast'] });
  run(bare, root(), true)();
  assert.deepEqual(bare.calls, ['ready', 'expand']);
});
