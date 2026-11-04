// 把 lucide-react 的图标定义转成原生渲染层用的折线数据。
//
// 桌面格子是 Direct2D 画的，用不了 SVG；但界面图标要和网页那边是同一套
// （lucide），所以这里读 node_modules 里的图标定义，把 path / circle / rect
// 这些图元采样成折线，生成 crates/hexbench-fence/src/icons.rs。
//
// 用法：node scripts/gen-icons.mjs
// 换了 lucide 版本或者要多用一个图标，改下面的 ICONS 再跑一次。

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
const iconsDir = join(root, "node_modules/lucide-react/dist/esm/icons");
const outFile = join(root, "crates/hexbench-fence/src/icons.rs");

// Rust 常量名 → lucide 图标文件名。
const ICONS = [
  ["FOLDER", "folder"],
  ["CHEVRON_LEFT", "chevron-left"],
  ["SEARCH", "search"],
  ["LAYOUT_GRID", "layout-grid"],
  ["LIST", "list"],
  ["ELLIPSIS", "ellipsis"],
];

/** 从一个图标文件里取出 createLucideIcon("Name", [...]) 里的数组字面量。 */
function readChildren(file) {
  const text = readFileSync(join(iconsDir, `${file}.js`), "utf8");
  const at = text.indexOf("createLucideIcon(");
  if (at < 0) throw new Error(`${file}: 没有 createLucideIcon`);
  const start = text.indexOf("[", at);
  let depth = 0;
  let end = -1;
  for (let i = start; i < text.length; i += 1) {
    const ch = text[i];
    if (ch === '"' || ch === "'") {
      // 跳过字符串，里面不会有没配对的括号，但可能有 [
      const quote = ch;
      i += 1;
      while (i < text.length && text[i] !== quote) {
        if (text[i] === "\\") i += 1;
        i += 1;
      }
      continue;
    }
    if (ch === "[") depth += 1;
    else if (ch === "]") {
      depth -= 1;
      if (depth === 0) {
        end = i + 1;
        break;
      }
    }
  }
  if (end < 0) throw new Error(`${file}: 数组没闭合`);
  // 数组是合法的 JS 字面量（属性名没引号），本地依赖，直接求值。
  return new Function(`return ${text.slice(start, end)};`)();
}

// ---------- 图元 → 折线 ----------

const CURVE_STEPS = 16;

function dist(a, b) {
  return Math.hypot(a[0] - b[0], a[1] - b[1]);
}

function cubic(p0, p1, p2, p3, steps = CURVE_STEPS) {
  const out = [];
  for (let i = 1; i <= steps; i += 1) {
    const t = i / steps;
    const u = 1 - t;
    out.push([
      u * u * u * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t * t * t * p3[0],
      u * u * u * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t * t * t * p3[1],
    ]);
  }
  return out;
}

function quad(p0, p1, p2, steps = CURVE_STEPS) {
  const out = [];
  for (let i = 1; i <= steps; i += 1) {
    const t = i / steps;
    const u = 1 - t;
    out.push([
      u * u * p0[0] + 2 * u * t * p1[0] + t * t * p2[0],
      u * u * p0[1] + 2 * u * t * p1[1] + t * t * p2[1],
    ]);
  }
  return out;
}

/** SVG 椭圆弧（端点表示）→ 采样点。 */
function arc(p0, rx, ry, rotation, largeArc, sweep, p1) {
  if (rx === 0 || ry === 0) return [p1];
  const phi = (rotation * Math.PI) / 180;
  const cosPhi = Math.cos(phi);
  const sinPhi = Math.sin(phi);
  const dx = (p0[0] - p1[0]) / 2;
  const dy = (p0[1] - p1[1]) / 2;
  const x1 = cosPhi * dx + sinPhi * dy;
  const y1 = -sinPhi * dx + cosPhi * dy;
  rx = Math.abs(rx);
  ry = Math.abs(ry);
  const lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
  if (lambda > 1) {
    const scale = Math.sqrt(lambda);
    rx *= scale;
    ry *= scale;
  }
  const sign = largeArc === sweep ? -1 : 1;
  const num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
  const den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
  const coef = sign * Math.sqrt(Math.max(0, num / den));
  const cx1 = (coef * (rx * y1)) / ry;
  const cy1 = (coef * -(ry * x1)) / rx;
  const cx = cosPhi * cx1 - sinPhi * cy1 + (p0[0] + p1[0]) / 2;
  const cy = sinPhi * cx1 + cosPhi * cy1 + (p0[1] + p1[1]) / 2;
  const angle = (ux, uy, vx, vy) => {
    const dot = ux * vx + uy * vy;
    const len = Math.hypot(ux, uy) * Math.hypot(vx, vy);
    let a = Math.acos(Math.max(-1, Math.min(1, dot / len)));
    if (ux * vy - uy * vx < 0) a = -a;
    return a;
  };
  const start = angle(1, 0, (x1 - cx1) / rx, (y1 - cy1) / ry);
  let delta = angle(
    (x1 - cx1) / rx,
    (y1 - cy1) / ry,
    (-x1 - cx1) / rx,
    (-y1 - cy1) / ry,
  );
  if (!sweep && delta > 0) delta -= Math.PI * 2;
  if (sweep && delta < 0) delta += Math.PI * 2;
  const steps = Math.max(4, Math.ceil(Math.abs(delta) / (Math.PI / 12)));
  const out = [];
  for (let i = 1; i <= steps; i += 1) {
    const a = start + (delta * i) / steps;
    out.push([
      cx + cosPhi * rx * Math.cos(a) - sinPhi * ry * Math.sin(a),
      cy + sinPhi * rx * Math.cos(a) + cosPhi * ry * Math.sin(a),
    ]);
  }
  return out;
}

/** SVG path 的 d 属性 → 若干条折线。 */
function parsePath(d) {
  const tokens = d.match(/[a-zA-Z]|-?(?:\d*\.\d+|\d+\.?)(?:e[-+]?\d+)?/gi) ?? [];
  const paths = [];
  let current = null;
  let x = 0;
  let y = 0;
  let startX = 0;
  let startY = 0;
  let lastCtrl = null;
  let lastQuad = null;
  let command = "";
  let index = 0;
  const number = () => Number(tokens[index++]);
  const point = (relative) => {
    const px = number();
    const py = number();
    return relative ? [x + px, y + py] : [px, py];
  };
  const push = (p, relative = false) => {
    const nx = relative ? x + p[0] : p[0];
    const ny = relative ? y + p[1] : p[1];
    if (current === null) {
      current = [[nx, ny]];
      paths.push(current);
    } else {
      current.push([nx, ny]);
    }
    x = nx;
    y = ny;
    return [nx, ny];
  };
  const pushAll = (pts, relative = false) => {
    for (const p of pts) {
      const nx = relative ? x + p[0] : p[0];
      const ny = relative ? y + p[1] : p[1];
      if (current === null) {
        current = [];
        paths.push(current);
      }
      current.push([nx, ny]);
      x = nx;
      y = ny;
    }
  };

  while (index < tokens.length) {
    const token = tokens[index];
    if (/^[a-zA-Z]$/.test(token)) {
      command = token;
      index += 1;
      if (command === "Z" || command === "z") {
        if (current) current.push([startX, startY]);
        x = startX;
        y = startY;
        current = null;
        lastCtrl = null;
        lastQuad = null;
        continue;
      }
    }
    const relative = command === command.toLowerCase();
    switch (command.toUpperCase()) {
      case "M": {
        const p = point(relative);
        startX = p[0];
        startY = p[1];
        x = p[0];
        y = p[1];
        paths.push([[p[0], p[1]]]);
        current = paths[paths.length - 1];
        lastCtrl = null;
        lastQuad = null;
        // 后面的坐标对按 lineto 处理
        command = relative ? "l" : "L";
        break;
      }
      case "L": {
        const p = point(relative);
        push(p);
        lastCtrl = null;
        lastQuad = null;
        break;
      }
      case "H": {
        const px = number();
        push([relative ? x + px : px, y]);
        lastCtrl = null;
        lastQuad = null;
        break;
      }
      case "V": {
        const py = number();
        push([x, relative ? y + py : py]);
        lastCtrl = null;
        lastQuad = null;
        break;
      }
      case "C": {
        const c1 = point(relative);
        const c2 = point(relative);
        const p = point(relative);
        pushAll(cubic([x, y], c1, c2, p));
        lastCtrl = c2;
        lastQuad = null;
        break;
      }
      case "S": {
        const c1 = lastCtrl ? [2 * x - lastCtrl[0], 2 * y - lastCtrl[1]] : [x, y];
        const c2 = point(relative);
        const p = point(relative);
        pushAll(cubic([x, y], c1, c2, p));
        lastCtrl = c2;
        lastQuad = null;
        break;
      }
      case "Q": {
        const c = point(relative);
        const p = point(relative);
        pushAll(quad([x, y], c, p));
        lastQuad = c;
        lastCtrl = null;
        break;
      }
      case "T": {
        const c = lastQuad ? [2 * x - lastQuad[0], 2 * y - lastQuad[1]] : [x, y];
        const p = point(relative);
        pushAll(quad([x, y], c, p));
        lastQuad = c;
        lastCtrl = null;
        break;
      }
      case "A": {
        const rx = number();
        const ry = number();
        const rotation = number();
        const largeArc = number();
        const sweep = number();
        const p = point(relative);
        pushAll(arc([x, y], rx, ry, rotation, largeArc, sweep, p));
        lastCtrl = null;
        lastQuad = null;
        break;
      }
      default:
        throw new Error(`不认识的 path 命令 ${command}`);
    }
    if (index < tokens.length && !/^[a-zA-Z]$/.test(tokens[index])) {
      // 省略重复命令（M 已经在上面改成 L）
      continue;
    }
  }
  return paths.filter((path) => path.length > 1);
}

function circlePath(cx, cy, r) {
  const steps = 32;
  const path = [];
  for (let i = 0; i <= steps; i += 1) {
    const a = (i / steps) * Math.PI * 2;
    path.push([cx + r * Math.cos(a), cy + r * Math.sin(a)]);
  }
  return [path];
}

function ellipsePath(cx, cy, rx, ry) {
  const steps = 32;
  const path = [];
  for (let i = 0; i <= steps; i += 1) {
    const a = (i / steps) * Math.PI * 2;
    path.push([cx + rx * Math.cos(a), cy + ry * Math.sin(a)]);
  }
  return [path];
}

function rectPath(x, y, w, h, rx) {
  if (!rx) {
    return [[[x, y], [x + w, y], [x + w, y + h], [x, y + h], [x, y]]];
  }
  const r = Math.min(rx, w / 2, h / 2);
  const path = [];
  const quarter = (cx, cy, from) => {
    const steps = 8;
    const pts = [];
    for (let i = 0; i <= steps; i += 1) {
      const a = from + (i / steps) * (Math.PI / 2);
      pts.push([cx + r * Math.cos(a), cy + r * Math.sin(a)]);
    }
    return pts;
  };
  path.push(...quarter(x + w - r, y + r, -Math.PI / 2));
  path.push(...quarter(x + w - r, y + h - r, 0));
  path.push(...quarter(x + r, y + h - r, Math.PI / 2));
  path.push(...quarter(x + r, y + r, Math.PI));
  return [path];
}

function linePath(x1, y1, x2, y2) {
  return [[[x1, y1], [x2, y2]]];
}

function pointsPath(points, close) {
  const nums = (points.match(/-?(?:\d*\.\d+|\d+\.?)(?:e[-+]?\d+)?/gi) ?? []).map(Number);
  const path = [];
  for (let i = 0; i + 1 < nums.length; i += 2) path.push([nums[i], nums[i + 1]]);
  if (close) path.push(path[0]);
  return [path];
}

const num = (value) => (value === undefined ? 0 : Number(value));

function elementPaths(element) {
  const [tag, props] = element;
  switch (tag) {
    case "path":
      return parsePath(props.d);
    case "circle":
      return circlePath(num(props.cx), num(props.cy), num(props.r));
    case "ellipse":
      return ellipsePath(num(props.cx), num(props.cy), num(props.rx), num(props.ry));
    case "rect":
      return rectPath(num(props.x), num(props.y), num(props.width), num(props.height), num(props.rx));
    case "line":
      return linePath(num(props.x1), num(props.y1), num(props.x2), num(props.y2));
    case "polyline":
      return pointsPath(props.points, false);
    case "polygon":
      return pointsPath(props.points, true);
    default:
      throw new Error(`不认识的图元 ${tag}`);
  }
}

// ---------- 生成 ----------

const round = (value) => (Math.round(value * 100) / 100).toFixed(2);

const lines = [];
lines.push("//! 由 `scripts/gen-icons.mjs` 从 lucide-react 的图标数据生成，不要手改。");
lines.push("//!");
lines.push("//! 坐标是 lucide 的 24×24 网格（描边 2）。原生渲染按 `size / 24` 缩放后");
lines.push("//! 描边绘制；网页那边直接用 lucide-react 的同名图标，两边是同一份形状。");
lines.push("");
lines.push("/// 一条折线：首尾不闭合，按描边画。");
lines.push("pub type Polyline = &'static [(f32, f32)];");
lines.push("");
lines.push("pub struct Icon {");
lines.push("    pub paths: &'static [Polyline],");
lines.push("}");
lines.push("");

for (const [name, file] of ICONS) {
  const children = readChildren(file);
  const paths = [];
  for (const element of children) {
    for (const path of elementPaths(element)) {
      if (path.length > 1) paths.push(path);
    }
  }
  lines.push(`pub const ${name}: Icon = Icon {`);
  lines.push("    paths: &[");
  for (const path of paths) {
    const points = path.map(([x, y]) => `(${round(x)}, ${round(y)})`).join(", ");
    lines.push(`        &[${points}],`);
  }
  lines.push("    ],");
  lines.push("};");
  lines.push("");
}

writeFileSync(outFile, `${lines.join("\n")}\n`);
console.log(`wrote ${outFile} (${ICONS.length} icons)`);
