import { describe, it, expect } from 'vitest';
import { findNotesFolder, collectNotes, noteFilename, NOTES_FOLDER } from '$lib/notes.js';

const folder = (id, name, parentId = null) => ({ id, name, parentId, blobId: null });
const file = (id, name, parentId = null) => ({ id, name, parentId, blobId: `b${id}`, type: 'text/markdown' });

describe('findNotesFolder', () => {
  it('finds the root-level Notes folder', () => {
    const nodes = [folder('1', 'Docs'), folder('2', NOTES_FOLDER)];
    expect(findNotesFolder(nodes)?.id).toBe('2');
  });

  it('matches case-insensitively, so an existing "notes" folder is reused', () => {
    expect(findNotesFolder([folder('1', 'notes')])?.id).toBe('1');
    expect(findNotesFolder([folder('1', 'NOTES')])?.id).toBe('1');
  });

  it('ignores a nested folder of the same name', () => {
    // Only the root one counts; a Notes folder inside another folder is
    // somebody else's organisation, not our store.
    expect(findNotesFolder([folder('1', NOTES_FOLDER, 'parent')])).toBeNull();
  });

  it('ignores a *file* called Notes', () => {
    expect(findNotesFolder([file('1', 'Notes')])).toBeNull();
  });

  it('returns null when absent', () => {
    expect(findNotesFolder([])).toBeNull();
    expect(findNotesFolder([folder('1', 'Other')])).toBeNull();
  });
});

describe('collectNotes', () => {
  it('returns markdown files in the folder', () => {
    const nodes = [folder('n', 'Notes'), file('a', 'One.md', 'n'), file('b', 'Two.md', 'n')];
    expect(collectNotes(nodes, 'n').map((x) => x.name)).toEqual(['One.md', 'Two.md']);
  });

  it('recurses into subfolders and records the path', () => {
    // An existing collection is very likely organised into subfolders; showing
    // only the top level would look like missing data.
    const nodes = [
      folder('n', 'Notes'),
      folder('s', 'Work', 'n'),
      file('a', 'Top.md', 'n'),
      file('b', 'Deep.md', 's'),
    ];
    const found = collectNotes(nodes, 'n');
    expect(found.map((x) => x.name).sort()).toEqual(['Deep.md', 'Top.md']);
    expect(found.find((x) => x.name === 'Deep.md').folderPath).toBe('Work');
  });

  it('excludes non-markdown files', () => {
    const nodes = [folder('n', 'Notes'), file('a', 'note.md', 'n'), file('b', 'image.png', 'n')];
    expect(collectNotes(nodes, 'n').map((x) => x.name)).toEqual(['note.md']);
  });

  it('ignores anything outside the folder', () => {
    const nodes = [folder('n', 'Notes'), file('a', 'inside.md', 'n'), file('b', 'outside.md', null)];
    expect(collectNotes(nodes, 'n').map((x) => x.name)).toEqual(['inside.md']);
  });

  it('returns nothing without a folder id', () => {
    expect(collectNotes([file('a', 'x.md')], null)).toEqual([]);
  });

  it('terminates on a parent cycle rather than hanging', () => {
    const nodes = [
      { id: 'a', name: 'A', parentId: 'b', blobId: null },
      { id: 'b', name: 'B', parentId: 'a', blobId: null },
    ];
    expect(() => collectNotes(nodes, 'a')).not.toThrow();
  });
});

describe('noteFilename', () => {
  it('appends the extension', () => {
    expect(noteFilename('Shopping', [])).toBe('Shopping.md');
  });

  it('replaces characters that are illegal or awkward across filesystems', () => {
    expect(noteFilename('a/b:c*d?e"f<g>h|i', [])).toBe('a-b-c-d-e-f-g-h-i.md');
  });

  it('collapses runs of whitespace', () => {
    expect(noteFilename('  spaced   out  ', [])).toBe('spaced out.md');
  });

  it('falls back to Untitled for an empty title', () => {
    expect(noteFilename('', [])).toBe('Untitled.md');
    expect(noteFilename('   ', [])).toBe('Untitled.md');
    expect(noteFilename(null, [])).toBe('Untitled.md');
  });

  it('does not collide with an existing note', () => {
    expect(noteFilename('Note', ['Note.md'])).toBe('Note (2).md');
    expect(noteFilename('Note', ['Note.md', 'Note (2).md'])).toBe('Note (3).md');
  });

  it('caps very long titles', () => {
    expect(noteFilename('x'.repeat(400), []).length).toBeLessThanOrEqual(124);
  });
});
