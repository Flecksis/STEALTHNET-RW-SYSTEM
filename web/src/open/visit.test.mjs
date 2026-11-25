import { test } from 'node:test';
import assert from 'node:assert/strict';
import { STAY_MS, takeVisit, watchStay } from './visit.ts';

const TOKEN = 'ab'.repeat(32);
const OTHER = 'cd'.repeat(32);

function fakePage({ hash = '', saved = null, storage = true } = {}) {
  const kept = new Map(saved ? [['sn.open', typeof saved === 'string' ? saved : JSON.stringify(saved)]] : []);
  const addresses = [];
  const page = {
    location: { hash, pathname: '/open-app' },
    history: {
      state: { idx: 0 },
      replaceState: (state, title, url) => {
        addresses.push([state, title, url]);
        page.location.hash = '';
      },
    },
  };
  Object.defineProperty(page, 'sessionStorage', {
    get() {
      if (!storage) {
        throw new Error('SecurityError');
      }
      return { getItem: (key) => kept.get(key) ?? null, setItem: (key, value) => kept.set(key, value) };
    },
  });
  return { page, kept, addresses };
}

test('ссылка берётся из адреса после решётки и сразу из него убирается', () => {
  const { page, kept, addresses } = fakePage({ hash: `#${TOKEN}` });
  const visit = takeVisit(page);
  assert.equal(visit.token, TOKEN);
  assert.match(visit.holder, /^[0-9a-f]{32}$/);
  assert.deepEqual(addresses, [[{ idx: 0 }, '', '/open-app']]);
  assert.equal(page.location.hash, '');
  assert.deepEqual(JSON.parse(kept.get('sn.open')), visit);
});

test('после перезагрузки то же окно открывает ссылку снова, другое окно получает свой ключ', () => {
  const first = fakePage({ hash: `#${TOKEN}` });
  const visit = takeVisit(first.page);
  assert.deepEqual(takeVisit(first.page), visit);
  const again = fakePage({ hash: `#${TOKEN}`, saved: visit });
  assert.deepEqual(takeVisit(again.page), visit);
  const elsewhere = takeVisit(fakePage({ hash: `#${TOKEN}` }).page);
  assert.equal(elsewhere.token, TOKEN);
  assert.notEqual(elsewhere.holder, visit.holder);
});

test('новая ссылка в том же окне заменяет прежнюю', () => {
  const { page, kept } = fakePage({ hash: `#${TOKEN}` });
  const visit = takeVisit(page);
  page.location.hash = `#${OTHER}`;
  const next = takeVisit(page);
  assert.equal(next.token, OTHER);
  assert.notEqual(next.holder, visit.holder);
  assert.equal(JSON.parse(kept.get('sn.open')).token, OTHER);
});

test('без ссылки и с мусором в адресе открывать нечего', () => {
  assert.equal(takeVisit(fakePage().page), null);
  for (const hash of ['#', '#abc', `#${TOKEN}0`, `#${TOKEN.toUpperCase()}`, '#happ://add/https://evil.example', '#https://evil.example', `#x${TOKEN}`]) {
    const { page, addresses } = fakePage({ hash });
    assert.equal(takeVisit(page), null, hash);
    assert.deepEqual(addresses, [], hash);
  }
  for (const saved of ['мусор', '[1]', 'null', '5', JSON.stringify({ token: TOKEN }), JSON.stringify({ token: 'x', holder: 'y' }), JSON.stringify({ token: 7, holder: 8 })]) {
    assert.equal(takeVisit(fakePage({ saved }).page), null, saved);
  }
});

test('без доступа к хранилищу первый переход работает, а ссылка всё равно уходит из адреса', () => {
  const { page, addresses } = fakePage({ hash: `#${TOKEN}`, storage: false });
  const visit = takeVisit(page);
  assert.equal(visit.token, TOKEN);
  assert.equal(addresses.length, 1);
  assert.equal(takeVisit(page), null);
});

function fakeScreen() {
  const timers = new Map();
  const listeners = new Set();
  let id = 0;
  const page = {
    document: {
      visibilityState: 'visible',
      addEventListener: (name, cb) => listeners.add(cb),
      removeEventListener: (name, cb) => listeners.delete(cb),
    },
    setTimeout: (run, ms) => {
      id += 1;
      timers.set(id, { run, ms });
      return id;
    },
    clearTimeout: (timer) => timers.delete(timer),
  };
  return {
    page,
    timers,
    listeners,
    show(state) {
      page.document.visibilityState = state;
      for (const cb of [...listeners]) {
        cb();
      }
    },
    wait() {
      for (const [key, { run }] of [...timers]) {
        timers.delete(key);
        run();
      }
    },
  };
}

test('страница осталась перед глазами: через две секунды показывается, что делать', () => {
  const screen = fakeScreen();
  let stayed = 0;
  watchStay(screen.page, () => {
    stayed += 1;
  });
  assert.deepEqual([...screen.timers.values()].map(({ ms }) => ms), [STAY_MS]);
  assert.equal(STAY_MS, 2000);
  assert.equal(stayed, 0);
  screen.wait();
  assert.equal(stayed, 1);
});

test('приложение открылось: подсказка появляется только после возвращения на страницу', () => {
  const screen = fakeScreen();
  let stayed = 0;
  const stop = watchStay(screen.page, () => {
    stayed += 1;
  });
  screen.show('hidden');
  screen.wait();
  assert.equal(stayed, 0);
  screen.show('visible');
  assert.equal(stayed, 1);
  stop();
  assert.equal(screen.listeners.size, 0);
});

test('слежение останавливается вместе со страницей', () => {
  const screen = fakeScreen();
  let stayed = 0;
  const stop = watchStay(screen.page, () => {
    stayed += 1;
  });
  stop();
  assert.equal(screen.timers.size, 0);
  screen.show('hidden');
  screen.show('visible');
  screen.wait();
  assert.equal(stayed, 0);
});
