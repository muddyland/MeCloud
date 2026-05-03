import { defineConfig } from 'vitest/config';
import { resolve } from 'path';

export default defineConfig({
  resolve: {
    alias: {
      '$lib': resolve('./src/lib'),
      // Stub SvelteKit virtual modules that aren't available in a test runner
      '$app/environment': resolve('./src/tests/mocks/app-environment.js'),
      '$app/stores':      resolve('./src/tests/mocks/app-stores.js'),
    },
  },
  test: {
    globals: true,
    environment: 'node',
    include: ['src/tests/**/*.test.js'],
  },
});
