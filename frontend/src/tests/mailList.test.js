import { describe, it, expect } from 'vitest';
import {
  appendUnique, withThreadSizes, groupByThread, groupIds,
  conversationMessages, participants, navigationOrder, shownInConversation, cardSummary,
} from '../lib/mailList.js';

const msg = (id, threadId, at, extra = {}) => ({
  id, threadId, receivedAt: `2026-10-${String(at).padStart(2, '0')}T10:00:00Z`,
  keywords: { $seen: true }, from: [{ name: id.toUpperCase(), email: `${id}@x.test` }], ...extra,
});

describe('appendUnique / withThreadSizes', () => {
  it('skips repeats across a page boundary', () => {
    expect(appendUnique([msg('a', 't', 2)], [msg('a', 't', 2), msg('b', 'u', 1)]).map((e) => e.id))
      .toEqual(['a', 'b']);
  });
  it('defaults a missing thread to size 1', () => {
    expect(withThreadSizes([msg('a', 't', 1)], [])[0].threadSize).toBe(1);
  });
});

describe('groupByThread', () => {
  const list = [
    msg('c', 't1', 5, { threadSize: 3 }),
    msg('x', 't2', 4),
    msg('a', 't1', 3, { keywords: {} }),
    msg('y', undefined, 2),
  ];
  const groups = groupByThread(list);

  it('keeps the order of each conversation\'s newest message', () => {
    expect(groups.map((g) => g.key)).toEqual(['t1', 't2', 'solo:y']);
  });
  it('collects the listed messages newest first, with the newest as the face', () => {
    expect(groups[0].latest.id).toBe('c');
    expect(groupIds(groups[0])).toEqual(['c', 'a']);
  });
  it('sizes by the whole thread, and counts unread messages', () => {
    expect(groups[0].size).toBe(3);
    expect(groups[0].unread).toBe(1);
    expect(groups[1].size).toBe(1);
  });
  it('never merges messages without a thread id', () => {
    expect(groupByThread([msg('p', undefined, 1), msg('q', undefined, 1)])).toHaveLength(2);
  });
});

describe('conversationMessages', () => {
  const group = groupByThread([msg('c', 't', 5, { keywords: {} }), msg('a', 't', 1)])[0];

  it('falls back to the listed messages before the thread loads', () => {
    expect(conversationMessages(group).map((e) => e.id)).toEqual(['c', 'a']);
  });
  it('adds messages from other folders, newest first, preferring the list copy', () => {
    const thread = [msg('a', 't', 1), msg('sent', 't', 3, { mailboxIds: { sent: true } }), msg('c', 't', 5)];
    const out = conversationMessages(group, thread);
    expect(out.map((e) => e.id)).toEqual(['c', 'sent', 'a']);
    expect(out[0].keywords).toEqual({});      // the list copy, still unread
  });
  it('keeps a listed message the thread fetch did not return', () => {
    expect(conversationMessages(group, [msg('a', 't', 1)]).map((e) => e.id)).toEqual(['c', 'a']);
  });
});

describe('participants', () => {
  it('names each sender once, newest first, with your own address as Me', () => {
    const list = [
      msg('c', 't', 3, { from: [{ name: 'Tom', email: 'tom@x.test' }] }),
      msg('b', 't', 2, { from: [{ name: 'Ada', email: 'ADA@x.test' }] }),
      msg('a', 't', 1, { from: [{ name: 'Tom O', email: 'tom@x.test' }] }),
    ];
    expect(participants(list, 'ada@x.test')).toEqual(['Tom', 'Me']);
  });
});

describe('navigationOrder', () => {
  const groups = groupByThread([msg('c', 't', 5, { threadSize: 2 }), msg('a', 't', 1), msg('x', 'u', 4)]);
  it('stops once on a collapsed conversation', () => {
    expect(navigationOrder(groups, new Set(), new Map()).map((e) => e.id)).toEqual(['c', 'x']);
  });
  it('skips messages the conversation view hides', () => {
    const threads = new Map([['t', { list: [msg('a', 't', 1), msg('gone', 't', 3, { mailboxIds: { trash: true } }), msg('c', 't', 5)] }]]);
    const keep = (m) => !m.mailboxIds?.trash;
    expect(navigationOrder(groups, new Set(['t']), threads, keep).map((e) => e.id)).toEqual(['c', 'a', 'x']);
  });
  it('walks every message of an expanded one', () => {
    expect(navigationOrder(groups, new Set(['t']), new Map()).map((e) => e.id)).toEqual(['c', 'a', 'x']);
  });
});

describe('shownInConversation', () => {
  const hidden = new Set(['trash', 'junk']);
  it('leaves out a copy that is only in Trash or Junk', () => {
    expect(shownInConversation({ mailboxIds: { trash: true } }, 'inbox', hidden)).toBe(false);
  });
  it('shows it when Trash is the folder being viewed', () => {
    expect(shownInConversation({ mailboxIds: { trash: true } }, 'trash', hidden)).toBe(true);
  });
  it('shows Sent copies and messages without mailbox data', () => {
    expect(shownInConversation({ mailboxIds: { sent: true } }, 'inbox', hidden)).toBe(true);
    expect(shownInConversation({}, 'inbox', hidden)).toBe(true);
  });
});

describe('thread members and the collapsed card', () => {
  const members = [
    msg('a', 't', 1, { mailboxIds: { inbox: true }, from: [{ name: 'Tom', email: 'tom@x.test' }] }),
    msg('r', 't', 2, { mailboxIds: { sent: true }, from: [{ name: 'Ada', email: 'ada@x.test' }] }),
    msg('d', 't', 3, { mailboxIds: { trash: true }, from: [{ name: 'Tom', email: 'tom@x.test' }] }),
    msg('c', 't', 4, { mailboxIds: { inbox: true }, from: [{ name: 'Tom', email: 'tom@x.test' }] }),
  ];
  const list = withThreadSizes([msg('c', 't', 4), msg('a', 't', 1)], [{ id: 't', emailIds: ['a', 'r', 'd', 'c'] }], members);

  it('attaches the members newest first', () => {
    expect(list[0].threadSize).toBe(4);
    expect(list[0].threadMembers.map((m) => m.id)).toEqual(['c', 'd', 'r', 'a']);
  });
  it('counts and names what the open conversation will show', () => {
    const group = groupByThread(list)[0];
    const keep = (m) => shownInConversation(m, 'inbox', new Set(['trash']));
    expect(cardSummary(group, keep, 'ada@x.test')).toEqual({ count: 3, people: ['Tom', 'Me'] });
  });
  it('falls back to the thread size without member data', () => {
    const group = groupByThread(withThreadSizes([msg('c', 't', 4)], [{ id: 't', emailIds: ['a', 'c'] }]))[0];
    expect(cardSummary(group)).toEqual({ count: 2, people: ['C'] });
  });
});
