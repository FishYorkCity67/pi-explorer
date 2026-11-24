/**
 * Kamera in Zellen-Koordinaten. x/y = Mittelpunkt, s = CSS-Pixel pro Zelle.
 * - Mausrad/Pinch: Zielwerte, denen die Kamera weich folgt.
 * - Flüge: "Smooth and efficient zooming and panning" (van Wijk & Nuij) – erst raus, dann rein.
 */
export interface View {
  x: number;
  y: number;
  s: number;
}

interface Flight {
  t0: number;
  dur: number;
  S: number;
  from: View;
  to: View;
  r0: number;
  rho: number;
  u1: number;
  dir: [number, number];
  w0: number;
  zoomOnly: boolean;
  onDone?: () => void;
}

const RHO = 1.42;

export class Camera {
  x = 0;
  y = 0;
  s = 20;
  tx = 0;
  ty = 0;
  ts = 20;
  minS = 0.01;
  maxS = 400;
  private flight: Flight | null = null;
  private anchor: { wx: number; wy: number; px: number; py: number } | null = null;
  /** Wurfgeschwindigkeit in Zellen/s */
  vx = 0;
  vy = 0;
  vw = 1;
  vh = 1;

  setViewport(w: number, h: number) {
    this.vw = Math.max(1, w);
    this.vh = Math.max(1, h);
  }

  get animating() {
    return (
      this.flight !== null ||
      Math.abs(this.vx) + Math.abs(this.vy) > 1e-3 ||
      Math.abs(Math.log(this.ts / this.s)) > 1e-4 ||
      Math.abs(this.tx - this.x) * this.s > 0.05 ||
      Math.abs(this.ty - this.y) * this.s > 0.05
    );
  }

  get flying() {
    return this.flight !== null;
  }

  jump(v: View) {
    this.flight = null;
    this.anchor = null;
    this.vx = this.vy = 0;
    this.x = this.tx = v.x;
    this.y = this.ty = v.y;
    this.s = this.ts = this.clampS(v.s);
  }

  clampS(s: number) {
    return Math.min(this.maxS, Math.max(this.minS, s));
  }

  /** Bildschirmpunkt (CSS px) → Welt */
  toWorld(px: number, py: number, v: View = this): [number, number] {
    return [v.x + (px - this.vw / 2) / v.s, v.y + (py - this.vh / 2) / v.s];
  }

  /** Zoom um einen Bildschirmpunkt (auf die Zielwerte, weich) */
  zoomAt(px: number, py: number, factor: number, smooth = true) {
    this.flight = null;
    const base: View = smooth ? { x: this.tx, y: this.ty, s: this.ts } : this;
    const [wx, wy] = this.toWorld(px, py, base);
    const ns = this.clampS(base.s * factor);
    const nx = wx - (px - this.vw / 2) / ns;
    const ny = wy - (py - this.vh / 2) / ns;
    this.tx = nx;
    this.ty = ny;
    this.ts = ns;
    this.anchor = smooth ? { wx, wy, px, py } : null;
    if (!smooth) {
      this.x = nx;
      this.y = ny;
      this.s = ns;
    }
  }

  /** Verschieben um Bildschirm-Pixel (sofort) */
  panBy(dx: number, dy: number) {
    this.flight = null;
    if (this.anchor) {
      // laufenden Zoom übernehmen, Verankerung lösen
      this.anchor.px += dx;
      this.anchor.py += dy;
    }
    this.x -= dx / this.s;
    this.y -= dy / this.s;
    this.tx -= dx / this.s;
    this.ty -= dy / this.s;
    // Zoom-Ziel mitführen, falls gerade gezoomt wird
  }

  fling(vxPx: number, vyPx: number) {
    this.vx = -vxPx / this.s;
    this.vy = -vyPx / this.s;
  }

  stop() {
    this.flight = null;
    this.anchor = null;
    this.vx = this.vy = 0;
    this.tx = this.x;
    this.ty = this.y;
    this.ts = this.s;
  }

  flyTo(to: View, now: number, onDone?: () => void) {
    this.vx = this.vy = 0;
    this.anchor = null;
    to = { ...to, s: this.clampS(to.s) };
    const from: View = { x: this.x, y: this.y, s: this.s };
    // Weltbreite des Bildschirms
    const w0 = this.vw / from.s;
    const w1 = this.vw / to.s;
    const dx = to.x - from.x;
    const dy = to.y - from.y;
    const u1 = Math.hypot(dx, dy);
    let S: number;
    let r0 = 0;
    const zoomOnly = u1 < 1e-6 * Math.max(w0, w1);
    if (zoomOnly) {
      S = Math.abs(Math.log(w1 / w0)) / RHO;
    } else {
      const b = (i: 0 | 1) => {
        const wi = i === 0 ? w0 : w1;
        return (w1 * w1 - w0 * w0 + (i === 0 ? 1 : -1) * RHO ** 4 * u1 * u1) / (2 * wi * RHO * RHO * u1);
      };
      const r = (bi: number) => Math.log(-bi + Math.sqrt(bi * bi + 1));
      r0 = r(b(0));
      const r1 = r(b(1));
      S = (r1 - r0) / RHO;
    }
    if (!Number.isFinite(S) || S < 1e-6) {
      this.jump(to);
      onDone?.();
      return;
    }
    const dur = Math.min(2600, Math.max(650, 420 * Math.sqrt(S) + 250 * S));
    this.flight = {
      t0: now,
      dur,
      S,
      from,
      to,
      r0,
      rho: RHO,
      u1,
      dir: zoomOnly ? [0, 0] : [dx / u1, dy / u1],
      w0,
      zoomOnly,
      onDone,
    };
  }

  /** Einen Zeitschritt rechnen; true = etwas hat sich bewegt */
  update(now: number, dt: number): boolean {
    const f = this.flight;
    if (f) {
      let t = Math.min(1, (now - f.t0) / f.dur);
      // weich an- und auslaufen
      const e = t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
      const s = e * f.S;
      let u: number;
      let w: number;
      if (f.zoomOnly) {
        const k = f.to.s < f.from.s ? 1 : -1;
        u = 0;
        w = f.w0 * Math.exp(k * f.rho * s);
      } else {
        const { r0, rho, w0 } = f;
        u = (w0 / (rho * rho)) * Math.cosh(r0) * Math.tanh(rho * s + r0) - (w0 / (rho * rho)) * Math.sinh(r0);
        w = (w0 * Math.cosh(r0)) / Math.cosh(rho * s + r0);
      }
      this.x = f.from.x + f.dir[0] * u;
      this.y = f.from.y + f.dir[1] * u;
      this.s = this.vw / w;
      if (t >= 1) {
        this.x = f.to.x;
        this.y = f.to.y;
        this.s = f.to.s;
        this.flight = null;
        this.tx = this.x;
        this.ty = this.y;
        this.ts = this.s;
        f.onDone?.();
      } else {
        this.tx = this.x;
        this.ty = this.y;
        this.ts = this.s;
      }
      return true;
    }
    let moved = false;
    // Wurf
    if (Math.abs(this.vx) + Math.abs(this.vy) > 1e-3) {
      this.x += this.vx * dt;
      this.y += this.vy * dt;
      this.tx += this.vx * dt;
      this.ty += this.vy * dt;
      const decay = Math.exp(-dt * 4.2);
      this.vx *= decay;
      this.vy *= decay;
      if (Math.hypot(this.vx, this.vy) * this.s < 8) this.vx = this.vy = 0;
      moved = true;
    }
    // weich zum Ziel (kritisch gedämpft angenähert)
    const k = 1 - Math.exp(-dt * 16);
    if (Math.abs(Math.log(this.ts / this.s)) > 1e-4 || Math.abs(this.tx - this.x) * this.s > 0.05 || Math.abs(this.ty - this.y) * this.s > 0.05) {
      this.s = Math.exp(Math.log(this.s) + (Math.log(this.ts) - Math.log(this.s)) * k);
      const a = this.anchor;
      if (a) {
        this.x = a.wx - (a.px - this.vw / 2) / this.s;
        this.y = a.wy - (a.py - this.vh / 2) / this.s;
        this.tx = a.wx - (a.px - this.vw / 2) / this.ts;
        this.ty = a.wy - (a.py - this.vh / 2) / this.ts;
      } else {
        this.x += (this.tx - this.x) * k;
        this.y += (this.ty - this.y) * k;
      }
      moved = true;
    } else if (this.s !== this.ts || this.x !== this.tx || this.y !== this.ty) {
      this.s = this.ts;
      this.x = this.tx;
      this.y = this.ty;
      this.anchor = null;
      moved = true;
    } else {
      this.anchor = null;
    }
    return moved;
  }
}
