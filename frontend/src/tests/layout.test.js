import { describe, it, expect } from 'vitest';
import { listPaneClass, detailPaneClass, visiblePanes } from '$lib/layout.js';

/*
 * Regression cover for a blank page.
 *
 * The Notes app put its list in the off-canvas drawer while the detail pane was
 * hidden for want of a selection. On a phone that meant *both* panes were
 * hidden and the page rendered empty — no error, nothing in the console, just
 * black. It only became reachable by selecting a note from somewhere else.
 */

const CASES = [
  { compact: false, hasSelection: false },
  { compact: false, hasSelection: true },
  { compact: true,  hasSelection: false },
  { compact: true,  hasSelection: true },
];

describe('list/detail pane visibility', () => {
  it('always shows at least one pane', () => {
    for (const { compact, hasSelection } of CASES) {
      const panes = visiblePanes(compact, hasSelection);
      expect(panes.list || panes.detail,
        `both panes hidden at compact=${compact} hasSelection=${hasSelection}`).toBe(true);
    }
  });

  it('shows both panes on a wide screen, whatever the selection', () => {
    expect(visiblePanes(false, false)).toEqual({ list: true, detail: true });
    expect(visiblePanes(false, true)).toEqual({ list: true, detail: true });
  });

  it('shows exactly one pane when compact', () => {
    expect(visiblePanes(true, false)).toEqual({ list: true, detail: false });
    expect(visiblePanes(true, true)).toEqual({ list: false, detail: true });
  });

  it('gives the visible pane the remaining space when compact', () => {
    expect(listPaneClass(true, false)).toContain('flex-1');
    expect(detailPaneClass(true, true)).toContain('flex-1');
  });

  it('keeps the list at its fixed width on a wide screen', () => {
    expect(listPaneClass(false, true)).toBe('flex-shrink-0');
    expect(listPaneClass(false, false)).toBe('flex-shrink-0');
  });

  it('never emits a class string that is both hidden and sized', () => {
    for (const { compact, hasSelection } of CASES) {
      for (const cls of [listPaneClass(compact, hasSelection),
                         detailPaneClass(compact, hasSelection)]) {
        if (cls.includes('hidden')) expect(cls).not.toContain('flex-1');
      }
    }
  });
});
