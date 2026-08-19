// 生成占位图标（深色圆角底 + 三个“聊天点”）。
// 正式发布前请换成真实 logo：npx tauri icon path/to/logo.png（推荐 ≥1024px 方形 PNG）
import { deflateSync } from 'node:zlib';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const OUT = join(dirname(fileURLToPath(import.meta.url)), '..', 'src-tauri', 'icons');

// ---------- 最小 PNG 编码器（无依赖） ----------
const CRC_TABLE = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();

const crc32 = (buf) => {
  let c = -1;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
};

const chunk = (type, data) => {
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0);
  out.write(type, 4, 'ascii');
  data.copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
};

function png(size, pixel) {
  const raw = Buffer.alloc(size * (1 + size * 4));
  let o = 0;
  for (let y = 0; y < size; y++) {
    raw[o++] = 0; // filter: none
    for (let x = 0; x < size; x++) {
      const [r, g, b, a] = pixel(x, y, size);
      raw[o++] = r;
      raw[o++] = g;
      raw[o++] = b;
      raw[o++] = a;
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type: RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// ---------- ICO 容器（内嵌 PNG，Vista+ 支持） ----------
function ico(pngBuf, size) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(1, 4); // count
  const entry = Buffer.alloc(16);
  entry[0] = size >= 256 ? 0 : size; // 0 = 256
  entry[1] = size >= 256 ? 0 : size;
  entry.writeUInt16LE(1, 4); // planes
  entry.writeUInt16LE(32, 6); // bpp
  entry.writeUInt32LE(pngBuf.length, 8);
  entry.writeUInt32LE(22, 12);
  return Buffer.concat([header, entry, pngBuf]);
}

// ---------- 图案：深色渐变圆角底 + DeepSeek 蓝三点 ----------
const clamp01 = (v) => Math.max(0, Math.min(1, v));
const sdRoundRect = (px, py, hw, hh, r) => {
  const qx = Math.abs(px - hw) - hw + r;
  const qy = Math.abs(py - hh) - hh + r;
  return Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - r;
};

const pixel = (x, y, s) => {
  const c = s / 2;
  const rect = sdRoundRect(x + 0.5, y + 0.5, c, c, s * 0.2);
  if (rect > 1) return [0, 0, 0, 0];
  const t = clamp01(y / s);
  let r = 26 + (16 - 26) * t; // #1a1e2c → #10121b
  let g = 30 + (18 - 30) * t;
  let b = 44 + (27 - 44) * t;
  const dotR = s * 0.075;
  const dot = Math.min(
    ...[0.3, 0.5, 0.7].map(
      (fx) => Math.hypot(x + 0.5 - s * fx, y + 0.5 - s * 0.5) - dotR,
    ),
  );
  if (dot < 0) {
    [r, g, b] = [77, 107, 254]; // #4d6bfe
  } else if (dot < 1) {
    const k = 1 - dot;
    r += (77 - r) * k;
    g += (107 - g) * k;
    b += (254 - b) * k;
  }
  return [Math.round(r), Math.round(g), Math.round(b), Math.round(clamp01(0.5 - rect) * 255)];
};

mkdirSync(OUT, { recursive: true });
writeFileSync(join(OUT, '32x32.png'), png(32, pixel));
writeFileSync(join(OUT, '128x128.png'), png(128, pixel));
writeFileSync(join(OUT, '128x128@2x.png'), png(256, pixel));
writeFileSync(join(OUT, 'icon.png'), png(512, pixel)); // 备用：以后 `tauri icon` 的源图
writeFileSync(join(OUT, 'icon.ico'), ico(png(256, pixel), 256));
console.log('icons written to', OUT);
