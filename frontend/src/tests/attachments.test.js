import { describe, it, expect } from 'vitest';
import { selectDriveAttachments } from '$lib/attachments.js';

const node = (over = {}) =>
  ({ id: 'n1', name: 'a.png', blobId: 'b1', type: 'image/png', size: 100, ...over });

describe('selectDriveAttachments', () => {
  it('turns picked nodes into finished attachment rows', () => {
    const { rows, skipped, duplicates } = selectDriveAttachments([node()]);
    expect(rows).toEqual([{ name: 'a.png', type: 'image/png', size: 100, blobId: 'b1' }]);
    expect(skipped).toBe(0);
    expect(duplicates).toBe(0);
  });

  it('handles an absent or empty selection', () => {
    expect(selectDriveAttachments(null).rows).toEqual([]);
    expect(selectDriveAttachments([]).rows).toEqual([]);
  });

  it('skips folders and nodes with no blob', () => {
    // A folder is exactly "no blobId", and has nothing to attach.
    const rows = selectDriveAttachments([
      node({ blobId: null, name: 'Photos' }),
      node({ blobId: undefined }),
      node({ blobId: 'b2', name: 'ok.png' }),
    ]).rows;
    expect(rows.map((r) => r.name)).toEqual(['ok.png']);
  });

  it('refuses a file already on the message', () => {
    const { rows, duplicates } = selectDriveAttachments([node({ blobId: 'b1' })], {
      attachedBlobIds: new Set(['b1']),
    });
    expect(rows).toEqual([]);
    expect(duplicates).toBe(1);
  });

  it('refuses the same blob twice within one selection', () => {
    // Reachable through search from two folders, or named twice in a batch.
    const { rows, duplicates } = selectDriveAttachments([
      node({ id: 'n1', blobId: 'b1' }),
      node({ id: 'n2', blobId: 'b1' }),
    ]);
    expect(rows).toHaveLength(1);
    expect(duplicates).toBe(1);
  });

  it('stops at the size budget, counting what is already attached', () => {
    const { rows, skipped } = selectDriveAttachments(
      [node({ blobId: 'b1', size: 60 })],
      { usedBytes: 50, maxTotalBytes: 100 },
    );
    expect(rows).toEqual([]);
    expect(skipped).toBe(1);
  });

  it('lets a smaller file through after an oversized one is refused', () => {
    // Spending the budget in order, rather than bailing out, keeps one huge
    // file from starving everything selected behind it.
    const { rows, skipped } = selectDriveAttachments([
      node({ blobId: 'big', name: 'big.png', size: 900 }),
      node({ blobId: 'small', name: 'small.png', size: 10 }),
    ], { maxTotalBytes: 100 });
    expect(rows.map((r) => r.name)).toEqual(['small.png']);
    expect(skipped).toBe(1);
  });

  it('treats an unreported size as zero rather than NaN', () => {
    const { rows } = selectDriveAttachments(
      [node({ size: undefined }), node({ blobId: 'b2', size: 'nonsense' })],
      { maxTotalBytes: 10 },
    );
    expect(rows.map((r) => r.size)).toEqual([0, 0]);
  });

  it('falls back to opaque bytes and a placeholder name', () => {
    const { rows } = selectDriveAttachments([{ blobId: 'b9' }]);
    expect(rows[0]).toMatchObject({ name: 'attachment', type: 'application/octet-stream' });
  });
});
