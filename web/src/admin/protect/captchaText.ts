export type CaptchaTrouble = { problem: string; foreign?: string };

export function captchaTrouble(trouble: CaptchaTrouble, when: string): string {
  return trouble.foreign
    ? `Ответ капчи ${when} получен на домене ${trouble.foreign}, которого нет в поле «Домены»: клиент получил отказ. Если это ваш домен, добавьте его в «Настроить».`
    : `Последняя проверка капчи ${when} не удалась: ${trouble.problem}. Пока капча не отвечает, пробный период и регистрация по почте не выдаются.`;
}
