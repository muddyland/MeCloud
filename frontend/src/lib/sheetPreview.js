/**
 * Read-only spreadsheet previews: .xlsx/.xlsm, .ods, and .csv/.tsv.
 *
 * Values only. Each sheet becomes a grid of strings — what the cell holds,
 * with dates shown as dates and numbers without float noise — and nothing
 * else: no formatting, formulas, charts or images. The grid is rendered by a
 * Svelte component as text, so nothing from the file is ever treated as HTML.
 *
 * Big sheets are cut to MAX_ROWS x MAX_COLS and say so; a preview that hangs
 * the tab on a 500,000-row export is worse than one that shows the top.
 */
import { attr, byName, children, openZip, relId, readXml, relationships, OfficeReadError } from './officeZip.js';

export const MAX_ROWS = 1000;
export const MAX_COLS = 100;
const MAX_SHEETS = 50;

/**
 * @typedef {{ name: string, rows: string[][], truncated: boolean, hidden?: boolean }} Sheet
 * @returns {Promise<{ sheets: Sheet[] }>}
 */
export async function readSpreadsheet(bytes, name, deps = {}) {
  const ext = String(name ?? '').toLowerCase().split('.').pop();
  if (ext === 'csv' || ext === 'tsv') {
    return { sheets: [csvSheet(decodeText(bytes), name, ext === 'tsv' ? '\t' : null)] };
  }
  const doc = await openZip(bytes, deps);
  if (doc.zip.file('content.xml') && !doc.zip.file('xl/workbook.xml')) return readOds(doc);
  return readXlsx(doc);
}

// ── Grids ───────────────────────────────────────────────────────────────────

/** A grid with trailing empty rows and columns removed, then capped. */
export function finishGrid(rows) {
  let lastRow = -1;
  let lastCol = -1;
  rows.forEach((row, r) => {
    for (let c = (row?.length ?? 0) - 1; c >= 0; c--) {
      if (row[c] !== undefined && row[c] !== '') {
        lastRow = r;
        if (c > lastCol) lastCol = c;
        break;
      }
    }
  });
  const truncated = lastRow + 1 > MAX_ROWS || lastCol + 1 > MAX_COLS;
  const height = Math.min(lastRow + 1, MAX_ROWS);
  const width = Math.min(lastCol + 1, MAX_COLS);
  const out = [];
  for (let r = 0; r < height; r++) {
    const row = rows[r] ?? [];
    out.push(Array.from({ length: width }, (_, c) => row[c] ?? ''));
  }
  return { rows: out, truncated };
}

/** "A" -> 0, "AB" -> 27, from a cell reference like "AB12". */
export function columnIndex(ref) {
  const letters = String(ref ?? '').match(/^[A-Za-z]+/)?.[0] ?? '';
  let n = 0;
  for (const ch of letters.toUpperCase()) n = n * 26 + (ch.charCodeAt(0) - 64);
  return n - 1;
}

/** Spreadsheet column label for an index: 0 -> "A", 27 -> "AB". */
export function columnLabel(index) {
  let n = index + 1;
  let out = '';
  while (n > 0) {
    const r = (n - 1) % 26;
    out = String.fromCharCode(65 + r) + out;
    n = Math.floor((n - 1) / 26);
  }
  return out;
}

/** A number as a person would write it: no 0.30000000000000004. */
export function formatNumber(value) {
  const n = Number(value);
  if (!Number.isFinite(n)) return String(value ?? '');
  return String(Number(n.toPrecision(15)));
}

// ── Dates ───────────────────────────────────────────────────────────────────
//
// Excel stores a date as a plain number of days, and only the cell's number
// format says it is a date. Built-in formats 14-22 and 45-47 are dates and
// times; a custom format is one if it uses date or time letters outside
// quoted text and [colour]/[condition] sections.

const BUILTIN_DATE_FORMATS = new Set([14, 15, 16, 17, 18, 19, 20, 21, 22, 45, 46, 47]);

export function isDateFormat(id, code) {
  if (BUILTIN_DATE_FORMATS.has(Number(id))) return true;
  if (!code) return false;
  const bare = String(code)
    .replace(/"[^"]*"/g, '')        // quoted literals
    .replace(/\\./g, '')            // escaped characters
    .replace(/\[[^\]]*\]/g, '')     // [Red], [>100], [$-409]
    .replace(/_.|\*./g, '');        // padding directives
  return /[dmyhs]/i.test(bare) && !/^general$/i.test(bare.trim());
}

/** An Excel serial date as text: "2026-10-22", "2026-10-22 14:30" or "14:30". */
export function serialToText(serial, date1904 = false) {
  const n = Number(serial);
  if (!Number.isFinite(n)) return String(serial ?? '');
  const epoch = date1904 ? Date.UTC(1904, 0, 1) : Date.UTC(1899, 11, 30);
  const ms = Math.round(n * 86400000);
  const d = new Date(epoch + ms);
  const pad = (v) => String(v).padStart(2, '0');
  const day = `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`;
  const time = `${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`;
  if (n < 1 && n >= 0) return time;
  return Math.abs(n % 1) > 1e-9 ? `${day} ${time}` : day;
}

// ── .xlsx ───────────────────────────────────────────────────────────────────

function textOf(node) {
  // Rich text runs keep their text in <t>; phonetic guides (<rPh>) are not
  // part of the value.
  return byName(node, 't').filter((t) => t.parentNode?.localName !== 'rPh')
    .map((t) => t.textContent).join('');
}

async function readXlsx(doc) {
  const workbook = await readXml(doc, 'xl/workbook.xml');
  if (!workbook) throw new OfficeReadError('This spreadsheet could not be read.');
  const rels = await relationships(doc, 'xl/workbook.xml');
  const date1904 = ['1', 'true'].includes(String(attr(byName(workbook, 'workbookPr')[0], 'date1904')));

  const shared = byName(await readXml(doc, 'xl/sharedStrings.xml'), 'si').map(textOf);

  const styles = await readXml(doc, 'xl/styles.xml');
  const customFormats = new Map(byName(styles, 'numFmt').map((f) => [attr(f, 'numFmtId'), attr(f, 'formatCode')]));
  const cellXfs = byName(styles, 'cellXfs')[0];
  const dateStyles = children(cellXfs, 'xf').map((xf) => {
    const id = attr(xf, 'numFmtId') ?? '0';
    return isDateFormat(id, customFormats.get(id));
  });

  const sheets = [];
  for (const sheet of byName(workbook, 'sheet').slice(0, MAX_SHEETS)) {
    const rel = rels.get(relId(sheet));
    if (!rel || rel.external) continue;
    const xml = await readXml(doc, rel.target);
    if (!xml) continue;   // a chartsheet or a damaged part: skip, keep the rest
    const rows = [];
    let cut = false;   // a value exists beyond the row or column cap
    let nextRow = 0;
    for (const row of byName(xml, 'row')) {
      const r = Number(attr(row, 'r')) - 1;
      const rowIndex = Number.isInteger(r) && r >= 0 ? r : nextRow;
      nextRow = rowIndex + 1;
      const cells = [];
      let nextCol = 0;
      for (const c of children(row, 'c')) {
        const ref = attr(c, 'r');
        const col = ref ? columnIndex(ref) : nextCol;
        nextCol = col + 1;
        if (rowIndex >= MAX_ROWS || col >= MAX_COLS) {
          // Only a cell with a value counts; styled empty cells are common.
          if (children(c, 'v').length || children(c, 'is').length) cut = true;
          continue;
        }
        cells[col] = cellValue(c, { shared, dateStyles, date1904 });
      }
      if (rowIndex >= MAX_ROWS) { if (cut) break; continue; }
      rows[rowIndex] = cells;
    }
    const grid = finishGrid(rows);
    sheets.push({ name: attr(sheet, 'name') ?? `Sheet ${sheets.length + 1}`,
                  hidden: ['hidden', 'veryHidden'].includes(attr(sheet, 'state')),
                  ...grid, truncated: grid.truncated || cut });
  }
  if (!sheets.length) throw new OfficeReadError('This spreadsheet has no sheets to show.');
  return { sheets };
}

function cellValue(c, { shared, dateStyles, date1904 }) {
  const type = attr(c, 't') ?? 'n';
  const v = children(c, 'v')[0]?.textContent ?? '';
  switch (type) {
    case 's': return shared[Number(v)] ?? '';
    case 'inlineStr': return textOf(children(c, 'is')[0]);
    case 'b': return v === '1' ? 'TRUE' : 'FALSE';
    case 'e': case 'str': return v;
    case 'd': return v.replace('T', ' ').replace(/(:\d\d)(\.\d+)?Z?$/, '$1');
    default: {
      if (v === '') return '';
      const style = Number(attr(c, 's') ?? 0);
      return dateStyles[style] ? serialToText(v, date1904) : formatNumber(v);
    }
  }
}

// ── .ods ────────────────────────────────────────────────────────────────────
//
// OpenDocument stores the text each cell displays, so no formats to decode.
// Rows and cells can be "repeated" — a blank sheet is often one row repeated
// a million times — so repeats are expanded only as far as the cap.

function odsCellText(cell) {
  const paras = children(cell, 'p');
  if (paras.length) return paras.map((p) => p.textContent).join('\n');
  return attr(cell, 'value') ?? attr(cell, 'date-value') ?? attr(cell, 'boolean-value') ?? '';
}

function odsRows(table) {
  // Rows can sit directly in the table or inside header/row groups; nested
  // tables (in a cell) are someone else's.
  return byName(table, 'table-row').filter((row) => {
    let p = row.parentNode;
    while (p && p !== table) { if (p.localName === 'table') return false; p = p.parentNode; }
    return true;
  });
}

async function readOds(doc) {
  const content = await readXml(doc, 'content.xml');
  if (!content) throw new OfficeReadError('This spreadsheet could not be read.');
  const sheets = [];
  for (const table of byName(content, 'table').filter((t) => t.parentNode?.localName === 'spreadsheet').slice(0, MAX_SHEETS)) {
    const rows = [];
    let cut = false;   // content exists beyond the row cap
    for (const row of odsRows(table)) {
      const cells = [];
      for (const cell of [...row.children].filter((c) => c.localName === 'table-cell' || c.localName === 'covered-table-cell')) {
        const text = cell.localName === 'covered-table-cell' ? '' : odsCellText(cell);
        const repeat = Math.max(1, Math.min(Number(attr(cell, 'number-columns-repeated')) || 1, MAX_COLS + 1));
        for (let i = 0; i < repeat && cells.length <= MAX_COLS; i++) cells.push(text);
      }
      const blank = cells.every((t) => t === '');
      const room = MAX_ROWS - rows.length;
      if (room <= 0) {
        // Past the cap: keep scanning only to learn whether anything is cut.
        if (!blank) { cut = true; break; }
        continue;
      }
      // Repeats count towards position even when blank, so a row after a
      // million blank ones is a million rows down, not the next line.
      const repeat = Math.max(1, Number(attr(row, 'number-rows-repeated')) || 1);
      for (let i = 0, n = Math.min(repeat, room); i < n; i++) rows.push(cells);
      if (repeat > room && !blank) cut = true;
    }
    const grid = finishGrid(rows);
    sheets.push({ name: attr(table, 'name') ?? `Sheet ${sheets.length + 1}`, ...grid,
                  truncated: grid.truncated || cut });
  }
  if (!sheets.length) throw new OfficeReadError('This spreadsheet has no sheets to show.');
  return { sheets };
}

// ── .csv / .tsv ─────────────────────────────────────────────────────────────

function decodeText(bytes) {
  const text = new TextDecoder('utf-8').decode(bytes);
  return text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
}

/** The likeliest delimiter, judged from the first line outside quotes. */
export function sniffDelimiter(text) {
  const firstLine = [];
  let quoted = false;
  for (const ch of text) {
    if (ch === '"') quoted = !quoted;
    else if (!quoted && (ch === '\n' || ch === '\r')) break;
    else if (!quoted) firstLine.push(ch);
  }
  const counts = [',', ';', '\t', '|'].map((d) => [d, firstLine.filter((c) => c === d).length]);
  counts.sort((a, b) => b[1] - a[1]);
  return counts[0][1] > 0 ? counts[0][0] : ',';
}

/** RFC 4180 parsing: quoted fields, "" escapes, CRLF or LF, capped. */
export function parseCsv(text, delimiter = ',') {
  const rows = [];
  let row = [];
  let field = '';
  let quoted = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (quoted) {
      if (ch === '"') {
        if (text[i + 1] === '"') { field += '"'; i++; } else quoted = false;
      } else field += ch;
      continue;
    }
    if (ch === '"' && field === '') quoted = true;
    else if (ch === delimiter) { row.push(field); field = ''; }
    else if (ch === '\n' || ch === '\r') {
      if (ch === '\r' && text[i + 1] === '\n') i++;
      row.push(field); field = '';
      rows.push(row); row = [];
      if (rows.length > MAX_ROWS) return rows;
    } else field += ch;
  }
  if (field !== '' || row.length) { row.push(field); rows.push(row); }
  return rows;
}

function csvSheet(text, name, delimiter) {
  const rows = parseCsv(text, delimiter ?? sniffDelimiter(text));
  return { name: String(name ?? 'Sheet'), ...finishGrid(rows) };
}
