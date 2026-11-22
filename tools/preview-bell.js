// 把铃铛按 16/20/32 像素渲染并放大，方便肉眼确认托盘里长什么样
const fs = require('fs');
const zlib = require('zlib');

function inBell(x, y) {
  const dx = Math.abs(x - 0.5);
  // 提手
  const ky = y - 0.085;
  if (dx * dx + ky * ky < 0.0023) return true;
  // 穹顶
  if (y > 0.115 && y <= 0.445) {
    const t = (y - 0.445) / 0.33;
    const half = 0.245 * Math.sqrt(Math.max(0, 1 - t * t));
    if (dx < half) return true;
  }
  // 直壁
  if (y > 0.445 && y <= 0.555 && dx < 0.245) return true;
  // 外张
  if (y > 0.555 && y <= 0.595) {
    const t = (y - 0.555) / 0.04;
    if (dx < 0.245 + 0.09 * t) return true;
  }
  // 底沿
  if (y > 0.595 && y <= 0.655 && dx < 0.335) return true;
  // 铃锤
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
      const i = (py * size + px) * 4;
      buf[i] = rgb[0]; buf[i + 1] = rgb[1]; buf[i + 2] = rgb[2];
      buf[i + 3] = Math.round((hits / (ss * ss)) * 255);
    }
  }
  return buf;
}

function png(size, rgb, dark, scale) {
  const rgba = render(size, rgb);
  const W = size * scale, H = size * scale;
  const raw = Buffer.alloc((W * 4 + 1) * H);
  for (let y = 0; y < H; y++) {
    raw[y * (W * 4 + 1)] = 0;
    for (let x = 0; x < W; x++) {
      const sx = Math.floor(x / scale), sy = Math.floor(y / scale);
      const i = (sy * size + sx) * 4, a = rgba[i + 3] / 255, bg = dark ? 0x23 : 0xf3;
      const o = y * (W * 4 + 1) + 1 + x * 4;
      raw[o] = Math.round(rgba[i] * a + bg * (1 - a));
      raw[o + 1] = Math.round(rgba[i + 1] * a + bg * (1 - a));
      raw[o + 2] = Math.round(rgba[i + 2] * a + bg * (1 - a));
      raw[o + 3] = 255;
    }
  }
  const idat = zlib.deflateSync(raw);
  const crcT = [];
  for (let n = 0; n < 256; n++) { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; crcT[n] = c >>> 0; }
  const chunk = (type, data) => {
    const len = Buffer.alloc(4); len.writeUInt32BE(data.length, 0);
    const t = Buffer.from(type, 'ascii');
    let c = 0xffffffff;
    for (const b of Buffer.concat([t, data])) c = crcT[(c ^ b) & 0xff] ^ (c >>> 8);
    const crc = Buffer.alloc(4); crc.writeUInt32BE((c ^ 0xffffffff) >>> 0, 0);
    return Buffer.concat([len, t, data, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(W, 0); ihdr.writeUInt32BE(H, 4); ihdr[8] = 8; ihdr[9] = 6;
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr), chunk('IDAT', idat), chunk('IEND', Buffer.alloc(0))]);
}

fs.mkdirSync('preview', { recursive: true });
fs.writeFileSync('preview/bell16.png', png(16, [0x1f, 0x7a, 0xe0], true, 14));
fs.writeFileSync('preview/bell20.png', png(20, [0x1f, 0x7a, 0xe0], true, 12));
fs.writeFileSync('preview/bell32.png', png(32, [0x1f, 0x7a, 0xe0], false, 8));
console.log('preview ok');
