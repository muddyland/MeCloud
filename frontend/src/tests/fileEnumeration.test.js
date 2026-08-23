import { describe, it, expect, vi, beforeEach } from 'vitest';

// vi.hoisted so the spy itself is the module export -- a forwarding wrapper
// gets called by the mock machinery with no arguments as well as by the code
// under test, which makes the implementation see calls that never happened.
const { jmapPost } = vi.hoisted(() => ({ jmapPost: vi.fn() }));
vi.mock('$lib/api.js', () => ({ jmapPost }));

const { fetchFileNodes, maxObjectsInGet } = await import('$lib/files.js');

/*
 * The bug this covers, observed live rather than inferred:
 *
 *   FileNode/set  -> created: { probe: { id: "19" } }      (the folder is real)
 *   FileNode/get ids:null -> exactly 500 nodes, no node 19 (the listing is not)
 *   FileNode/set  -> notCreated: alreadyExists, existingId "19"
 *
 * RFC 8620 section 5.1 conditions `ids: null` on the record count staying under
 * maxObjectsInGet and says the server should answer requestTooLarge past it.
 * This one truncates instead, so the listing looks whole and is not — and the
 * missing folder still blocks its own name, which is what surfaced as "it says
 * it already exists but there is nothing there".
 */

const CAP = 500;
const session = { capabilities: { 'urn:ietf:params:jmap:core': { maxObjectsInGet: CAP } } };
const node = (id) => ({ id, name: `n${id}`, parentId: null, blobId: null });

beforeEach(() => {
  // mockClear plus an explicit default, rather than mockReset. With mockReset
  // the spy was observed being invoked with no arguments at all, which any
  // implementation that destructures its first argument then blows up on.
  // Giving every test a benign default and letting it install its own
  // implementation over the top sidesteps that entirely.
  jmapPost.mockClear();
  jmapPost.mockImplementation(async () => ({ methodResponses: [] }));
});

/** Serve `total` nodes through paged FileNode/query + FileNode/get. */
function servePaged(total, { cap = CAP } = {}) {
  const all = Array.from({ length: total }, (_, i) => node(String(i)));
  jmapPost.mockImplementation(async (calls) => {
    const [, args] = calls[0];
    const page = all.slice(args.position, args.position + Math.min(args.limit, cap));
    return {
      methodResponses: [
        ['FileNode/query', { ids: page.map((n) => n.id) }, 'q'],
        ['FileNode/get', { list: page }, 'g'],
      ],
    };
  });
  return all;
}

describe('fetchFileNodes', () => {
  it('pages past the per-request ceiling', async () => {
    servePaged(1200);
    const { nodes, incomplete } = await fetchFileNodes('m', session);
    expect(nodes).toHaveLength(1200);
    expect(incomplete).toBe(false);
    // The folder that used to fall off the end is now present.
    expect(nodes.some((n) => n.id === '600')).toBe(true);
  });

  it('stops cleanly when the drive fits in one page', async () => {
    servePaged(12);
    const { nodes } = await fetchFileNodes('m', session);
    expect(nodes).toHaveLength(12);
    expect(jmapPost).toHaveBeenCalledTimes(2);   // one full page, one empty
  });

  it('advances by what the server returned, not what was asked for', async () => {
    // A server free to cap `limit` below the request would otherwise leave
    // gaps, because stepping by pageSize skips whatever it did not send.
    servePaged(700, { cap: 100 });
    const { nodes } = await fetchFileNodes('m', session);
    expect(nodes).toHaveLength(700);
  });

  it('does not loop forever against a server that ignores position', async () => {
    const page = Array.from({ length: 10 }, (_, i) => node(String(i)));
    jmapPost.mockResolvedValue({
      methodResponses: [
        ['FileNode/query', { ids: page.map((n) => n.id) }, 'q'],
        ['FileNode/get', { list: page }, 'g'],
      ],
    });
    const { nodes } = await fetchFileNodes('m', session);
    expect(nodes).toHaveLength(10);
    expect(jmapPost.mock.calls.length).toBeLessThan(5);
  });

  it('falls back to ids:null when the server has no FileNode/query', async () => {
    jmapPost
      .mockRejectedValueOnce(Object.assign(new Error('unknown'), { type: 'unknownMethod' }))
      .mockResolvedValueOnce({
        methodResponses: [['FileNode/get', { list: [node('a'), node('b')] }, 'f']],
      });
    const { nodes, incomplete } = await fetchFileNodes('m', session);
    expect(nodes).toHaveLength(2);
    expect(incomplete).toBe(false);
  });

  it('reports incompleteness when the fallback lands exactly on the ceiling', async () => {
    // The live symptom: exactly 500 back, and no way to reach 501.
    jmapPost
      .mockRejectedValueOnce(new Error('no query'))
      .mockResolvedValueOnce({
        methodResponses: [['FileNode/get', {
          list: Array.from({ length: CAP }, (_, i) => node(String(i))),
        }, 'f']],
      });
    const { nodes, incomplete } = await fetchFileNodes('m', session);
    expect(nodes).toHaveLength(CAP);
    expect(incomplete).toBe(true);
  });
});

describe('maxObjectsInGet', () => {
  it('reads the advertised ceiling', () => {
    expect(maxObjectsInGet(session)).toBe(500);
  });

  it('is null when the server does not advertise one', () => {
    expect(maxObjectsInGet({ capabilities: {} })).toBeNull();
    expect(maxObjectsInGet(null)).toBeNull();
  });
});
