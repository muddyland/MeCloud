import adapter from '@sveltejs/adapter-static';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  kit: {
    adapter: adapter({
      fallback: 'index.html'
    }),
    // The hydration bootstrap is one inline <script>. Hash mode puts its
    // SHA-256 in a <meta> CSP on the page. The backend's header names the same
    // hash (computed from the index.html it serves), so this is the second of
    // two agreeing policies, and the one that still applies if the build is
    // served by something else (vite preview, a static host). Only script-src
    // is set here; everything else stays in the backend header.
    csp: {
      mode: 'hash',
      directives: {
        'script-src': ['self'],
      },
    },
  }
};

export default config;
