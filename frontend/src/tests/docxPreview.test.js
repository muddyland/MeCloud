// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import JSZip from 'jszip';
import { renderDocxDocument } from '$lib/docxPreview.js';
import { isWordDocument, isPreviewable } from '$lib/fileTypes.js';

const W = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main';
const R = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const PNG = Uint8Array.from(atob(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='
), (c) => c.charCodeAt(0));

function image(rid) {
  return `<w:r><w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">
    <wp:extent cx="9525" cy="9525"/><wp:docPr id="1" name="p"/>
    <a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
      <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">
        <pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">
          <pic:nvPicPr><pic:cNvPr id="1" name="p"/><pic:cNvPicPr/></pic:nvPicPr>
          <pic:blipFill><a:blip r:embed="${rid}"/></pic:blipFill>
          <pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="9525" cy="9525"/></a:xfrm></pic:spPr>
        </pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>`;
}

/** A small but hostile .docx: a script link, a tracking image, a real image. */
async function hostileDocx() {
  const zip = new JSZip();
  zip.file('[Content_Types].xml', `<?xml version="1.0"?>
    <Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
      <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
      <Default Extension="xml" ContentType="application/xml"/>
      <Default Extension="png" ContentType="image/png"/>
      <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
    </Types>`);
  zip.file('_rels/.rels', `<?xml version="1.0"?>
    <Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
      <Relationship Id="r1" Type="${R}/officeDocument" Target="word/document.xml"/>
    </Relationships>`);
  zip.file('word/_rels/document.xml.rels', `<?xml version="1.0"?>
    <Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
      <Relationship Id="rJs" Type="${R}/hyperlink" Target="javascript:alert(document.domain)" TargetMode="External"/>
      <Relationship Id="rWeb" Type="${R}/hyperlink" Target="https://example.org/page" TargetMode="External"/>
      <Relationship Id="rImg" Type="${R}/image" Target="media/dot.png"/>
      <Relationship Id="rTrack" Type="${R}/image" Target="https://track.example/pixel.png" TargetMode="External"/>
    </Relationships>`);
  zip.file('word/media/dot.png', PNG);
  zip.file('word/document.xml', `<?xml version="1.0"?>
    <w:document xmlns:w="${W}" xmlns:r="${R}"><w:body>
      <w:p><w:r><w:t>Hello from Word</w:t></w:r></w:p>
      <w:p><w:hyperlink r:id="rJs"><w:r><w:t>click me</w:t></w:r></w:hyperlink></w:p>
      <w:p><w:hyperlink r:id="rWeb"><w:r><w:t>a real link</w:t></w:r></w:hyperlink></w:p>
      <w:p>${image('rImg')}${image('rTrack')}</w:p>
    </w:body></w:document>`);
  return zip.generateAsync({ type: 'arraybuffer' });
}

describe('renderDocxDocument', () => {
  it('renders the text of a document', async () => {
    const html = await renderDocxDocument(await hostileDocx());
    expect(html).toContain('Hello from Word');
    expect(html).toMatch(/^<!DOCTYPE html>|^<html/i);
  });

  it('never lets a script link through', async () => {
    const html = await renderDocxDocument(await hostileDocx());
    expect(html).not.toMatch(/javascript:/i);
    expect(html).not.toMatch(/<script/i);
  });

  it('loads images only from inside the file', async () => {
    const html = await renderDocxDocument(await hostileDocx());
    expect(html).not.toContain('track.example');
    expect(html).toMatch(/src="data:image\/png;base64,/);
  });

  it('keeps ordinary links, opening outside the frame', async () => {
    const html = await renderDocxDocument(await hostileDocx());
    expect(html).toMatch(/<a [^>]*href="https:\/\/example\.org\/page"[^>]*target="_blank"/);
  });

  it('rejects something that is not a Word file', async () => {
    await expect(renderDocxDocument(new TextEncoder().encode('not a zip').buffer)).rejects.toThrow();
  });
});

describe('isWordDocument', () => {
  const node = (name, type = '') => ({ blobId: 'b', name, type });
  it('recognises .docx by name or media type', () => {
    expect(isWordDocument(node('report.docx'))).toBe(true);
    expect(isWordDocument(node('blob', 'application/vnd.openxmlformats-officedocument.wordprocessingml.document'))).toBe(true);
    expect(isPreviewable(node('report.DOCX'))).toBe(true);
  });
  it('leaves formats it cannot read alone', () => {
    for (const name of ['old.doc', 'draft.odt', 'notes.rtf', 'essay.pages']) {
      expect(isWordDocument(node(name))).toBe(false);
      expect(isPreviewable(node(name))).toBe(false);
    }
  });
  it('is never true for a folder', () => {
    expect(isWordDocument({ name: 'x.docx', blobId: null })).toBe(false);
  });
});
