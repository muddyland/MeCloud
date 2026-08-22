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

// ── Geometry helpers ──────────────────────────────────────────────────────────
//
// Everything is drawn into a supersampled buffer and box-filtered down at the
// end. Antialiasing every shape by hand would be far more code, and circles and
// arcs look visibly ragged at 192px without it.

const SS = 4;                    // supersampling factor

function set(px, size, x, y, r, g, b, a = 255) {
  if (x < 0 || x >= size || y < 0 || y >= size) return;
  const i = (y * size + x) * 4;
  px[i] = r; px[i + 1] = g; px[i + 2] = b; px[i + 3] = a;
}

function fillRect(px, size, x1, y1, x2, y2, [r, g, b]) {
  for (let y = Math.max(0, y1 | 0); y < Math.min(size, y2 | 0); y++)
    for (let x = Math.max(0, x1 | 0); x < Math.min(size, x2 | 0); x++)
      set(px, size, x, y, r, g, b);
}

function fillCircle(px, size, cx, cy, radius, [r, g, b]) {
  const r2 = radius * radius;
  for (let y = Math.max(0, Math.floor(cy - radius)); y <= Math.min(size - 1, Math.ceil(cy + radius)); y++) {
    const dy = y - cy;
    const span = Math.sqrt(Math.max(0, r2 - dy * dy));
    for (let x = Math.max(0, Math.floor(cx - span)); x <= Math.min(size - 1, Math.ceil(cx + span)); x++) {
      set(px, size, x, y, r, g, b);
    }
  }
}

function fillRoundRect(px, size, x, y, w, h, radius, colour) {
  const rad = Math.min(radius, w / 2, h / 2);
  fillRect(px, size, x + rad, y, x + w - rad, y + h, colour);
  fillRect(px, size, x, y + rad, x + w, y + h - rad, colour);
  fillCircle(px, size, x + rad, y + rad, rad, colour);
  fillCircle(px, size, x + w - rad, y + rad, rad, colour);
  fillCircle(px, size, x + rad, y + h - rad, rad, colour);
  fillCircle(px, size, x + w - rad, y + h - rad, rad, colour);
}

/** The upper half of a ring — the lock's shackle. */
function fillUpperArc(px, size, cx, cy, outer, inner, [r, g, b]) {
  for (let y = Math.max(0, Math.floor(cy - outer)); y <= Math.min(size - 1, Math.ceil(cy)); y++) {
    for (let x = Math.max(0, Math.floor(cx - outer)); x <= Math.min(size - 1, Math.ceil(cx + outer)); x++) {
      const d = Math.hypot(x - cx, y - cy);
      if (d <= outer && d >= inner) set(px, size, x, y, r, g, b);
    }
  }
}

/** Box-filter the supersampled buffer down to the final size. */
function downsample(src, bigSize, outSize) {
  const out = Buffer.alloc(outSize * outSize * 4);
  const n = SS * SS;
  for (let y = 0; y < outSize; y++) {
    for (let x = 0; x < outSize; x++) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const i = (((y * SS) + sy) * bigSize + (x * SS) + sx) * 4;
          r += src[i]; g += src[i + 1]; b += src[i + 2]; a += src[i + 3];
        }
      }
      const o = (y * outSize + x) * 4;
      out[o] = r / n; out[o + 1] = g / n; out[o + 2] = b / n; out[o + 3] = a / n;
    }
  }
  return out;
}

// ── The mark: a cloud with a padlock ─────────────────────────────────────────

const BLUE  = [37, 99, 235];     // blue-600, matching theme_color
const WHITE = [255, 255, 255];

/**
 * Coordinates are fractions of the content box, so the same drawing works at
 * every size and for the maskable variant, which has to keep its content inside
 * the centre 80% because launchers crop it to arbitrary shapes.
 */
function drawCloudLock(px, size, maskable) {
  const pad = Math.round(size * (maskable ? 0.14 : 0.035));
  const box = size - pad * 2;
  const X = (f) => pad + f * box;
  const Y = (f) => pad + f * box;
  const S = (f) => f * box;

  fillRect(px, size, 0, 0, size, size, BLUE);

  // Cloud: three lobes of deliberately different sizes over a wide flat base.
  // Equal lobes read as a single blob at icon sizes rather than as a cloud.
  fillRoundRect(px, size, X(0.11), Y(0.480), S(0.78), S(0.240), S(0.100), WHITE);
  fillCircle(px, size, X(0.300), Y(0.485), S(0.160), WHITE);
  fillCircle(px, size, X(0.500), Y(0.395), S(0.215), WHITE);
  fillCircle(px, size, X(0.710), Y(0.495), S(0.170), WHITE);

  // Padlock, knocked out of the cloud so it reads at favicon size without
  // introducing a third colour. Sized to sit wholly inside the white, since
  // anything overhanging the cloud would be blue on blue and invisible.
  fillUpperArc(px, size, X(0.50), Y(0.505), S(0.098), S(0.062), BLUE);
  fillRoundRect(px, size, X(0.50) - S(0.108), Y(0.500), S(0.216), S(0.165), S(0.028), BLUE);

  // Keyhole, back in white so the body does not read as a plain block.
  fillCircle(px, size, X(0.50), Y(0.556), S(0.024), WHITE);
  fillRect(px, size, X(0.50) - S(0.014), Y(0.556), X(0.50) + S(0.014), Y(0.618), WHITE);
}

function renderIcon(size, maskable) {
  const big = size * SS;
  const buf = Buffer.alloc(big * big * 4);
  drawCloudLock(buf, big, maskable);
  return downsample(buf, big, size);
}

// ── Generate files ────────────────────────────────────────────────────────────

const icons = [
  { name: 'icon-192.png',          size: 192, maskable: false },
  { name: 'icon-512.png',          size: 512, maskable: false },
  { name: 'icon-maskable-192.png', size: 192, maskable: true  },
  { name: 'icon-maskable-512.png', size: 512, maskable: true  },
  { name: 'apple-touch-icon.png',  size: 180, maskable: false },
  { name: 'favicon-32.png',        size: 32,  maskable: false },
];

for (const { name, size, maskable } of icons) {
  const path = `${OUT}/${name}`;
  const pixels = renderIcon(size, maskable);
  writeFileSync(path, makePNG(size, (px) => pixels.copy(px)));
  console.log(`  ${path}`);
}
