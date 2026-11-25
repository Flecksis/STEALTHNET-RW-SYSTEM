import type { BlockKind } from '../../site/landingModel';

export type CssPlace = 'page' | BlockKind;

export const CSS_PLACES: CssPlace[] = ['page', 'hero', 'features', 'steps', 'plans', 'servers', 'faq', 'cta', 'footer'];
