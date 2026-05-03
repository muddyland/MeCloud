#!/usr/bin/env node
/**
 * Generates PNG app icons using only Node.js built-ins (no npm deps).
 * Produces 192, 512, and 180-px icons for web manifest and Apple touch.
 */
import { deflateSync }        from 'zlib';
import { writeFileSync, mkdirSync } from 'fs';
import { fileURLToPath }      from 'url';
import { dirname, resolve }   from 'path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT = resolve(__dirname, '../static/icons');
mkdirSync(OUT, { recursive: true });

// ── PNG helpers ───────────────────────────────────────────────────────────────

function crc32(buf) {
  let c = 0xFFFFFFFF;
  for (const b of buf) { c ^= b; for (let i = 0; i < 8; i++) c = (c & 1) ? (0xEDB88320 ^ (c >>> 1)) : (c >>> 1); }
  return (c ^ 0xFFFFFFFF) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const t   = Buffer.from(type, 'ascii');
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(Buffer.concat([t, data])));
  return Buffer.concat([len, t, data, crc]);
}

function makePNG(size, draw) {
  const px = Buffer.alloc(size * size * 4, 0);
  draw(px, size);

  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0); ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; ihdr[9] = 6;  // 8-bit RGBA

  // Filter-byte-per-row (None = 0) + raw RGBA
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0;
    px.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  }

  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 6 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// ── Icon drawing ──────────────────────────────────────────────────────────────

function set(px, size, x, y, r, g, b, a = 255) {
  if (x < 0 || x >= size || y < 0 || y >= size) return;
  const i = (y * size + x) * 4;
  px[i] = r; px[i+1] = g; px[i+2] = b; px[i+3] = a;
}

function fillRect(px, size, x1, y1, x2, y2, r, g, b) {
  for (let y = y1; y < y2; y++)
    for (let x = x1; x < x2; x++)
      set(px, size, x, y, r, g, b);
}

// Blue-600 background + white envelope with V-flap and corner folds
function drawEnvelope(px, size, maskable = false) {
  // Maskable icons need content inside the centre 80% (safe zone)
  const pad   = maskable ? Math.round(size * 0.12) : Math.round(size * 0.06);
  const inner = size - pad * 2;

  // Background: blue-600 #2563eb
  fillRect(px, size, 0, 0, size, size, 37, 99, 235);

  // White envelope body
  const ex = pad + Math.round(inner * 0.07);
  const ey = pad + Math.round(inner * 0.22);
  const ew = inner - Math.round(inner * 0.14);
  const eh = Math.round(inner * 0.56);
  fillRect(px, size, ex, ey, ex + ew, ey + eh, 255, 255, 255);

  // Blue V-flap at top (triangle pointing downward)
  const cx = ex + Math.round(ew / 2);
  const flapD = Math.round(eh * 0.46);
  for (let dy = 0; dy <= flapD; dy++) {
    const hw = Math.round((ew / 2) * (1 - dy / flapD));
    for (let x = cx - hw; x <= cx + hw; x++)
      set(px, size, x, ey + dy, 37, 99, 235);
  }

  // Blue fold triangles at bottom corners (closed-envelope look)
  const foldD = Math.round(eh * 0.40);
  for (let dy = 0; dy < foldD; dy++) {
    const hw = Math.round((ew / 2) * (dy / foldD));
    fillRect(px, size, ex,              ey + eh - 1 - dy, ex + hw,       ey + eh - dy, 37, 99, 235);
    fillRect(px, size, ex + ew - hw, ey + eh - 1 - dy, ex + ew,      ey + eh - dy, 37, 99, 235);
  }
}

// ── Generate files ────────────────────────────────────────────────────────────

const icons = [
  { name: 'icon-192.png',          size: 192, maskable: false },
  { name: 'icon-512.png',          size: 512, maskable: false },
  { name: 'icon-maskable-192.png', size: 192, maskable: true  },
  { name: 'icon-maskable-512.png', size: 512, maskable: true  },
  { name: 'apple-touch-icon.png',  size: 180, maskable: false },
];

for (const { name, size, maskable } of icons) {
  const path = `${OUT}/${name}`;
  writeFileSync(path, makePNG(size, (px, s) => drawEnvelope(px, s, maskable)));
  console.log(`  ${path}`);
}
