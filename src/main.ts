import "@fontsource/jetbrains-mono/300.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import "@fontsource/jetbrains-mono/600.css";
import "@fontsource/jetbrains-mono/700.css";
import "./styles.css";

import {
  android,
  api,
  inTauri,
  type ComputeInfo,
  type DeviceInfo,
  type Mode as CalcMode,
  type SearchKind,
  type SearchOut,
  type ShapeOut,
  type Status,
  type StorageOption,
} from "./api";
import { Camera, type View } from "./camera";
import { fmtBytes, fmtClock, fmtCompact, fmtDuration, fmtInt, fmtZoom, parseCount, shortCount } from "./format";
import { buildGlyphs } from "./glyphs";
import { PALETTE, Renderer, type Mode, type ShapeOverlay } from "./renderer";

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

// ------------------------------------------------------------------ Elemente
const canvas = $<HTMLCanvasElement>("view");
const ui = $("ui");
const startEl = $("start");
const hudText = $("hudText");
const wInput = $<HTMLInputElement>("wInput");
const kindEl = $<HTMLSelectElement>("kind");
const queryEl = $<HTMLInputElement>("query");
const fmtToggle = $("fmtToggle");
const fmtAll = $<HTMLInputElement>("fmtAll");
const resultEl = $("result");
const statsEl = $("stats");
const hudBtn = $("hudBtn");
const shapePop = $("shapePop");
const toastEl = $("toast");

// ------------------------------------------------------------------ Zustand
const S = {
  n: 0,
  width: 1000,
  mode: 0 as Mode,
  phase: 0,
  hlStart: 0,
  hlEnd: 0,
  rects: [] as number[][],
  dim: false,
  shape: null as ShapeOverlay | null,
  status: "READY",
  dirty: true,
  computing: false,
  info: null as ComputeInfo | null,
  lastSearch: null as null | { label: string; ms: number; total: number },
};

type SearchRes = { kind: "search"; out: SearchOut; index: number; pages: Map<number, [number, number][]>; query: string; type: SearchKind };
type ShapeRes = {
  kind: "shape";
  out: ShapeOut;
  index: number;
  pages: Map<number, [number, number][]>;
  cells: boolean[];
  onlyPainted: boolean;
  width: number;
  tolerance: number;
};
let res: SearchRes | ShapeRes | null = null;

const cam = new Camera();
let renderer: Renderer;
let device: DeviceInfo | null = null;
const touch = matchMedia("(hover: none)").matches;

// ------------------------------------------------------------------ Hilfen
let toastTimer = 0;
function toast(msg: string, ms = 2200) {
  toastEl.textContent = msg;
  toastEl.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (toastEl.hidden = true), ms);
}

const rows = () => Math.max(1, Math.ceil(S.n / S.width));

function fitView(): View {
  const r = rows();
  const s = Math.min((cam.vw * 0.92) / S.width, (cam.vh * 0.74) / r);
  return { x: S.width / 2, y: r / 2, s };
}

function resetView(): View {
  const s = touch || cam.vw < 760 ? 22 : 28;
  // erste Zeile knapp unter der Kopfzeile (deren Höhe hängt vom Layout ab)
  const header = document.getElementById("top")!.getBoundingClientRect().bottom;
  const topPad = Math.max(100, header + 24);
  return { x: (cam.vw / 2 - 24) / s, y: (cam.vh / 2 - topPad) / s, s };
}

function updateZoomLimits() {
  if (!S.n) return;
  cam.minS = Math.min(fitView().s * 0.35, 2);
  cam.maxS = 420;
}

function centerIndex(): number {
  const col = Math.min(S.width - 1, Math.max(0, Math.floor(cam.x)));
  const row = Math.min(rows() - 1, Math.max(0, Math.floor(cam.y)));
  return Math.min(S.n - 1, row * S.width + col);
}

function clampCam() {
  if (!S.n || cam.flying) return;
  const hx = (cam.vw / cam.s) * 0.5;
  const hy = (cam.vh / cam.s) * 0.5;
  const r = rows();
  const cl = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
  const nx = cl(cam.x, -hx * 0.85, S.width + hx * 0.85);
  const ny = cl(cam.y, -hy * 0.85, r + hy * 0.85);
  if (nx !== cam.x || ny !== cam.y) {
    cam.tx += nx - cam.x;
    cam.ty += ny - cam.y;
    cam.x = nx;
    cam.y = ny;
    cam.vx = cam.vy = 0;
  }
}

function setStatus(s: string) {
  S.status = s;
  updateHud();
}

// ------------------------------------------------------------------ HUD
function updateHud() {
  if (!S.n) return;
  const z = cam.s / fitView().s;
  if (cam.vw < 760) {
    // kompakt fürs Handy; die Zeilenbreite steht im Feld darüber
    hudText.innerHTML =
      `π <b>${fmtCompact(S.n)}</b> · POS <b>${fmtInt(centerIndex() + 1)}</b> · <b>${fmtZoom(z)}</b> · <span class="st">${S.status}</span>`;
    return;
  }
  hudText.innerHTML =
    `π <b>${S.n >= 1e8 ? fmtCompact(S.n) : fmtInt(S.n)}</b> · W <b>${fmtInt(S.width)}</b> · POS <b>${fmtInt(centerIndex() + 1)}</b>` +
    ` · ZOOM <b>${fmtZoom(z)}</b> · <span class="st">${S.status}</span>`;
}

function updateLegend() {
  const lg = $("legend");
  if (S.mode === 2) {
    lg.innerHTML = PALETTE.map(
      (c, i) => `<i style="background:${c};color:${i === 0 || i === 1 || i === 2 ? "#ccd" : "#111"};${i === 0 ? "outline:1px solid #333" : ""}">${i}</i>`,
    ).join("");
  } else if (S.mode === 3) {
    lg.innerHTML = `<i style="background:#000;outline:1px solid #333;color:#ccd;width:34px">0–4</i><i style="background:#eceef2;color:#111;width:34px">5–9</i>`;
  } else if (S.mode === 1) {
    lg.innerHTML = `<span style="letter-spacing:.14em">01–26 = A–Z</span>`;
  } else lg.innerHTML = "";
}

// ------------------------------------------------------------------ Modi und Breite
function setMode(m: Mode) {
  S.mode = m;
  document.querySelectorAll<HTMLButtonElement>(".tab").forEach((t) => t.classList.toggle("active", Number(t.dataset.mode) === m));
  if (m === 1 && S.hlEnd > S.hlStart) S.phase = S.hlStart % 2;
  updateLegend();
  S.dirty = true;
}

function setWidth(w: number, keepCenter = true) {
  w = Math.round(w);
  const maxW = Math.max(1, Math.min(S.n || 1, 1_000_000));
  w = Math.min(maxW, Math.max(1, w));
  if (w === S.width) {
    wInput.value = fmtInt(w);
    return;
  }
  const idx = S.n ? centerIndex() : 0;
  const fx = cam.x - Math.floor(cam.x);
  const fy = cam.y - Math.floor(cam.y);
  const oldWidth = S.width;
  S.width = w;
  wInput.value = fmtInt(w);
  if (keepCenter && S.n) cam.jump({ x: (idx % w) + fx, y: Math.floor(idx / w) + fy, s: cam.s });
  // geladenes Fenster muss an Zeilen der neuen Breite ausgerichtet sein
  if (windowed && oldWidth > 0) ensureWindow(Math.floor(idx / w), true);
  if (S.hlEnd > S.hlStart) S.rects = rangeRects(S.hlStart, S.hlEnd - S.hlStart);
  if (res?.kind === "shape" && res.width !== w) {
    clearHighlight();
    hideResult();
    toast("Row width changed – search the shape again");
  }
  updateZoomLimits();
  S.dirty = true;
}

function defaultWidth(n: number) {
  const target = Math.sqrt(n * 1.6);
  const nice = [10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];
  return nice.reduce((a, b) => (Math.abs(Math.log(b / target)) < Math.abs(Math.log(a / target)) ? b : a));
}

// ------------------------------------------------------------------ Hervorheben
function rangeRects(start: number, len: number): number[][] {
  const W = S.width;
  const end = start + len - 1;
  const r0 = Math.floor(start / W);
  const c0 = start % W;
  const r1 = Math.floor(end / W);
  const c1 = end % W;
  if (r0 === r1) return [[c0, r0, c1 + 1, r0 + 1]];
  const out = [[c0, r0, W, r0 + 1]];
  if (r1 > r0 + 1) out.push([0, r0 + 1, W, r1]);
  out.push([0, r1, c1 + 1, r1 + 1]);
  return out;
}

function clearHighlight() {
  S.hlStart = S.hlEnd = 0;
  S.rects = [];
  S.dim = false;
  S.shape = null;
  S.dirty = true;
}

function flyToRects(rects: number[][], readable = true) {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const r of rects) {
    x0 = Math.min(x0, r[0]);
    y0 = Math.min(y0, r[1]);
    x1 = Math.max(x1, r[2]);
    y1 = Math.max(y1, r[3]);
  }
  const sx = Math.max(1, x1 - x0);
  const sy = Math.max(1, y1 - y0);
  const lo = readable ? (cam.vw < 760 ? 18 : 22) : 6;
  const s = Math.min(64, Math.max(lo, Math.min((cam.vw * 0.42) / sx, (cam.vh * 0.32) / sy)));
  cam.flyTo({ x: (x0 + x1) / 2, y: (y0 + y1) / 2 + (cam.vh * 0.06) / s, s }, performance.now());
}

// ------------------------------------------------------------------ Ergebnis-Panel
function hideResult() {
  resultEl.hidden = true;
  res = null;
}

function showPanel(cls: "" | "miss" | "err") {
  resultEl.className = cls;
  resultEl.hidden = false;
}

function showError(msg: string) {
  res = null;
  clearHighlight();
  $("resConv").innerHTML = "";
  $("resTitle").textContent = "CAN'T SEARCH THAT";
  $("resSub").textContent = msg;
  $("resExtra").innerHTML = "";
  $("resNav").hidden = true;
  showPanel("err");
}

function esc(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

function spaced(d: string) {
  return d.replace(/(\d\d)(?=\d)/g, "$1 ");
}

async function runSearch() {
  if (!S.n || S.computing) return;
  const q = queryEl.value.trim();
  if (!q) {
    queryEl.focus();
    return;
  }
  let type = kindEl.value as SearchKind;
  if (type === "birthday" && fmtAll.checked) type = "birthday_all";
  setStatus("SEARCHING");
  try {
    const out = await api.search(type, q);
    res = { kind: "search", out, index: 0, pages: new Map(), query: q, type };
    S.lastSearch = { label: out.patterns.map((p) => p.digits).join(" / "), ms: out.ms, total: out.total };
    queryEl.blur();
    if (out.total > 0) {
      await focusHit(0);
    } else {
      showMiss(res);
    }
  } catch (e) {
    showError(String(e));
  } finally {
    setStatus("READY");
    if (!statsEl.hidden) renderStats();
  }
}

function convLine(r: SearchRes): string {
  const p = r.out.patterns;
  if (r.type === "word") return `WORD <b>${esc(r.out.letters ?? "")}</b> → <b>${spaced(p[0].digits)}</b> &nbsp;(A=01 … Z=26)`;
  if (r.type.startsWith("birthday")) return `DATE <b>${esc(r.query)}</b> → <b>${p[0].digits}</b>`;
  return "";
}

function showMiss(r: SearchRes) {
  clearHighlight();
  const p = r.out.patterns;
  $("resConv").innerHTML = convLine(r);
  $("resTitle").textContent = "NOT FOUND";
  const L = Math.min(...p.map((x) => x.digits.length));
  const lines = p
    .map((x) => `<b>${x.digits}</b> (${x.digits.length} digits)`)
    .join(", ");
  const expected = 10 ** L;
  $("resSub").innerHTML =
    `${lines} ${p.length > 1 ? "do" : "does"} not appear in the first <b>${fmtInt(r.out.searched)}</b> digits.<br>` +
    `No surprise: a random ${L}-digit sequence first shows up around position <b>${L <= 15 ? fmtInt(expected) : "10^" + L}</b> on average` +
    (expected > r.out.searched ? ` – try computing more digits or a shorter sequence.` : ` – this one is just unlucky.`);
  $("resExtra").innerHTML = "";
  $("resNav").hidden = true;
  showPanel("miss");
}

async function hitAt(r: SearchRes, i: number): Promise<[number, number]> {
  if (i < r.out.hits.length) return r.out.hits[i];
  const page = Math.floor(i / 5000) * 5000;
  let p = r.pages.get(page);
  if (!p) {
    p = await api.getHits(page, 5000);
    r.pages.set(page, p);
  }
  return p[i - page];
}

async function focusHit(i: number) {
  const r = res;
  if (!r || r.kind !== "search") return;
  // durchblättern lassen sich die gespeicherten Treffer (bei Millionen Treffern die ersten 2 Mio.)
  const total = Math.max(1, r.out.stored);
  i = ((i % total) + total) % total;
  r.index = i;
  const [pos, pi] = await hitAt(r, i);
  if (res !== r) return;
  const pat = r.out.patterns[pi];
  const len = pat.digits.length;
  S.shape = null;
  S.hlStart = pos - 1;
  S.hlEnd = pos - 1 + len;
  S.rects = rangeRects(pos - 1, len);
  S.dim = true;
  if (S.mode === 1 || r.type === "word") {
    S.phase = (pos - 1) % 2;
    if (r.type === "word" && S.mode !== 1) setMode(1);
  }
  S.dirty = true;
  ensureWindow(Math.floor((pos - 1) / S.width), false, Math.ceil(cam.vh / 22));
  flyToRects(S.rects);

  $("resConv").innerHTML = convLine(r);
  $("resTitle").textContent = `FOUND – POSITION ${fmtInt(pos)}`;
  $("resSub").innerHTML =
    `<b>${pat.digits}</b> (${len} digits) · ${fmtInt(pat.count)} occurrence(s) in ${fmtInt(r.out.searched)} digits` +
    ` · ${r.out.ms < 1 ? r.out.ms.toFixed(2) : fmtInt(r.out.ms)} ms`;
  $("resExtra").innerHTML =
    r.out.patterns.length > 1
      ? r.out.patterns
          .map((p, k) => `<span class="fmt${k === pi ? " cur" : ""}">${p.label} ${p.digits} · ${fmtInt(p.count)}</span>`)
          .join("<br>")
      : "";
  $("resNav").hidden = total <= 1;
  $("navText").textContent = `${fmtInt(i + 1)} / ${fmtInt(total)}${r.out.total > r.out.stored ? " (first)" : ""}`;
  showPanel("");
}

// ------------------------------------------------------------------ Formensuche
const SHAPE_N = 6;
const shapeCells: boolean[] = new Array(SHAPE_N * SHAPE_N).fill(false);
const grid = $("shapeGrid");
for (let i = 0; i < SHAPE_N * SHAPE_N; i++) {
  const c = document.createElement("div");
  c.className = "c";
  c.dataset.i = String(i);
  grid.appendChild(c);
}
function paintCell(el: Element | null, v: boolean) {
  if (!(el instanceof HTMLElement) || !el.dataset.i) return;
  const i = Number(el.dataset.i);
  shapeCells[i] = v;
  el.classList.toggle("on", v);
}
let paintValue: boolean | null = null;
grid.addEventListener("pointerdown", (e) => {
  const el = e.target as HTMLElement;
  if (!el.dataset.i) return;
  e.preventDefault();
  paintValue = !shapeCells[Number(el.dataset.i)];
  paintCell(el, paintValue);
  grid.setPointerCapture(e.pointerId);
});
grid.addEventListener("pointermove", (e) => {
  if (paintValue === null) return;
  paintCell(document.elementFromPoint(e.clientX, e.clientY), paintValue);
});
const endPaint = () => (paintValue = null);
grid.addEventListener("pointerup", endPaint);
grid.addEventListener("pointercancel", endPaint);

const tolEl = $<HTMLInputElement>("tol");
tolEl.addEventListener("input", () => ($("tolVal").textContent = tolEl.value));

function toggleShape(open: boolean = shapePop.hidden === true) {
  if (!S.n) return;
  shapePop.hidden = !open;
  $("shapeBtn").classList.toggle("on", open);
}

$("shapeClose").onclick = () => toggleShape(false);
$("shapeClear").onclick = () => {
  shapeCells.fill(false);
  grid.querySelectorAll(".c").forEach((c) => c.classList.remove("on"));
};
$("shapeSearch").onclick = runShapeSearch;

async function runShapeSearch() {
  if (!shapeCells.some(Boolean)) {
    toast("Paint at least one cell");
    return;
  }
  const tolerance = Number(tolEl.value);
  const onlyPainted = $<HTMLInputElement>("onlyPainted").checked;
  setStatus("SEARCHING");
  try {
    const out = await api.shapeSearch([...shapeCells], SHAPE_N, S.width, tolerance, onlyPainted);
    res = { kind: "shape", out, index: 0, pages: new Map(), cells: [...shapeCells], onlyPainted, width: S.width, tolerance };
    S.lastSearch = { label: `shape ${out.boxW}×${out.boxH}`, ms: out.ms, total: out.matchCount };
    toggleShape(false);
    if (out.bestPos === null) {
      showError(`The digits don't fill ${out.boxH} rows at width ${fmtInt(S.width)}.`);
    } else if (out.matchCount > 0) {
      await focusShapeMatch(0);
    } else {
      focusShape(out.bestPos, out.bestMismatches ?? 0, true);
    }
  } catch (e) {
    showError(String(e));
  } finally {
    setStatus("READY");
    if (!statsEl.hidden) renderStats();
  }
}

function shapeMask(r: ShapeRes): number[] {
  const m: number[] = [];
  for (let y = 0; y < r.out.boxH; y++) {
    let bits = 0;
    for (let x = 0; x < r.out.boxW; x++) if (r.cells[(r.out.boxY + y) * SHAPE_N + r.out.boxX + x]) bits |= 1 << x;
    m.push(bits);
  }
  return m;
}

function focusShape(pos: number, mm: number, bestOnly: boolean) {
  const r = res;
  if (!r || r.kind !== "shape") return;
  const o = r.out;
  const idx = pos - 1;
  const col = idx % S.width;
  const row = Math.floor(idx / S.width);
  S.hlStart = S.hlEnd = 0;
  S.shape = { col, row, w: o.boxW, h: o.boxH, mask: shapeMask(r), onlyPainted: r.onlyPainted };
  S.rects = [[col, row, col + o.boxW, row + o.boxH]];
  S.dim = true;
  if (S.mode !== 3) setMode(3);
  S.dirty = true;
  ensureWindow(row, false, Math.ceil(cam.vh / 10));
  const s = Math.min(70, Math.max(10, (Math.min(cam.vw, cam.vh) * 0.32) / Math.max(o.boxW, o.boxH)));
  cam.flyTo({ x: col + o.boxW / 2, y: row + o.boxH / 2 + (cam.vh * 0.06) / s, s }, performance.now());

  const pct = o.checkedCells ? (100 * (o.checkedCells - mm)) / o.checkedCells : 0;
  $("resConv").innerHTML = `SHAPE <b>${o.boxW}×${o.boxH}</b> · ${o.checkedCells} cells checked${r.onlyPainted ? " (painted only)" : ""} · row width <b>${fmtInt(r.width)}</b>`;
  $("resTitle").textContent = bestOnly
    ? `BEST MATCH ${Math.round(pct)} % – POSITION ${fmtInt(pos)}`
    : `${mm === 0 ? "EXACT MATCH" : `MATCH ${Math.round(pct)} %`} – POSITION ${fmtInt(pos)}`;
  $("resSub").innerHTML =
    (bestOnly
      ? `No placement within ${r.tolerance} deviation(s). Closest: <b>${mm}</b> cell(s) off.`
      : `<b>${fmtInt(o.matchCount)}</b> placement(s) with ≤ ${r.tolerance} deviation(s) · this one: ${mm} off`) +
    ` · ${fmtInt(o.positions)} positions in ${fmtInt(o.ms)} ms`;
  $("resExtra").innerHTML = `<span style="color:var(--gold)">■</span> painted &amp; white &nbsp; <span style="color:var(--red)">■</span> deviation`;
  $("resNav").hidden = bestOnly || o.matchCount <= 1;
  $("navText").textContent = bestOnly ? "" : `${fmtInt(r.index + 1)} / ${fmtInt(Math.min(o.matchCount, o.stored))}${o.matchCount > o.stored ? "+" : ""}`;
  showPanel(bestOnly ? "miss" : "");
}

async function focusShapeMatch(i: number) {
  const r = res;
  if (!r || r.kind !== "shape") return;
  const total = Math.min(r.out.matchCount, r.out.stored);
  if (!total) return;
  i = ((i % total) + total) % total;
  r.index = i;
  let m: [number, number];
  if (i < r.out.matches.length) m = r.out.matches[i];
  else {
    const page = Math.floor(i / 5000) * 5000;
    let p = r.pages.get(page);
    if (!p) {
      p = await api.getShapeMatches(page, 5000);
      r.pages.set(page, p);
    }
    m = p[i - page];
  }
  focusShape(m[0], m[1], false);
}

function step(d: number) {
  if (!res) return;
  if (res.kind === "search" && res.out.stored > 1) focusHit(res.index + d);
  else if (res.kind === "shape" && res.out.matchCount > 1) focusShapeMatch(res.index + d);
}

// ------------------------------------------------------------------ Statistik
let statsTimer = 0;
async function renderStats() {
  if (!S.n) return;
  try {
    const st = await api.stats();
    const n = st.digits || 1;
    $("freq").innerHTML = st.counts
      .map((c, d) => {
        const pct = (c / n) * 100;
        const dev = pct - 10;
        return `<div class="r"><span>${d}</span><span class="bar"><i style="width:${Math.min(100, (pct / 12) * 100)}%"></i><u style="left:${(10 / 12) * 100}%"></u></span><span class="pct">${pct.toFixed(3)} %</span><span class="dev">${dev >= 0 ? "+" : "−"}${Math.abs(dev).toFixed(3)}</span></div>`;
      })
      .join("");
    const i = st.info;
    const phaseNames: Record<string, string> = {
      SERIES: "SERIES",
      DIVIDE: "DIVISION",
      CONVERT: "TO DECIMAL",
      ROOTS: "ROOTS (NEWTON)",
      MULTIPLY: "MULTIPLY",
    };
    $("compStats").innerHTML = i
      ? `<dt>MODE</dt><dd>${i.mode === "split" ? `SPLIT · ${fmtBytes(i.budget)}` : "FAST"}</dd>` +
        `<dt>DIGITS</dt><dd>${fmtInt(i.digits)}</dd>` +
        `<dt>TIME</dt><dd>${fmtDuration(i.seconds)}</dd>` +
        `<dt>SPEED</dt><dd>${fmtInt(i.digits / Math.max(i.seconds, 1e-6))} digits/s</dd>` +
        i.phases.map(([p, s]) => `<dt>${phaseNames[p] ?? p}</dt><dd>${fmtDuration(s)}</dd>`).join("") +
        `<dt>THREADS</dt><dd>${device?.cores ?? "?"}</dd>`
      : "";
    $("memStats").innerHTML =
      `<dt>APP NOW</dt><dd>${fmtBytes(st.rss)}</dd>` +
      (i ? `<dt>PEAK WHILE COMPUTING</dt><dd>${fmtBytes(i.peakRss)}</dd>` : "") +
      (i && i.diskPeak ? `<dt>TEMP FILES (PEAK)</dt><dd>${fmtBytes(i.diskPeak)}</dd><dt>DIGITS FILE</dt><dd>${fmtBytes(i.digits)}</dd>` : "") +
      `<dt>FREE ON DEVICE</dt><dd>${fmtBytes(st.availRam)}</dd>`;
    const ls = S.lastSearch;
    $("searchStats").innerHTML = ls
      ? `<dt>QUERY</dt><dd>${esc(ls.label.length > 22 ? ls.label.slice(0, 21) + "…" : ls.label)}</dd><dt>TIME</dt><dd>${ls.ms < 1 ? ls.ms.toFixed(2) : fmtInt(ls.ms)} ms</dd><dt>HITS</dt><dd>${fmtInt(ls.total)}</dd>`
      : `<dt>—</dt><dd></dd>`;
    $("runStats").innerHTML = `<dt>DIGIT</dt><dd>${st.runDigit} × ${st.runLen}</dd><dt>AT POSITION</dt><dd>${fmtInt(st.runPos)}</dd>`;
  } catch (e) {
    console.error(e);
  }
}

function toggleStats(open: boolean = statsEl.hidden === true) {
  if (!S.n) return;
  statsEl.hidden = !open;
  hudBtn.textContent = open ? "−HUD" : "+HUD";
  hudBtn.classList.toggle("on", open);
  clearInterval(statsTimer);
  if (open) {
    renderStats();
    statsTimer = window.setInterval(renderStats, 2500);
  }
}

// ------------------------------------------------------------------ Startbildschirm
const PRESETS = [10_000, 100_000, 1_000_000, 10_000_000, 100_000_000, 1_000_000_000];
const BUDGETS_MB = [512, 1024, 2048, 4096, 8192];
let chosen = 1_000_000;
let calcMode: CalcMode = "fast";
let budgetMb = 2048;
let forceCompute = false;
const presetsEl = $("presets");
const customEl = $<HTMLInputElement>("customDigits");
const budgetsEl = $("budgets");
for (const p of PRESETS) {
  const b = document.createElement("button");
  b.textContent = shortCount(p);
  b.dataset.n = String(p);
  b.onclick = () => {
    customEl.value = "";
    choose(p);
  };
  presetsEl.appendChild(b);
}
for (const mb of BUDGETS_MB) {
  const b = document.createElement("button");
  b.textContent = mb >= 1024 ? `${mb / 1024} GB` : `${mb} MB`;
  b.dataset.mb = String(mb);
  b.onclick = () => {
    budgetMb = mb;
    refreshModeUi();
  };
  budgetsEl.appendChild(b);
}
document.querySelectorAll<HTMLButtonElement>("#modes button").forEach((b) => {
  b.onclick = () => {
    calcMode = b.dataset.calc as CalcMode;
    refreshModeUi();
  };
});

/** Budgets, die zum Gerät passen; Standard: 2 GB oder das größte bis halber RAM */
function refreshModeUi() {
  document.querySelectorAll<HTMLButtonElement>("#modes button").forEach((b) => b.classList.toggle("on", b.dataset.calc === calcMode));
  $("budgetRow").hidden = calcMode !== "split";
  const total = device?.totalRam ?? 16 * 1024 ** 3;
  budgetsEl.querySelectorAll<HTMLButtonElement>("button").forEach((b) => {
    const mb = Number(b.dataset.mb);
    b.disabled = mb * 1024 * 1024 > total * 0.8;
    b.classList.toggle("on", mb === budgetMb);
  });
  choose(customEl.value ? parseCount(customEl.value) : chosen);
}

function maxDigits(): number {
  if (!device) return 1_000_000_000;
  return calcMode === "split" ? device.maxDigitsSplit : device.maxDigitsFast;
}

// ------------------------------------------------------------------ Ort für Hilfsdateien
let storage: StorageOption[] = [];
let tempDir: string | null = null;
let tempDirChosenByUser = false;
const tempSel = $<HTMLSelectElement>("tempDir");

async function loadStorage() {
  try {
    storage = await api.storageOptions();
  } catch {
    storage = [];
  }
  if (!tempDir && storage.length) tempDir = storage[0].path;
  renderStorage();
}

function renderStorage() {
  tempSel.innerHTML = storage
    .map((s) => `<option value="${esc(s.path)}">${esc(s.label)} · ${fmtBytes(s.free)} free · ${esc(s.path)}</option>`)
    .join("");
  if (tempDir) tempSel.value = tempDir;
}

tempSel.addEventListener("change", () => {
  tempDir = tempSel.value;
  tempDirChosenByUser = true;
  choose(customEl.value ? parseCount(customEl.value) : chosen);
});

let estSeq = 0;
async function choose(n: number | null) {
  presetsEl.querySelectorAll("button").forEach((b) => b.classList.toggle("on", Number((b as HTMLElement).dataset.n) === n && !customEl.value));
  customEl.classList.toggle("on", !!customEl.value && n !== null);
  const est = $("estimate");
  const warn = $("warn");
  const btn = $<HTMLButtonElement>("computeBtn");
  forceCompute = false;
  btn.textContent = "COMPUTE π";
  const max = maxDigits();
  if (n === null || n < 1 || n > max) {
    est.innerHTML = `Enter a number between 1 and ${fmtInt(max)} – letters work too: <b>250m</b>, <b>1.5b</b>, <b>750k</b>.`;
    warn.hidden = true;
    btn.disabled = true;
    return;
  }
  chosen = n;
  btn.disabled = false;
  if (!device) {
    est.innerHTML = "Measuring this device…";
    return;
  }
  const seq = ++estSeq;
  let e = await api.estimate(n, calcMode, budgetMb, tempDir);
  if (seq !== estSeq) return;
  // Plattenmodus: Laufwerk zeigen; reicht der gewählte Ort nicht, den mit dem meisten Platz nehmen
  const diskRow = $("diskRow");
  diskRow.hidden = !e.needsDisk;
  if (e.needsDisk) {
    if (!storage.length) await loadStorage();
    const cur = storage.find((s) => s.path === tempDir);
    if (!cur || (cur.free < e.diskNeeded && !tempDirChosenByUser)) {
      const best = storage.find((s) => s.free >= e.diskNeeded) ?? [...storage].sort((a, b) => b.free - a.free)[0];
      if (best && best.path !== tempDir) {
        tempDir = best.path;
        renderStorage();
        e = await api.estimate(n, calcMode, budgetMb, tempDir);
        if (seq !== estSeq) return;
      }
    }
  }
  const split = calcMode === "split";
  est.innerHTML =
    `<b>${fmtInt(n)}</b> digits · about <b>${fmtDuration(e.seconds)}</b>${split ? " (rough)" : ""} · peak RAM about <b>${fmtBytes(e.peakRam)}</b><br>` +
    (e.needsDisk
      ? `more than ${fmtCompact(e.maxDigits)} digits don't fit into ${fmtBytes(budgetMb * 1024 * 1024)} – intermediate results go to temporary files: ` +
        `about <b>${fmtBytes(e.diskNeeded)}</b> needed, <b>${fmtBytes(e.diskFree)}</b> free · they are deleted when no longer needed<br>`
      : split
        ? `with ${fmtBytes(budgetMb * 1024 * 1024)} up to about <b>${fmtCompact(e.maxDigits)}</b> digits fit in RAM; more uses temporary files · `
        : "") +
    `this device: ${device.cores} threads · ${fmtBytes(e.availRam)} of ${fmtBytes(e.totalRam)} free`;
  if (e.needsDisk && e.diskNeeded > e.diskFree) {
    warn.hidden = false;
    warn.className = "danger";
    warn.innerHTML = `Not enough space for the temporary files: about ${fmtBytes(e.diskNeeded)} needed, only ${fmtBytes(e.diskFree)} free there. Pick another drive or fewer digits.`;
    btn.disabled = true;
    return;
  }
  const tooMany = false;
  if (e.level === "ok" && !tooMany) {
    warn.hidden = true;
  } else {
    warn.hidden = false;
    const danger = e.level === "danger" || tooMany;
    warn.className = danger ? "danger" : "";
    warn.innerHTML = tooMany
      ? `${fmtCompact(n)} digits don't fit into ${fmtBytes(budgetMb * 1024 * 1024)}. Pick a bigger budget or at most ${fmtCompact(e.maxDigits)} digits – or press again to try anyway.`
      : e.level === "danger"
        ? `Not enough free memory: this needs about ${fmtBytes(e.peakRam)}, but only ${fmtBytes(e.availRam)} are free. ` +
          `The system will probably close the app. ${split ? "Pick a smaller budget or fewer digits" : "Try SPLIT mode or fewer digits"}, or press again to try anyway.`
        : `This needs about ${fmtBytes(e.peakRam)} of RAM while computing (${fmtBytes(e.availRam)} free). ` +
          `Close other apps first${device.os === "android" ? " – Android may close the app if memory runs low" : ""}.`;
    if (danger) btn.textContent = "COMPUTE ANYWAY";
  }
}

customEl.addEventListener("input", () => {
  const v = parseCount(customEl.value);
  if (!customEl.value.trim()) choose(chosen);
  else choose(v);
});
customEl.addEventListener("keydown", (e) => {
  if (e.key === "Enter") startCompute();
});

let pollTimer = 0;
async function startCompute() {
  if (S.computing) return;
  const btn = $<HTMLButtonElement>("computeBtn");
  if (btn.disabled) return;
  const warnDanger = !$("warn").hidden && $("warn").classList.contains("danger");
  if (warnDanger && !forceCompute) {
    forceCompute = true;
    btn.textContent = "PRESS AGAIN TO CONFIRM";
    return;
  }
  try {
    await api.startCompute(chosen, calcMode, budgetMb, $("diskRow").hidden ? null : tempDir);
  } catch (e) {
    computeFailed(String(e));
    return;
  }
  // Android: Vordergrund-Dienst hält die App wach, auch bei ausgeschaltetem Bildschirm
  const bridge = android();
  if (bridge && device?.statusFile) {
    bridge.requestNotifications();
    bridge.start(device.statusFile);
  }
  showComputing();
}

/** Fortschrittsanzeige zeigen und den Rechenstatus regelmäßig abfragen. */
function showComputing() {
  S.computing = true;
  S.n = 0;
  hideResult();
  clearHighlight();
  toggleStats(false);
  renderer.allocate(1);
  startEl.hidden = false;
  startEl.classList.remove("fade");
  ui.hidden = true;
  $("startErr").hidden = true;
  $("backBtn").hidden = true;
  $<HTMLButtonElement>("computeBtn").hidden = true;
  setStartInputsDisabled(true);
  $("progress").hidden = false;
  $("bgNote").textContent = android()
    ? "Keeps computing with the screen off or the app in the background (see notification)."
    : device?.os === "windows"
      ? "Keeps computing when minimized. Closing the window moves it to the tray; the PC won't go to sleep meanwhile."
      : "";
  setProgress("SERIES", 0);
  clearTimeout(pollTimer);
  poll();
}

async function poll() {
  let st: Status;
  try {
    st = await api.status();
  } catch (e) {
    pollTimer = window.setTimeout(poll, 500);
    return;
  }
  setProgress(st.phase || "SERIES", st.fraction);
  $("pTime").textContent = (st.disk > 0 ? `TEMP ${fmtBytes(st.disk)} · ` : "") + fmtClock(st.elapsed);
  if (st.state === "running") {
    pollTimer = window.setTimeout(poll, 250);
  } else if (st.state === "done") {
    // Im Hintergrund meldet der Android-Dienst „fertig“ selbst; sichtbar braucht es das nicht
    if (document.visibilityState === "visible") android()?.stop();
    try {
      await loadDigits(st.digits);
      enterExplorer();
    } catch (e) {
      computeFailed(String(e));
    }
  } else {
    android()?.stop();
    computeFailed(st.state === "error" ? st.message : null);
  }
}

function setStartInputsDisabled(v: boolean) {
  presetsEl.querySelectorAll("button").forEach((b) => ((b as HTMLButtonElement).disabled = v));
  document.querySelectorAll<HTMLButtonElement>("#modes button").forEach((b) => (b.disabled = v));
  customEl.disabled = v;
  if (v) budgetsEl.querySelectorAll<HTMLButtonElement>("button").forEach((b) => (b.disabled = true));
}

function setProgress(phase: string, f: number) {
  $("pFill").style.width = `${(f * 100).toFixed(1)}%`;
  $("pPhase").textContent = `${phase} ${Math.floor(f * 100)} %`;
}

function resetStartUi() {
  S.computing = false;
  clearTimeout(pollTimer);
  $("progress").hidden = true;
  $<HTMLButtonElement>("computeBtn").hidden = false;
  setStartInputsDisabled(false);
  refreshModeUi();
}

function computeFailed(msg: string | null) {
  resetStartUi();
  if (msg) {
    $("startErr").textContent = msg;
    $("startErr").hidden = false;
  }
}

// ------------------------------------------------------------------ Ziffern auf die Grafikkarte
// Bei sehr vielen Stellen liegt nur ein Fenster auf der GPU; es wandert mit der Kamera mit.
const RESIDENT = touch ? 48_000_000 : 200_000_000;
let loadSeq = 0;
let windowed = false;

/** Fenster ab Zeile `startRow` laden. `onProgress` für die Startanzeige. */
async function loadWindow(startRow: number, onProgress?: (f: number) => void) {
  const W = S.width;
  const rowsFit = Math.max(1, Math.floor(renderer.n / W));
  const totalRows = Math.ceil(S.n / W);
  startRow = Math.max(0, Math.min(startRow, totalRows - rowsFit));
  const start = startRow * W;
  const len = Math.min(S.n - start, rowsFit * W);
  const seq = ++loadSeq;
  renderer.setWindow(start);
  S.dirty = true;
  const chunk = renderer.chunkDigits;
  for (let off = 0; off < len; off += chunk) {
    const data = await api.getDigits(start + off, Math.min(chunk, len - off));
    if (seq !== loadSeq) return;
    renderer.upload(off, data);
    S.dirty = true;
    onProgress?.(Math.min(1, (off + data.length) / len));
  }
}

function windowCovers(row: number): boolean {
  const start = renderer.winStart / S.width;
  const rows = renderer.loaded / S.width;
  return row >= start && row < start + Math.max(1, rows);
}

/**
 * Fenster nachladen, wenn der sichtbare Bereich (oder ein Ziel) den geladenen verlässt.
 * Das neue Fenster beginnt knapp vor dem Ziel: geladen wird von vorn, so ist das Ziel
 * schon im ersten Block sichtbar.
 */
function ensureWindow(row: number, force = false, visRows?: number) {
  if (!windowed) return;
  const W = S.width;
  const rowsFit = Math.max(1, Math.floor(renderer.n / W));
  const totalRows = Math.ceil(S.n / W);
  const start = renderer.winStart / W;
  const end = start + rowsFit;
  const vis = visRows ?? Math.ceil(cam.vh / Math.max(cam.s, 1e-6));
  const wide = vis >= rowsFit * 0.8;
  let ok: boolean;
  if (wide) {
    // weit herausgezoomt: Fenster nur grob mitführen
    ok = (row >= start + rowsFit * 0.25 || start === 0) && (row < end - rowsFit * 0.25 || end >= totalRows);
  } else {
    const top = row - vis / 2;
    const bot = row + vis / 2;
    ok = (top >= start || start === 0) && (bot < end || end >= totalRows);
  }
  if (ok && !force && Number.isInteger(start) && windowCovers(Math.min(row, totalRows - 1))) return;
  if (ok && !force && Number.isInteger(start)) return; // wird gerade geladen
  const lead = wide ? rowsFit / 2 : Math.min(rowsFit / 4, vis * 1.5);
  loadWindow(Math.floor(row - lead));
}

async function loadDigits(n: number) {
  S.n = n;
  windowed = n > RESIDENT;
  renderer.allocate(Math.min(n, RESIDENT));
  const gl = renderer.gl;
  if (gl.getError() === gl.OUT_OF_MEMORY) throw new Error("Not enough graphics memory for that many digits.");
  S.width = defaultWidth(n);
  await loadWindow(0, (f) => {
    $("pFill").style.width = `${(f * 100).toFixed(1)}%`;
    $("pPhase").textContent = `TO GPU ${Math.floor(f * 100)} %`;
  });
  if (gl.getError() === gl.OUT_OF_MEMORY) throw new Error("Not enough graphics memory for that many digits.");
}

function enterExplorer() {
  resetStartUi();
  const w = S.width;
  S.width = 0;
  setWidth(w, false);
  updateZoomLimits();
  ui.hidden = false;
  startEl.classList.add("fade");
  setTimeout(() => {
    if (startEl.classList.contains("fade")) startEl.hidden = true;
  }, 650);
  cam.jump(fitView());
  setStatus("READY");
  S.dirty = true;
  setTimeout(() => cam.flyTo(resetView(), performance.now()), 450);
  if (!statsEl.hidden) renderStats();
}

function showStart() {
  startEl.hidden = false;
  requestAnimationFrame(() => startEl.classList.remove("fade"));
  $("backBtn").hidden = !S.n;
  toggleShape(false);
}

$("computeBtn").onclick = startCompute;
$("cancelBtn").onclick = () => api.cancelCompute();
$("backBtn").onclick = () => {
  startEl.classList.add("fade");
  setTimeout(() => (startEl.hidden = true), 650);
};
$("newBtn").onclick = showStart;

// ------------------------------------------------------------------ Bedienelemente
document.querySelectorAll<HTMLButtonElement>(".tab").forEach((t) => (t.onclick = () => setMode(Number(t.dataset.mode) as Mode)));
$("wDec").onclick = (e) => setWidth(S.width - ((e as MouseEvent).shiftKey ? 10 : 1));
$("wInc").onclick = (e) => setWidth(S.width + ((e as MouseEvent).shiftKey ? 10 : 1));
wInput.addEventListener("change", () => {
  const v = parseCount(wInput.value);
  if (v) setWidth(v);
  else wInput.value = fmtInt(S.width);
});
wInput.addEventListener("focus", () => wInput.select());

const placeholders: Record<string, string> = {
  number: "type, then Enter",
  birthday: "DD.MM.YYYY, then Enter",
  word: "a word, then Enter",
};
kindEl.addEventListener("change", () => {
  queryEl.placeholder = placeholders[kindEl.value];
  queryEl.inputMode = kindEl.value === "word" ? "text" : kindEl.value === "birthday" ? "decimal" : "numeric";
  fmtToggle.hidden = kindEl.value !== "birthday";
  queryEl.value = "";
  queryEl.focus();
});
queryEl.inputMode = "numeric";
queryEl.addEventListener("keydown", (e) => {
  if (e.key === "Enter") runSearch();
});
$("searchBtn").onclick = runSearch;
$("shapeBtn").onclick = () => toggleShape();
hudBtn.onclick = () => toggleStats();
$("resClose").onclick = () => {
  hideResult();
  clearHighlight();
};
$("prevBtn").onclick = () => step(-1);
$("nextBtn").onclick = () => step(1);

window.addEventListener("keydown", (e) => {
  const t = e.target as HTMLElement;
  const typing = t instanceof HTMLInputElement || t instanceof HTMLSelectElement || t instanceof HTMLTextAreaElement;
  if (!startEl.hidden && !startEl.classList.contains("fade")) {
    if (e.key === "Enter" && !typing) startCompute();
    return;
  }
  if (typing) {
    if (e.key === "Escape") t.blur();
    return;
  }
  if (e.ctrlKey || e.metaKey || e.altKey) return;
  switch (e.key) {
    case "1":
    case "2":
    case "3":
    case "4":
      setMode((Number(e.key) - 1) as Mode);
      break;
    case "/":
      e.preventDefault();
      queryEl.focus();
      break;
    case "s":
    case "S":
      toggleShape();
      break;
    case "f":
    case "F":
      cam.flyTo(fitView(), performance.now());
      break;
    case "r":
    case "R":
      cam.flyTo(resetView(), performance.now());
      break;
    case "h":
    case "H":
      toggleStats();
      break;
    case "ArrowRight":
    case "n":
      step(1);
      break;
    case "ArrowLeft":
    case "p":
      step(-1);
      break;
    case "[":
      setWidth(S.width - (e.shiftKey ? 10 : 1));
      break;
    case "]":
      setWidth(S.width + (e.shiftKey ? 10 : 1));
      break;
    case "{":
      setWidth(S.width - 10);
      break;
    case "}":
      setWidth(S.width + 10);
      break;
    case "+":
    case "=":
      cam.zoomAt(cam.vw / 2, cam.vh / 2, 1.6);
      break;
    case "-":
      cam.zoomAt(cam.vw / 2, cam.vh / 2, 1 / 1.6);
      break;
    case "Escape":
      if (!shapePop.hidden) toggleShape(false);
      else if (!resultEl.hidden) {
        hideResult();
        clearHighlight();
      }
      break;
    default:
      return;
  }
  S.dirty = true;
});

// ------------------------------------------------------------------ Zeigen, Ziehen, Zoomen
const pointers = new Map<number, { x: number; y: number }>();
let vel = { x: 0, y: 0 };
let lastMoveT = 0;
let pinch: { d: number; mx: number; my: number } | null = null;
let downInfo: { t: number; x: number; y: number; moved: boolean } | null = null;
let lastTap = { t: 0, x: 0, y: 0 };

function pinchState() {
  const [a, b] = [...pointers.values()];
  return { d: Math.hypot(a.x - b.x, a.y - b.y) || 1, mx: (a.x + b.x) / 2, my: (a.y + b.y) / 2 };
}

canvas.addEventListener("pointerdown", (e) => {
  if (!S.n) return;
  canvas.setPointerCapture(e.pointerId);
  pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
  cam.stop();
  vel = { x: 0, y: 0 };
  canvas.classList.add("dragging");
  if (pointers.size === 2) pinch = pinchState();
  if (pointers.size === 1) downInfo = { t: e.timeStamp, x: e.clientX, y: e.clientY, moved: false };
  else downInfo = null;
});

canvas.addEventListener("pointermove", (e) => {
  const prev = pointers.get(e.pointerId);
  if (!prev) return;
  const cur = { x: e.clientX, y: e.clientY };
  pointers.set(e.pointerId, cur);
  if (pointers.size === 1) {
    const dx = cur.x - prev.x;
    const dy = cur.y - prev.y;
    cam.panBy(dx, dy);
    const dt = Math.max(1, e.timeStamp - lastMoveT);
    lastMoveT = e.timeStamp;
    vel = { x: vel.x * 0.6 + ((dx / dt) * 1000) * 0.4, y: vel.y * 0.6 + ((dy / dt) * 1000) * 0.4 };
    if (downInfo && Math.hypot(cur.x - downInfo.x, cur.y - downInfo.y) > 6) downInfo.moved = true;
  } else if (pointers.size === 2 && pinch) {
    const p = pinchState();
    cam.zoomAt(p.mx, p.my, p.d / pinch.d, false);
    cam.panBy(p.mx - pinch.mx, p.my - pinch.my);
    pinch = p;
  }
  S.dirty = true;
});

function pointerEnd(e: PointerEvent) {
  if (!pointers.has(e.pointerId)) return;
  pointers.delete(e.pointerId);
  if (pointers.size < 2) pinch = null;
  if (pointers.size === 0) {
    canvas.classList.remove("dragging");
    if (e.timeStamp - lastMoveT < 90 && Math.hypot(vel.x, vel.y) > 120) cam.fling(vel.x, vel.y);
    if (downInfo && !downInfo.moved && e.timeStamp - downInfo.t < 300) {
      // Doppeltippen / Doppelklick = hineinzoomen
      if (e.timeStamp - lastTap.t < 320 && Math.hypot(e.clientX - lastTap.x, e.clientY - lastTap.y) < 30) {
        cam.zoomAt(e.clientX, e.clientY, 2.5);
        lastTap.t = 0;
      } else lastTap = { t: e.timeStamp, x: e.clientX, y: e.clientY };
    }
    downInfo = null;
  }
}
canvas.addEventListener("pointerup", pointerEnd);
canvas.addEventListener("pointercancel", pointerEnd);

canvas.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    if (!S.n) return;
    const dy = e.deltaY * (e.deltaMode === 1 ? 33 : e.deltaMode === 2 ? cam.vh : 1);
    cam.zoomAt(e.clientX, e.clientY, Math.exp(-dy * (e.ctrlKey ? 0.01 : 0.0018)));
    S.dirty = true;
  },
  { passive: false },
);

// ------------------------------------------------------------------ Bildschleife
let lastT = performance.now();
let hudT = 0;
let winCheckT = 0;
function frame(now: number) {
  const dt = Math.min(0.05, (now - lastT) / 1000);
  lastT = now;
  const resized = renderer.resize();
  cam.setViewport(canvas.clientWidth, canvas.clientHeight);
  if (resized) updateZoomLimits();
  const moved = cam.update(now, dt);
  clampCam();
  // Fenster nachziehen, sobald die Kamera ruht
  if (windowed && S.n && !cam.flying && now - winCheckT > 300) {
    winCheckT = now;
    ensureWindow(Math.floor(cam.y));
  }
  const pulsing = S.rects.length > 0 && S.n > 0;
  if (moved || resized || S.dirty || pulsing) {
    renderer.draw({
      camX: cam.x,
      camY: cam.y,
      scale: cam.s,
      dpr: window.devicePixelRatio || 1,
      width: S.width,
      total: S.n,
      mode: S.mode,
      phase: S.phase,
      time: now / 1000,
      hlStart: S.hlStart,
      hlEnd: S.hlEnd,
      dim: S.dim,
      rects: S.rects,
      shape: S.shape,
    });
    if (moved || resized || S.dirty || now - hudT > 250) {
      updateHud();
      hudT = now;
    }
    S.dirty = false;
  }
  requestAnimationFrame(frame);
}

// ------------------------------------------------------------------ Start
async function boot() {
  try {
    renderer = new Renderer(canvas);
  } catch (e) {
    $("estimate").textContent = String(e);
    $<HTMLButtonElement>("computeBtn").disabled = true;
    return;
  }
  renderer.setGlyphs(await buildGlyphs('"JetBrains Mono"'));
  requestAnimationFrame(frame);
  updateLegend();
  if (!inTauri()) {
    $("estimate").innerHTML = "This page needs the π EXPLORER app (run <b>npm run tauri dev</b>).";
    $<HTMLButtonElement>("computeBtn").disabled = true;
    return;
  }
  choose(chosen);
  try {
    device = await api.deviceInfo();
  } catch (e) {
    $("estimate").textContent = String(e);
  }
  // Standard-Budget: 2 GB, auf kleinen Geräten höchstens ein Drittel des RAMs
  if (device) {
    const third = device.totalRam / 3 / 1024 / 1024;
    budgetMb = BUDGETS_MB.filter((b) => b <= Math.max(512, third)).reduce((a, b) => (b <= 2048 ? b : a), 512);
  }
  refreshModeUi();
  // Läuft schon eine Rechnung (App war im Hintergrund) oder ist eine fertig? Dann anhängen.
  try {
    const st = await api.status();
    if ((st.state === "running" || st.state === "done") && st.digits > 0) {
      // Auswahl so zeigen, wie gerechnet wird
      calcMode = st.mode === "split" ? "split" : "fast";
      if (st.budget) budgetMb = Math.round(st.budget / 1024 / 1024);
      chosen = st.digits;
      refreshModeUi();
      showComputing();
      return;
    }
  } catch (e) {
    console.error(e);
  }
  // Testhilfe: ?autostart=100000&mode=split&budget=512
  const q = new URLSearchParams(location.search);
  const auto = q.get("autostart");
  if (auto) {
    chosen = Number(auto);
    if (q.get("mode") === "split") calcMode = "split";
    if (q.get("budget")) budgetMb = Number(q.get("budget"));
    startCompute();
  }
}

document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible" && !S.computing && S.n > 0) android()?.stop();
});

boot();
(window as any).__pi = {
  S,
  cam,
  api,
  runSearch,
  setWidth,
  setMode,
  focusHit,
  toggleShape,
  toggleStats,
  shapeCells,
  renderer: () => renderer,
  setCalc: (m: CalcMode, mb: number) => {
    calcMode = m;
    budgetMb = mb;
    refreshModeUi();
  },
};
