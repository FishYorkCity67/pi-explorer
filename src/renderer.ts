/**
 * WebGL2-Renderer: Ein Vollbild-Shader berechnet für jedes Pixel die Ziffernzelle und liest die
 * Ziffer aus einer R8UI-Array-Textur. Gezeichnet wird also nur, was sichtbar ist – egal wie viele
 * Millionen Ziffern es sind.
 */
import { GLYPH_COUNT, GLYPH_SIZE, type GlyphAtlas } from "./glyphs";

export type Mode = 0 | 1 | 2 | 3; // NUMBERS, LETTERS, COLORS, BLACK & WHITE

export const PALETTE: string[] = [
  "#000000", // 0 Schwarz
  "#1A2A8C", // 1 Dunkelblau
  "#7040E0", // 2 Blauviolett
  "#5CC6F2", // 3 Hellblau
  "#2EB35A", // 4 Grün
  "#F2D43A", // 5 Gelb
  "#F8B566", // 6 Hellorange
  "#F2861C", // 7 Orange
  "#E0362C", // 8 Rot
  "#D8329C", // 9 Magenta
];

export interface ShapeOverlay {
  col: number;
  row: number;
  w: number;
  h: number;
  /** Bitmaske pro Zeile (Bit c = Spalte c gemalt) */
  mask: number[];
  onlyPainted: boolean;
}

export interface DrawParams {
  camX: number;
  camY: number;
  /** CSS-Pixel pro Zelle */
  scale: number;
  dpr: number;
  width: number;
  /** Gesamtzahl der Ziffern */
  total: number;
  mode: Mode;
  phase: number;
  time: number;
  /** 0-basiert [start, end) */
  hlStart: number;
  hlEnd: number;
  dim: boolean;
  /** Welt-Rechtecke [x0, y0, x1, y1] in Zellen */
  rects: number[][];
  shape: ShapeOverlay | null;
}

const VS = `#version 300 es
in vec2 aPos;
void main() { gl_Position = vec4(aPos, 0.0, 1.0); }`;

const FS = `#version 300 es
precision highp float;
precision highp int;
precision highp usampler2DArray;
precision mediump sampler2DArray;

uniform usampler2DArray uDigits;
uniform sampler2DArray uGlyphs;
uniform vec2 uRes;
uniform ivec2 uCamI;
uniform vec2 uCamF;
uniform float uScale;
uniform float uDpr;
uniform int uW;
uniform int uLastRow;
uniform int uLastCol;
uniform int uWinRow;
uniform int uWinLen;
uniform int uL;
uniform int uMode;
uniform int uPhase;
uniform float uTime;
uniform int uHlStart;
uniform int uHlEnd;
uniform int uDim;
uniform vec4 uRects[3];
uniform int uRectN;
uniform int uShapeOn;
uniform ivec4 uShapeBox;
uniform int uShapeMask[8];
uniform int uShapeOnly;
uniform vec3 uPalette[10];
uniform float uCoverage[${GLYPH_COUNT}];
out vec4 outColor;

const vec3 GOLD = vec3(0.910, 0.690, 0.294);
const vec3 BG_A = vec3(0.043, 0.063, 0.125);
const vec3 BG_B = vec3(0.024, 0.031, 0.047);
const vec3 TEXT = vec3(0.78, 0.82, 0.88);
const vec3 DIMC = vec3(0.25, 0.31, 0.41);
const vec3 RED  = vec3(0.86, 0.27, 0.22);

int digitAt(int idx) {
  int per = uL * uL;
  int layer = idx / per;
  int r = idx - layer * per;
  int y = r / uL;
  int x = r - y * uL;
  return int(texelFetch(uDigits, ivec3(x, y, layer), 0).r);
}

float glyph(int g, vec2 uv, float cellPx) {
  float lod = log2(${GLYPH_SIZE}.0 / max(cellPx, 1.0));
  float a = textureLod(uGlyphs, vec3(uv, float(g)), max(lod, 0.0)).r;
  if (cellPx > ${GLYPH_SIZE}.0) {
    float w = 0.5 * ${GLYPH_SIZE}.0 / cellPx + 0.03;
    a = smoothstep(0.5 - w, 0.5 + w, a);
  }
  return a;
}

vec3 cellColor(ivec2 c, vec2 uv, float cellPx, vec3 bg, out float inData) {
  inData = 0.0;
  if (c.x < 0 || c.x >= uW || c.y < 0) return bg;
  if (c.y > uLastRow || (c.y == uLastRow && c.x > uLastCol)) return bg;
  inData = 1.0;
  // Index lokal im geladenen Fenster (spart Grafikspeicher bei sehr vielen Stellen)
  int lr = c.y - uWinRow;
  int idx = lr * uW + c.x;
  if (lr < 0 || idx >= uWinLen) {
    float st = step(0.5, fract((float(c.x) + float(c.y)) * 0.5));
    return bg + vec3(0.018, 0.022, 0.032) * (0.6 + 0.4 * st);
  }
  int d = digitAt(idx);
  bool hl = idx >= uHlStart && idx < uHlEnd;

  bool inShape = false;
  int sState = 0; // 1 = gemalt & weiß, 2 = Abweichung, 3 = ungemalt & schwarz
  if (uShapeOn == 1) {
    int sx = c.x - uShapeBox.x;
    int sy = c.y - uShapeBox.y;
    if (sx >= 0 && sy >= 0 && sx < uShapeBox.z && sy < uShapeBox.w) {
      inShape = true;
      bool painted = ((uShapeMask[sy] >> sx) & 1) == 1;
      bool white = d >= 5;
      if (painted) sState = white ? 1 : 2;
      else sState = (white && uShapeOnly == 0) ? 2 : 3;
    }
  }
  bool dim = uDim == 1 && !hl && !inShape;
  float zoomT = smoothstep(3.0, 9.0, cellPx);
  vec3 cellBg = bg + vec3(0.010, 0.014, 0.024);
  vec3 col;

  if (uMode >= 2) {
    vec3 base = uMode == 2 ? uPalette[d] : (d >= 5 ? vec3(0.92, 0.93, 0.95) : vec3(0.0));
    if (dim) base = mix(bg, base, 0.22);
    if (hl) {
      if (uMode == 3) base = d >= 5 ? GOLD : GOLD * 0.22;
      float e = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y)) * cellPx;
      float frame = cellPx > 6.0 ? 1.0 - smoothstep(1.0 * uDpr, 2.2 * uDpr, e) : 0.0;
      base = mix(base, GOLD, frame);
    }
    if (sState == 1) base = GOLD;
    else if (sState == 2) base = mix(base, RED, 0.8);
    if (cellPx > 14.0) {
      float e = min(uv.x, uv.y) * cellPx;
      base = mix(base, base * 0.5 + bg * 0.2, (1.0 - smoothstep(0.0, 1.0 * uDpr, e)) * 0.7);
    }
    col = base;
  } else if (uMode == 0) {
    // rausgezoomt: Helligkeit je Ziffer -> sichtbare Textur
    float far = 0.07 + 0.048 * float(d);
    float a = mix(far, glyph(d, uv, cellPx), zoomT);
    vec3 fg = hl ? GOLD : (dim ? DIMC : TEXT);
    if (hl) cellBg = mix(bg, GOLD, 0.10);
    if (sState == 1) { fg = GOLD; cellBg = mix(bg, GOLD, 0.14); }
    else if (sState == 2) fg = RED;
    col = mix(cellBg, fg, clamp(a, 0.0, 1.0));
  } else {
    // Buchstaben: Paare 01–26 = A–Z
    int ps = idx - ((idx + 2 - uPhase) % 2);
    int off = idx - ps;
    int v = -1;
    if (ps >= 0 && ps + 1 < uWinLen) v = digitAt(ps) * 10 + digitAt(ps + 1);
    bool hlP = ps >= 0 && ((ps >= uHlStart && ps < uHlEnd) || (ps + 1 >= uHlStart && ps + 1 < uHlEnd));
    vec3 fg = hlP ? GOLD : ((uDim == 1 && !inShape) ? DIMC : TEXT);
    float a;
    if (v >= 1 && v <= 26) {
      float gu = float(off) + uv.x - 0.5;
      float ga = (gu >= 0.0 && gu <= 1.0) ? glyph(9 + v, vec2(gu, uv.y), cellPx) : 0.0;
      a = mix(uCoverage[9 + v] * 0.65, ga, zoomT);
    } else {
      vec2 q = vec2(float(off) + uv.x - 1.0, uv.y - 0.5) * cellPx;
      float r = max(cellPx * 0.07, 0.6 * uDpr);
      float dt = 1.0 - smoothstep(r - 0.7 * uDpr, r + 0.7 * uDpr, length(q));
      a = mix(0.025, dt * 0.5, zoomT);
      fg = mix(fg, DIMC, hlP ? 0.3 : 0.65);
    }
    if (hlP) cellBg = mix(bg, GOLD, 0.10);
    col = mix(cellBg, fg, clamp(a, 0.0, 1.0));
  }
  return col;
}

vec3 sampleAt(vec2 p, float cellPx, vec3 bg, out float inData) {
  vec2 rel = (p - 0.5 * uRes) / uScale + uCamF;
  vec2 fl = floor(rel);
  if (abs(fl.x) > 1.0e9 || abs(fl.y) > 1.0e9) { inData = 0.0; return bg; }
  ivec2 c = uCamI + ivec2(fl);
  return cellColor(c, rel - fl, cellPx, bg, inData);
}

void main() {
  vec2 p = vec2(gl_FragCoord.x, uRes.y - gl_FragCoord.y);
  vec2 q = p / uRes - 0.5;
  float vig = smoothstep(0.15, 0.95, length(q * vec2(uRes.x / uRes.y, 1.0)));
  vec3 bg = mix(BG_A, BG_B, vig);
  float cellPx = uScale;
  float inData;
  vec3 col;
  if (cellPx < 1.0) {
    vec3 acc = vec3(0.0);
    float ind = 0.0;
    for (int i = 0; i < 4; i++) {
      vec2 o = vec2(float(i & 1), float(i >> 1)) * 0.5 - 0.25;
      float t;
      acc += sampleAt(p + o, cellPx, bg, t);
      ind += t;
    }
    col = acc * 0.25;
    inData = ind * 0.25;
  } else {
    col = sampleAt(p, cellPx, bg, inData);
  }

  if (uRectN > 0) {
    vec2 w = (p - 0.5 * uRes) / uScale + uCamF;
    float dmin = 1.0e9;
    vec2 bbMin = vec2(1.0e9);
    vec2 bbMax = vec2(-1.0e9);
    for (int i = 0; i < 3; i++) {
      if (i >= uRectN) break;
      vec4 r = uRects[i];
      vec2 cc = 0.5 * (r.xy + r.zw);
      vec2 h = 0.5 * (r.zw - r.xy);
      vec2 dd = abs(w - cc) - h;
      float sd = length(max(dd, 0.0)) + min(max(dd.x, dd.y), 0.0);
      dmin = min(dmin, sd * uScale);
      bbMin = min(bbMin, r.xy);
      bbMax = max(bbMax, r.zw);
    }
    float pulse = 0.5 + 0.5 * sin(uTime * 2.6);
    if (dmin > 0.0) {
      float R = 8.0 * uDpr + 0.22 * cellPx;
      col += GOLD * exp(-dmin / R) * (0.20 + 0.12 * pulse);
    }
    if (uShapeOn == 1) {
      float line = 1.0 - smoothstep(0.7 * uDpr, 1.8 * uDpr, abs(dmin - 1.2 * uDpr));
      col = mix(col, GOLD, line);
    }
    vec2 sz = (bbMax - bbMin) * uScale;
    float small = 1.0 - smoothstep(10.0 * uDpr, 34.0 * uDpr, max(sz.x, sz.y));
    if (small > 0.0) {
      vec2 cc = (0.5 * (bbMin + bbMax) - uCamF) * uScale + 0.5 * uRes;
      float dist = length(p - cc);
      float rr = (15.0 + 5.0 * pulse) * uDpr;
      float ring = exp(-pow((dist - rr) / (1.5 * uDpr), 2.0));
      float ring2 = exp(-pow((dist - rr * 1.9) / (1.1 * uDpr), 2.0)) * 0.35;
      col += GOLD * (ring + ring2) * small;
    }
  }
  col *= 1.0 - 0.16 * vig;
  outColor = vec4(col, 1.0);
}`;

function hexToRgb(h: string): [number, number, number] {
  const n = parseInt(h.slice(1), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}

export class Renderer {
  readonly gl: WebGL2RenderingContext;
  private prog: WebGLProgram;
  private u: Record<string, WebGLUniformLocation | null> = {};
  private digitTex: WebGLTexture | null = null;
  private glyphTex: WebGLTexture;
  readonly L: number;
  n = 0;
  uploaded = 0;
  private coverage = new Float32Array(GLYPH_COUNT);

  constructor(readonly canvas: HTMLCanvasElement) {
    const gl = canvas.getContext("webgl2", { antialias: false, alpha: false, powerPreference: "high-performance" });
    if (!gl) throw new Error("WebGL2 is not available on this device");
    this.gl = gl;
    this.L = Math.min(4096, gl.getParameter(gl.MAX_TEXTURE_SIZE) as number);
    this.prog = this.link(VS, FS);
    gl.useProgram(this.prog);
    const names = [
      "uDigits", "uGlyphs", "uRes", "uCamI", "uCamF", "uScale", "uDpr", "uW", "uLastRow", "uLastCol", "uWinRow", "uWinLen", "uL", "uMode",
      "uPhase", "uTime", "uHlStart", "uHlEnd", "uDim", "uRects", "uRectN", "uShapeOn", "uShapeBox",
      "uShapeMask", "uShapeOnly", "uPalette", "uCoverage",
    ];
    for (const n of names) this.u[n] = gl.getUniformLocation(this.prog, n);
    gl.uniform1i(this.u.uDigits, 0);
    gl.uniform1i(this.u.uGlyphs, 1);
    gl.uniform3fv(this.u.uPalette, new Float32Array(PALETTE.flatMap(hexToRgb)));

    // Vollbild-Dreieck
    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const loc = gl.getAttribLocation(this.prog, "aPos");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);

    this.glyphTex = gl.createTexture()!;
    this.allocate(1);
  }

  private link(vs: string, fs: string): WebGLProgram {
    const gl = this.gl;
    const mk = (type: number, src: string) => {
      const s = gl.createShader(type)!;
      gl.shaderSource(s, src);
      gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) || "shader");
      return s;
    };
    const p = gl.createProgram()!;
    gl.attachShader(p, mk(gl.VERTEX_SHADER, vs));
    gl.attachShader(p, mk(gl.FRAGMENT_SHADER, fs));
    gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p) || "link");
    return p;
  }

  setGlyphs(a: GlyphAtlas) {
    const gl = this.gl;
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, this.glyphTex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texImage3D(gl.TEXTURE_2D_ARRAY, 0, gl.R8, GLYPH_SIZE, GLYPH_SIZE, GLYPH_COUNT, 0, gl.RED, gl.UNSIGNED_BYTE, a.data);
    gl.generateMipmap(gl.TEXTURE_2D_ARRAY);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    this.coverage.set(a.coverage);
    gl.useProgram(this.prog);
    gl.uniform1fv(this.u.uCoverage, this.coverage);
  }

  /** Erste geladene Ziffer (global, 0-basiert, Vielfaches der Zeilenbreite) */
  winStart = 0;
  /** Anzahl geladener Ziffern ab winStart */
  loaded = 0;

  /** Ziffern-Textur mit Platz für n Ziffern anlegen (alte wird freigegeben). */
  allocate(n: number) {
    const gl = this.gl;
    const L = this.L;
    if (this.digitTex) gl.deleteTexture(this.digitTex);
    this.digitTex = gl.createTexture();
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, this.digitTex);
    const per = L * L;
    const layers = Math.max(1, Math.ceil(n / per));
    const height = layers === 1 ? Math.max(1, Math.ceil(n / L)) : L;
    gl.texStorage3D(gl.TEXTURE_2D_ARRAY, 1, gl.R8UI, L, height, layers);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    this.n = n;
    this.uploaded = 0;
    this.winStart = 0;
    this.loaded = 0;
  }

  /** Neues Fenster beginnen: ab globalem Index start wird neu geladen. */
  setWindow(start: number) {
    this.winStart = start;
    this.loaded = 0;
    this.uploaded = 0;
  }

  /** Größe eines Upload-Blocks: ganze Zeilen, teilt eine Layer-Fläche exakt. */
  get chunkDigits() {
    return this.L * Math.min(this.L, 1024);
  }

  /** Daten ab `offset` (Vielfaches von L, Block liegt in einer Layer). */
  upload(offset: number, data: Uint8Array) {
    const gl = this.gl;
    const L = this.L;
    const per = L * L;
    const layer = Math.floor(offset / per);
    const y = (offset % per) / L;
    const rows = Math.ceil(data.length / L);
    let src = data;
    if (data.length !== rows * L) {
      src = new Uint8Array(rows * L);
      src.set(data);
    }
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, this.digitTex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texSubImage3D(gl.TEXTURE_2D_ARRAY, 0, 0, y, layer, L, rows, 1, gl.RED_INTEGER, gl.UNSIGNED_BYTE, src);
    this.uploaded = Math.max(this.uploaded, offset + data.length);
    this.loaded = this.uploaded;
  }

  resize(): boolean {
    const dpr = Math.min(window.devicePixelRatio || 1, 3);
    const w = Math.max(1, Math.round(this.canvas.clientWidth * dpr));
    const h = Math.max(1, Math.round(this.canvas.clientHeight * dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
      return true;
    }
    return false;
  }

  draw(p: DrawParams) {
    const gl = this.gl;
    const u = this.u;
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.useProgram(this.prog);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, this.digitTex);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, this.glyphTex);
    const ix = Math.floor(p.camX);
    const iy = Math.floor(p.camY);
    const dpr = this.canvas.width / Math.max(1, this.canvas.clientWidth);
    gl.uniform2f(u.uRes, this.canvas.width, this.canvas.height);
    gl.uniform2i(u.uCamI, ix, iy);
    gl.uniform2f(u.uCamF, p.camX - ix, p.camY - iy);
    gl.uniform1f(u.uScale, p.scale * dpr);
    gl.uniform1f(u.uDpr, dpr);
    gl.uniform1i(u.uW, Math.max(1, p.width));
    const W = Math.max(1, p.width);
    const last = p.total - 1;
    gl.uniform1i(u.uLastRow, last < 0 ? -1 : Math.floor(last / W));
    gl.uniform1i(u.uLastCol, last < 0 ? -1 : last % W);
    gl.uniform1i(u.uWinRow, Math.floor(this.winStart / W));
    gl.uniform1i(u.uWinLen, Math.min(this.loaded, this.n));
    const clampI = (v: number) => Math.max(-1, Math.min(2_000_000_000, v));
    gl.uniform1i(u.uL, this.L);
    gl.uniform1i(u.uMode, p.mode);
    gl.uniform1i(u.uPhase, (((p.phase - (this.winStart % 2)) % 2) + 2) % 2);
    gl.uniform1f(u.uTime, p.time);
    gl.uniform1i(u.uHlStart, clampI(p.hlStart - this.winStart));
    gl.uniform1i(u.uHlEnd, clampI(p.hlEnd - this.winStart));
    gl.uniform1i(u.uDim, p.dim ? 1 : 0);
    const rects = new Float32Array(12);
    const rn = Math.min(3, p.rects.length);
    for (let i = 0; i < rn; i++) {
      const r = p.rects[i];
      rects.set([r[0] - ix, r[1] - iy, r[2] - ix, r[3] - iy], i * 4);
    }
    gl.uniform4fv(u.uRects, rects);
    gl.uniform1i(u.uRectN, rn);
    const s = p.shape;
    gl.uniform1i(u.uShapeOn, s ? 1 : 0);
    if (s) {
      gl.uniform4i(u.uShapeBox, s.col, s.row, s.w, s.h);
      const m = new Int32Array(8);
      s.mask.slice(0, 8).forEach((v, i) => (m[i] = v));
      gl.uniform1iv(u.uShapeMask, m);
      gl.uniform1i(u.uShapeOnly, s.onlyPainted ? 1 : 0);
    }
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
}
