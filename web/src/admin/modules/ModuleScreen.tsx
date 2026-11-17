import { useParams } from 'react-router';
import { Broadcasts } from '../broadcast/Broadcasts';
import { Pending } from '../pages/Pending';
import { Promo } from '../promo/Promo';
import { Referral } from '../referral/Referral';

export function ModuleScreen() {
  const { module = '' } = useParams();
  switch (module) {
    case 'broadcast':
      return <Broadcasts />;
    case 'promo':
      return <Promo />;
    case 'referral':
      return <Referral />;
    default:
      return <Pending title="Модуль" />;
  }
}
