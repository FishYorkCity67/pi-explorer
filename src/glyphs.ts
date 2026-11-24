/** Glyphen-Atlas: 0–9 und A–Z als Graustufen-Layer (für eine sampler2DArray-Textur). */
export const GLYPH_SIZE = 128;
export const GLYPH_COUNT = 36; // 0..9, A..Z

export interface GlyphAtlas {
  data: Uint8Array; // GLYPH_COUNT Layer à GLYPH_SIZE²
  coverage: number[]; // mittlere Deckung je Glyph
}

export async function buildGlyphs(font: string): Promise<GlyphAtlas> {
  try {
    await document.fonts.load(`500 96px ${font}`);
  } catch {
    /* Ersatzschrift */
  }
  const S = GLYPH_SIZE;
  const c = document.createElement("canvas");
  c.width = S;
  c.height = S;
  const ctx = c.getContext("2d", { willReadFrequently: true })!;
  const data = new Uint8Array(S * S * GLYPH_COUNT);
  const coverage: number[] = [];
  const chars = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
  ctx.font = `500 ${Math.round(S * 0.66)}px ${font}, ui-monospace, monospace`;
  ctx.textAlign = "center";
  ctx.textBaseline = "alphabetic";
  // senkrecht zentrieren anhand der Ziffer 0
  const m = ctx.measureText("0");
  const asc = m.actualBoundingBoxAscent;
  const desc = m.actualBoundingBoxDescent;
  const baseline = S / 2 + (asc - desc) / 2;
  for (let i = 0; i < GLYPH_COUNT; i++) {
    ctx.clearRect(0, 0, S, S);
    ctx.fillStyle = "#fff";
    ctx.fillText(chars[i], S / 2, baseline);
    const px = ctx.getImageData(0, 0, S, S).data;
    let sum = 0;
    const off = i * S * S;
    for (let p = 0; p < S * S; p++) {
      const a = px[p * 4 + 3];
      data[off + p] = a;
      sum += a;
    }
    coverage.push(sum / (S * S * 255));
  }
  return { data, coverage };
}
