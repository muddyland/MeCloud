import { defineConfig } from 'vitest/config';
import { resolve } from 'path';

export default defineConfig({
  // #lib/* resolves through package.json "imports", so needs no alias here.
  resolve: {
    alias: {
      // Stub SvelteKit virtual modules that aren't available in a test runner
      '$app/env': resolve('./src/tests/mocks/app-env.js'),
    },
  },
  test: {
    globals: true,
    environment: 'node',
    include: ['src/tests/**/*.test.js'],
  },
});
