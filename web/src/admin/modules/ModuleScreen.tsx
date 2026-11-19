import { lazy } from 'react';
import { useParams } from 'react-router';
import { Pending } from '../pages/Pending';

const Analytics = lazy(() => import('../analytics/Analytics').then((m) => ({ default: m.Analytics })));
const Broadcasts = lazy(() => import('../broadcast/Broadcasts').then((m) => ({ default: m.Broadcasts })));
const Gifts = lazy(() => import('../gifts/Gifts').then((m) => ({ default: m.Gifts })));
const Contests = lazy(() => import('../contests/Contests').then((m) => ({ default: m.Contests })));
const Promo = lazy(() => import('../promo/Promo').then((m) => ({ default: m.Promo })));
const PromoLinks = lazy(() => import('../promolinks/PromoLinks').then((m) => ({ default: m.PromoLinks })));
const Referral = lazy(() => import('../referral/Referral').then((m) => ({ default: m.Referral })));
const Roulette = lazy(() => import('../roulette/Roulette').then((m) => ({ default: m.Roulette })));
const Utm = lazy(() => import('../utm/Utm').then((m) => ({ default: m.Utm })));

export function ModuleScreen() {
  const { module = '' } = useParams();
  switch (module) {
    case 'analytics':
      return <Analytics />;
    case 'broadcast':
      return <Broadcasts />;
    case 'promo':
      return <Promo />;
    case 'gifts':
      return <Gifts />;
    case 'promolinks':
      return <PromoLinks />;
    case 'referral':
      return <Referral />;
    case 'contests':
      return <Contests />;
    case 'roulette':
      return <Roulette />;
    case 'utm':
      return <Utm />;
    default:
      return <Pending title="Модуль" />;
  }
}
