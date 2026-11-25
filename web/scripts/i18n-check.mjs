import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const root = new URL('..', import.meta.url).pathname;
const src = join(root, 'src');
const areas = ['cabinet', 'mini', 'site', 'pay', 'verify', 'open', 'lib'].map((dir) => join(src, dir));
const dictDir = join(src, 'lib', 'i18n');
const strict = !process.argv.includes('--report');
const showLoose = process.argv.includes('--loose');

function walk(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      if (path !== dictDir) {
        out.push(...walk(path));
      }
    } else if (/\.(tsx?|mjs)$/.test(name)) {
      out.push(path);
    }
  }
  return out;
}

function unquote(raw) {
  return raw.replace(/\\(.)/g, (_, ch) => (ch === 'n' ? '\n' : ch));
}

const literal = String.raw`'((?:\\.|[^'\\])*)'|"((?:\\.|[^"\\])*)"`;
const entry = new RegExp(String.raw`^\s*(?:${literal}|([A-Za-zА-Яа-яЁё_][\wА-Яа-яЁё]*))\s*:\s*(?:${literal})\s*,?\s*$`, 'u');

const dict = new Map();
const duplicates = [];
for (const name of readdirSync(dictDir)) {
  if (!/^en-.*\.ts$/.test(name)) {
    continue;
  }
  const lines = readFileSync(join(dictDir, name), 'utf8').split('\n');
  for (const line of lines) {
    const found = entry.exec(line);
    if (!found) {
      continue;
    }
    const key = unquote(found[1] ?? found[2] ?? found[3]);
    const value = unquote(found[4] ?? found[5]);
    if (dict.has(key) && dict.get(key).value !== value) {
      duplicates.push(`${name}: «${key}» уже есть в ${dict.get(key).file} с другим переводом`);
    }
    dict.set(key, { value, file: name });
  }
}

const call = new RegExp(String.raw`\b(?:t|msg)\(\s*(?:${literal}|(\x60[^\x60]*\x60))`, 'g');
const cyrillic = /[А-Яа-яЁё]/;
const missing = [];
const dynamic = [];
const loose = [];
const used = new Set();

for (const file of areas.flatMap(walk)) {
  const text = readFileSync(file, 'utf8');
  const rel = relative(root, file);
  for (const match of text.matchAll(call)) {
    if (match[3] !== undefined) {
      if (cyrillic.test(match[3])) {
        dynamic.push(`${rel}: ${match[3].slice(0, 80)}`);
      }
      continue;
    }
    const key = unquote(match[1] ?? match[2]);
    used.add(key);
    if (!cyrillic.test(key)) {
      continue;
    }
    if (!dict.has(key)) {
      missing.push(`${rel}: «${key}»`);
    }
  }
  if (showLoose) {
    const lines = text.split('\n');
    lines.forEach((line, index) => {
      if (!cyrillic.test(line) || /\b(t|msg)\(/.test(line) || /^\s*(\/\/|\*)/.test(line)) {
        return;
      }
      loose.push(`${rel}:${index + 1}: ${line.trim().slice(0, 100)}`);
    });
  }
}

for (const [key, { value, file }] of dict) {
  if (cyrillic.test(value)) {
    missing.push(`${file}: в переводе «${key}» осталась кириллица`);
  }
}

const unused = [...dict.keys()].filter((key) => !used.has(key));

if (duplicates.length) {
  console.log(`Повторы с разным переводом (${duplicates.length}):\n  ${duplicates.join('\n  ')}`);
}
if (dynamic.length) {
  console.log(`Шаблонные строки в t() — нужна строка с {подстановкой} (${dynamic.length}):\n  ${dynamic.join('\n  ')}`);
}
if (loose.length) {
  console.log(`Русский текст вне t() (${loose.length}):\n  ${loose.join('\n  ')}`);
}
if (process.argv.includes('--unused') && unused.length) {
  console.log(`Переводы без использования (${unused.length}):\n  ${unused.join('\n  ')}`);
}
if (missing.length) {
  console.log(`Нет английского перевода (${missing.length}):\n  ${missing.join('\n  ')}`);
}
const failed = missing.length + dynamic.length + duplicates.length;
console.log(`Переводов: ${dict.size}, строк в t(): ${used.size}, проблем: ${failed}`);
if (strict && failed > 0) {
  process.exit(1);
}
