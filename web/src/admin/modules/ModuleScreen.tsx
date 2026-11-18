import { useParams } from 'react-router';
import { Broadcasts } from '../broadcast/Broadcasts';
import { Gifts } from '../gifts/Gifts';
import { Pending } from '../pages/Pending';
import { Promo } from '../promo/Promo';
import { PromoLinks } from '../promolinks/PromoLinks';
import { Referral } from '../referral/Referral';

export function ModuleScreen() {
  const { module = '' } = useParams();
  switch (module) {
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
    default:
      return <Pending title="Модуль" />;
  }
}
