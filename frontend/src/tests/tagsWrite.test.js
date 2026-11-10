import { describe, it, expect, vi, beforeEach } from 'vitest';

const { jmapPost } = vi.hoisted(() => ({ jmapPost: vi.fn() }));
vi.mock('#lib/api.js', () => ({ jmapPost, sessionExpired: vi.fn() }));
vi.mock('#lib/stores/activity.js', () => ({ begin: vi.fn(), end: vi.fn() }));

// Blob transfer is XHR and fetch against the backend; the fake server below
// keeps blobs in memory instead. Everything else in files.js stays real, so
// the FileNode/set calls tags.js makes are the ones the app would send.
const { blobs, counter } = vi.hoisted(() => ({ blobs: new Map(), counter: { n: 0 } }));
vi.mock('#lib/files.js', async (importOriginal) => ({
  ...(await importOriginal()),
  uploadBlob: vi.fn(async (file) => {
    const text = await file.text();
    const blobId = `blob-${++counter.n}`;
    blobs.set(blobId, text);
    return { blobId, size: new TextEncoder().encode(text).length, type: file.type };
  }),
  fetchBlobBytes: vi.fn(async (node) => new TextEncoder().encode(blobs.get(node.blobId)).buffer),
}));

const { JmapMethodError } = await import('#lib/jmapErrors.js');
const { resetFileNodeDialect } = await import('#lib/files.js');
const {
  setTags, loadTagIndex, serializeIndex, parseIndex, TagIndexError,
  GAVE_UP_MESSAGE, MAX_WRITE_ATTEMPTS, RESERVED_FOLDER, TAG_INDEX_NAME,
} = await import('#lib/tags.js');

const session = { capabilities: { 'urn:ietf:params:jmap:core': {}, 'urn:ietf:params:jmap:filenode': {} } };

/**
 * A FileNode server with a real state string: every successful write moves it
 * on, and a /set whose ifInState is stale is refused with stateMismatch, the
 * way RFC 8620 section 5.3 says. `beforeSet` lets a test play a second client
 * writing in between our read and our write.
 */
let server;
function resetServer() {
  server = { state: 1, nodes: new Map(), seq: 0, sets: [], beforeSet: null };
}

function addNode(node) {
  server.nodes.set(node.id, node);
  server.state += 1;
  return node;
}

function putIndex(text, { folderId = 'm', fileId = 't' } = {}) {
  if (!server.nodes.has(folderId)) addNode({ id: folderId, name: RESERVED_FOLDER, parentId: null, blobId: null });
  const blobId = `seed-${++counter.n}`;
  blobs.set(blobId, text);
  const existing = server.nodes.get(fileId);
  addNode({ ...(existing ?? { id: fileId, name: TAG_INDEX_NAME, parentId: folderId, type: 'application/json' }), blobId, size: text.length });
}

function readIndex() {
  const file = [...server.nodes.values()].find((n) => n.name === TAG_INDEX_NAME);
  return file ? blobs.get(file.blobId) : null;
}

beforeEach(() => {
  resetServer();
  resetFileNodeDialect();
  blobs.clear();
  jmapPost.mockReset();
  jmapPost.mockImplementation(async (calls) => {
    const [method, args] = calls[0];
    if (method === 'FileNode/get') {
      const ids = args.ids ?? [...server.nodes.keys()];
      return { methodResponses: [['FileNode/get', {
        state: String(server.state),
        list: ids.filter((id) => server.nodes.has(id)).map((id) => ({ ...server.nodes.get(id) })),
        notFound: ids.filter((id) => !server.nodes.has(id)),
      }, 'g']] };
    }
    if (method === 'FileNode/query') throw new Error('no query here');
    if (method !== 'FileNode/set') return { methodResponses: [] };

    server.sets.push(args);
    server.beforeSet?.();
    if (args.ifInState && args.ifInState !== String(server.state)) {
      throw new JmapMethodError({ type: 'stateMismatch' }, 'f');
    }
    const out = {};
    for (const [key, props] of Object.entries(args.create ?? {})) {
      const clash = [...server.nodes.values()].find(
        (n) => (n.parentId ?? null) === (props.parentId ?? null) && n.name === props.name,
      );
      if (clash) {
        out.notCreated = { [key]: { type: 'alreadyExists', existingId: clash.id } };
        continue;
      }
      const node = addNode({ id: `n${++server.seq}`, blobId: null, ...props });
      out.created = { [key]: { id: node.id } };
    }
    for (const [id, patch] of Object.entries(args.update ?? {})) {
      if (!server.nodes.has(id)) { out.notUpdated = { [id]: { type: 'notFound' } }; continue; }
      addNode({ ...server.nodes.get(id), ...patch });
      out.updated = { [id]: null };
    }
    return { methodResponses: [['FileNode/set', out, 'f']] };
  });
});

const listing = () => [...server.nodes.values()];

describe('setTags', () => {
  it('creates .mecloud and tags.json on the first write, conditionally', async () => {
    addNode({ id: 'f1', name: 'policy.pdf', parentId: null, blobId: 'x' });
    const result = await setTags('a', session, ['f1'], { add: ['Tax'] }, { nodes: listing(), incomplete: false });

    const folder = listing().find((n) => n.name === RESERVED_FOLDER);
    const file = listing().find((n) => n.name === TAG_INDEX_NAME);
    expect(folder.parentId).toBeNull();
    expect(file.parentId).toBe(folder.id);
    expect(file.mediaType).toBe('application/json');
    expect(readIndex()).toBe('{\n  "version": 1,\n  "tags": {\n    "f1": [\n      "Tax"\n    ]\n  }\n}\n');
    expect(result.index.tags.get('f1')).toEqual(['Tax']);
    expect(result.node.id).toBe(file.id);
    // Both writes carried the state they were based on.
    expect(server.sets).toHaveLength(2);
    for (const set of server.sets) expect(set.ifInState).toBeTruthy();
  });

  it('retries on stateMismatch and keeps the other client\'s change', async () => {
    putIndex(JSON.stringify({ version: 1, tags: { f1: ['Tax'] } }));
    let raced = false;
    server.beforeSet = () => {
      if (raced) return;
      raced = true;
      // Another device tags f2 between our read and our write.
      putIndex(serializeIndex(parseIndex(JSON.stringify({ tags: { f1: ['Tax'], f2: ['Home'] } }))));
    };

    const result = await setTags('a', session, ['f1'], { add: ['2026'] }, { nodes: listing() });
    const saved = JSON.parse(readIndex());
    expect(saved.tags).toEqual({ f1: ['Tax', '2026'], f2: ['Home'] });
    expect(result.index.tags.get('f2')).toEqual(['Home']);
    expect(server.sets).toHaveLength(2);
    expect(server.sets[0].ifInState).not.toBe(server.sets[1].ifInState);
  });

  it(`gives up after ${MAX_WRITE_ATTEMPTS} conflicts with a clear error`, async () => {
    putIndex('{"version":1,"tags":{}}');
    server.beforeSet = () => { server.state += 1; };
    const err = await setTags('a', session, ['f1'], { add: ['Tax'] }, { nodes: listing() }).catch((e) => e);
    expect(err).toBeInstanceOf(TagIndexError);
    expect(err.message).toBe(GAVE_UP_MESSAGE);
    expect(server.sets).toHaveLength(MAX_WRITE_ATTEMPTS);
  });

  it('merges into an index another client created that the listing did not show', async () => {
    putIndex(JSON.stringify({ version: 1, tags: { f2: ['Home'] } }));
    // An empty listing: as far as this tab knows there is no index yet.
    await setTags('a', session, ['f1'], { add: ['Tax'] }, { nodes: [] });
    expect(JSON.parse(readIndex()).tags).toEqual({ f1: ['Tax'], f2: ['Home'] });
    expect(listing().filter((n) => n.name === RESERVED_FOLDER)).toHaveLength(1);
    expect(listing().filter((n) => n.name === TAG_INDEX_NAME)).toHaveLength(1);
  });

  it('refuses to overwrite a malformed index', async () => {
    putIndex('{ this is not json');
    const err = await setTags('a', session, ['f1'], { add: ['Tax'] }, { nodes: listing() }).catch((e) => e);
    expect(err).toBeInstanceOf(TagIndexError);
    expect(err.readOnly).toBe(true);
    expect(server.sets).toHaveLength(0);
    expect(readIndex()).toBe('{ this is not json');
  });

  it('refuses to write an index from a newer version', async () => {
    const text = JSON.stringify({ version: 2, tags: { f1: ['Tax'] } });
    putIndex(text);
    const err = await setTags('a', session, ['f1'], { add: ['Home'] }, { nodes: listing() }).catch((e) => e);
    expect(err.readOnly).toBe(true);
    expect(server.sets).toHaveLength(0);
    expect(readIndex()).toBe(text);
  });

  it('preserves unknown top-level keys', async () => {
    putIndex(JSON.stringify({ version: 1, tags: {}, colours: { Tax: 'red' } }));
    await setTags('a', session, ['f1'], { add: ['Tax'] }, { nodes: listing() });
    expect(JSON.parse(readIndex())).toEqual({ version: 1, tags: { f1: ['Tax'] }, colours: { Tax: 'red' } });
  });

  it('does not write when nothing changes', async () => {
    putIndex(serializeIndex(parseIndex(JSON.stringify({ tags: { f1: ['Tax'] } }))));
    await setTags('a', session, ['f1'], { add: ['tax'] }, { nodes: listing() });
    expect(server.sets).toHaveLength(0);
  });

  describe('pruning', () => {
    beforeEach(() => {
      addNode({ id: 'f1', name: 'a.txt', parentId: null, blobId: 'x' });
      putIndex(JSON.stringify({ tags: { f1: ['Tax'], gone: ['Old'], fresh: ['New'] } }));
    });

    it('drops deleted files from a complete listing, once the server confirms they are gone', async () => {
      const seen = listing();
      // Added and tagged by another client after this tab's listing was taken.
      addNode({ id: 'fresh', name: 'b.txt', parentId: null, blobId: 'y' });
      await setTags('a', session, ['f1'], { add: ['2026'] }, { nodes: seen, incomplete: false });
      expect(JSON.parse(readIndex()).tags).toEqual({ f1: ['Tax', '2026'], fresh: ['New'] });
    });

    it('never prunes from an incomplete listing', async () => {
      await setTags('a', session, ['f1'], { add: ['2026'] }, { nodes: listing(), incomplete: true });
      expect(Object.keys(JSON.parse(readIndex()).tags).sort()).toEqual(['f1', 'fresh', 'gone']);
    });
  });
});

describe('loadTagIndex', () => {
  it('reads the index found in the listing', async () => {
    putIndex(JSON.stringify({ version: 1, tags: { f1: ['Tax'] } }));
    const { index, node } = await loadTagIndex('a', session, { nodes: listing() });
    expect(index.tags.get('f1')).toEqual(['Tax']);
    expect(node.name).toBe(TAG_INDEX_NAME);
  });

  it('is empty and writable with no index, and fetches the listing itself if not given one', async () => {
    const { index, node } = await loadTagIndex('a', session);
    expect(index.tags.size).toBe(0);
    expect(index.writable).toBe(true);
    expect(node).toBeNull();
  });

  it('reports a malformed index instead of throwing', async () => {
    putIndex('[1,2,3]');
    const { index } = await loadTagIndex('a', session, { nodes: listing() });
    expect(index.writable).toBe(false);
    expect(index.error).toBeTruthy();
  });
});
