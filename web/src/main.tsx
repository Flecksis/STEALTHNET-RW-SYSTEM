import '@fontsource-variable/onest';
import './styles/tokens.css';
import './styles/base.css';
import { lazy, StrictMode, Suspense } from 'react';
import { createRoot } from 'react-dom/client';
import { BrowserRouter, Route, Routes, useLocation } from 'react-router';
import { reloadOnStaleChunks } from './lib/chunks';
import { guessLang, t, useApplied } from './lib/i18n';
import { captureReferral } from './lib/referral';
import { useSite } from './lib/site';
import type { Site } from './lib/types';
import { captureUtm } from './lib/utm';

const AdminApp = lazy(() => import('./admin/AdminApp').then((m) => ({ default: m.AdminApp })));
const Soon = lazy(() => import('./site/Soon').then((m) => ({ default: m.Soon })));
const CabinetApp = lazy(() => import('./cabinet/CabinetApp').then((m) => ({ default: m.CabinetApp })));
const MiniApp = lazy(() => import('./mini/MiniApp').then((m) => ({ default: m.MiniApp })));
const MiniPreview = lazy(() => import('./mini/MiniPreview').then((m) => ({ default: m.MiniPreview })));
const Landing = lazy(() => import('./site/Landing').then((m) => ({ default: m.Landing })));
const LandingPreview = lazy(() => import('./site/Landing').then((m) => ({ default: m.LandingPreview })));
const PayPage = lazy(() => import('./pay/PayPage').then((m) => ({ default: m.PayPage })));
const VerifyPage = lazy(() => import('./verify/VerifyPage').then((m) => ({ default: m.VerifyPage })));
const DocumentPage = lazy(() => import('./site/DocumentPage').then((m) => ({ default: m.DocumentPage })));
const DocumentPreview = lazy(() => import('./site/DocumentPage').then((m) => ({ default: m.DocumentPreview })));

function SitePage() {
  const site = useSite();
  const { pathname } = useLocation();
  const path = pathname.length > 1 ? pathname.replace(/\/+$/, '') : pathname;
  const doc = site?.documents?.find((d) => d.page && d.url === path);
  if (site && doc) {
    return <DocumentPage site={site} path={doc.url} />;
  }
  return <NotFound site={site} />;
}

function NotFound({ site }: { site: Site | null }) {
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
          <Route path="/admin/document-preview" element={<DocumentPreview />} />
          <Route path="/admin/mini-preview" element={<MiniPreview />} />
          <Route path="/admin/*" element={<AdminApp />} />
          <Route path="/cabinet/*" element={<CabinetApp />} />
          <Route path="/app/*" element={<MiniApp />} />
          <Route path="/pay/:id" element={<PayPage />} />
          <Route path="/verify/:token" element={<VerifyPage />} />
          <Route path="/" element={<Landing />} />
          <Route path="*" element={<SitePage />} />
        </Routes>
      </Suspense>
    </BrowserRouter>
  );
}

reloadOnStaleChunks();
captureReferral();
captureUtm();

const root = document.getElementById('root');
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
