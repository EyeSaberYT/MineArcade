// Generates the MineArcade pixel-art icon set (PNG + ICO) with no dependencies.
// Run with: npm run icons
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { deflateSync } from 'node:zlib';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const iconDir = join(root, 'src-tauri', 'icons');
const assetDir = join(root, 'src', 'assets');

// 16x16 art: o = outline, b = background, M = letter, space = transparent
const ART = [
  '   oooooooooo   ',
  '  obbbbbbbbbbo  ',
  ' obbbbbbbbbbbbo ',
  'obbbbbbbbbbbbbbo',
  'obbMMbbbbbbMMbbo',
  'obbMMMbbbbMMMbbo',
  'obbMMMMbbMMMMbbo',
  'obbMMbMMMMbMMbbo',
  'obbMMbbMMbbMMbbo',
  'obbMMbbbbbbMMbbo',
  'obbMMbbbbbbMMbbo',
  'obbMMbbbbbbMMbbo',
  'obbbbbbbbbbbbbbo',
  ' obbbbbbbbbbbbo ',
  '  obbbbbbbbbbo  ',
  '   oooooooooo   ',
];

const OUTLINE = [45, 226, 230];
const FILL = [26, 20, 51];
const TOP = [255, 61, 242];
const BOTTOM = [255, 210, 63];

function pixel(x, y) {
  switch (ART[y][x]) {
    case 'o':
      return [...OUTLINE, 255];
    case 'b':
      return [...FILL, 255];
    case 'M': {
      const t = (y - 4) / 7;
      return [0, 1, 2].map((i) => Math.round(TOP[i] + (BOTTOM[i] - TOP[i]) * t)).concat(255);
    }
    default:
      return [0, 0, 0, 0];
  }
}

const CRC_TABLE = new Uint32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}

function png(size) {
  const stride = size * 4 + 1;
  const raw = Buffer.alloc(stride * size);
  for (let y = 0; y < size; y++) {
    raw[y * stride] = 0; // filter: none
    for (let x = 0; x < size; x++) {
      const p = pixel(Math.floor((x * 16) / size), Math.floor((y * 16) / size));
      raw.set(p, y * stride + 1 + x * 4);
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw)),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

function ico(sizes) {
  const images = sizes.map(png);
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(images.length, 4);
  let offset = 6 + 16 * images.length;
  const entries = images.map((image, i) => {
    const entry = Buffer.alloc(16);
    const size = sizes[i] >= 256 ? 0 : sizes[i];
    entry[0] = size;
    entry[1] = size;
    entry.writeUInt16LE(1, 4); // planes
    entry.writeUInt16LE(32, 6); // bits per pixel
    entry.writeUInt32LE(image.length, 8);
    entry.writeUInt32LE(offset, 12);
    offset += image.length;
    return entry;
  });
  return Buffer.concat([header, ...entries, ...images]);
}

mkdirSync(iconDir, { recursive: true });
mkdirSync(assetDir, { recursive: true });
writeFileSync(join(iconDir, '32x32.png'), png(32));
writeFileSync(join(iconDir, '128x128.png'), png(128));
writeFileSync(join(iconDir, '128x128@2x.png'), png(256));
writeFileSync(join(iconDir, 'icon.png'), png(512));
writeFileSync(join(iconDir, 'icon.ico'), ico([16, 32, 48, 64, 128, 256]));
writeFileSync(join(assetDir, 'logo.png'), png(128));
console.log('Icons written to', iconDir);
