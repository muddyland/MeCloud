import { describe, it, expect, vi, beforeEach } from 'vitest';

const { jmapPost } = vi.hoisted(() => ({ jmapPost: vi.fn() }));
vi.mock('#lib/api.js', () => ({ jmapPost, sessionExpired: vi.fn() }));
vi.mock('#lib/stores/activity.js', () => ({ begin: vi.fn(), end: vi.fn() }));

const {
  createFolder, createFile, updateNode, renameNode, fetchFileNodes, getFileNodesByIds,
  normalizeNode, encodeNodeProps, isDialectError, fileNodeDialect, resetFileNodeDialect,
  FILENODE_18, FILENODE_LEGACY,
} = await import('#lib/files.js');

/*
 * Stalwart 0.16 and 1.0 speak different drafts of JMAP File Storage, and the
 * move between them broke every FileNode write MeCloud makes. These fakes
 * answer the way each server was observed to:
 *
 *   0.16  create {name, parentId, blobId, type}     -> created
 *         create {..., "@type": "File"}             -> invalidProperties ["@type"]
 *         create {..., mediaType}                   -> invalidProperties ["mediaType"]
 *   1.0   create {name, parentId, ...} (no @type)   -> invalidProperties ["@type"]
 *         create {"@type": "File", ..., mediaType}  -> created
 *         create/update {..., type}                 -> invalidProperties ["type"]
 */

const session = { capabilities: { 'urn:ietf:params:jmap:core': {}, 'urn:ietf:params:jmap:filenode': {} } };

function fakeServer(draft) {
  const writes = [];
  jmapPost.mockImplementation(async (calls) => {
    const [method, args] = calls[0];
    if (method !== 'FileNode/set') return { methodResponses: [] };
    const out = {};
    for (const op of ['create', 'update']) {
      for (const [key, props] of Object.entries(args[op] ?? {})) {
        writes.push({ op, props });
        const bad = draft === '1.0'
          ? [...('type' in props ? ['type'] : []), ...(op === 'create' && !('@type' in props) ? ['@type'] : [])]
          : ['@type', 'mediaType'].filter((p) => p in props);
        if (bad.length) {
          out[op === 'create' ? 'notCreated' : 'notUpdated'] = {
            [key]: { type: 'invalidProperties', properties: bad },
          };
        } else if (op === 'create') {
          out.created = { [key]: { id: `id-${writes.length}` } };
        } else {
          out.updated = { [key]: null };
        }
      }
    }
    return { methodResponses: [['FileNode/set', out, 'f']] };
  });
  return writes;
}

beforeEach(() => {
  jmapPost.mockClear();
  jmapPost.mockImplementation(async () => ({ methodResponses: [] }));
  resetFileNodeDialect();
});

describe('FileNode writes on Stalwart 1.0 (filenode-18)', () => {
  it('creates folders and files with @type and mediaType on the first try', async () => {
    const writes = fakeServer('1.0');
    const folder = await createFolder('a', session, 'Docs');
    const file = await createFile('a', session, { name: 'x.md', blobId: 'B', type: 'text/markdown', size: 3 });
    expect(folder.id).toBeTruthy();
    expect(file.id).toBeTruthy();
    expect(writes.map((w) => w.props)).toEqual([
      { name: 'Docs', parentId: null, '@type': 'Directory' },
      { name: 'x.md', parentId: null, blobId: 'B', size: 3, '@type': 'File', mediaType: 'text/markdown' },
    ]);
    expect(file.type).toBe('text/markdown');
    expect(fileNodeDialect()).toBe(FILENODE_18);
  });

  it('saves new contents with mediaType, never type', async () => {
    const writes = fakeServer('1.0');
    await updateNode('a', session, 'n1', { blobId: 'B2', size: 4, type: 'text/markdown' });
    expect(writes).toEqual([{ op: 'update', props: { blobId: 'B2', size: 4, mediaType: 'text/markdown' } }]);
  });

  it('recovers when it had guessed the legacy draft', async () => {
    resetFileNodeDialect(FILENODE_LEGACY);
    const writes = fakeServer('1.0');
    const file = await createFile('a', session, { name: 'x', blobId: 'B', type: 'text/plain', size: 1 });
    expect(file.id).toBeTruthy();
    expect(writes).toHaveLength(2);
    expect(fileNodeDialect()).toBe(FILENODE_18);
  });
});

describe('FileNode writes on Stalwart 0.16 (legacy draft)', () => {
  it('falls back to the legacy shape once, then remembers it', async () => {
    const writes = fakeServer('0.16');
    const folder = await createFolder('a', session, 'Docs');
    expect(folder.id).toBeTruthy();
    expect(writes.map((w) => w.props)).toEqual([
      { name: 'Docs', parentId: null, '@type': 'Directory' },
      { name: 'Docs', parentId: null },
    ]);
    expect(fileNodeDialect()).toBe(FILENODE_LEGACY);

    await createFile('a', session, { name: 'x', blobId: 'B', type: 'text/plain', size: 1 });
    await updateNode('a', session, 'n1', { blobId: 'B2', size: 2, type: 'text/plain' });
    expect(writes.slice(2).map((w) => w.props)).toEqual([
      { name: 'x', parentId: null, blobId: 'B', size: 1, type: 'text/plain' },
      { blobId: 'B2', size: 2, type: 'text/plain' },
    ]);
  });

  it('a rename is the same on both drafts and teaches nothing', async () => {
    const writes = fakeServer('0.16');
    await renameNode('a', session, 'n1', 'new name');
    expect(writes).toEqual([{ op: 'update', props: { name: 'new name' } }]);
    expect(fileNodeDialect()).toBeNull();
  });

  it('still reports a real refusal', async () => {
    jmapPost.mockImplementation(async () => ({
      methodResponses: [['FileNode/set', { notCreated: { nf: { type: 'alreadyExists', existingId: 'X' } } }, 'f']],
    }));
    await expect(createFolder('a', session, 'Docs')).rejects.toThrow();
    expect(jmapPost).toHaveBeenCalledTimes(1);
  });
});

describe('reading nodes from either draft', () => {
  it('exposes the 1.0 mediaType as type', () => {
    expect(normalizeNode({ id: '1', '@type': 'File', mediaType: 'image/png', blobId: 'B' }).type).toBe('image/png');
    expect(fileNodeDialect()).toBe(FILENODE_18);
  });

  it('leaves 0.16 nodes alone and learns the legacy draft', () => {
    const n = { id: '1', nodeType: 'file', type: 'image/png', blobId: 'B' };
    expect(normalizeNode(n)).toBe(n);
    expect(fileNodeDialect()).toBe(FILENODE_LEGACY);
  });

  it('normalises listings and by-id fetches', async () => {
    jmapPost.mockImplementation(async (calls) => {
      if (calls[0][0] === 'FileNode/query') {
        const first = calls[0][1].position === 0;
        return { methodResponses: [
          ['FileNode/query', { ids: first ? ['1'] : [] }, 'q'],
          ['FileNode/get', { list: first ? [{ id: '1', '@type': 'File', mediaType: 'text/plain', blobId: 'B' }] : [] }, 'g'],
        ] };
      }
      return { methodResponses: [['FileNode/get', { list: [{ id: '2', '@type': 'File', mediaType: 'image/jpeg' }], notFound: [] }, 'f']] };
    });
    const { nodes } = await fetchFileNodes('a', session);
    expect(nodes[0].type).toBe('text/plain');
    const { list } = await getFileNodesByIds('a', session, ['2']);
    expect(list[0].type).toBe('image/jpeg');
  });
});

describe('encoding helpers', () => {
  it('encodes for each draft', () => {
    const props = { name: 'n', blobId: 'B', type: 'text/plain' };
    expect(encodeNodeProps(props, { kind: 'file', dialect: FILENODE_18 }))
      .toEqual({ name: 'n', blobId: 'B', '@type': 'File', mediaType: 'text/plain' });
    expect(encodeNodeProps(props, { kind: 'file', dialect: FILENODE_LEGACY }))
      .toEqual({ name: 'n', blobId: 'B', type: 'text/plain' });
    expect(encodeNodeProps({ name: 'n' }, { dialect: FILENODE_18 })).toEqual({ name: 'n' });
  });

  it('recognises only dialect refusals', () => {
    expect(isDialectError({ type: 'invalidProperties', properties: ['@type'] })).toBe(true);
    expect(isDialectError({ type: 'invalidProperties', properties: ['type'] })).toBe(true);
    expect(isDialectError({ type: 'invalidProperties', properties: ['name'] })).toBe(false);
    expect(isDialectError({ type: 'alreadyExists' })).toBe(false);
    expect(isDialectError(null)).toBe(false);
  });
});
