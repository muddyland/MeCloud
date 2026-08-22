import { describe, it, expect } from 'vitest';
import {
  applyAction, toggleWrap, toggleLinePrefix, lineRange, stripPrefix, ACTION_NAMES,
} from '$lib/markdownEdit.js';

/** Compact notation: "he[llo] world" marks the selection. */
const parse = (marked) => {
  const start = marked.indexOf('[');
  const end = marked.indexOf(']') - 1;
  return { text: marked.replace(/[[\]]/g, ''), start, end };
};
const show = ({ text, start, end }) =>
  `${text.slice(0, start)}[${text.slice(start, end)}]${text.slice(end)}`;
const run = (action, marked) => show(applyAction(action, parse(marked)));

describe('inline formatting', () => {
  it('wraps a selection', () => {
    expect(run('bold', 'he[llo] world')).toBe('he**[llo]** world');
    expect(run('italic', '[hi]')).toBe('*[hi]*');
    expect(run('code', '[x]')).toBe('`[x]`');
    expect(run('strike', '[x]')).toBe('~~[x]~~');
  });

  it('unwraps when the markers sit outside the selection', () => {
    // The usual state after a previous toggle: pressing again removes it.
    expect(run('bold', '**[hello]** world')).toBe('[hello] world');
  });

  it('unwraps when the markers are inside the selection', () => {
    // What you get by selecting the formatted text including its markers.
    expect(run('bold', '[**hello**] world')).toBe('[hello] world');
  });

  it('inserts a selected placeholder when there is no selection', () => {
    expect(run('bold', '[]')).toBe('**[bold text]**');
    expect(run('code', '[]')).toBe('`[code]`');
  });

  it('does not mistake bold for italic', () => {
    // '*' is a prefix of '**', so a naive check would unwrap the wrong one.
    const out = applyAction('italic', parse('**[hi]**'));
    expect(out.text).toBe('**hi**'.replace('hi', '*hi*'));
  });

  it('leaves the document alone for an unknown action', () => {
    const state = parse('he[llo]');
    expect(applyAction('nonsense', state)).toEqual(state);
  });
});

describe('block prefixes', () => {
  it('applies a heading', () => {
    expect(run('h1', '[]Title')).toBe('[# Title]');
    expect(run('h2', 'Ti[]tle')).toBe('[## Title]');
  });

  it('replaces an existing heading level rather than stacking', () => {
    expect(run('h2', '[]# Title')).toBe('[## Title]');
    expect(run('h3', '[]## Title')).toBe('[### Title]');
  });

  it('toggles the same heading off', () => {
    expect(run('h1', '[]# Title')).toBe('[Title]');
  });

  it('numbers an ordered list across the selection', () => {
    expect(run('ol', '[a\nb\nc]')).toBe('[1. a\n2. b\n3. c]');
  });

  it('converts a bulleted list to an ordered one', () => {
    expect(run('ol', '[- a\n- b]')).toBe('[1. a\n2. b]');
  });

  it('applies bullets, tasks and quotes', () => {
    expect(run('ul', '[a\nb]')).toBe('[- a\n- b]');
    expect(run('task', '[a]')).toBe('[- [ ] a]');
    expect(run('quote', '[a\nb]')).toBe('[> a\n> b]');
  });

  it('toggles a list off', () => {
    expect(run('ul', '[- a\n- b]')).toBe('[a\nb]');
  });

  it('preserves indentation when stripping', () => {
    expect(stripPrefix('    - item')).toBe('    item');
    expect(stripPrefix('  ## head')).toBe('  head');
    expect(stripPrefix('plain')).toBe('plain');
  });
});

describe('lineRange', () => {
  it('expands to whole lines', () => {
    expect(lineRange('one\ntwo\nthree', 5, 6)).toEqual([4, 7]);
  });

  it('handles the first and last line', () => {
    expect(lineRange('one\ntwo', 1, 1)).toEqual([0, 3]);
    expect(lineRange('one\ntwo', 5, 5)).toEqual([4, 7]);
  });
});

describe('link', () => {
  it('uses the selection as the label and selects the url', () => {
    expect(run('link', '[click]')).toBe('[click]([url])');
  });

  it('selects the label when there is no selection', () => {
    expect(run('link', '[]')).toBe('[[text]](url)');
  });
});

describe('blocks', () => {
  it('fences a selection as a code block', () => {
    const out = applyAction('codeblock', parse('[const x = 1]'));
    expect(out.text).toBe('```\nconst x = 1\n```');
    expect(out.text.slice(out.start, out.end)).toBe('const x = 1');
  });

  it('inserts a horizontal rule on its own line', () => {
    expect(applyAction('hr', parse('text[]')).text).toBe('text\n\n---');
  });

  it('does not add blank lines that are already there', () => {
    expect(applyAction('hr', parse('text\n\n[]')).text).toBe('text\n\n---');
  });
});

describe('action registry', () => {
  it('exposes every action the toolbar offers', () => {
    for (const name of ['bold', 'italic', 'strike', 'code', 'h1', 'h2', 'h3',
                        'quote', 'ul', 'ol', 'task', 'link', 'codeblock', 'hr']) {
      expect(ACTION_NAMES).toContain(name);
    }
  });

  it('never returns undefined text for any action', () => {
    for (const name of ACTION_NAMES) {
      const out = applyAction(name, { text: 'sample', start: 0, end: 6 });
      expect(typeof out.text).toBe('string');
      expect(Number.isInteger(out.start)).toBe(true);
      expect(Number.isInteger(out.end)).toBe(true);
    }
  });
});
