import { describe, it, expect } from 'vitest';
import {
  formatBytes, extensionOf, fileKind, isPreviewable, validateName,
  breadcrumbTrail, compareNodes, uniqueName, canThumbnail, MAX_THUMBNAIL_BYTES,
} from '$lib/fileTypes.js';

const folder = (over = {}) => ({ id: 'f1', name: 'Docs', blobId: null, ...over });
const file = (over = {}) => ({ id: 'n1', name: 'a.txt', blobId: 'b1', type: 'text/plain', ...over });

describe('formatBytes', () => {
  it('formats across units', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(999)).toBe('999 B');
    expect(formatBytes(1024)).toBe('1.0 KB');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(1024 ** 2)).toBe('1.0 MB');
    expect(formatBytes(1024 ** 3 * 2.5)).toBe('2.5 GB');
  });

  it('renders unknown sizes as a dash rather than "0 B"', () => {
    // A folder has no size; claiming it is empty would be wrong.
    expect(formatBytes(null)).toBe('—');
    expect(formatBytes(undefined)).toBe('—');
    expect(formatBytes('')).toBe('—');
    expect(formatBytes(-1)).toBe('—');
    expect(formatBytes('nonsense')).toBe('—');
  });
});

describe('extensionOf', () => {
  it('extracts and lowercases', () => {
    expect(extensionOf('Report.PDF')).toBe('pdf');
    expect(extensionOf('archive.tar.gz')).toBe('gz');
  });

  it('returns empty for names without a usable extension', () => {
    expect(extensionOf('README')).toBe('');
    expect(extensionOf('.gitignore')).toBe('');   // dotfile, not an extension
    expect(extensionOf('trailing.')).toBe('');
    expect(extensionOf(null)).toBe('');
  });
});

describe('fileKind', () => {
  it('treats a null blobId as a folder, per the FileNode spec', () => {
    expect(fileKind(folder())).toBe('folder');
    expect(fileKind({ name: 'x' })).toBe('folder');   // blobId absent
  });

  it('prefers the server-supplied media type', () => {
    expect(fileKind(file({ type: 'image/png' }))).toBe('image');
    expect(fileKind(file({ type: 'video/mp4' }))).toBe('video');
    expect(fileKind(file({ type: 'audio/mpeg' }))).toBe('audio');
    expect(fileKind(file({ type: 'application/pdf' }))).toBe('pdf');
  });

  it('falls back to the extension when the type is generic', () => {
    // Uploads very often arrive as application/octet-stream.
    const generic = { type: 'application/octet-stream' };
    expect(fileKind(file({ ...generic, name: 'main.py' }))).toBe('code');
    expect(fileKind(file({ ...generic, name: 'data.csv' }))).toBe('sheet');
    expect(fileKind(file({ ...generic, name: 'photos.zip' }))).toBe('archive');
    expect(fileKind(file({ ...generic, name: 'deck.pptx' }))).toBe('slides');
  });

  it('falls back to "file" for the genuinely unknown', () => {
    expect(fileKind(file({ type: 'application/octet-stream', name: 'blob.qqq' }))).toBe('file');
  });
});

describe('isPreviewable', () => {
  it('allows media and text', () => {
    expect(isPreviewable(file({ type: 'image/png' }))).toBe(true);
    expect(isPreviewable(file({ type: 'application/pdf' }))).toBe(true);
    expect(isPreviewable(file({ type: 'text/plain' }))).toBe(true);
  });

  it('refuses folders and unknown binaries', () => {
    expect(isPreviewable(folder())).toBe(false);
    expect(isPreviewable(file({ type: 'application/octet-stream', name: 'x.bin' }))).toBe(false);
  });
});

describe('validateName', () => {
  it('accepts ordinary names', () => {
    expect(validateName('Report 2026.pdf')).toBe('');
    expect(validateName('réunion')).toBe('');
  });

  it('rejects what the FileNode spec forbids', () => {
    expect(validateName('a/b')).toMatch(/\//);
    expect(validateName('.')).toMatch(/reserved/i);
    expect(validateName('..')).toMatch(/reserved/i);
  });

  it('rejects empty and whitespace-only names', () => {
    expect(validateName('')).toMatch(/empty/i);
    expect(validateName('   ')).toMatch(/empty/i);
    expect(validateName(null)).toMatch(/empty/i);
  });

  it('rejects control characters, which are invisible in the UI', () => {
    expect(validateName(`a${String.fromCharCode(1)}b`)).toMatch(/control/i);
    expect(validateName(`a${String.fromCharCode(127)}b`)).toMatch(/control/i);
  });

  it('enforces the server-advertised name length', () => {
    expect(validateName('x'.repeat(120), { maxLength: 100 })).toMatch(/100/);
    expect(validateName('x'.repeat(90), { maxLength: 100 })).toBe('');
  });
});

describe('breadcrumbTrail', () => {
  const nodes = new Map([
    ['a', { id: 'a', name: 'A', parentId: null }],
    ['b', { id: 'b', name: 'B', parentId: 'a' }],
    ['c', { id: 'c', name: 'C', parentId: 'b' }],
  ]);

  it('builds a root-first trail', () => {
    expect(breadcrumbTrail(nodes, 'c').map((n) => n.name)).toEqual(['A', 'B', 'C']);
  });

  it('is empty at the root', () => {
    expect(breadcrumbTrail(nodes, null)).toEqual([]);
  });

  it('terminates on a parentId cycle instead of hanging the UI', () => {
    const cyclic = new Map([
      ['x', { id: 'x', name: 'X', parentId: 'y' }],
      ['y', { id: 'y', name: 'Y', parentId: 'x' }],
    ]);
    expect(breadcrumbTrail(cyclic, 'x').length).toBeLessThanOrEqual(2);
  });

  it('stops cleanly when an ancestor is missing', () => {
    const orphan = new Map([['z', { id: 'z', name: 'Z', parentId: 'gone' }]]);
    expect(breadcrumbTrail(orphan, 'z').map((n) => n.name)).toEqual(['Z']);
  });
});

describe('compareNodes', () => {
  it('always puts folders first, whatever the sort', () => {
    const f = folder({ name: 'zzz' });
    const n = file({ name: 'aaa' });
    expect(compareNodes(f, n, { key: 'name' })).toBeLessThan(0);
    expect(compareNodes(f, n, { key: 'name', ascending: false })).toBeLessThan(0);
    expect(compareNodes(f, n, { key: 'size' })).toBeLessThan(0);
  });

  it('sorts names naturally, so file10 follows file9', () => {
    const list = [file({ name: 'file10' }), file({ name: 'file9' }), file({ name: 'file1' })];
    const sorted = [...list].sort((a, b) => compareNodes(a, b));
    expect(sorted.map((n) => n.name)).toEqual(['file1', 'file9', 'file10']);
  });

  it('sorts by size and date, and honours direction', () => {
    const small = file({ name: 'a', size: 10 });
    const big = file({ name: 'b', size: 5000 });
    expect(compareNodes(small, big, { key: 'size' })).toBeLessThan(0);
    expect(compareNodes(small, big, { key: 'size', ascending: false })).toBeGreaterThan(0);

    const older = file({ name: 'a', modified: '2020-01-01T00:00:00Z' });
    const newer = file({ name: 'b', modified: '2026-01-01T00:00:00Z' });
    expect(compareNodes(older, newer, { key: 'modified' })).toBeLessThan(0);
  });
});

describe('uniqueName', () => {
  it('leaves a free name alone', () => {
    expect(uniqueName('a.txt', ['b.txt'])).toBe('a.txt');
  });

  it('suffixes before the extension, not after', () => {
    expect(uniqueName('a.txt', ['a.txt'])).toBe('a (2).txt');
    expect(uniqueName('a.txt', ['a.txt', 'a (2).txt'])).toBe('a (3).txt');
  });

  it('handles extensionless names', () => {
    expect(uniqueName('README', ['README'])).toBe('README (2)');
  });

  it('accepts a Set as well as an array', () => {
    expect(uniqueName('a.txt', new Set(['a.txt']))).toBe('a (2).txt');
  });
});

describe('canThumbnail', () => {
  const image = (over = {}) =>
    ({ id: 'i1', name: 'p.png', blobId: 'b1', type: 'image/png', size: 1024, ...over });

  it('accepts the image types the backend serves inline', () => {
    for (const type of ['image/png', 'image/jpeg', 'image/gif', 'image/webp',
                        'image/avif', 'image/bmp']) {
      expect(canThumbnail(image({ type }))).toBe(true);
    }
  });

  it('is case-insensitive about the media type', () => {
    expect(canThumbnail(image({ type: 'IMAGE/PNG' }))).toBe(true);
  });

  it('refuses image types the backend will not serve inline', () => {
    // SVG can carry script, so it comes back as a download — an <img> pointed
    // at it renders as broken, not as a picture.
    expect(canThumbnail(image({ type: 'image/svg+xml', name: 'a.svg' }))).toBe(false);
    expect(canThumbnail(image({ type: 'image/tiff', name: 'a.tiff' }))).toBe(false);
  });

  it('refuses folders and anything that is not an image', () => {
    expect(canThumbnail(folder())).toBe(false);
    expect(canThumbnail(file())).toBe(false);
    expect(canThumbnail(null)).toBe(false);
  });

  it('refuses an image with no blob to fetch', () => {
    expect(canThumbnail({ name: 'p.png', type: 'image/png', blobId: null })).toBe(false);
  });

  it('refuses images past the size ceiling', () => {
    expect(canThumbnail(image({ size: MAX_THUMBNAIL_BYTES + 1 }))).toBe(false);
    expect(canThumbnail(image({ size: MAX_THUMBNAIL_BYTES }))).toBe(true);
    expect(canThumbnail(image({ size: 4096 }), { maxBytes: 1024 })).toBe(false);
  });

  it('allows an image whose size the server did not report', () => {
    // Refusing would silently drop the thumbnail for every such file.
    expect(canThumbnail(image({ size: undefined }))).toBe(true);
    expect(canThumbnail(image({ size: null }))).toBe(true);
  });
});
