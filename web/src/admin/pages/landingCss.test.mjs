import assert from 'node:assert/strict';
import { test } from 'node:test';
import { closeBrace, cssOut, insertImage, insertRule, joinStyles, lengthProblem, lineRange, newLine, problemText, readTexts, sameCss, shiftLines } from './landingCss.ts';

const apply = (text, edit) => (edit ? text.slice(0, edit.from) + edit.text + text.slice(edit.to) : text);
const at = (marked) => {
  const caret = marked.indexOf('|');
  return { text: marked.replace('|', ''), caret };
};
const withCaret = (text, edit) => {
  const next = apply(text, edit);
  return `${next.slice(0, edit.caret)}|${next.slice(edit.caret)}`;
};

test('тексты CSS читаются по известным местам и сравниваются без хвостовых пробелов', () => {
  assert.deepEqual(readTexts({ page: '.a {}', hero: '', video: 'x', faq: 5, cta: null }), { page: '.a {}' });
  assert.deepEqual(readTexts(null), {});
  assert.deepEqual(cssOut({ page: '.a {}\n\n', hero: '  \n' }), { page: '.a {}' });
  assert.equal(sameCss({ page: '.a {}\n' }, { page: '.a {}', hero: '' }), true);
  assert.equal(sameCss({ page: '.a {}' }, { page: '.b {}' }), false);
  assert.equal(joinStyles({ hero: 'h\n', page: 'p\n', footer: 'f\n' }), 'p\nh\nf\n');
});

test('новая строка держит отступ и раскрывает скобки', () => {
  const run = (marked) => {
    const { text, caret } = at(marked);
    return withCaret(text, newLine(text, caret, caret));
  };
  assert.equal(run('.a {|}'), '.a {\n  |\n}');
  assert.equal(run('.a {|'), '.a {\n  |');
  assert.equal(run('.a {\n  color: red;|\n}'), '.a {\n  color: red;\n  |\n}');
  assert.equal(run('@media x {\n  .a {|\n}'), '@media x {\n  .a {\n    |\n}');
  assert.equal(run('.a {}|'), '.a {}\n|');
});

test('закрывающая скобка на пустой строке убирает лишний отступ', () => {
  const { text, caret } = at('.a {\n  |');
  assert.equal(withCaret(text, closeBrace(text, caret, caret)), '.a {\n}|');
  assert.equal(closeBrace('.a { color: red;  ', 18, 18), null);
  assert.equal(closeBrace('', 0, 0), null);
});

test('Tab сдвигает строки вправо, Shift+Tab — обратно', () => {
  assert.equal(withCaret('ab', shiftLines('ab', 1, 1, false)), 'a  |b');
  assert.equal(apply('a\nb\n\nc', shiftLines('a\nb\n\nc', 0, 6, false)), '  a\n  b\n\n  c');
  assert.equal(apply('  a\n b\nc', shiftLines('  a\n b\nc', 3, 6, true)), 'a\nb\nc');
  assert.equal(shiftLines('a', 1, 1, true), null);
  assert.equal(withCaret('.a {\n    b', shiftLines('.a {\n    b', 10, 10, true)), '.a {\n  b|');
  const many = shiftLines('a\nb', 0, 3, false);
  assert.deepEqual([many.anchor, many.caret], [0, 7]);
});

test('правило вставляется между правилами, а не внутрь чужого', () => {
  const run = (marked) => {
    const { text, caret } = at(marked);
    return withCaret(text, insertRule(text, caret, '.b {\n  \n}'));
  };
  assert.equal(run('|'), '.b {\n  |\n}');
  assert.equal(run('.a {\n  color|: red;\n}\n\n.c {}'), '.a {\n  color: red;\n}\n\n.b {\n  |\n}\n\n.c {}');
  assert.equal(run('.a {}|'), '.a {}\n\n.b {\n  |\n}');
  assert.equal(run('.a {}\n\n|'), '.a {}\n\n.b {\n  |\n}');
  assert.equal(run('.a { content: "}"|; }'), '.a { content: "}"; }\n\n.b {\n  |\n}');
  assert.equal(run('/* { */\n|'), '/* { */\n\n.b {\n  |\n}');
  assert.equal(run('.a|, .c {}'), '.a, .c {}\n\n.b {\n  |\n}');
});

test('картинка вставляется туда, где стоит курсор', () => {
  const run = (marked, page = true) => {
    const { text, caret } = at(marked);
    return apply(text, insertImage(text, caret, '/i.png', page));
  };
  const full = 'background: url("/i.png") center / cover no-repeat;';
  assert.equal(run('.a { background: | }'), '.a { background: url("/i.png") }');
  assert.equal(run('.a {\n  |\n}'), `.a {\n  ${full}\n}`);
  assert.equal(run('.a {\n  color: red;|\n}'), `.a {\n  color: red;\n  ${full}\n}`);
  assert.equal(run('|'), `.ld-hero {\n  ${full}\n}`);
  assert.equal(run('.a {}|'), `.a {}\n\n.ld-hero {\n  ${full}\n}`);
  assert.equal(run('|', false), full);
  assert.equal(run('background: |', false), 'background: url("/i.png")');
  assert.equal(run('gap: 0;\n|', false), `gap: 0;\n${full}`);
});

test('строка с ошибкой находится, а текст ошибки читается как фраза', () => {
  assert.deepEqual(lineRange('a\nbc\nd', 2), { from: 2, to: 4 });
  assert.deepEqual(lineRange('a\nbc\nd', 9), { from: 5, to: 6 });
  assert.equal(problemText({ line: 7, text: 'не закрыта скобка {' }), 'Строка 7: не закрыта скобка {');
  assert.equal(problemText({ line: null, text: 'не длиннее 10 символов' }), 'Не длиннее 10 символов');
  assert.equal(lengthProblem('я'.repeat(10), 10), null);
  assert.equal(lengthProblem('😀'.repeat(10), 10), null);
  assert.deepEqual(lengthProblem('я'.repeat(11), 10), { line: null, text: 'не длиннее 10 символов' });
});
