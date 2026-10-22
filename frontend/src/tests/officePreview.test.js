// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import JSZip from 'jszip';
import {
  readSpreadsheet, parseCsv, sniffDelimiter, columnIndex, columnLabel,
  formatNumber, isDateFormat, serialToText, finishGrid, MAX_ROWS,
} from '$lib/sheetPreview.js';
import { readPresentation } from '$lib/slidePreview.js';
import { sniffRasterImage, resolveTarget, OfficeReadError } from '$lib/officeZip.js';
import { isSpreadsheet, isPresentation, isPreviewable } from '$lib/fileTypes.js';

const fixture = (name) => {
  const buf = readFileSync(resolve('src/tests/fixtures', name));
  return buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength);
};
const enc = (s) => new TextEncoder().encode(s).buffer;

describe('xlsx', () => {
  it('reads every sheet with values as text, dates as dates', async () => {
    const { sheets } = await readSpreadsheet(fixture('budget.xlsx'), 'budget.xlsx');
    expect(sheets.map((s) => s.name)).toEqual(['Budget', 'Tides', 'Secret']);
    const [budget] = sheets;
    expect(budget.rows[0].slice(0, 5)).toEqual(['Item', 'Days', 'Cost', 'Booked', 'Paid']);
    expect(budget.rows[1].slice(0, 5)).toEqual(['Boat hire', '3', '1200', '2026-10-26', 'TRUE']);
    expect(budget.rows[2].slice(0, 5)).toEqual(['Plates', '', '340.3', '2026-10-28 14:30', 'FALSE']);
    expect(budget.rows[0][6]).toBe('06:12');
    expect(budget.rows[5][5]).toBe('far cell');
  });
  it('marks hidden sheets', async () => {
    const { sheets } = await readSpreadsheet(fixture('budget.xlsx'), 'budget.xlsx');
    expect(sheets.find((s) => s.name === 'Secret').hidden).toBe(true);
    expect(sheets.find((s) => s.name === 'Budget').hidden).toBe(false);
  });
});

describe('xlsx beyond the cap', () => {
  it('says the sheet was cut when a value sits past the row or column cap', async () => {
    const zip = await JSZip.loadAsync(fixture('budget.xlsx'));
    const sheet = await zip.file('xl/worksheets/sheet1.xml').async('string');
    const far = sheet.replace('</sheetData>',
      '<row r="5000"><c r="A5000" t="inlineStr"><is><t>far down</t></is></c></row></sheetData>');
    zip.file('xl/worksheets/sheet1.xml', far);
    const { sheets } = await readSpreadsheet(await zip.generateAsync({ type: 'arraybuffer' }), 'b.xlsx');
    expect(sheets[0].truncated).toBe(true);
    expect(JSON.stringify(sheets[0].rows)).not.toContain('far down');
    expect(sheets[0].rows[1][0]).toBe('Boat hire');
  });
  it('is not cut by styled but empty cells far away', async () => {
    const zip = await JSZip.loadAsync(fixture('budget.xlsx'));
    const sheet = await zip.file('xl/worksheets/sheet1.xml').async('string');
    zip.file('xl/worksheets/sheet1.xml', sheet.replace('</sheetData>', '<row r="9000"><c r="A9000" s="1"/></row></sheetData>'));
    const { sheets } = await readSpreadsheet(await zip.generateAsync({ type: 'arraybuffer' }), 'b.xlsx');
    expect(sheets[0].truncated).toBe(false);
  });
});

describe('ods', () => {
  it('reads displayed text and survives a million repeated blank rows', async () => {
    const { sheets } = await readSpreadsheet(fixture('survey.ods'), 'survey.ods');
    const [s] = sheets;
    expect(s.name).toBe('Survey');
    expect(s.rows[0]).toEqual(['Day', 'Plates', 'Weather']);
    expect(s.rows[1]).toEqual(['Monday', '6', 'Clear']);
    // The row after a million blank ones really is a million rows down:
    // it is past the cap, so it is not shown, and the sheet says it was cut.
    expect(s.rows).toHaveLength(2);
    expect(s.truncated).toBe(true);
  });
});

describe('csv', () => {
  it('parses quotes, escaped quotes, embedded newlines and CRLF', () => {
    expect(parseCsv('a,"b,c","say ""hi"""\r\n"multi\nline",2,3\n')).toEqual([
      ['a', 'b,c', 'say "hi"'], ['multi\nline', '2', '3'],
    ]);
  });
  it('sniffs the delimiter from the first line', () => {
    expect(sniffDelimiter('a;b;c\n1;2;3')).toBe(';');
    expect(sniffDelimiter('a\tb\n1\t2')).toBe('\t');
    expect(sniffDelimiter('"x;y",b,c')).toBe(',');
  });
  it('reads a csv file, dropping a BOM', async () => {
    const bytes = new Uint8Array([0xef, 0xbb, 0xbf, ...new TextEncoder().encode('Day;Low\nMon;06:12\n')]);
    const { sheets } = await readSpreadsheet(bytes.buffer, 'tides.csv');
    expect(sheets[0].rows).toEqual([['Day', 'Low'], ['Mon', '06:12']]);
  });
  it('caps a huge file', async () => {
    const big = Array.from({ length: MAX_ROWS + 50 }, (_, i) => `${i},x`).join('\n');
    const { sheets } = await readSpreadsheet(enc(big), 'big.csv');
    expect(sheets[0].rows).toHaveLength(MAX_ROWS);
    expect(sheets[0].truncated).toBe(true);
  });
});

describe('spreadsheet helpers', () => {
  it('converts between column letters and indexes', () => {
    expect(columnIndex('A1')).toBe(0);
    expect(columnIndex('AB12')).toBe(27);
    expect(columnLabel(0)).toBe('A');
    expect(columnLabel(27)).toBe('AB');
    expect(columnLabel(701)).toBe('ZZ');
  });
  it('writes numbers without float noise', () => {
    expect(formatNumber('340.30000000000001')).toBe('340.3');
    expect(formatNumber('1E-3')).toBe('0.001');
  });
  it('recognises date formats, and not currency or text', () => {
    expect(isDateFormat(14)).toBe(true);
    expect(isDateFormat(164, 'yyyy-mm-dd hh:mm')).toBe(true);
    expect(isDateFormat(165, '"£"#,##0.00')).toBe(false);
    expect(isDateFormat(166, '[Red]0.00')).toBe(false);
    expect(isDateFormat(167, '"days"0')).toBe(false);
    expect(isDateFormat(0, 'General')).toBe(false);
  });
  it('turns serial dates into text, in both date systems', () => {
    expect(serialToText(46321)).toBe('2026-10-26');
    expect(serialToText(0.25)).toBe('06:00');
    expect(serialToText(46321.5)).toBe('2026-10-26 12:00');
    expect(serialToText(44859, true)).toBe('2026-10-26');
  });
  it('trims empty edges', () => {
    expect(finishGrid([['a', '', ''], ['', '', ''], []])).toEqual({ rows: [['a']], truncated: false });
  });
});

describe('pptx', () => {
  it('outlines every slide by title and text, without speaker notes', async () => {
    const { slides } = await readPresentation(fixture('deck.pptx'), 'deck.pptx');
    expect(slides.map((s) => s.title)).toEqual(['Coastal Survey', 'Plan', 'Budget', 'Backup slide']);
    expect(slides[0].lines).toEqual(['October 2026']);
    expect(slides[1].lines).toEqual(['Boat on Monday', 'Plates by Wednesday']);
    expect(slides[2].lines).toEqual(['Boat', '£1,200', 'Plates', '£340']);
    expect(JSON.stringify(slides)).not.toContain('speaker notes');
  });
  it('marks hidden slides', async () => {
    const { slides } = await readPresentation(fixture('deck.pptx'), 'deck.pptx');
    expect(slides.map((s) => !!s.hidden)).toEqual([false, false, false, true]);
  });
  it('carries the cover thumbnail as a checked raster image', async () => {
    const { thumbnail } = await readPresentation(fixture('deck.pptx'), 'deck.pptx');
    expect(thumbnail.type).toBe('image/jpeg');
  });
});

describe('odp', () => {
  it('outlines slides by their title frame', async () => {
    const { slides, thumbnail } = await readPresentation(fixture('deck.odp'), 'deck.odp');
    expect(slides).toEqual([
      { number: 1, title: 'Coastal Survey', lines: ['October 2026'] },
      { number: 2, title: 'Plan', lines: ['Boat on Monday', 'Plates by Wednesday'] },
    ]);
    expect(thumbnail).toBeNull();
  });
});

describe('hostile files', () => {
  it('never uses a thumbnail that is not really a raster image', async () => {
    const zip = await JSZip.loadAsync(fixture('deck.pptx'));
    zip.file('docProps/thumbnail.jpeg', '<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"/>');
    const { thumbnail } = await readPresentation(await zip.generateAsync({ type: 'arraybuffer' }), 'x.pptx');
    expect(thumbnail).toBeNull();
  });
  it('refuses a part that inflates past the limit before inflating it', async () => {
    const zip = new JSZip();
    zip.file('content.xml', 'a'.repeat(45 * 1024 * 1024), { compression: 'DEFLATE' });
    const bytes = await zip.generateAsync({ type: 'arraybuffer', compression: 'DEFLATE' });
    expect(bytes.byteLength).toBeLessThan(1024 * 1024);
    await expect(readSpreadsheet(bytes, 'bomb.ods')).rejects.toThrow(/too large/);
  });
  it('says so for something that is not a document at all', async () => {
    await expect(readSpreadsheet(enc('not a zip'), 'x.xlsx')).rejects.toBeInstanceOf(OfficeReadError);
    await expect(readPresentation(enc('not a zip'), 'x.pptx')).rejects.toBeInstanceOf(OfficeReadError);
  });
  it('keeps relationship targets inside the package', () => {
    expect(resolveTarget('xl/workbook.xml', '../../../etc/passwd')).toBe('etc/passwd');
    expect(resolveTarget('xl/workbook.xml', 'worksheets/sheet1.xml')).toBe('xl/worksheets/sheet1.xml');
    expect(resolveTarget('xl/workbook.xml', '/xl/worksheets/sheet2.xml')).toBe('xl/worksheets/sheet2.xml');
  });
  it('sniffs only real raster signatures', () => {
    expect(sniffRasterImage(new Uint8Array([0x89, 0x50, 0x4e, 0x47]))).toBe('image/png');
    expect(sniffRasterImage(new TextEncoder().encode('<svg'))).toBeNull();
  });
});

describe('which files preview', () => {
  const node = (name, type = '') => ({ blobId: 'b', name, type });
  it('previews modern spreadsheets, csv and presentations', () => {
    for (const n of ['a.xlsx', 'a.xlsm', 'a.ods', 'a.csv', 'a.tsv']) expect(isSpreadsheet(node(n))).toBe(true);
    for (const n of ['a.pptx', 'a.ppsx', 'a.odp']) expect(isPresentation(node(n))).toBe(true);
    expect(isPreviewable(node('a.xlsx'))).toBe(true);
  });
  it('leaves binary and Apple formats as downloads', () => {
    for (const n of ['a.xls', 'a.numbers', 'a.ppt', 'a.key']) expect(isPreviewable(node(n))).toBe(false);
  });
});
