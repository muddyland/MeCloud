import { describe, it, expect } from 'vitest';
import {
  todayBounds, isToday, eventEnd, compareEvents, summariseFiles, summariseMail,
  recentFiles, recentNotes, senderLabel,
} from '$lib/dashboard.js';

// A fixed "now" so these do not drift with the wall clock.
const NOW = new Date(2026, 4, 15, 14, 30);        // 15 May 2026, 14:30 local
const at = (h, m = 0, day = 15) => new Date(2026, 4, day, h, m).toISOString();

describe('todayBounds', () => {
  it('spans local midnight to midnight', () => {
    const { after, before } = todayBounds(NOW);
    expect(new Date(after).getDate()).toBe(15);
    expect(new Date(after).getHours()).toBe(0);
    expect(new Date(before).getDate()).toBe(16);
  });
});

describe('eventEnd', () => {
  it('prefers an explicit end', () => {
    expect(eventEnd({ start: at(9), end: at(10) }).getHours()).toBe(10);
  });

  it('derives an end from an ISO-8601 duration', () => {
    expect(eventEnd({ start: at(9), duration: 'PT1H30M' }).getHours()).toBe(10);
    expect(eventEnd({ start: at(9), duration: 'PT1H30M' }).getMinutes()).toBe(30);
    expect(eventEnd({ start: at(9), duration: 'P1D' }).getDate()).toBe(16);
  });

  it('falls back to the start when the duration is absent or unparseable', () => {
    expect(eventEnd({ start: at(9) }).getHours()).toBe(9);
    expect(eventEnd({ start: at(9), duration: 'nonsense' }).getHours()).toBe(9);
  });

  it('returns null without a usable start', () => {
    expect(eventEnd({})).toBeNull();
    expect(eventEnd({ start: 'not-a-date' })).toBeNull();
  });
});

describe('isToday', () => {
  it('includes an event starting today', () => {
    expect(isToday({ start: at(9) }, NOW)).toBe(true);
  });

  it('includes a multi-day event on its middle day', () => {
    // Starting yesterday and ending tomorrow is still "on" today — checking
    // only the start date would drop it from the agenda.
    expect(isToday({ start: at(9, 0, 14), duration: 'P2D' }, NOW)).toBe(true);
  });

  it('excludes yesterday and tomorrow', () => {
    expect(isToday({ start: at(9, 0, 14) }, NOW)).toBe(false);
    expect(isToday({ start: at(9, 0, 16) }, NOW)).toBe(false);
  });

  it('includes an event ending exactly at midnight tonight', () => {
    expect(isToday({ start: at(23), duration: 'PT1H' }, NOW)).toBe(true);
  });

  it('rejects a malformed event rather than throwing', () => {
    expect(isToday({}, NOW)).toBe(false);
    expect(isToday({ start: 'nope' }, NOW)).toBe(false);
    expect(isToday(null, NOW)).toBe(false);
  });
});

describe('compareEvents', () => {
  it('sorts by start time', () => {
    const list = [{ start: at(15) }, { start: at(9) }, { start: at(12) }];
    expect(list.sort(compareEvents).map((e) => new Date(e.start).getHours()))
      .toEqual([9, 12, 15]);
  });

  it('puts all-day entries first', () => {
    const list = [{ start: at(9) }, { start: at(0), showWithoutTime: true }];
    expect(list.sort(compareEvents)[0].showWithoutTime).toBe(true);
  });
});

describe('summariseMail', () => {
  const boxes = [
    { role: 'inbox', name: 'Inbox', unreadEmails: 3, totalEmails: 120 },
    { role: 'drafts', name: 'Drafts', unreadEmails: 0, totalEmails: 2 },
    { role: null, name: 'Work', unreadEmails: 5, totalEmails: 40 },
  ];

  it('reports inbox and account-wide figures separately', () => {
    const s = summariseMail(boxes);
    expect(s.unread).toBe(3);          // inbox only — what the tile shows
    expect(s.unreadAll).toBe(8);       // every folder
    expect(s.inboxTotal).toBe(120);
    expect(s.total).toBe(162);
    expect(s.drafts).toBe(2);
    expect(s.mailboxes).toBe(3);
  });

  it('copes with an account that has no inbox role', () => {
    const s = summariseMail([{ role: null, totalEmails: 5, unreadEmails: 1 }]);
    expect(s.unread).toBe(0);
    expect(s.total).toBe(5);
  });

  it('handles an empty account', () => {
    expect(summariseMail([])).toMatchObject({ unread: 0, total: 0, mailboxes: 0 });
  });
});

describe('summariseFiles', () => {
  const nodes = [
    { id: 'n', name: 'Notes', parentId: null, blobId: null },
    { id: 'a', name: 'a.md', parentId: 'n', blobId: 'b1', size: 100 },
    { id: 'b', name: 'b.md', parentId: 'n', blobId: 'b2', size: 200 },
    { id: 'd', name: 'Docs', parentId: null, blobId: null },
    { id: 'c', name: 'c.pdf', parentId: 'd', blobId: 'b3', size: 700 },
  ];

  it('counts files and folders separately and sums bytes', () => {
    const s = summariseFiles(nodes);
    expect(s.files).toBe(3);
    expect(s.folders).toBe(2);
    expect(s.bytes).toBe(1000);
  });

  it('counts notes from the Notes folder', () => {
    expect(summariseFiles(nodes).notes).toBe(2);
  });

  it('falls back to counting markdown anywhere when there is no Notes folder', () => {
    const loose = [{ id: 'x', name: 'stray.md', parentId: null, blobId: 'b', size: 1 }];
    expect(summariseFiles(loose).notes).toBe(1);
  });

  it('treats a missing size as zero rather than NaN', () => {
    const s = summariseFiles([{ id: 'a', name: 'a.txt', blobId: 'b' }]);
    expect(s.bytes).toBe(0);
  });

  it('handles an empty drive', () => {
    expect(summariseFiles([])).toMatchObject({ files: 0, folders: 0, bytes: 0, notes: 0 });
  });
});

describe('recentFiles', () => {
  // Deliberately out of order, so a passing test means it sorted rather than
  // that it happened to keep the input order.
  const nodes = [
    { id: 'n', name: 'Notes', parentId: null, blobId: null },
    { id: 'note', name: 'a.md', parentId: 'n', blobId: 'b1', size: 10, modified: at(12) },
    { id: 'sub', name: 'attachments', parentId: 'n', blobId: null },
    { id: 'img', name: 'shot.png', parentId: 'sub', blobId: 'b2', size: 20, modified: at(13) },
    { id: 'd', name: 'Docs', parentId: null, blobId: null, modified: at(13, 30) },
    { id: 'old', name: 'old.pdf', parentId: 'd', blobId: 'b3', size: 30, modified: at(9) },
    { id: 'new', name: 'new.pdf', parentId: 'd', blobId: 'b4', size: 40, modified: at(11) },
  ];

  it('lists files newest first', () => {
    expect(recentFiles(nodes).map((n) => n.id)).toEqual(['new', 'old']);
  });

  it('leaves folders out', () => {
    // "Docs" is the most recently touched node of all and still must not show:
    // a folder's timestamp moves whenever anything lands in it.
    expect(recentFiles(nodes).some((n) => n.id === 'd')).toBe(false);
  });

  it('leaves the whole Notes subtree out, attachments included', () => {
    const ids = recentFiles(nodes).map((n) => n.id);
    expect(ids).not.toContain('note');
    expect(ids).not.toContain('img');
  });

  it('honours the limit', () => {
    expect(recentFiles(nodes, { limit: 1 })).toHaveLength(1);
  });

  it('does not reorder the caller\'s array', () => {
    const input = [...nodes];
    recentFiles(input);
    expect(input.map((n) => n.id)).toEqual(nodes.map((n) => n.id));
  });

  it('sorts a node with no modified date last rather than throwing', () => {
    const undated = [
      { id: 'u', name: 'u.txt', parentId: null, blobId: 'b' },
      { id: 't', name: 't.txt', parentId: null, blobId: 'b', modified: at(9) },
    ];
    expect(recentFiles(undated).map((n) => n.id)).toEqual(['t', 'u']);
  });
});

describe('recentNotes', () => {
  const nodes = [
    { id: 'n', name: 'Notes', parentId: null, blobId: null },
    { id: 'a', name: 'a.md', parentId: 'n', blobId: 'b1', modified: at(9) },
    { id: 'b', name: 'b.md', parentId: 'n', blobId: 'b2', modified: at(14) },
    { id: 'c', name: 'c.pdf', parentId: 'n', blobId: 'b3', modified: at(15) },
  ];

  it('lists notes newest first and ignores non-Markdown', () => {
    expect(recentNotes(nodes).map((n) => n.id)).toEqual(['b', 'a']);
  });

  it('falls back to Markdown anywhere when there is no Notes folder', () => {
    const loose = [{ id: 'x', name: 'stray.md', parentId: null, blobId: 'b', modified: at(9) }];
    expect(recentNotes(loose).map((n) => n.id)).toEqual(['x']);
  });

  it('honours the limit', () => {
    expect(recentNotes(nodes, { limit: 1 }).map((n) => n.id)).toEqual(['b']);
  });

  it('handles an empty drive', () => {
    expect(recentNotes([])).toEqual([]);
  });
});

describe('senderLabel', () => {
  it('prefers the display name', () => {
    expect(senderLabel({ from: [{ name: 'Ada', email: 'ada@example.com' }] })).toBe('Ada');
  });

  it('falls back to the address when there is no name', () => {
    expect(senderLabel({ from: [{ email: 'ada@example.com' }] })).toBe('ada@example.com');
    expect(senderLabel({ from: [{ name: '   ', email: 'ada@example.com' }] }))
      .toBe('ada@example.com');
  });

  it('says so rather than rendering blank when From is missing', () => {
    expect(senderLabel({})).toBe('Unknown sender');
    expect(senderLabel({ from: [] })).toBe('Unknown sender');
  });
});
