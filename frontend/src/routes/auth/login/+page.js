export const prerender = false;

/** @type {import('./$types').PageLoad} */
export async function load() {
  if (typeof window !== 'undefined') {
    window.location.href = '/auth/login';
  }
  return {};
}
