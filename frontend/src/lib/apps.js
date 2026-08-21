/**
 * The application list, shared by the top-bar tabs and the command palette.
 *
 * A plain module rather than a component export: `export const` inside a
 * Svelte instance script declares a *prop*, not something importable.
 */
export const APPS = [
  { id: 'mail',     label: 'Mail',     href: '/' },
  { id: 'calendar', label: 'Calendar', href: '/calendar' },
  { id: 'contacts', label: 'Contacts', href: '/contacts' },
  { id: 'files',    label: 'Files',    href: '/files' },
];

/** Which app a pathname belongs to. */
export function activeApp(pathname) {
  if (pathname === '/calendar') return 'calendar';
  if (pathname === '/contacts') return 'contacts';
  if (pathname === '/files') return 'files';
  return 'mail';
}
