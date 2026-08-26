import { describe, it, expect, vi, beforeEach } from 'vitest';

// vi.hoisted so the spy itself is the module export — see fileEnumeration.test.js.
const { jmapPost } = vi.hoisted(() => ({ jmapPost: vi.fn() }));
vi.mock('$lib/api.js', () => ({ jmapPost }));

const { getDashboardSummary } = await import('$lib/dashboard.js');

/*
 * The dashboard had the same bug the Files listing had: FileNode/get with
 * `ids: null` comes back truncated at maxObjectsInGet, so every count on the
 * overview was capped at 500 no matter how big the drive was. These cover the
 * fix from the outside — what the dashboard asks for, and what it reports.
 */

const CAP = 500;
const INBOX = 'mb1';

const session = {
  capabilities: {
    'urn:ietf:params:jmap:core': { maxObjectsInGet: CAP },
    'urn:ietf:params:jmap:mail': {},
    'urn:ietf:params:jmap:filenode': {},
  },
};

const MAILBOXES = [
  { id: INBOX, name: 'Inbox', role: 'inbox', unreadEmails: 42, totalEmails: 900 },
  { id: 'mb2', name: 'Drafts', role: 'drafts', unreadEmails: 0, totalEmails: 3 },
];

/** A drive of `count` files in one folder, plus the folder itself. */
function drive(count) {
  const nodes = [{ id: 'root', name: 'Docs', parentId: null, blobId: null }];
  for (let i = 0; i < count; i += 1) {
    nodes.push({
      id: `f${i}`, name: `file-${i}.txt`, parentId: 'root',
      blobId: `b${i}`, size: 10, modified: `2026-05-${String((i % 28) + 1).padStart(2, '0')}T00:00:00Z`,
    });
  }
  return nodes;
}

/** Answer each method call the dashboard makes, paging FileNode/query properly. */
function serve(nodes, { unread = [] } = {}) {
  jmapPost.mockImplementation(async (calls) => {
    const [name, args] = calls[0];

    if (name === 'Mailbox/get') {
      const responses = [['Mailbox/get', { list: MAILBOXES }, 'mb']];
      return { methodResponses: responses };
    }

    if (name === 'FileNode/query') {
      const page = nodes.slice(args.position, args.position + Math.min(args.limit, CAP));
      return {
        methodResponses: [
          ['FileNode/query', { ids: page.map((n) => n.id) }, 'q'],
          ['FileNode/get', { list: page }, 'g'],
        ],
      };
    }

    if (name === 'Email/query') {
      return {
        methodResponses: [
          ['Email/query', { ids: unread.map((e) => e.id) }, 'uq'],
          ['Email/get', { list: unread }, 'ug'],
        ],
      };
    }

    return { methodResponses: [] };
  });
}

/** Every method call the dashboard issued, flattened. */
function methodCalls() {
  return jmapPost.mock.calls.flatMap(([calls]) => calls ?? []);
}

beforeEach(() => {
  jmapPost.mockClear();
  jmapPost.mockImplementation(async () => ({ methodResponses: [] }));
});

describe('getDashboardSummary — files', () => {
  it('counts a drive larger than one request can carry', async () => {
    serve(drive(1200));
    const summary = await getDashboardSummary('acct', session);

    // 500 here would be the old bug: the server's per-call ceiling reported as
    // if it were the size of the drive.
    expect(summary.files.files).toBe(1200);
    expect(summary.files.folders).toBe(1);
    expect(summary.files.bytes).toBe(12000);
    expect(summary.files.incomplete).toBe(false);
  });

  it('never asks for every node in one call', async () => {
    serve(drive(600));
    await getDashboardSummary('acct', session);

    const gets = methodCalls().filter(([name]) => name === 'FileNode/get');
    expect(gets.length).toBeGreaterThan(0);
    for (const [, args] of gets) expect(args.ids).not.toBeNull();
  });

  it('reports the newest files, newest first, without folders', async () => {
    serve([
      { id: 'd', name: 'Docs', parentId: null, blobId: null, modified: '2026-05-20T00:00:00Z' },
      { id: 'a', name: 'a.txt', parentId: 'd', blobId: 'b1', size: 1, modified: '2026-05-01T00:00:00Z' },
      { id: 'b', name: 'b.txt', parentId: 'd', blobId: 'b2', size: 1, modified: '2026-05-10T00:00:00Z' },
    ]);
    const summary = await getDashboardSummary('acct', session);
    expect(summary.recentFiles.map((n) => n.id)).toEqual(['b', 'a']);
  });

  it('leaves files out entirely when the server has no file storage', async () => {
    const noFiles = { capabilities: { 'urn:ietf:params:jmap:mail': {} } };
    serve(drive(3));
    const summary = await getDashboardSummary('acct', noFiles);

    expect(summary.files).toBeNull();
    expect(summary.recentFiles).toBeNull();
    expect(summary.recentNotes).toBeNull();
    expect(methodCalls().some(([name]) => name.startsWith('FileNode/'))).toBe(false);
  });
});

describe('getDashboardSummary — unread mail', () => {
  const unread = [
    { id: 'e1', subject: 'Older', from: [{ name: 'Ada', email: 'ada@example.com' }],
      receivedAt: '2026-05-14T09:00:00Z' },
    { id: 'e2', subject: 'Newer', from: [{ email: 'grace@example.com' }],
      receivedAt: '2026-05-15T09:00:00Z' },
  ];

  it('asks only for unread messages in the Inbox', async () => {
    serve(drive(2), { unread });
    await getDashboardSummary('acct', session);

    const [, args] = methodCalls().find(([name]) => name === 'Email/query');
    expect(args.filter).toMatchObject({ inMailbox: INBOX, notKeyword: '$seen' });
  });

  it('returns them newest first even when /get does not', async () => {
    // /get is free to answer in any order (RFC 8620 section 5.1), so a list
    // captioned "newest first" has to sort rather than trust it.
    serve(drive(2), { unread });
    const summary = await getDashboardSummary('acct', session);
    expect(summary.unread.map((e) => e.id)).toEqual(['e2', 'e1']);
  });

  it('leaves the rest of the overview standing when the message list fails', async () => {
    serve(drive(2), { unread });
    const paged = jmapPost.getMockImplementation();
    jmapPost.mockImplementation(async (calls) => {
      if (calls[0][0] === 'Email/query') throw new Error('nope');
      return paged(calls);
    });

    const summary = await getDashboardSummary('acct', session);
    // null, not [] — an inbox with 42 unread and nothing listed is a failure,
    // and it must not be drawn as inbox zero.
    expect(summary.unread).toBeNull();
    expect(summary.mail.unread).toBe(42);
    expect(summary.files.files).toBe(2);
  });

  it('skips the request entirely when there is no inbox', async () => {
    jmapPost.mockImplementation(async (calls) => {
      const [name] = calls[0];
      if (name === 'Mailbox/get') {
        return { methodResponses: [['Mailbox/get', { list: [] }, 'mb']] };
      }
      return { methodResponses: [] };
    });

    const summary = await getDashboardSummary('acct', { capabilities: { 'urn:ietf:params:jmap:mail': {} } });
    expect(summary.unread).toEqual([]);
    expect(methodCalls().some(([name]) => name === 'Email/query')).toBe(false);
  });
});
