import { useParams } from 'react-router';
import { Analytics } from '../analytics/Analytics';
import { Broadcasts } from '../broadcast/Broadcasts';
import { Gifts } from '../gifts/Gifts';
import { Contests } from '../contests/Contests';
import { Pending } from '../pages/Pending';
import { Promo } from '../promo/Promo';
import { PromoLinks } from '../promolinks/PromoLinks';
import { Referral } from '../referral/Referral';
import { Roulette } from '../roulette/Roulette';
import { Utm } from '../utm/Utm';

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
