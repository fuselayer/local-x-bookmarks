import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';

// Tauri serves the built assets from `dist` over its own protocol, so relative
// asset paths are required — an absolute `/assets/...` would resolve against
// the custom protocol root and 404.
export default defineConfig({
  plugins: [svelte()],
  base: './',
  clearScreen: false,
  resolve: {
    alias: {
      // One canonical fixture, shared with the CLI and the Rust tests, so the
      // browser-dev fallback and `xdl import` can never drift apart.
      '@fixtures': fileURLToPath(new URL('../fixtures', import.meta.url)),
    },
  },
  server: {
    port: 5273,
    strictPort: true,
    fs: {
      // The alias points outside `ui/`, so Vite's dev server has to be told
      // the repo root is readable.
      allow: ['..'],
    },
  },
  build: {
    // WebView2 / WKWebView / WebKitGTK on a modern OS all handle ES2022.
    target: 'es2022',
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: false,
  },
});
