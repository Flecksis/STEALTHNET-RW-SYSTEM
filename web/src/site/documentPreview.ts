import type { Lang } from '../lib/i18n';
import type { Theme } from '../lib/theme';

export const DOC_PREVIEW = { ready: 'sn-doc-preview-ready', draft: 'sn-doc-preview-draft' } as const;

export type DocDraft = {
  type: typeof DOC_PREVIEW.draft;
  title: string;
  html: string;
  lang: Lang;
  theme: Theme;
  path: string | null;
  updated_at: string | null;
};
