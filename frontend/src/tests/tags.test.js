import { describe, it, expect } from 'vitest';
import { get } from 'svelte/store';
import {
  normalizeTag, canonicalize, parseIndex, serializeIndex, applyChange, prune, emptyIndex,
  parseTagQuery, matchesQuery, searchNodes, tagCounts, knownTags, compareCodePoints,
  isReservedFolder, isReservedNode, reservedIds, withoutReserved, findTagIndexNodes,
  MAX_TAGS_PER_FILE, MALFORMED_MESSAGE, NEWER_VERSION_MESSAGE,
} from '#lib/tags.js';
import { recentFiles, summariseFiles } from '#lib/dashboard.js';
import {
  fileNodes, fileSearch, currentFolderId, tagIndex, visibleNodes, childrenByParent,
  allChildrenByParent, visibleFileNodes, tagCounts as tagCountsStore, totalUsage,
} from '#lib/stores/files.js';

const folder = (id, name, parentId = null) => ({ id, name, parentId, blobId: null });
const file = (id, name, parentId = null, extra = {}) => ({ id, name, parentId, blobId: `b-${id}`, size: 10, ...extra });
const indexOf = (obj) => ({ ...emptyIndex(), tags: new Map(Object.entries(obj)) });

describe('normalizeTag', () => {
  it('trims, strips leading # and collapses whitespace', () => {
    expect(normalizeTag('  Tax  ')).toBe('Tax');
    expect(normalizeTag('#Tax')).toBe('Tax');
    expect(normalizeTag('### Tax')).toBe('Tax');
    expect(normalizeTag('Insurance \t\n  2026')).toBe('Insurance 2026');
    expect(normalizeTag('\u00a0Tax\u3000')).toBe('Tax');
  });

  it('only strips # at the start', () => {
    expect(normalizeTag('C#')).toBe('C#');
    expect(normalizeTag('#C#')).toBe('C#');
  });

  it('rejects empty, comma and control characters', () => {
    for (const bad of ['', '   ', '#', '## ', 'a,b', 'tab\u0007bell', 'del\u007f', 'c1\u0090', null, undefined, 42]) {
      expect(normalizeTag(bad), JSON.stringify(bad)).toBeNull();
    }
  });

  it('counts length in code points, not UTF-16 units or bytes', () => {
    // 40 emoji are 80 UTF-16 units and 160 bytes, but 40 characters.
    expect(normalizeTag('😀'.repeat(40))).toBe('😀'.repeat(40));
    expect(normalizeTag('😀'.repeat(41))).toBeNull();
    expect(normalizeTag('é'.repeat(40))).toBe('é'.repeat(40));
    expect(normalizeTag('a'.repeat(41))).toBeNull();
    // Length is checked after trimming and collapsing.
    expect(normalizeTag(`  ${'a'.repeat(40)}  `)).toBe('a'.repeat(40));
  });

  it('treats U+FEFF as a character rather than whitespace, like Rust', () => {
    expect(normalizeTag('\ufeffTax')).toBe('\ufeffTax');
  });
});

describe('canonicalize', () => {
  it('dedupes case-insensitively keeping the first', () => {
    expect(canonicalize(['Tax', 'tax', 'TAX', ' #tax'])).toEqual(['Tax']);
  });

  it('adopts the spelling already in the index', () => {
    expect(canonicalize(['tax', 'home'], ['Tax', 'Kitchen'])).toEqual(['Tax', 'home']);
  });

  it('drops invalid tags', () => {
    expect(canonicalize(['ok', '', 'a,b', 'x'.repeat(41)])).toEqual(['ok']);
  });

  it(`caps a file at ${MAX_TAGS_PER_FILE} tags`, () => {
    const many = Array.from({ length: 30 }, (_, i) => `t${i}`);
    const out = canonicalize(many);
    expect(out).toHaveLength(MAX_TAGS_PER_FILE);
    expect(out[0]).toBe('t0');
    expect(out.at(-1)).toBe('t19');
  });

  it('counts the cap after duplicates are removed', () => {
    const list = ['a', 'A', ...Array.from({ length: 20 }, (_, i) => `t${i}`)];
    expect(canonicalize(list)).toHaveLength(20);
    expect(canonicalize(list)).toContain('t18');
  });
});

describe('parseIndex / serializeIndex', () => {
  it('reads a missing file as empty and writable', () => {
    const index = parseIndex(null);
    expect(index.tags.size).toBe(0);
    expect(index.writable).toBe(true);
  });

  it('round-trips, preserving unknown keys', () => {
    const text = JSON.stringify({
      version: 1,
      tags: { n2: ['Tax'], n1: ['Insurance 2026', 'Home'] },
      colours: { Tax: '#f00' },
      note: 'written by the future',
    });
    const index = parseIndex(text);
    expect(index.writable).toBe(true);
    expect(index.tags.get('n1')).toEqual(['Insurance 2026', 'Home']);

    const out = serializeIndex(index);
    const back = JSON.parse(out);
    expect(back).toEqual({
      version: 1,
      tags: { n1: ['Insurance 2026', 'Home'], n2: ['Tax'] },
      colours: { Tax: '#f00' },
      note: 'written by the future',
    });
    // And a second trip is byte-identical: nothing drifts between saves.
    expect(serializeIndex(parseIndex(out))).toBe(out);
  });

  it('writes pretty, sorted, with a trailing newline', () => {
    const out = serializeIndex(indexOf({ b: ['Two'], a: ['One', 'Uno'] }));
    expect(out).toBe(
      '{\n'
      + '  "version": 1,\n'
      + '  "tags": {\n'
      + '    "a": [\n      "One",\n      "Uno"\n    ],\n'
      + '    "b": [\n      "Two"\n    ]\n'
      + '  }\n'
      + '}\n',
    );
    // The same as JSON.stringify would give for non-numeric keys.
    expect(out).toBe(`${JSON.stringify({ version: 1, tags: { a: ['One', 'Uno'], b: ['Two'] } }, null, 2)}\n`);
  });

  it('sorts integer-like ids as strings, not numerically', () => {
    const out = serializeIndex(indexOf({ 10: ['x'], 9: ['y'], a: ['z'] }));
    // A JS object would put "9" before "10"; the file must not.
    expect(out.indexOf('"10"')).toBeLessThan(out.indexOf('"9"'));
    expect(out.indexOf('"9"')).toBeLessThan(out.indexOf('"a"'));
  });

  it('writes an empty map as {} and never writes []', () => {
    const index = applyChange(indexOf({ a: ['x'] }), ['a'], { remove: ['x'] });
    expect(serializeIndex(index)).toBe('{\n  "version": 1,\n  "tags": {}\n}\n');
  });

  it('treats a missing version as 1', () => {
    const index = parseIndex('{"tags":{"a":["x"]}}');
    expect(index.version).toBe(1);
    expect(index.writable).toBe(true);
    expect(index.tags.get('a')).toEqual(['x']);
  });

  it('reads a newer version but refuses to write it', () => {
    const index = parseIndex(JSON.stringify({ version: 2, tags: { a: ['Tax'] }, more: true }));
    expect(index.tags.get('a')).toEqual(['Tax']);
    expect(index.writable).toBe(false);
    expect(index.error).toBe(NEWER_VERSION_MESSAGE);
  });

  it('shows nothing from a newer version whose tags it cannot read', () => {
    const index = parseIndex(JSON.stringify({ version: 3, tags: [['a', 'Tax']] }));
    expect(index.tags.size).toBe(0);
    expect(index.writable).toBe(false);
  });

  it('refuses to write a malformed file', () => {
    for (const text of ['', 'not json', '[]', 'null', '42', '{"version":"1"}', '{"version":0}',
      '{"version":1.5}', '{"tags":[]}', '{"tags":"x"}', '{"version":1,"tags":null}']) {
      const index = parseIndex(text);
      expect(index.writable, text).toBe(false);
      expect(index.error, text).toBe(MALFORMED_MESSAGE);
      expect(index.tags.size, text).toBe(0);
    }
  });

  it('skips values that are not arrays of strings', () => {
    const index = parseIndex(JSON.stringify({
      tags: { a: ['ok'], b: 'Tax', c: [1, 2], d: ['x', null], e: [], f: { 0: 'x' } },
    }));
    expect([...index.tags.keys()]).toEqual(['a']);
    expect(index.writable).toBe(true);
  });

  it('keeps a "__proto__" id as an id', () => {
    const index = parseIndex('{"version":1,"tags":{"__proto__":["x"]}}');
    expect(index.tags.get('__proto__')).toEqual(['x']);
    expect(JSON.parse(serializeIndex(index)).tags).toHaveProperty('__proto__');
  });
});

describe('applyChange', () => {
  it('adds and removes, case-insensitively', () => {
    const index = indexOf({ a: ['Tax', 'Home'] });
    const next = applyChange(index, ['a'], { add: ['2026'], remove: ['tax'] });
    expect(next.tags.get('a')).toEqual(['Home', '2026']);
    expect(index.tags.get('a')).toEqual(['Tax', 'Home']);      // not mutated
  });

  it('applies to several nodes and joins existing spellings', () => {
    const index = indexOf({ a: ['Tax'], z: ['Kitchen'] });
    const next = applyChange(index, ['a', 'b'], { add: ['tax', 'kitchen'] });
    expect(next.tags.get('a')).toEqual(['Tax', 'Kitchen']);
    expect(next.tags.get('b')).toEqual(['Tax', 'Kitchen']);
  });

  it('set replaces, and lets a lone tag change case', () => {
    const next = applyChange(indexOf({ a: ['tax'] }), ['a'], { set: ['Tax', 'Home'] });
    expect(next.tags.get('a')).toEqual(['Tax', 'Home']);
  });

  it('deletes the entry when no tags are left', () => {
    const next = applyChange(indexOf({ a: ['Tax'], b: ['Tax'] }), ['a'], { set: [] });
    expect(next.tags.has('a')).toBe(false);
    expect(next.tags.get('b')).toEqual(['Tax']);
  });

  it('caps at 20 after adding', () => {
    const start = Array.from({ length: 19 }, (_, i) => `t${i}`);
    const next = applyChange(indexOf({ a: start }), ['a'], { add: ['x', 'y', 'z'] });
    expect(next.tags.get('a')).toHaveLength(20);
    expect(next.tags.get('a').at(-1)).toBe('x');
  });
});

describe('prune', () => {
  const index = indexOf({ live: ['x'], gone: ['y'] });

  it('drops ids missing from a complete listing', () => {
    expect([...prune(index, ['live']).tags.keys()]).toEqual(['live']);
  });

  it('never prunes from an incomplete listing', () => {
    expect(prune(index, ['live'], { incomplete: true })).toBe(index);
  });
});

describe('knownTags and tagCounts', () => {
  it('agree on spelling by id order and count live ids only', () => {
    const tags = new Map([['b', ['tax']], ['a', ['Tax', 'Home']], ['dead', ['Tax']]]);
    expect(knownTags(tags)).toEqual(['Tax', 'Home']);
    expect(tagCounts(tags, ['a', 'b'])).toEqual([{ tag: 'Home', count: 1 }, { tag: 'Tax', count: 2 }]);
  });

  it('compares code points the way Rust orders strings', () => {
    // UTF-16 order puts U+FF5E before U+1F600 (a surrogate pair); code point order does not.
    expect(compareCodePoints('\uff5e', '😀')).toBeLessThan(0);
    expect(['b', 'a', 'B'].sort(compareCodePoints)).toEqual(['B', 'a', 'b']);
  });
});

describe('search queries', () => {
  const tags = new Map([
    ['1', ['Tax', 'Insurance 2026']],
    ['2', ['Tax']],
    ['3', ['C#']],
  ]);
  const nodes = [file('1', 'policy.pdf'), file('2', 'receipt.png'), file('3', 'notes.txt'), file('4', 'taxi.jpg')];
  const names = (q) => searchNodes(nodes, tags, q).map((n) => n.name);

  it('parses # queries into exact tag keys', () => {
    expect(parseTagQuery('#Tax')).toEqual({ tags: ['tax'], text: '' });
    expect(parseTagQuery('#Insurance 2026 #tax')).toEqual({ tags: ['insurance 2026', 'tax'], text: '' });
    expect(parseTagQuery('#C#')).toEqual({ tags: ['c#'], text: '' });
    expect(parseTagQuery('  Tax ')).toEqual({ tags: null, text: 'tax' });
  });

  it('a plain query matches names or tags by substring', () => {
    expect(names('tax')).toEqual(['policy.pdf', 'receipt.png', 'taxi.jpg']);
    expect(names('2026')).toEqual(['policy.pdf']);
    expect(names('RECEIPT')).toEqual(['receipt.png']);
  });

  it('#tag is an exact tag filter, ignoring names', () => {
    expect(names('#tax')).toEqual(['policy.pdf', 'receipt.png']);
    expect(names('#ta')).toEqual([]);
    expect(names('#c#')).toEqual(['notes.txt']);
  });

  it('several #tags must all match', () => {
    expect(names('#tax #insurance 2026')).toEqual(['policy.pdf']);
    expect(names('#tax #c#')).toEqual([]);
  });

  it('a lone # shows everything tagged', () => {
    expect(names('#')).toEqual(['policy.pdf', 'receipt.png', 'notes.txt']);
  });

  it('matchesQuery tolerates nodes without tags', () => {
    expect(matchesQuery(file('x', 'a'), undefined, parseTagQuery('#a'))).toBe(false);
    expect(matchesQuery(file('x', 'a'), undefined, parseTagQuery('a'))).toBe(true);
  });
});

describe('the reserved .mecloud folder', () => {
  const nodes = [
    folder('m', '.mecloud'),
    file('t', 'tags.json', 'm', { size: 100 }),
    folder('m2', 'sub', 'm'),
    file('deep', 'deep.txt', 'm2'),
    folder('d', 'Docs'),
    folder('nested', '.mecloud', 'd'),       // only the top-level one is reserved
    file('a', 'a.txt', 'd'),
  ];
  const byId = new Map(nodes.map((n) => [n.id, n]));

  it('recognises only the top-level folder and its subtree', () => {
    expect(isReservedFolder(byId.get('m'))).toBe(true);
    expect(isReservedFolder(byId.get('nested'))).toBe(false);
    expect(isReservedFolder(file('f', '.mecloud'))).toBe(false);
    expect([...reservedIds(nodes)].sort()).toEqual(['deep', 'm', 'm2', 't']);
    expect(isReservedNode(byId.get('deep'), byId)).toBe(true);
    expect(isReservedNode(byId.get('a'), byId)).toBe(false);
    expect(withoutReserved(nodes).map((n) => n.id)).toEqual(['d', 'nested', 'a']);
  });

  it('finds the index file', () => {
    expect(findTagIndexNodes(nodes)).toMatchObject({ folder: { id: 'm' }, file: { id: 't' } });
    expect(findTagIndexNodes([folder('d', 'Docs')])).toEqual({ folder: null, file: null });
  });

  it('is left out of the dashboard but still counted in storage', () => {
    const dated = nodes.map((n, i) => ({ ...n, modified: new Date(2026, 0, i + 1).toISOString() }));
    expect(recentFiles(dated).map((n) => n.id)).toEqual(['a']);
    const s = summariseFiles(nodes);
    expect(s.files).toBe(1);
    expect(s.folders).toBe(2);
    expect(s.bytes).toBe(100 + 10 + 10);
  });

  it('is hidden from the listing, the tree and search, not from usage or name checks', () => {
    fileNodes.set(nodes);
    currentFolderId.set(null);
    fileSearch.set('');
    expect(get(visibleNodes).map((n) => n.id)).toEqual(['d']);
    expect((get(childrenByParent).get(null) ?? []).map((n) => n.id)).toEqual(['d']);
    expect(get(childrenByParent).get('m')).toBeUndefined();
    expect((get(allChildrenByParent).get(null) ?? []).map((n) => n.id)).toEqual(['m', 'd']);
    expect(get(visibleFileNodes)).toHaveLength(3);

    fileSearch.set('tags');
    expect(get(visibleNodes)).toEqual([]);
    fileSearch.set('deep');
    expect(get(visibleNodes)).toEqual([]);
    expect(get(totalUsage)).toBe(120);
    fileSearch.set('');
  });
});

describe('the Files store with tags', () => {
  it('searches by tag across the drive and counts tags on visible files', () => {
    fileNodes.set([
      folder('d', 'Docs'), file('a', 'policy.pdf', 'd'), file('b', 'receipt.png'),
      folder('m', '.mecloud'), file('t', 'tags.json', 'm'),
    ]);
    tagIndex.set({
      index: { ...emptyIndex(), tags: new Map([['a', ['Tax']], ['b', ['tax', 'Home']], ['t', ['Hidden']], ['gone', ['Tax']]]) },
      node: null, loaded: true, error: '',
    });
    currentFolderId.set(null);

    fileSearch.set('#Tax');
    expect(get(visibleNodes).map((n) => n.id).sort()).toEqual(['a', 'b']);
    fileSearch.set('home');
    expect(get(visibleNodes).map((n) => n.id)).toEqual(['b']);
    fileSearch.set('hidden');
    expect(get(visibleNodes)).toEqual([]);

    expect(get(tagCountsStore)).toEqual([{ tag: 'Home', count: 1 }, { tag: 'Tax', count: 2 }]);
    fileSearch.set('');
    tagIndex.set({ index: emptyIndex(), node: null, loaded: false, error: '' });
  });
});
