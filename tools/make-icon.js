// 生成 ntfy-win 的 app.ico（铃铛）与预览 PNG
const fs = require('fs');
const zlib = require('zlib');

// 归一化坐标 (0..1) 判断是否落在铃铛内
function inBell(x, y) {
  const dx = Math.abs(x - 0.5);
  // 顶部提手
  const ky = y - 0.085;
  if (dx * dx + ky * ky < 0.0023) return true;
  // 穹顶：以 (0.5, 0.445) 为中心的椭圆上半
  if (y > 0.115 && y <= 0.445) {
    const t = (y - 0.445) / 0.33;
    const half = 0.245 * Math.sqrt(Math.max(0, 1 - t * t));
    if (dx < half) return true;
  }
  // 直壁
  if (y > 0.445 && y <= 0.555 && dx < 0.245) return true;
  // 向外张开
  if (y > 0.555 && y <= 0.595) {
    const t = (y - 0.555) / 0.04;
    if (dx < 0.245 + 0.09 * t) return true;
  }
  // 底沿
  if (y > 0.595 && y <= 0.655 && dx < 0.335) return true;
  // 铃锤，和底沿留出空隙，16px 下才能分开
  const cy = y - 0.83;
  return dx * dx + cy * cy < 0.0081;
}

function render(size, rgb, ss = 4) {
  const buf = Buffer.alloc(size * size * 4);
  for (let py = 0; py < size; py++) {
    for (let px = 0; px < size; px++) {
      let hits = 0;
      for (let sy = 0; sy < ss; sy++) {
        for (let sx = 0; sx < ss; sx++) {
          const x = (px + (sx + 0.5) / ss) / size;
          const y = (py + (sy + 0.5) / ss) / size;
          if (inBell(x, y)) hits++;
        }
      }
      const a = Math.round((hits / (ss * ss)) * 255);
      const i = (py * size + px) * 4;
      buf[i] = rgb[0]; buf[i + 1] = rgb[1]; buf[i + 2] = rgb[2]; buf[i + 3] = a;
    }
  }
  return buf;
}

// ---- 写 ICO（32bpp BGRA，自下而上 + AND 掩码）----
function bmpData(size, rgba) {
  const header = Buffer.alloc(40);
  header.writeUInt32LE(40, 0);
  header.writeInt32LE(size, 4);
  header.writeInt32LE(size * 2, 8); // XOR + AND
  header.writeUInt16LE(1, 12);
  header.writeUInt16LE(32, 14);
  header.writeUInt32LE(0, 16);
  header.writeUInt32LE(size * size * 4, 20);
  const xor = Buffer.alloc(size * size * 4);
  for (let y = 0; y < size; y++) {
    const srcRow = (size - 1 - y) * size * 4;
    for (let x = 0; x < size; x++) {
      const s = srcRow + x * 4;
      const d = y * size * 4 + x * 4;
      xor[d] = rgba[s + 2];     // B
      xor[d + 1] = rgba[s + 1]; // G
      xor[d + 2] = rgba[s];     // R
      xor[d + 3] = rgba[s + 3]; // A
    }
  }
  const maskRow = Math.ceil(size / 32) * 4;
  const and = Buffer.alloc(maskRow * size, 0);
  return Buffer.concat([header, xor, and]);
}

const sizes = [16, 20, 24, 32, 48];
const color = [0x1f, 0x7a, 0xe0]; // 蓝色
const images = sizes.map(s => ({ size: s, data: bmpData(s, render(s, color)) }));

const dir = Buffer.alloc(6);
dir.writeUInt16LE(0, 0);
dir.writeUInt16LE(1, 2);
dir.writeUInt16LE(images.length, 4);

let offset = 6 + images.length * 16;
const entries = [];
for (const img of images) {
  const e = Buffer.alloc(16);
  e.writeUInt8(img.size === 256 ? 0 : img.size, 0);
  e.writeUInt8(img.size === 256 ? 0 : img.size, 1);
  e.writeUInt8(0, 2);
  e.writeUInt8(0, 3);
  e.writeUInt16LE(1, 4);
  e.writeUInt16LE(32, 6);
  e.writeUInt32LE(img.data.length, 8);
  e.writeUInt32LE(offset, 12);
  entries.push(e);
  offset += img.data.length;
}
fs.writeFileSync(process.argv[2] + '/app.ico', Buffer.concat([dir, ...entries, ...images.map(i => i.data)]));

// ---- 写预览 PNG（48px，白底叠加 + 深底叠加）----
function png(size, rgb, dark) {
  const rgba = render(size, rgb);
  const raw = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0;
    for (let x = 0; x < size; x++) {
      const i = (y * size + x) * 4;
      const a = rgba[i + 3] / 255;
      const bg = dark ? 0x23 : 0xf3;
      const o = y * (size * 4 + 1) + 1 + x * 4;
      raw[o] = Math.round(rgba[i] * a + bg * (1 - a));
      raw[o + 1] = Math.round(rgba[i + 1] * a + bg * (1 - a));
      raw[o + 2] = Math.round(rgba[i + 2] * a + bg * (1 - a));
      raw[o + 3] = 255;
    }
  }
  const idat = zlib.deflateSync(raw);
  const chunk = (type, data) => {
    const len = Buffer.alloc(4); len.writeUInt32BE(data.length, 0);
    const t = Buffer.from(type, 'ascii');
    const crcTable = [];
    for (let n = 0; n < 256; n++) { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; crcTable[n] = c >>> 0; }
    let c = 0xffffffff;
    for (const b of Buffer.concat([t, data])) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
    const crc = Buffer.alloc(4); crc.writeUInt32BE((c ^ 0xffffffff) >>> 0, 0);
    return Buffer.concat([len, t, data, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0); ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; ihdr[9] = 6; ihdr[10] = 0; ihdr[11] = 0; ihdr[12] = 0;
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr), chunk('IDAT', idat), chunk('IEND', Buffer.alloc(0))
  ]);
}
fs.writeFileSync(process.argv[2] + '/icon-dark.png', png(96, color, true));
fs.writeFileSync(process.argv[2] + '/icon-light.png', png(96, color, false));
console.log('icon written');
