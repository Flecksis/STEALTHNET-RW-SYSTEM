import '@fontsource-variable/onest';
import './styles/tokens.css';
import './styles/base.css';
import { lazy, StrictMode, Suspense } from 'react';
import { createRoot } from 'react-dom/client';
import { BrowserRouter, Route, Routes } from 'react-router';
import { captureReferral } from './lib/referral';

const AdminApp = lazy(() => import('./admin/AdminApp').then((m) => ({ default: m.AdminApp })));
const Soon = lazy(() => import('./site/Soon').then((m) => ({ default: m.Soon })));
const CabinetApp = lazy(() => import('./cabinet/CabinetApp').then((m) => ({ default: m.CabinetApp })));
const MiniApp = lazy(() => import('./mini/MiniApp').then((m) => ({ default: m.MiniApp })));
const Landing = lazy(() => import('./site/Landing').then((m) => ({ default: m.Landing })));
const LandingPreview = lazy(() => import('./site/Landing').then((m) => ({ default: m.LandingPreview })));
const PayPage = lazy(() => import('./pay/PayPage').then((m) => ({ default: m.PayPage })));

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
          <Route path="*" element={<Soon title="Страница не найдена" text="Такой страницы нет. Перейдите на главную или откройте Telegram-бота." />} />
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
