import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  CUT_NOTE,
  HIDDEN,
  ISSUE_FORM,
  ISSUES_REPO,
  MAX_URL,
  NO_TITLE,
  SECTIONS,
  browserName,
  clean,
  composeBody,
  fitIssueUrl,
  formFields,
  issueUrl,
  pagePath,
  systemName,
  titleFrom,
} from './text.ts';

const plain = { repo: ISSUES_REPO, form: null };
const form = { repo: ISSUES_REPO, form: 'bug.yml' };
const env = {
  version: '0.1.164 от 18.11.2026, a2a5cf8',
  remnawave: '3.4.11',
  page: '/admin/tariffs',
  browser: 'Chrome 131, macOS; экран 1440×900; язык ru-RU',
};
const report = (fields) => ({ what: '', steps: '', expected: '', errors: null, ...env, ...fields });
const ENV_TAIL =
  '### Версия панели\n\n0.1.164 от 18.11.2026, a2a5cf8\n\n### Версия Remnawave\n\n3.4.11\n\n### Страница\n\n/admin/tariffs\n\n### Браузер и система\n\nChrome 131, macOS; экран 1440×900; язык ru-RU';

test('почты заменяются на [скрыто]', () => {
  assert.equal(clean('пишет ivan.petrov+vpn@mail.ru и admin@почта.рф'), `пишет ${HIDDEN} и ${HIDDEN}`);
});

test('токены, ключи и пароли скрываются', () => {
  const cases = [
    ['бот 123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw', `бот ${HIDDEN}`],
    ['jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abcDEF123', `jwt ${HIDDEN}`],
    ['uuid 5b1e8a1c-1111-2222-3333-444455556666', `uuid ${HIDDEN}`],
    ['секрет a3f9e8b7c6d5e4f3a2b1', `секрет ${HIDDEN}`],
    ['ключ live_Xk9aB2cD3eF4gH5iJ6kL7mN8', `ключ ${HIDDEN}`],
    ['api_key=secret123', `api_key=${HIDDEN}`],
    ['{"token": "zzz"}', `{"token": "${HIDDEN}"}`],
    ['Пароль: qwerty', `Пароль: ${HIDDEN}`],
    ['Bearer abcdefgh12345678', `Bearer ${HIDDEN}`],
  ];
  for (const [input, expected] of cases) {
    assert.equal(clean(input), expected, input);
  }
});

test('ссылки теряют адрес сервера, параметры и коды в пути', () => {
  assert.equal(
    clean('подписка https://sub.example.com/api/sub/Ab3dEf7hJk?x=1#frag не открывается'),
    `подписка https://${HIDDEN}/api/sub/${HIDDEN}?${HIDDEN}#${HIDDEN} не открывается`,
  );
  assert.equal(clean('vless://5b1e8a1c-1111-2222-3333-444455556666@1.2.3.4:443?security=reality#name'), `vless://${HIDDEN}`);
  assert.equal(clean('panel.example.com/admin/clients/42'), `${HIDDEN}/admin/clients/42`);
  assert.equal(clean('страница /admin/clients?q=ivan'), `страница /admin/clients?${HIDDEN}`);
});

test('адреса серверов, телефоны, номера и ники скрываются', () => {
  assert.equal(clean('нода 185.112.59.123:8443'), `нода ${HIDDEN}`);
  assert.equal(clean('нода 2001:db8::8a2e:370:7334'), `нода ${HIDDEN}`);
  assert.equal(clean('звонить +7 (999) 123-45-67 или 8 999 123 45 67'), `звонить ${HIDDEN} или ${HIDDEN}`);
  assert.equal(clean('telegram id 1234567890, ник @ivan_petrov'), `telegram id ${HIDDEN}, ник @${HIDDEN}`);
});

test('версии, время и обычный текст остаются как есть', () => {
  const text =
    'Версия v0.1.160 (a2a5cf8), Remnawave 2.3.1, время 15:42:10, 18.11.2026, файл 2026-11-18-bug-reports.md, код admin_2fa_required_for_settings, serde::de::Error, Цена 1 490 ₽. Почему?';
  assert.equal(clean(text), text);
});

test('повторная очистка ничего не меняет', () => {
  const once = clean('a@b.ru https://x.example.com/sub/Ab3dEf7hJk?y=2 185.112.59.123 token=abc @ivan_petrov 5b1e8a1c-1111-2222-3333-444455556666');
  assert.equal(clean(once), once);
});

test('длинная строка без пробелов чистится быстро', () => {
  const started = performance.now();
  clean(`${'a1'.repeat(50000)} ${'B'.repeat(50000)}token=${'x'.repeat(50000)}`);
  assert.ok(performance.now() - started < 1000);
});

test('путь страницы без параметров, номеров и кодов', () => {
  assert.equal(pagePath('/admin/clients/42?tab=subs#top'), '/admin/clients/:id');
  assert.equal(pagePath('/admin/m/broadcast/5b1e8a1c-1111-2222-3333-444455556666'), '/admin/m/broadcast/:id');
  assert.equal(pagePath('/admin/clients/ivan@mail.ru'), '/admin/clients/:id');
  assert.equal(pagePath('/admin/settings/bot/texts'), '/admin/settings/bot/texts');
  assert.equal(pagePath('https://panel.example.com/admin'), null);
  assert.equal(pagePath(undefined), null);
});

test('браузер и система определяются по строке браузера', () => {
  const cases = [
    ['Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36', 'Chrome 131', 'macOS'],
    ['Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1', 'Safari 17.4', 'iOS 17.4'],
    ['Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:132.0) Gecko/20100101 Firefox/132.0', 'Firefox 132', 'Windows'],
    ['Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36', 'Chrome 131', 'Android 14'],
    ['Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 YaBrowser/24.12.0.0 Safari/537.36', 'Яндекс Браузер 24', 'Windows'],
    ['Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0', 'Edge 131', 'Windows'],
  ];
  for (const [ua, browser, system] of cases) {
    assert.equal(browserName(ua), browser);
    assert.equal(systemName(ua), system);
  }
});

test('заголовок берётся из первой строки, чистится и режется по слову', () => {
  assert.equal(titleFrom('\n  Не сохраняется тариф\nподробности'), 'Не сохраняется тариф');
  assert.equal(titleFrom('   '), NO_TITLE);
  assert.equal(titleFrom('Клиент ivan@mail.ru не может оплатить'), `Клиент ${HIDDEN} не может оплатить`);
  const long = titleFrom('При сохранении тарифа появляется ошибка 500, а после обновления страницы цена остаётся старой и ничего не меняется');
  assert.ok(long.endsWith('…'));
  assert.ok(Array.from(long).length <= 81);
  assert.ok(!long.includes('  '));
});


test('разделы совпадают с полями формы в репозитории ошибок', () => {
  assert.equal(ISSUES_REPO, 'STEALTHNET-APP/stealthnet-issues');
  assert.equal(ISSUE_FORM, 'bug.yml');
  assert.deepEqual(Object.keys(SECTIONS), ['what', 'steps', 'expected', 'version', 'remnawave', 'page', 'browser']);
});

test('текст собирается по разделам, пустые разделы пропускаются', () => {
  const body = composeBody(report({ what: 'Не сохраняется тариф', expected: 'Сохранится' }));
  assert.equal(body, `### Что случилось\n\nНе сохраняется тариф\n\n### Что ожидали\n\nСохранится\n\n${ENV_TAIL}`);
});

test('ошибки интерфейса дописываются в «Что случилось» блоком кода и только когда приложены', () => {
  assert.ok(!composeBody(report({ what: 'x', errors: [] })).includes('Ошибки интерфейса'));
  const body = composeBody(report({ what: 'x', errors: ['15:40:12 TypeError: ```boom``` у a@b.ru\n  at Xe (/assets/index-B3kd9fLq.js:12:3456)'] }));
  assert.equal(
    body,
    `### Что случилось\n\nx\n\nОшибки интерфейса:\n\n\`\`\`text\n15:40:12 TypeError: '''boom''' у ${HIDDEN}\n  at Xe (/assets/index-B3kd9fLq.js:12:3456)\n\`\`\`\n\n${ENV_TAIL}`,
  );
});

test('без шаблона ссылка ведёт на новую задачу с заголовком и текстом', () => {
  const url = new URL(issueUrl(plain, 'Заголовок', 'Текст & ещё'));
  assert.equal(`${url.origin}${url.pathname}`, `https://github.com/${ISSUES_REPO}/issues/new`);
  assert.equal(url.searchParams.get('title'), 'Заголовок');
  assert.equal(url.searchParams.get('body'), 'Текст & ещё');
  assert.equal(url.searchParams.get('template'), null);
});

test('с шаблоном текст раскладывается по полям формы', () => {
  const body = composeBody(report({ what: 'Не сохраняется', steps: '1. Открыть\n2. Нажать', errors: ['15:40:12 TypeError: boom'] }));
  const url = new URL(issueUrl(form, 'Заголовок', body));
  assert.equal(`${url.origin}${url.pathname}`, `https://github.com/${ISSUES_REPO}/issues/new`);
  assert.deepEqual([...url.searchParams.keys()], ['template', 'title', 'what', 'steps', 'version', 'remnawave', 'page', 'browser']);
  assert.equal(url.searchParams.get('template'), 'bug.yml');
  assert.equal(url.searchParams.get('title'), 'Заголовок');
  assert.equal(url.searchParams.get('what'), 'Не сохраняется\n\nОшибки интерфейса:\n\n```text\n15:40:12 TypeError: boom\n```');
  assert.equal(url.searchParams.get('steps'), '1. Открыть\n2. Нажать');
  assert.equal(url.searchParams.get('version'), env.version);
  assert.equal(url.searchParams.get('remnawave'), env.remnawave);
  assert.equal(url.searchParams.get('page'), env.page);
  assert.equal(url.searchParams.get('browser'), env.browser);
});

test('однострочные поля формы собираются в одну строку, текст без заголовков идёт в «Что случилось»', () => {
  const url = new URL(issueUrl(form, 'T', '### Что случилось\n\nx\n\n### Страница\n\n/admin/clients\n\n/admin/tariffs'));
  assert.equal(url.searchParams.get('page'), '/admin/clients; /admin/tariffs');
  assert.deepEqual(formFields('без заголовков\n#### Своё\nещё'), [['what', 'без заголовков\n#### Своё\nещё']]);
});

test('короткий отчёт уходит целиком', () => {
  const body = composeBody(report({ what: 'Не сохраняется тариф' }));
  for (const target of [plain, form]) {
    const fit = fitIssueUrl(target, 'Заголовок', body);
    assert.equal(fit.cut, false);
    assert.equal(fit.url, issueUrl(target, 'Заголовок', body));
  }
});

test('длинный отчёт обрезается до предела адреса, сведения о панели остаются целыми', () => {
  const body = composeBody(report({ what: 'Очень длинное описание ошибки. '.repeat(500), errors: ['TypeError: boom'] }));
  for (const target of [plain, form]) {
    const fit = fitIssueUrl(target, 'Заголовок', body);
    assert.equal(fit.cut, true);
    assert.ok(fit.url.length <= MAX_URL);
    assert.ok(fit.url.length > MAX_URL - 200);
    const params = new URL(fit.url).searchParams;
    const fields = Object.fromEntries(target.form ? params : formFields(params.get('body')));
    assert.ok(fields.what.endsWith(`\n\n${CUT_NOTE}`));
    const head = fields.what.slice(0, -(CUT_NOTE.length + 2));
    assert.ok(body.includes(head));
    assert.match(body.charAt(body.indexOf(head) + head.length), /\s/);
    assert.equal(fields.version, env.version);
    assert.equal(fields.remnawave, env.remnawave);
    assert.equal(fields.page, env.page);
    assert.equal(fields.browser, env.browser);
  }
});

test('обрезается самое длинное поле, короткие остаются', () => {
  const body = composeBody(report({ what: 'Коротко', steps: 'Шаг. '.repeat(2000) }));
  const fields = Object.fromEntries(new URL(fitIssueUrl(form, 'Заголовок', body).url).searchParams);
  assert.equal(fields.what, 'Коротко');
  assert.ok(fields.steps.startsWith('Шаг. Шаг.'));
  assert.ok(fields.steps.endsWith(CUT_NOTE));
  assert.equal(fields.version, env.version);
});

test('обрезка не рвёт символы и закрывает блок кода', () => {
  const body = composeBody(report({ what: '\u{1F600}'.repeat(3000) }));
  const fit = fitIssueUrl(plain, 'Заголовок', body);
  assert.equal(fit.cut, true);
  assert.ok(!new URL(fit.url).searchParams.get('body').includes('�'));
  const log = composeBody(report({ what: 'x', errors: ['TypeError: boom\n  at Xe (/assets/index.js:1:2)\n'.repeat(400)] }));
  const what = new URL(fitIssueUrl(form, 'Заголовок', log).url).searchParams.get('what');
  assert.ok(what.startsWith('x\n\nОшибки интерфейса:\n\n```text\n'));
  assert.equal((what.match(/^```/gm) ?? []).length % 2, 0);
  assert.ok(what.endsWith(`\`\`\`\n\n${CUT_NOTE}`));
});
