import { describe, it, expect, vi } from 'vitest';
import { createUploader } from '$lib/uploads.js';

function harness({ uploadFails = new Set(), folderFails = new Set(), existing = {} } = {}) {
  const calls = [];
  const rows = new Map();
  let nextRow = 0;
  let nextNode = 0;
  const created = [];
  const uploader = createUploader({
    uploadFile: async (file, { parentId, name, onProgress }) => {
      calls.push(['file', name, parentId]);
      onProgress(0.5);
      if (uploadFails.has(file.name)) throw new Error('boom');
      return { id: `n${++nextNode}`, name, parentId };
    },
    createFolder: async (name, parentId) => {
      calls.push(['dir', name, parentId]);
      return folderFails.has(name) ? null : { id: `d${++nextNode}`, name, parentId };
    },
    siblingNames: (parentId) => existing[parentId ?? 'root'] ?? [],
    onCreated: (n) => created.push(n),
    rows: {
      add: (row) => { const id = ++nextRow; rows.set(id, row); return id; },
      patch: (id, f) => rows.set(id, { ...rows.get(id), ...f }),
      remove: (id) => rows.delete(id),
    },
    rowLingerMs: 0,
  });
  return { uploader, calls, rows, created };
}

const file = (name) => ({ name });

describe('uploadBatch', () => {
  it('gives clashing names distinct names within the batch and against the folder', async () => {
    const h = harness({ existing: { root: ['a.txt'] } });
    await h.uploader.uploadBatch([file('a.txt'), file('a.txt')], null);
    const names = h.calls.map((c) => c[1]);
    expect(new Set(names).size).toBe(2);
    expect(names).not.toContain('a.txt');
  });

  it('counts failures and keeps the failed row with its error', async () => {
    const h = harness({ uploadFails: new Set(['bad.txt']) });
    const { failures } = await h.uploader.uploadBatch([file('ok.txt'), file('bad.txt')], 'p');
    expect(failures).toBe(1);
    const failed = [...h.rows.values()].find((r) => r.error);
    expect(failed).toMatchObject({ name: 'bad.txt', error: 'boom', done: true });
  });

  it('reports every created node', async () => {
    const h = harness();
    await h.uploader.uploadBatch([file('x'), file('y')], null);
    expect(h.created.map((n) => n.name)).toEqual(['x', 'y']);
  });
});

describe('uploadTree', () => {
  it('creates each folder before its contents, with the new id as parent', async () => {
    const h = harness();
    await h.uploader.uploadTree([
      { kind: 'dir', name: 'docs', children: [{ kind: 'file', name: 'a.md', file: file('a.md') }] },
    ], null);
    expect(h.calls[0]).toEqual(['dir', 'docs', null]);
    expect(h.calls[1][0]).toBe('file');
    expect(h.calls[1][2]).toBe(h.created[0].id);
  });

  it('stops a branch whose folder the server did not confirm', async () => {
    const h = harness({ folderFails: new Set(['docs']) });
    await expect(h.uploader.uploadTree([
      { kind: 'dir', name: 'docs', children: [{ kind: 'file', name: 'a.md', file: file('a.md') }] },
    ], null)).rejects.toThrow(/docs/);
    expect(h.calls.filter((c) => c[0] === 'file')).toHaveLength(0);
  });

  it('adds up failures from nested folders', async () => {
    const h = harness({ uploadFails: new Set(['b']) });
    const { failures } = await h.uploader.uploadTree([
      { kind: 'dir', name: 'd', children: [{ kind: 'file', name: 'b', file: file('b') }] },
      { kind: 'file', name: 'b', file: file('b') },
    ], null);
    expect(failures).toBe(2);
  });
});
