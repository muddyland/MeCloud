import { describe, it, expect } from 'vitest';
import {
  todayBounds, isToday, eventEnd, compareEvents, summariseFiles, summariseMail,
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
