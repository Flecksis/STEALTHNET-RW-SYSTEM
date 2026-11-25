import type { CaptchaWidget } from './captcha';
import { settleLeft } from './settle';
import { solveWork } from './work';

export type HumanCheck = { kind: 'button' } | { kind: 'work'; bits: number } | ({ kind: 'captcha' } & CaptchaWidget);

export type CheckPrompt = {
  token: string;
  text: string;
  check?: HumanCheck;
};

export type CheckAnswer = {
  confirm: string;
  proof?: string;
  captcha?: string;
};

const shown = new WeakMap<CheckPrompt, number>();

export function startWork(prompt: CheckPrompt): Promise<string | null> | null {
  shown.set(prompt, performance.now());
  const check = prompt.check;
  if (check?.kind !== 'work') {
    return null;
  }
  return solveWork(prompt.token, check.bits).catch(() => null);
}

export function captchaOf(prompt: CheckPrompt | null): CaptchaWidget | null {
  const check = prompt?.check;
  if (check?.kind !== 'captcha') {
    return null;
  }
  return { provider: check.provider, site_key: check.site_key, action: check.action, ...(check.cdata ? { cdata: check.cdata } : {}) };
}

export async function answerOf(prompt: CheckPrompt, work: Promise<string | null> | null): Promise<CheckAnswer> {
  const proof = work ? await work : null;
  const left = settleLeft(shown.get(prompt), performance.now());
  if (left > 0) {
    await new Promise((resolve) => setTimeout(resolve, left));
  }
  return proof ? { confirm: prompt.token, proof } : { confirm: prompt.token };
}
