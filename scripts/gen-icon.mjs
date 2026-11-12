/**
 * 生成 src-tauri/icons/icon.ico（零依赖，纯 Node）。
 * 256x256 32bpp：圆角深色底 + 蓝色 ">" + 绿色下划线。
 */
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SIZE = 256;
const bg = [30, 30, 46, 255]; // #1e1e2e
const accent = [137, 180, 250, 255]; // #89b4fa
const green = [166, 227, 161, 255]; // #a6e3a1
const R = 48; // 圆角半径

const px = new Uint8Array(SIZE * SIZE * 4);
function set(x, y, c) {
  const i = (y * SIZE + x) * 4;
  px[i] = c[0];
  px[i + 1] = c[1];
  px[i + 2] = c[2];
  px[i + 3] = c[3];
}

function inRoundedRect(x, y) {
  const min = 8;
  const max = SIZE - 8;
  if (x < min || y < min || x >= max || y >= max) return false;
  const cx = Math.max(min + R, Math.min(x, max - 1 - R));
  const cy = Math.max(min + R, Math.min(y, max - 1 - R));
  const dx = x - cx;
  const dy = y - cy;
  return dx * dx + dy * dy <= R * R;
}

function distToSeg(px0, py0, x1, y1, x2, y2) {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const l2 = dx * dx + dy * dy;
  let t = l2 === 0 ? 0 : ((px0 - x1) * dx + (py0 - y1) * dy) / l2;
  t = Math.max(0, Math.min(1, t));
  return Math.hypot(px0 - (x1 + t * dx), py0 - (y1 + t * dy));
}

for (let y = 0; y < SIZE; y++) {
  for (let x = 0; x < SIZE; x++) {
    if (!inRoundedRect(x, y)) {
      set(x, y, [0, 0, 0, 0]);
      continue;
    }
    let c = bg;
    const w = 10; // 笔画半宽
    if (distToSeg(x, y, 86, 84, 142, 128) < w || distToSeg(x, y, 142, 128, 86, 172) < w) c = accent;
    else if (x >= 152 && x < 210 && y >= 154 && y < 172) c = green;
    set(x, y, c);
  }
}

// ---- 封装为 ICO ----
const xorSize = SIZE * SIZE * 4;
const andSize = (SIZE / 8) * SIZE;
const imgSize = 40 + xorSize + andSize;
const buf = Buffer.alloc(6 + 16 + imgSize);

// ICONDIR
buf.writeUInt16LE(0, 0);
buf.writeUInt16LE(1, 2); // type: icon
buf.writeUInt16LE(1, 4); // count

// ICONDIRENTRY（宽高 0 表示 256）
buf.writeUInt8(0, 6);
buf.writeUInt8(0, 7);
buf.writeUInt8(0, 8);
buf.writeUInt8(0, 9);
buf.writeUInt16LE(1, 10); // planes
buf.writeUInt16LE(32, 12); // bpp
buf.writeUInt32LE(imgSize, 14);
buf.writeUInt32LE(22, 18); // 数据偏移 = 6 + 16

// BITMAPINFOHEADER（高度为 2 倍：XOR + AND）
const off = 22;
buf.writeInt32LE(40, off);
buf.writeInt32LE(SIZE, off + 4);
buf.writeInt32LE(SIZE * 2, off + 8);
buf.writeUInt16LE(1, off + 12);
buf.writeUInt16LE(32, off + 14);

// 像素：自底向上、BGRA
let p = off + 40;
for (let y = SIZE - 1; y >= 0; y--) {
  for (let x = 0; x < SIZE; x++) {
    const i = (y * SIZE + x) * 4;
    buf[p++] = px[i + 2];
    buf[p++] = px[i + 1];
    buf[p++] = px[i];
    buf[p++] = px[i + 3];
  }
}
// AND mask 保持全 0（使用 alpha 通道）

const out = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "icons", "icon.ico");
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, buf);
console.log(`icon written: ${out} (${buf.length} bytes)`);
