import { test } from 'node:test';
import assert from 'node:assert/strict';
import { captchaTrouble } from './captchaText.ts';

test('сбой сервиса и ответ с чужого домена объясняются по-разному', () => {
  assert.equal(
    captchaTrouble({ problem: 'секретный ключ не подошёл' }, 'сегодня, 12:05'),
    'Последняя проверка капчи сегодня, 12:05 не удалась: секретный ключ не подошёл. Пока капча не отвечает, пробный период и регистрация по почте не выдаются.',
  );
  const foreign = captchaTrouble({ problem: 'ответ получен на домене evil.test, его нет в поле «Домены»', foreign: 'evil.test' }, 'сегодня, 12:05');
  assert.match(foreign, /^Ответ капчи сегодня, 12:05 получен на домене evil\.test/);
  assert.match(foreign, /«Домены».*«Настроить»/);
  assert.doesNotMatch(foreign, /не отвечает/);
});
