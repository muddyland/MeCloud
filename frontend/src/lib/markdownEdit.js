/**
 * Markdown editing transforms.
 *
 * Pure: every function takes `{ text, start, end }` and returns a new one, so
 * the awkward parts — toggling off, replacing a heading level rather than
 * stacking one, keeping the selection sensible afterwards — are testable
 * without a DOM or a component.
 */

/** Expand a selection to cover whole lines. */
export function lineRange(text, start, end) {
  const from = text.lastIndexOf('\n', Math.max(0, start - 1)) + 1;
  let to = text.indexOf('\n', end);
  if (to === -1) to = text.length;
  return [from, to];
}

/**
 * Toggle a paired inline marker around the selection.
 *
 * Handles both shapes an editor sees: the markers sitting just outside the
 * selection (the usual result of a previous toggle) and inside it (the result
 * of selecting the formatted text including its markers).
 */
function runLengthBefore(text, index, ch) {
  let n = 0;
  while (index - n - 1 >= 0 && text[index - n - 1] === ch) n += 1;
  return n;
}

function runLengthAfter(text, index, ch) {
  let n = 0;
  while (index + n < text.length && text[index + n] === ch) n += 1;
  return n;
}

export function toggleWrap(marker, { text, start, end }, placeholder = '') {
  const len = marker.length;
  const ch = marker[0];
  const selected = text.slice(start, end);

  /*
   * How many marker characters actually run against the selection, rather than
   * just whether `len` of them are present.
   *
   * `*` is a prefix of `**`, so a plain equality check treats the inner
   * asterisk of bold text as an italic wrapper: applying italic to `**hi**`
   * stripped one asterisk from each side and produced `*hi*` — it un-bolded
   * instead of adding italics. Comparing run lengths distinguishes the two.
   */
  const runBefore = runLengthBefore(text, start, ch);
  const runAfter = runLengthAfter(text, end, ch);
  const wrapsSelection = runBefore >= len && runAfter >= len
    // A single-character marker only counts when the run is exactly one;
    // longer means it belongs to a bigger marker.
    && !(len === 1 && (runBefore > 1 || runAfter > 1));

  // Markers immediately outside the selection — unwrap them.
  if (wrapsSelection) {
    return {
      text: text.slice(0, start - len) + selected + text.slice(end + len),
      start: start - len,
      end: end - len,
    };
  }

  // Markers inside the selection — strip them, subject to the same run-length
  // rule so italic does not eat half of a bold pair.
  const innerBefore = runLengthAfter(selected, 0, ch);
  const innerAfter = runLengthBefore(selected, selected.length, ch);
  const selectionIsWrapped = selected.length >= len * 2
    && innerBefore >= len && innerAfter >= len
    && !(len === 1 && (innerBefore > 1 || innerAfter > 1));

  if (selectionIsWrapped) {
    const inner = selected.slice(len, -len);
    return {
      text: text.slice(0, start) + inner + text.slice(end),
      start,
      end: start + inner.length,
    };
  }

  // Nothing to toggle: wrap. With no selection, drop in a placeholder and
  // select it so typing replaces it.
  const body = selected || placeholder;
  return {
    text: text.slice(0, start) + marker + body + marker + text.slice(end),
    start: start + len,
    end: start + len + body.length,
  };
}

// Prefixes that cannot coexist on one line: applying one replaces another.
const BLOCK_PREFIX = /^(\s*)(?:(#{1,6}\s+)|(>\s*)|(-\s+\[[ xX]\]\s+)|([-*+]\s+)|(\d+\.\s+))?/;

/** Strip any block prefix from a line, keeping its indentation. */
export function stripPrefix(line) {
  const match = line.match(BLOCK_PREFIX);
  const indent = match?.[1] ?? '';
  return indent + line.slice(match?.[0].length ?? 0);
}

/**
 * Apply a block prefix to every line the selection touches.
 *
 * Toggles off when every line already has exactly this prefix — the behaviour
 * people expect from a heading button pressed twice.
 *
 * @param {(index: number) => string} prefixFor lets ordered lists number themselves
 */
export function toggleLinePrefix(prefixFor, { text, start, end }) {
  const [from, to] = lineRange(text, start, end);
  const block = text.slice(from, to);
  const lines = block.split('\n');

  const allPrefixed = lines.every((line, i) => {
    const wanted = prefixFor(i);
    return line.trimStart().startsWith(wanted.trimStart()) && wanted.trim() !== '';
  });

  const next = lines
    .map((line, i) => (allPrefixed ? stripPrefix(line) : stripPrefix(line) === ''
      ? prefixFor(i)
      : prefixFor(i) + stripPrefix(line)))
    .join('\n');

  return {
    text: text.slice(0, from) + next + text.slice(to),
    start: from,
    end: from + next.length,
  };
}

/** Insert a block on its own lines, with blank-line separation. */
export function insertBlock(block, { text, start, end }, selectOffset = null) {
  const before = text.slice(0, start);
  const after = text.slice(end);
  const lead = before === '' || before.endsWith('\n\n') ? '' : before.endsWith('\n') ? '\n' : '\n\n';
  const tail = after.startsWith('\n') || after === '' ? '' : '\n';
  const inserted = lead + block + tail;

  const caret = start + lead.length + (selectOffset ?? block.length);
  return {
    text: before + inserted + after,
    start: caret,
    end: selectOffset === null ? caret : caret,
  };
}

/** Wrap the selection in a link, selecting whichever part needs typing. */
export function insertLink({ text, start, end }) {
  const selected = text.slice(start, end);
  const label = selected || 'text';
  const inserted = `[${label}](url)`;
  const urlStart = start + label.length + 3;
  return {
    text: text.slice(0, start) + inserted + text.slice(end),
    // Select `url` when there was a label, otherwise select the label first.
    start: selected ? urlStart : start + 1,
    end: selected ? urlStart + 3 : start + 1 + label.length,
  };
}

/** Fence the selection as a code block. */
export function insertCodeBlock({ text, start, end }) {
  const selected = text.slice(start, end) || 'code';
  const block = '```\n' + selected + '\n```';
  const result = insertBlock(block, { text, start, end }, 4);
  return { ...result, end: result.start + selected.length };
}

const ACTIONS = {
  bold:      (state) => toggleWrap('**', state, 'bold text'),
  italic:    (state) => toggleWrap('*', state, 'italic text'),
  strike:    (state) => toggleWrap('~~', state, 'struck text'),
  code:      (state) => toggleWrap('`', state, 'code'),
  h1:        (state) => toggleLinePrefix(() => '# ', state),
  h2:        (state) => toggleLinePrefix(() => '## ', state),
  h3:        (state) => toggleLinePrefix(() => '### ', state),
  quote:     (state) => toggleLinePrefix(() => '> ', state),
  ul:        (state) => toggleLinePrefix(() => '- ', state),
  ol:        (state) => toggleLinePrefix((i) => `${i + 1}. `, state),
  task:      (state) => toggleLinePrefix(() => '- [ ] ', state),
  link:      (state) => insertLink(state),
  codeblock: (state) => insertCodeBlock(state),
  hr:        (state) => insertBlock('---', state),
};

/**
 * Apply a named action.
 * @returns {{text: string, start: number, end: number}} unchanged state for an
 *          unknown action, so a bad name can never corrupt the document.
 */
export function applyAction(action, state) {
  const fn = ACTIONS[action];
  return fn ? fn(state) : state;
}

export const ACTION_NAMES = Object.keys(ACTIONS);
