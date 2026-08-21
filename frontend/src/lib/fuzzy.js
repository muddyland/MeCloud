/**
 * Subsequence fuzzy matching for the command palette.
 *
 * Pure and self-contained so ranking can be reasoned about and tested without
 * a DOM. The scoring is deliberately simple but tuned for the one thing that
 * matters in a launcher: typing a few letters should surface the item you meant,
 * and a prefix match should always beat a match buried in the middle.
 */

const SCORE_START        = 24;   // match at the very start of the string
const SCORE_WORD_START   = 16;   // match at the start of a word ("New Folder" ← "f")
const SCORE_CONSECUTIVE  = 10;   // each character adjacent to the previous match
const SCORE_CHAR         = 2;    // baseline for any matched character
const PENALTY_GAP        = 1;    // per skipped character between matches
const PENALTY_GAP_MAX    = 12;   // a long gap should not dominate the score
const BONUS_EXACT        = 12;   // the query appears verbatim, mid-string
const BONUS_FULL_PREFIX  = 58;   // ...and at the very start, which is far better
const BONUS_ACRONYM      = 25;   // every matched char begins a word ("nf" → New Folder)

const SEPARATORS = new Set([' ', '-', '_', '/', '.', ':', '(', ')', '[', ']']);

function isWordStart(text, index) {
  if (index === 0) return true;
  const prev = text[index - 1];
  if (SEPARATORS.has(prev)) return true;
  // camelCase / PascalCase boundary
  return prev === prev.toLowerCase() && text[index] === text[index].toUpperCase()
    && prev !== text[index].toLowerCase();
}

/**
 * Score `text` against `query`.
 *
 * @returns {{score: number, indices: number[]}|null} null when `query` is not a
 *          subsequence of `text`. `indices` are positions in `text` that
 *          matched, for highlighting.
 */
export function fuzzyMatch(query, text) {
  const q = String(query ?? '');
  const t = String(text ?? '');
  if (!q) return { score: 0, indices: [] };
  if (!t) return null;

  const ql = q.toLowerCase();
  const tl = t.toLowerCase();

  // Walk the query as a subsequence of the text, greedily.
  const indices = [];
  let ti = 0;
  for (let qi = 0; qi < ql.length; qi += 1) {
    const found = tl.indexOf(ql[qi], ti);
    if (found === -1) return null;
    indices.push(found);
    ti = found + 1;
  }

  let score = 0;
  let previous = -1;
  for (const index of indices) {
    score += SCORE_CHAR;
    if (index === 0) score += SCORE_START;
    else if (isWordStart(t, index)) score += SCORE_WORD_START;

    if (previous >= 0) {
      if (index === previous + 1) score += SCORE_CONSECUTIVE;
      else score -= Math.min(PENALTY_GAP_MAX, (index - previous - 1) * PENALTY_GAP);
    }
    previous = index;
  }

  // An initialism is a deliberate, high-confidence signal — "nf" for
  // "New Folder" should beat "Confirm", which merely contains the letters "nf"
  // buried inside a word.
  if (indices.length > 1 && indices.every((i) => isWordStart(t, i))) {
    score += BONUS_ACRONYM;
  }

  const exactAt = tl.indexOf(ql);
  if (exactAt === 0) score += BONUS_FULL_PREFIX + BONUS_EXACT;
  else if (exactAt > 0) score += BONUS_EXACT;

  // Shorter targets are usually the better answer for the same match quality:
  // "Sent" should beat "Sent Items Archive 2019" for the query "sent".
  score -= Math.min(10, Math.floor(t.length / 12));

  return { score, indices };
}

/** Convenience wrapper: the score alone, or -1 when there is no match. */
export function fuzzyScore(query, text) {
  return fuzzyMatch(query, text)?.score ?? -1;
}

/**
 * Rank `items` against `query`.
 *
 * An item may expose extra searchable text (a folder's parent path, a contact's
 * email) via `keywords`; the best-scoring field wins so typing an email address
 * finds the contact without polluting the displayed label.
 *
 * Ties keep the original order, so a caller's preferred ordering survives — and
 * an empty query returns everything untouched.
 */
export function rankItems(query, items, { limit = 50 } = {}) {
  const q = String(query ?? '').trim();
  if (!q) return items.slice(0, limit);

  const scored = [];
  items.forEach((item, order) => {
    const fields = [item.label, ...(item.keywords ?? [])].filter(Boolean);
    let best = null;
    let bestOnLabel = null;
    for (const field of fields) {
      const match = fuzzyMatch(q, field);
      if (!match) continue;
      if (!best || match.score > best.score) best = match;
      if (field === item.label && (!bestOnLabel || match.score > bestOnLabel.score)) {
        bestOnLabel = match;
      }
    }
    if (!best) return;
    scored.push({
      item,
      order,
      score: best.score + (item.boost ?? 0),
      // Only highlight when the label itself matched; highlighting nothing is
      // better than highlighting the wrong characters.
      indices: bestOnLabel?.indices ?? [],
    });
  });

  scored.sort((a, b) => (b.score - a.score) || (a.order - b.order));
  return scored.slice(0, limit).map(({ item, indices }) => ({ ...item, indices }));
}

/** Split `text` into {text, hit} runs for highlight rendering. */
export function highlightRuns(text, indices) {
  const t = String(text ?? '');
  if (!indices?.length) return [{ text: t, hit: false }];
  const hits = new Set(indices);
  const runs = [];
  let current = '';
  let currentHit = hits.has(0);
  for (let i = 0; i < t.length; i += 1) {
    const hit = hits.has(i);
    if (hit !== currentHit && current) {
      runs.push({ text: current, hit: currentHit });
      current = '';
    }
    currentHit = hit;
    current += t[i];
  }
  if (current) runs.push({ text: current, hit: currentHit });
  return runs;
}
