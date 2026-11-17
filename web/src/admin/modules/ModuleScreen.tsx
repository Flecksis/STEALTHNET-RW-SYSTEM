import { useParams } from 'react-router';
import { Pending } from '../pages/Pending';
import { Promo } from '../promo/Promo';
import { Referral } from '../referral/Referral';

export function ModuleScreen() {
  const { module = '' } = useParams();
  switch (module) {
    case 'promo':
      return <Promo />;
    case 'referral':
      return <Referral />;
    default:
      return <Pending title="Модуль" />;
  }
}
