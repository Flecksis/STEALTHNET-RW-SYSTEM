import type { CaptchaWidget } from './captcha';
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

export function startWork(prompt: CheckPrompt): Promise<string | null> | null {
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
  return proof ? { confirm: prompt.token, proof } : { confirm: prompt.token };
}
