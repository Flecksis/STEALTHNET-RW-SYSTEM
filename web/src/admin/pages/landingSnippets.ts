import type { CssPlace } from './landingCss';

export type Snippet = { name: string; css: string };

function rule(name: string, selector: string): Snippet {
  return { name, css: `${selector} {\n  \n}` };
}

export const SNIPPETS: Record<CssPlace, Snippet[]> = {
  page: [
    { name: 'Цвета всей страницы', css: '.ld {\n  --accent: #0f766e;\n  --accent-ink: #0f766e;\n  --btn: #0f766e;\n  --btn-ink: #ffffff;\n}' },
    rule('Шапка', '.ld-head'),
    rule('Название и логотип', '.ld-brand'),
    rule('Меню в шапке', '.ld-nav a'),
    rule('Заголовки блоков', '.ld-sec h2'),
    rule('Главные кнопки', '.ld-btn'),
    rule('Обычные кнопки', '.ld-btn2'),
    rule('Ссылки', '.ld-link'),
    rule('Тёмная тема', "[data-theme='dark'] .ld"),
    { name: 'Телефон', css: '@media (max-width: 600px) {\n  .ld {\n    \n  }\n}' },
  ],
  hero: [
    rule('Весь блок', '&'),
    rule('Заголовок', 'h1'),
    rule('Текст', '.ld-lead'),
    rule('Главная кнопка', '.ld-btn'),
    rule('Обычная кнопка', '.ld-btn2'),
    rule('Строка под кнопками', '.ld-hero-text > .ld-muted'),
    rule('Стандартная картинка', '.ld-art'),
    rule('Своя картинка', '.ld-pic-img'),
  ],
  features: [
    rule('Весь блок', '&'),
    rule('Заголовок', 'h2'),
    rule('Пункт', '.ld-item'),
    rule('Заголовок пункта', '.ld-item h3'),
    rule('Текст пункта', '.ld-item p'),
    rule('Ссылка', '.ld-link'),
    rule('Картинка', '.ld-item-pic'),
  ],
  steps: [
    rule('Весь блок', '&'),
    rule('Заголовок', 'h2'),
    rule('Шаг', '.ld-item'),
    rule('Номер шага', '.ld-step'),
    rule('Заголовок шага', '.ld-item h3'),
    rule('Текст шага', '.ld-item p'),
  ],
  plans: [
    rule('Весь блок', '&'),
    rule('Заголовок', 'h2'),
    rule('Карточка срока', '.ld-card'),
    rule('Цена', '.ld-card-price'),
    rule('Выделенная кнопка', '.ld-btn'),
    rule('Обычная кнопка', '.ld-btn2'),
    rule('Переключатель тарифов', '.ld-seg'),
    rule('Срок на телефоне', '.ld-term'),
  ],
  servers: [rule('Весь блок', '&'), rule('Колонка', '.ld-item'), rule('Заголовок колонки', '.ld-h2s'), rule('Текст', '.ld-item p')],
  faq: [rule('Весь блок', '&'), rule('Заголовок', 'h2'), rule('Вопрос с ответом', 'details'), rule('Вопрос', 'summary'), rule('Ответ', 'details p')],
  cta: [
    rule('Весь блок', '&'),
    rule('Плашка', '.ld-band'),
    rule('Заголовок', 'h2'),
    rule('Текст', '.ld-band p'),
    rule('Главная кнопка', '.ld-btn'),
    rule('Обычная кнопка', '.ld-btn2'),
  ],
  footer: [rule('Весь подвал', '&'), rule('Название', '.ld-foot-brand'), rule('Ссылки', '.ld-foot-nav a')],
};
