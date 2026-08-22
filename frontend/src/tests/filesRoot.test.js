import { describe, it, expect } from 'vitest';
import { get } from 'svelte/store';
import {
  fileNodes, currentFolderId, rootNodeId, activeParentId, visibleNodes, resetFilesState,
} from '$lib/stores/files.js';

/*
 * draft-ietf-jmap-filenode section 3.1: "parentId: The Id of the parent node,
 * or null if this is the root node." null identifies the root itself, not
 * "sits at the top level" — which is how this app had read it. Creating a
 * folder with parentId: null therefore declared a second root instead of a
 * child, producing a node that occupies its name but never appears under the
 * root the UI walks.
 *
 * Servers disagree on this, so the behaviour is detected per-server. These
 * tests pin both readings, and in particular that the looser one still works
 * exactly as before.
 */

const folder = (id, parentId = null, name = id) => ({ id, parentId, name, blobId: null });
const file   = (id, parentId = null, name = id) => ({ id, parentId, name, blobId: `b-${id}` });

describe('rootNodeId', () => {
  it('finds the root when the server exposes exactly one', () => {
    resetFilesState();
    fileNodes.set([folder('root'), folder('notes', 'root'), file('a', 'root')]);
    expect(get(rootNodeId)).toBe('root');
  });

  it('stays null when several nodes are parentless', () => {
    // The looser reading: the server means "top level", not "the root node".
    resetFilesState();
    fileNodes.set([folder('notes'), folder('docs'), file('a')]);
    expect(get(rootNodeId)).toBeNull();
  });

  it('stays null when the only parentless node is a file', () => {
    // A file cannot be a drive root, so this is the looser reading too.
    resetFilesState();
    fileNodes.set([file('loose')]);
    expect(get(rootNodeId)).toBeNull();
  });

  it('stays null for an empty drive', () => {
    resetFilesState();
    fileNodes.set([]);
    expect(get(rootNodeId)).toBeNull();
  });
});

describe('activeParentId', () => {
  it('is the root when no folder is open', () => {
    resetFilesState();
    fileNodes.set([folder('root'), folder('notes', 'root')]);
    expect(get(activeParentId)).toBe('root');
  });

  it('is the open folder once one is opened', () => {
    resetFilesState();
    fileNodes.set([folder('root'), folder('notes', 'root')]);
    currentFolderId.set('notes');
    expect(get(activeParentId)).toBe('notes');
  });

  it('falls back to null on a server using the looser reading', () => {
    // This is the no-op path: nothing about create or upload changes.
    resetFilesState();
    fileNodes.set([folder('notes'), folder('docs')]);
    expect(get(activeParentId)).toBeNull();
  });
});

describe('visibleNodes', () => {
  it('lists the root’s children rather than the root itself', () => {
    // The bug this fixes: with a real root node, keying the listing on null
    // showed the root as a lone entry and hid everything actually in the drive.
    resetFilesState();
    fileNodes.set([folder('root'), folder('notes', 'root'), file('a', 'root', 'a.txt')]);
    expect(get(visibleNodes).map((n) => n.id)).toEqual(['notes', 'a']);
  });

  it('still lists top-level nodes on a server using the looser reading', () => {
    resetFilesState();
    fileNodes.set([folder('notes'), file('a', null, 'a.txt')]);
    expect(get(visibleNodes).map((n) => n.id)).toEqual(['notes', 'a']);
  });

  it('a folder created under the root is visible immediately', () => {
    // The reported symptom, as a test: created, then invisible on reload.
    resetFilesState();
    fileNodes.set([folder('root'), folder('notes', 'root')]);
    fileNodes.update((l) => [...l, folder('new', 'root', 'New Folder')]);
    expect(get(visibleNodes).map((n) => n.name)).toContain('New Folder');
  });

  it('leaves orphans from the old bug listed at the top, so they can be deleted', () => {
    // A drive the old code has polluted: a real root plus folders created with
    // parentId null. The root is no longer identifiable, so this falls back to
    // listing every parentless node — which is what makes the orphans visible.
    resetFilesState();
    fileNodes.set([
      folder('root'), folder('notes', 'root'),
      folder('orphan1', null, 'Orphan One'), folder('orphan2', null, 'Orphan Two'),
    ]);
    expect(get(rootNodeId)).toBeNull();
    const names = get(visibleNodes).map((n) => n.name);
    expect(names).toContain('Orphan One');
    expect(names).toContain('Orphan Two');
  });

  it('does not mistake an ordinary top-level folder for a root', () => {
    // The false positive that a "has children" heuristic produces: on a server
    // using the looser reading, a top-level folder with contents is not a root.
    resetFilesState();
    fileNodes.set([folder('notes'), folder('docs'), file('n1', 'notes')]);
    expect(get(rootNodeId)).toBeNull();
    expect(get(visibleNodes).map((n) => n.id)).toEqual(['docs', 'notes']);
  });
});
