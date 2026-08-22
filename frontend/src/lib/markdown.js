import { marked } from 'marked';
import { sanitizeEmailHtml } from './sanitize.js';

/**
 * Markdown rendering for notes.
 *
 * Worth being explicit about the threat model, because it differs from mail:
 * email HTML renders inside a sandboxed iframe with no allow-scripts, so the
 * sanitiser there is defence in depth. A note renders in the *parent document*,
 * where script would run as first-party code. Here the sanitiser is the only
 * boundary, so every path goes through it — there is no "trusted" note, since a
 * .md file can arrive by any sync tool that writes to the file store.
 */

marked.setOptions({
  gfm: true,          // tables, strikethrough, task lists, autolinks
  breaks: true,       // a single newline is a line break, which is what note-takers expect
});

/*
 * GFM task lists emit `<input type="checkbox" disabled>`, and the sanitiser
 * forbids form controls — correct for mail, but it stripped the checkbox and
 * left `<li> done</li>`, so a ticked item and an unticked one rendered
 * identically. Rather than allowing `input` through the shared sanitiser just
 * for this, the marker is emitted as a span that the CSS draws as a checkbox.
 * Nothing interactive reaches the document either way.
 */
const renderer = new marked.Renderer();
renderer.checkbox = (token) => {
  const checked = typeof token === 'object' ? token?.checked : token;
  return `<span class="task-check${checked ? ' task-check--done' : ''}"></span>`;
};
marked.use({ renderer });

/**
 * Split leading YAML front matter from the body.
 *
 * Plenty of existing note collections (Obsidian, Jekyll, Logseq) start files
 * with a `---` block. Rendering it verbatim as a horizontal rule and a run of
 * stray text is the classic tell of a Markdown viewer that does not know about
 * it, so it is separated and kept out of the rendered body.
 *
 * @returns {{ frontMatter: string, body: string, attributes: Record<string,string> }}
 */
export function splitFrontMatter(text) {
  const source = String(text ?? '');
  const match = source.match(/^﻿?---[ \t]*\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/);
  if (!match) return { frontMatter: '', body: source, attributes: {} };

  const frontMatter = match[1];
  const body = source.slice(match[0].length);

  // Deliberately not a YAML parser — just the flat `key: value` pairs that
  // cover title/tags/date in practice. Anything else is preserved in
  // `frontMatter` and simply not interpreted.
  const attributes = {};
  for (const line of frontMatter.split(/\r?\n/)) {
    const pair = line.match(/^([A-Za-z0-9_-]+)\s*:\s*(.*)$/);
    if (pair) attributes[pair[1].toLowerCase()] = pair[2].trim().replace(/^["']|["']$/g, '');
  }
  return { frontMatter, body, attributes };
}

/** The first ATX/setext heading in the body, if there is one. */
export function firstHeading(body) {
  const atx = String(body ?? '').match(/^[ \t]{0,3}#{1,6}[ \t]+(.+?)[ \t]*#*[ \t]*$/m);
  if (atx) return atx[1].trim();
  const setext = String(body ?? '').match(/^[ \t]{0,3}(\S.*?)[ \t]*\r?\n[ \t]{0,3}=+[ \t]*$/m);
  return setext ? setext[1].trim() : '';
}

/** Strip the `.md` extension for display. */
export function noteName(filename) {
  return String(filename ?? '').replace(/\.(md|markdown|mdown|mkd)$/i, '');
}

/**
 * The title to show for a note: front-matter `title`, else the first heading,
 * else the filename. The filename always wins as a fallback so a note is never
 * nameless in the list.
 */
export function noteTitle(filename, text) {
  const { body, attributes } = splitFrontMatter(text);
  return attributes.title || firstHeading(body) || noteName(filename) || 'Untitled';
}

/** A one-line plain-text preview for the note list. */
export function excerpt(text, length = 120) {
  const { body } = splitFrontMatter(text);
  const plain = String(body ?? '')
    .replace(/^[ \t]{0,3}#{1,6}[ \t]+.*$/gm, ' ')      // headings
    .replace(/```[\s\S]*?```/g, ' ')                    // fenced code
    .replace(/`([^`]*)`/g, '$1')                        // inline code keeps its text
    .replace(/!\[[^\]]*\]\([^)]*\)/g, ' ')              // images
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')            // links keep their text
    .replace(/^[ \t]{0,3}>[ \t]?/gm, ' ')               // block quotes
    .replace(/^[ \t]{0,3}[-*+][ \t]+/gm, ' ')           // bullets
    .replace(/[*_~]/g, '')                              // emphasis marks
    .replace(/\s+/g, ' ')
    .trim();
  return plain.length > length ? `${plain.slice(0, length).trimEnd()}…` : plain;
}

/**
 * Render Markdown to sanitised HTML.
 *
 * Reuses the sanitiser the mail client already relies on — same forbidden tag
 * list, same link hardening (`target=_blank`, `rel="noopener noreferrer
 * nofollow"`). Remote images are allowed here, unlike mail: these are the
 * user's own notes rather than something a stranger sent, and blocking images
 * in your own document would be surprising.
 */
export function renderMarkdown(text) {
  const { body } = splitFrontMatter(text);
  const raw = marked.parse(body ?? '');
  return sanitizeEmailHtml(raw, { allowRemote: true }).html;
}

/** True when a FileNode looks like a Markdown document. */
export function isMarkdown(node) {
  if (!node?.name) return false;
  return /\.(md|markdown|mdown|mkd)$/i.test(node.name);
}
