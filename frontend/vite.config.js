import { sveltekit } from '@sveltejs/kit/vite';
import adapter from '@sveltejs/adapter-static';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  // Tailwind before SvelteKit, as its docs ask: it compiles app.css (and any
  // <style> that @reference-s it) itself, so no PostCSS config is needed.
  plugins: [
    tailwindcss(),
    sveltekit({
      adapter: adapter({
        fallback: 'index.html'
      }),
      // The hydration bootstrap is one inline <script>. Hash mode puts its
      // SHA-256 in a <meta> CSP on the page. The server's header names the
      // same hash (computed from the index.html it serves), so this is the
      // second of two agreeing policies, and the one that still applies if the
      // build is served by something else (vite preview, a static host). Only
      // script-src is set here; everything else stays in the server header.
      csp: {
        mode: 'hash',
        directives: {
          'script-src': ['self'],
        },
      },
    }),
  ],
  server: {
    proxy: {
      '/api': 'http://localhost:8000',
      '/auth': 'http://localhost:8000'
    }
  }
});
