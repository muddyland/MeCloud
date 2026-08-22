// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import {
  splitFrontMatter, firstHeading, noteName, noteTitle, excerpt,
  renderMarkdown, isMarkdown, imageResolver,
} from '$lib/markdown.js';

describe('splitFrontMatter', () => {
  it('separates a YAML block from the body', () => {
    const { frontMatter, body, attributes } =
      splitFrontMatter('---\ntitle: Hello\ntags: a, b\n---\n# Body\n');
    expect(frontMatter).toContain('title: Hello');
    expect(body).toBe('# Body\n');
    expect(attributes).toEqual({ title: 'Hello', tags: 'a, b' });
  });

  it('strips surrounding quotes from values', () => {
    expect(splitFrontMatter('---\ntitle: "Quoted"\n---\nx').attributes.title).toBe('Quoted');
  });

  it('leaves a document without front matter untouched', () => {
    const text = '# Just a heading\n\nBody';
    const { frontMatter, body } = splitFrontMatter(text);
    expect(frontMatter).toBe('');
    expect(body).toBe(text);
  });

  it('does not treat a mid-document rule as front matter', () => {
    const text = 'Intro\n\n---\n\nMore';
    expect(splitFrontMatter(text).body).toBe(text);
  });

  it('handles CRLF line endings', () => {
    expect(splitFrontMatter('---\r\ntitle: X\r\n---\r\nbody').attributes.title).toBe('X');
  });
});

describe('note titles', () => {
  it('strips the markdown extension', () => {
    expect(noteName('Shopping.md')).toBe('Shopping');
    expect(noteName('Notes.markdown')).toBe('Notes');
    expect(noteName('plain')).toBe('plain');
  });

  it('finds an ATX heading', () => {
    expect(firstHeading('# Hello\n\ntext')).toBe('Hello');
    expect(firstHeading('text\n\n## Later heading')).toBe('Later heading');
    expect(firstHeading('### Trailing hashes ###')).toBe('Trailing hashes');
  });

  it('finds a setext heading', () => {
    expect(firstHeading('Underlined\n=========\n')).toBe('Underlined');
  });

  it('prefers front matter, then heading, then filename', () => {
    expect(noteTitle('f.md', '---\ntitle: FM\n---\n# H')).toBe('FM');
    expect(noteTitle('f.md', '# H\n\nbody')).toBe('H');
    expect(noteTitle('My File.md', 'no heading here')).toBe('My File');
  });

  it('never returns empty', () => {
    expect(noteTitle('', '')).toBe('Untitled');
  });
});

describe('excerpt', () => {
  it('strips markdown syntax down to readable text', () => {
    expect(excerpt('# Title\n\nSome **bold** and `code` and [a link](http://x)'))
      .toBe('Some bold and code and a link');
  });

  it('ignores front matter and fenced code', () => {
    expect(excerpt('---\ntitle: T\n---\n```js\nconst x = 1;\n```\nAfter')).toBe('After');
  });

  it('truncates with an ellipsis', () => {
    const out = excerpt('x '.repeat(200), 20);
    expect(out.length).toBeLessThanOrEqual(21);
    expect(out.endsWith('…')).toBe(true);
  });
});

describe('isMarkdown', () => {
  it('recognises the usual extensions', () => {
    for (const name of ['a.md', 'a.MD', 'a.markdown', 'a.mdown', 'a.mkd']) {
      expect(isMarkdown({ name })).toBe(true);
    }
  });

  it('rejects everything else', () => {
    expect(isMarkdown({ name: 'a.txt' })).toBe(false);
    expect(isMarkdown({ name: 'md' })).toBe(false);
    expect(isMarkdown(null)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Rendering.
//
// This is the security boundary that matters: a note renders in the *parent*
// document, not the sandboxed iframe mail uses, so anything the sanitiser lets
// through would execute as first-party script.
// ---------------------------------------------------------------------------

describe('renderMarkdown', () => {
  it('renders ordinary markdown', () => {
    const html = renderMarkdown('# Title\n\nSome **bold** text.');
    expect(html).toContain('<h1');
    expect(html).toContain('<strong>bold</strong>');
  });

  it('renders GFM tables and task lists', () => {
    expect(renderMarkdown('| a | b |\n|---|---|\n| 1 | 2 |')).toContain('<table>');
    // Rendered as a styled span, not an <input> — the sanitiser forbids form
    // controls, and a stripped checkbox made done and todo indistinguishable.
    const tasks = renderMarkdown('- [x] done\n- [ ] todo');
    expect(tasks).toContain('task-check--done');
    expect(tasks).toContain('task-check"');
    expect(tasks).not.toContain('<input');
  });

  it('omits front matter from the output', () => {
    const html = renderMarkdown('---\ntitle: Secret\n---\n\nBody text');
    expect(html).toContain('Body text');
    expect(html).not.toContain('title: Secret');
  });

  it('strips script tags', () => {
    const html = renderMarkdown('Hi\n\n<script>alert(1)<\/script>');
    expect(html).not.toContain('<script');
    expect(html).not.toContain('alert(1)');
  });

  it('strips inline event handlers', () => {
    const html = renderMarkdown('<img src=x onerror="alert(1)">');
    expect(html).not.toContain('onerror');
  });

  it('strips javascript: URLs', () => {
    const html = renderMarkdown('[click](javascript:alert(1))');
    expect(html).not.toContain('javascript:');
  });

  it('strips iframes and objects', () => {
    const html = renderMarkdown('<iframe src="http://evil"></iframe><object data="x"></object>');
    expect(html).not.toContain('<iframe');
    expect(html).not.toContain('<object');
  });

  it('strips form controls', () => {
    const html = renderMarkdown('<form action="http://evil"><input name="p"></form>');
    expect(html).not.toContain('<form');
    expect(html).not.toContain('<input name');
  });

  it('hardens links against tab-nabbing and referrer leaks', () => {
    const html = renderMarkdown('[x](https://example.com)');
    expect(html).toContain('target="_blank"');
    expect(html).toContain('noopener');
    expect(html).toContain('noreferrer');
  });

  it('keeps images, since a note is the user\'s own document', () => {
    expect(renderMarkdown('![alt](https://example.com/a.png)')).toContain('<img');
  });

  it('handles empty and nullish input', () => {
    expect(renderMarkdown('')).toBe('');
    expect(renderMarkdown(null)).toBe('');
    expect(renderMarkdown(undefined)).toBe('');
  });
});


// ---------------------------------------------------------------------------
// Image resolution
//
// Real-world notes reference attachments by relative path. Verbatim examples
// from an Azure DevOps wiki export.
// ---------------------------------------------------------------------------

describe('image rendering', () => {
  const resolve = imageResolver((path) =>
    path.startsWith('.attachments.248538/')
      ? `/api/files/blob/B1?name=${encodeURIComponent(path)}&inline=true`
      : null);

  it('rewrites a percent-encoded wiki attachment to a blob URL', () => {
    const md = '![GET BUY IN FROM MANAGERS FIRST](.attachments.248538/image%20%284%29.png)';
    const html = renderMarkdown(md, { resolveImage: resolve });
    expect(html).toContain('<img');
    expect(html).toContain('/api/files/blob/B1');
    expect(html).toContain('alt="GET BUY IN FROM MANAGERS FIRST"');
    // The unresolved relative path must not survive into the document.
    expect(html).not.toContain('src=".attachments');
  });

  it('marks an attachment that is not in the file store', () => {
    const html = renderMarkdown('![alt](missing/nope.png)', { resolveImage: resolve });
    expect(html).toContain('note-missing-image');
    expect(html).not.toContain('<img');
  });

  it('passes absolute and inline sources straight through', () => {
    expect(renderMarkdown('![a](https://example.com/x.png)', { resolveImage: resolve }))
      .toContain('src="https://example.com/x.png"');
    expect(renderMarkdown('![a](data:image/png;base64,AAA)', { resolveImage: resolve }))
      .toContain('data:image/png');
  });

  it('still refuses a javascript: image source', () => {
    const html = renderMarkdown('![a](javascript:alert(1))', { resolveImage: resolve });
    expect(html).not.toContain('javascript:');
  });

  it('escapes a hostile alt text rather than emitting markup', () => {
    const html = renderMarkdown(
      '![" onerror="alert(1)](.attachments.248538/x.png)', { resolveImage: resolve });

    // Assert on the parsed DOM, not the string: "onerror" legitimately appears
    // inside the alt *value* as text. What matters is that the quote was
    // escaped so the attribute could not be broken out of, leaving no real
    // event-handler attribute on the element.
    const host = document.createElement('div');
    host.innerHTML = html;
    const img = host.querySelector('img');
    expect(img).not.toBeNull();
    expect(img.hasAttribute('onerror')).toBe(false);
    expect(img.getAttributeNames().some((n) => n.startsWith('on'))).toBe(false);
    expect(img.getAttribute('alt')).toContain('onerror');   // inert text
  });

  it('renders without a resolver, leaving the href as written', () => {
    // The Files preview may render before the node list has loaded.
    expect(renderMarkdown('![a](https://example.com/x.png)')).toContain('<img');
  });
});
