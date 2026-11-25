import type { HomeKind } from '../../mini/config';
import { HOME_NAMES, type Limits } from './model';

export type MiniPlace = 'page' | 'header' | HomeKind | 'tabs' | 'profile';

export const MINI_PLACES: MiniPlace[] = ['page', 'header', 'banners', 'greeting', 'subscription', 'actions', 'sections', 'tabs', 'profile'];

export const CSS_PATH = '/api/admin/settings/miniapp/css';

export function placeName(place: MiniPlace): string {
  switch (place) {
    case 'page':
      return 'Общий CSS';
    case 'header':
      return 'Шапка, CSS блока';
    case 'tabs':
      return 'CSS меню внизу';
    case 'profile':
      return 'CSS профиля';
    default:
      return `${HOME_NAMES[place]}, CSS блока`;
  }
}

export function placeHint(place: MiniPlace): string {
  switch (place) {
    case 'page':
      return 'Действует на все экраны мини-приложения, и на оплату тоже. Кабинет, лендинг и админка не изменятся';
    case 'header':
      return 'Действует только на шапку главной';
    case 'tabs':
      return 'Действует только на меню внизу';
    case 'profile':
      return 'Действует только на экран «Профиль»';
    default:
      return `Действует только на блок «${HOME_NAMES[place]}»`;
  }
}

export function placeSample(place: MiniPlace): string {
  const selector: Record<MiniPlace, string> = {
    page: '.mn-main',
    header: '.mn-brand',
    banners: '.mn-banner',
    greeting: 'h2',
    subscription: '.mn-main',
    actions: '.mn-pairbtn',
    sections: '.mn-row',
    tabs: '.mn-tab',
    profile: '.mn-card',
  };
  return place === 'page' || place === 'banners' || place === 'profile' ? `${selector[place]} {\n  border-radius: 22px;\n}` : `${selector[place]} {\n  color: #0f766e;\n}`;
}

export function placeMax(place: MiniPlace, limits: Limits): number {
  return place === 'page' ? limits.page_css : limits.block_css;
}
