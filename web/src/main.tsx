import '@fontsource-variable/onest';
import './styles/tokens.css';
import './styles/base.css';
import { lazy, StrictMode, Suspense } from 'react';
import { createRoot } from 'react-dom/client';
import { BrowserRouter, Route, Routes } from 'react-router';
import { guessLang, t, useApplied } from './lib/i18n';
import { captureReferral } from './lib/referral';
import { useSite } from './lib/site';

const AdminApp = lazy(() => import('./admin/AdminApp').then((m) => ({ default: m.AdminApp })));
const Soon = lazy(() => import('./site/Soon').then((m) => ({ default: m.Soon })));
const CabinetApp = lazy(() => import('./cabinet/CabinetApp').then((m) => ({ default: m.CabinetApp })));
const MiniApp = lazy(() => import('./mini/MiniApp').then((m) => ({ default: m.MiniApp })));
const Landing = lazy(() => import('./site/Landing').then((m) => ({ default: m.Landing })));
const LandingPreview = lazy(() => import('./site/Landing').then((m) => ({ default: m.LandingPreview })));
const PayPage = lazy(() => import('./pay/PayPage').then((m) => ({ default: m.PayPage })));

function NotFound() {
  const site = useSite();
  const ready = useApplied(site ? guessLang(site) : null);
  if (!ready) {
    return <div style={{ minHeight: '100vh', background: 'var(--bg)' }} aria-busy="true" />;
  }
  return <Soon title={t('Страница не найдена')} text={t('Такой страницы нет. Перейдите на главную или откройте Telegram-бота.')} />;
}

function App() {
  return (
    <BrowserRouter>
      <Suspense fallback={<div style={{ minHeight: '100vh', background: 'var(--bg)' }} aria-busy="true" />}>
        <Routes>
          <Route path="/admin/landing-preview" element={<LandingPreview />} />
          <Route path="/admin/*" element={<AdminApp />} />
          <Route path="/cabinet/*" element={<CabinetApp />} />
          <Route path="/app/*" element={<MiniApp />} />
          <Route path="/pay/:id" element={<PayPage />} />
          <Route path="/" element={<Landing />} />
          <Route path="*" element={<NotFound />} />
        </Routes>
      </Suspense>
    </BrowserRouter>
  );
}

captureReferral();

const root = document.getElementById('root');
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
