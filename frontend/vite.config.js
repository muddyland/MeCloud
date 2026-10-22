import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  // Tailwind before SvelteKit, as its docs ask: it compiles app.css (and any
  // <style> that @reference-s it) itself, so no PostCSS config is needed.
  plugins: [tailwindcss(), sveltekit()],
  server: {
    proxy: {
      '/api': 'http://localhost:8000',
      '/auth': 'http://localhost:8000'
    }
  }
});
