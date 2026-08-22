/**
 * The application list, shared by the top-bar tabs and the command palette.
 *
 * A plain module rather than a component export: `export const` inside a
 * Svelte instance script declares a *prop*, not something importable.
 */
export const APPS = [
  { id: 'home',     label: 'Home',     href: '/' },
  { id: 'mail',     label: 'Mail',     href: '/mail' },
  { id: 'calendar', label: 'Calendar', href: '/calendar' },
  { id: 'contacts', label: 'Contacts', href: '/contacts' },
  { id: 'notes',    label: 'Notes',    href: '/notes' },
  { id: 'files',    label: 'Files',    href: '/files' },
];

/** Everything except the dashboard, for places that list "the apps". */
export const SUB_APPS = APPS.filter((a) => a.id !== 'home');

/** Which app a pathname belongs to. */
export function activeApp(pathname) {
  const path = String(pathname ?? '/');
  if (path === '/' || path === '') return 'home';
  // startsWith so a future nested route (/mail/123) still highlights its app.
  const match = SUB_APPS.find((app) => path === app.href || path.startsWith(`${app.href}/`));
  return match ? match.id : 'home';
}

/** The app record for a pathname. */
export function appFor(pathname) {
  const id = activeApp(pathname);
  return APPS.find((a) => a.id === id) ?? APPS[0];
}
