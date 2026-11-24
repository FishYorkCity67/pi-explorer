const nf = new Intl.NumberFormat("en-US");

export const fmtInt = (n: number) => nf.format(Math.round(n));

export function fmtBytes(b: number): string {
  if (b >= 1024 ** 3) return `${(b / 1024 ** 3).toFixed(b >= 10 * 1024 ** 3 ? 0 : 1)} GB`;
  if (b >= 1024 ** 2) return `${Math.round(b / 1024 ** 2)} MB`;
  return `${Math.max(1, Math.round(b / 1024))} KB`;
}

export function fmtDuration(s: number): string {
  if (s < 0.01) return "< 0.01 s";
  if (s < 10) return `${s.toFixed(2)} s`;
  if (s < 90) return `${s.toFixed(1)} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min ${Math.round(s % 60)} s`;
  return `${Math.floor(s / 3600)} h ${Math.round((s % 3600) / 60)} min`;
}

export function fmtClock(s: number): string {
  const m = Math.floor(s / 60);
  const sec = s - m * 60;
  return `${String(m).padStart(2, "0")}:${sec.toFixed(1).padStart(4, "0")}`;
}

export function fmtZoom(z: number): string {
  if (z < 10) return `${z.toFixed(1)}×`;
  return `${fmtInt(z)}×`;
}

/** Kurz und lesbar: 480M, 1.25B, 75K */
export function fmtCompact(n: number): string {
  const f = (v: number, s: string) => `${v >= 100 ? Math.round(v) : v >= 10 ? +v.toFixed(1) : +v.toFixed(2)}${s}`;
  if (n >= 1e9) return f(n / 1e9, "B");
  if (n >= 1e6) return f(n / 1e6, "M");
  if (n >= 1e4) return f(n / 1e3, "K");
  return fmtInt(n);
}

/** "1M", "100K" usw. */
export function shortCount(n: number): string {
  if (n >= 1e9 && n % 1e9 === 0) return `${n / 1e9}B`;
  if (n >= 1e6 && n % 1e6 === 0) return `${n / 1e6}M`;
  if (n >= 1e3 && n % 1e3 === 0) return `${n / 1e3}K`;
  return fmtInt(n);
}

const SUFFIX: Record<string, number> = {
  k: 1e3,
  tsd: 1e3,
  m: 1e6,
  mio: 1e6,
  mil: 1e6,
  b: 1e9,
  bn: 1e9,
  mrd: 1e9,
  g: 1e9,
};

/**
 * "200m", "1.5b", "1,5 mrd", "750k", "1e9", "1,000,000", "1.000.000" → Zahl.
 * k/tsd = Tausend, m/mio = Million, b/bn/mrd = Milliarde.
 */
export function parseCount(s: string): number | null {
  let t = s.trim().toLowerCase().replace(/[\s_']/g, "");
  if (!t) return null;
  // Tausendertrenner "1,000,000" oder "1.000.000"
  if (/^\d{1,3}([.,]\d{3})+$/.test(t)) t = t.replace(/[.,]/g, "");
  t = t.replace(",", ".");
  const m = t.match(/^(\d+(?:\.\d+)?|\.\d+)([a-z]+|e\d+)?$/);
  if (!m) return null;
  let v = parseFloat(m[1]);
  const suf = m[2];
  if (suf) {
    if (/^e\d+$/.test(suf)) v *= 10 ** parseInt(suf.slice(1), 10);
    else if (SUFFIX[suf]) v *= SUFFIX[suf];
    else return null;
  }
  return Number.isFinite(v) ? Math.round(v) : null;
}
