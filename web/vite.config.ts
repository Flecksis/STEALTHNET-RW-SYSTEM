import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const api = process.env.SN_DEV_API ?? 'http://127.0.0.1:18080';

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: { '/api': { target: api, changeOrigin: false } },
  },
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
    sourcemap: false,
    target: 'es2020',
  },
});
