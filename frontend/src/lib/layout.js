/**
 * Pane visibility for the list/detail apps.
 *
 * Mail, Contacts and Notes all show a list beside a detail pane on a wide
 * screen and one at a time on a phone. Each page had its own copy of that
 * conditional, and Notes got it wrong in a way that was invisible until you
 * opened it on a phone: its list was in the off-canvas drawer *and* its detail
 * pane was hidden for want of a selection, so the page rendered blank.
 *
 * Centralising it means the invariant — something is always visible — is stated
 * once and tested once.
 */

/** Classes for the list pane. */
export function listPaneClass(compact, hasSelection) {
  if (!compact) return 'flex-shrink-0';
  return hasSelection ? 'hidden' : 'flex-1 min-w-0';
}

/** Classes for the detail pane. */
export function detailPaneClass(compact, hasSelection) {
  if (!compact) return 'flex-1 min-w-0';
  return hasSelection ? 'flex-1 min-w-0' : 'hidden';
}

/**
 * Which panes are showing. Exists so the "at least one" invariant can be
 * asserted directly rather than inferred from class strings.
 */
export function visiblePanes(compact, hasSelection) {
  return {
    list: !listPaneClass(compact, hasSelection).includes('hidden'),
    detail: !detailPaneClass(compact, hasSelection).includes('hidden'),
  };
}
