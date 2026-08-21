import { describe, it, expect } from 'vitest';
import { fuzzyMatch, fuzzyScore, rankItems, highlightRuns } from '$lib/fuzzy.js';

const labels = (items) => items.map((i) => i.label);

describe('fuzzyMatch', () => {
  it('matches a subsequence and reports where', () => {
    const m = fuzzyMatch('cal', 'Calendar');
    expect(m).not.toBeNull();
    expect(m.indices).toEqual([0, 1, 2]);
  });

  it('returns null when the query is not a subsequence', () => {
    expect(fuzzyMatch('zzz', 'Calendar')).toBeNull();
    // Order matters: 'a' never appears before a 'c' in "Calendar".
    expect(fuzzyMatch('ac', 'Calendar')).toBeNull();
    // ...but 'cla' does match, as C-a-l-e-n-d-a-r contains c, l, a in order.
    expect(fuzzyMatch('cla', 'Calendar')).not.toBeNull();
  });

  it('is case-insensitive', () => {
    expect(fuzzyMatch('CAL', 'calendar')).not.toBeNull();
  });

  it('treats an empty query as a neutral match', () => {
    expect(fuzzyMatch('', 'anything')).toEqual({ score: 0, indices: [] });
  });

  it('scores a prefix above a mid-string match', () => {
    expect(fuzzyScore('sent', 'Sent')).toBeGreaterThan(fuzzyScore('sent', 'Recently Sent'));
  });

  it('scores consecutive characters above scattered ones', () => {
    expect(fuzzyScore('abc', 'abcdef')).toBeGreaterThan(fuzzyScore('abc', 'axbxcx'));
  });

  it('rewards word-start matches, so initials work', () => {
    // "nf" should find "New Folder" via the two word starts.
    expect(fuzzyScore('nf', 'New Folder')).toBeGreaterThan(fuzzyScore('nf', 'Confirm'));
  });

  it('prefers the shorter of two equally good targets', () => {
    expect(fuzzyScore('sent', 'Sent')).toBeGreaterThan(fuzzyScore('sent', 'Sent Items Archive 2019'));
  });
});

describe('rankItems', () => {
  const items = [
    { label: 'Mail' }, { label: 'Calendar' }, { label: 'Contacts' }, { label: 'Files' },
    { label: 'New Folder' }, { label: 'Sign out' },
  ];

  it('returns everything untouched for an empty query', () => {
    expect(labels(rankItems('', items))).toEqual(labels(items));
    expect(labels(rankItems('   ', items))).toEqual(labels(items));
  });

  it('puts the intended item first', () => {
    expect(labels(rankItems('cal', items))[0]).toBe('Calendar');
    expect(labels(rankItems('fil', items))[0]).toBe('Files');
    expect(labels(rankItems('nf', items))[0]).toBe('New Folder');
  });

  it('drops non-matches entirely', () => {
    expect(rankItems('zzzz', items)).toEqual([]);
  });

  it('searches keywords without highlighting them on the label', () => {
    const withKeywords = [{ label: 'Sign out', keywords: ['logout', 'exit'] }];
    const [hit] = rankItems('logout', withKeywords);
    expect(hit.label).toBe('Sign out');
    // The match was on a keyword, so there is nothing to highlight in the label —
    // highlighting the wrong characters is worse than highlighting none.
    expect(hit.indices).toEqual([]);
  });

  it('highlights when the label itself matched', () => {
    const [hit] = rankItems('cal', items);
    expect(hit.indices).toEqual([0, 1, 2]);
  });

  it('honours an explicit boost', () => {
    const contest = [{ label: 'Contacts' }, { label: 'Contacts', boost: 100, id: 'boosted' }];
    expect(rankItems('con', contest)[0].id).toBe('boosted');
  });

  it('keeps original order for ties', () => {
    const tied = [{ label: 'aa', id: 1 }, { label: 'aa', id: 2 }];
    expect(rankItems('aa', tied).map((i) => i.id)).toEqual([1, 2]);
  });

  it('respects the limit', () => {
    expect(rankItems('a', items, { limit: 2 })).toHaveLength(2);
  });
});

describe('highlightRuns', () => {
  it('splits into hit and miss runs', () => {
    expect(highlightRuns('Calendar', [0, 1, 2])).toEqual([
      { text: 'Cal', hit: true },
      { text: 'endar', hit: false },
    ]);
  });

  it('handles matches in the middle', () => {
    expect(highlightRuns('abc', [1])).toEqual([
      { text: 'a', hit: false },
      { text: 'b', hit: true },
      { text: 'c', hit: false },
    ]);
  });

  it('returns one plain run when nothing matched', () => {
    expect(highlightRuns('abc', [])).toEqual([{ text: 'abc', hit: false }]);
    expect(highlightRuns('abc', undefined)).toEqual([{ text: 'abc', hit: false }]);
  });

  it('reassembles to the original string', () => {
    const runs = highlightRuns('New Folder', [0, 4, 5]);
    expect(runs.map((r) => r.text).join('')).toBe('New Folder');
  });
});
