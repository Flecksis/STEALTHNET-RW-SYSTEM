import '@fontsource-variable/onest';
import './styles/tokens.css';
import './styles/base.css';
import { lazy, StrictMode, Suspense } from 'react';
import { createRoot } from 'react-dom/client';
import { BrowserRouter, Route, Routes } from 'react-router';

const AdminApp = lazy(() => import('./admin/AdminApp').then((m) => ({ default: m.AdminApp })));
const Soon = lazy(() => import('./site/Soon').then((m) => ({ default: m.Soon })));
const CabinetApp = lazy(() => import('./cabinet/CabinetApp').then((m) => ({ default: m.CabinetApp })));

function App() {
  return (
    <BrowserRouter>
      <Suspense fallback={<div style={{ minHeight: '100vh', background: 'var(--bg)' }} aria-busy="true" />}>
        <Routes>
          <Route path="/admin/*" element={<AdminApp />} />
          <Route path="/cabinet/*" element={<CabinetApp />} />
          <Route path="/app/*" element={<Soon title="Мини-приложение скоро откроется" text="Мини-приложение ещё собирается. Пока подписку можно купить и продлить в Telegram-боте." />} />
          <Route path="*" element={<Soon title="Сервис работает" text="Сайт ещё собирается. Подписку можно купить и продлить в Telegram-боте." />} />
        </Routes>
      </Suspense>
    </BrowserRouter>
  );
}

const root = document.getElementById('root');
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
