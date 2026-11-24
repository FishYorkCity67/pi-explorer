import { invoke } from "@tauri-apps/api/core";

export type Mode = "fast" | "split";

export interface DeviceInfo {
  cores: number;
  totalRam: number;
  availRam: number;
  os: string;
  arch: string;
  maxDigitsFast: number;
  maxDigitsSplit: number;
  statusFile: string;
}

export interface Estimate {
  seconds: number;
  peakRam: number;
  availRam: number;
  totalRam: number;
  maxDigits: number;
  needsDisk: boolean;
  diskNeeded: number;
  diskFree: number;
  level: "ok" | "warn" | "danger";
}

export interface ComputeInfo {
  digits: number;
  mode: Mode;
  budget: number;
  seconds: number;
  phases: [string, number][];
  peakRss: number;
  diskPeak: number;
  tempDir: string;
}

export interface Status {
  state: "idle" | "running" | "done" | "cancelled" | "error";
  runId: number;
  phase: string;
  fraction: number;
  elapsed: number;
  digits: number;
  mode: Mode;
  budget: number;
  message: string;
  rss: number;
  disk: number;
  info: ComputeInfo | null;
}

export interface PatternOut {
  label: string;
  digits: string;
  count: number;
}

export interface SearchOut {
  patterns: PatternOut[];
  letters: string | null;
  total: number;
  stored: number;
  ms: number;
  /** [1-basierte Position, Musterindex] */
  hits: [number, number][];
  searched: number;
}

export interface ShapeOut {
  boxW: number;
  boxH: number;
  boxX: number;
  boxY: number;
  checkedCells: number;
  bestPos: number | null;
  bestMismatches: number | null;
  bestPercent: number | null;
  matchCount: number;
  stored: number;
  positions: number;
  ms: number;
  matches: [number, number][];
}

export interface StatsOut {
  digits: number;
  counts: number[];
  runDigit: number;
  runLen: number;
  runPos: number;
  info: ComputeInfo | null;
  rss: number;
  availRam: number;
}

export interface StorageOption {
  path: string;
  label: string;
  free: number;
}

export type SearchKind = "number" | "birthday" | "birthday_all" | "word";

export const api = {
  deviceInfo: () => invoke<DeviceInfo>("device_info"),
  storageOptions: () => invoke<StorageOption[]>("storage_options"),
  estimate: (digits: number, mode: Mode, budgetMb: number, tempDir: string | null) =>
    invoke<Estimate>("estimate", { digits, mode, budgetMb, tempDir }),
  startCompute: (digits: number, mode: Mode, budgetMb: number, tempDir: string | null) =>
    invoke<number>("start_compute", { digits, mode, budgetMb, tempDir }),
  status: () => invoke<Status>("compute_status"),
  cancelCompute: () => invoke<void>("cancel_compute"),
  async getDigits(offset: number, len: number): Promise<Uint8Array> {
    const buf = await invoke<ArrayBuffer | number[]>("get_digits", { offset, len });
    return buf instanceof ArrayBuffer ? new Uint8Array(buf) : Uint8Array.from(buf);
  },
  search: (kind: SearchKind, query: string) => invoke<SearchOut>("search", { kind, query }),
  getHits: (from: number, count: number) => invoke<[number, number][]>("get_hits", { from, count }),
  shapeSearch: (cells: boolean[], size: number, width: number, tolerance: number, onlyPainted: boolean) =>
    invoke<ShapeOut>("shape_search", { cells, size, width, tolerance, onlyPainted }),
  getShapeMatches: (from: number, count: number) =>
    invoke<[number, number][]>("get_shape_matches", { from, count }),
  stats: () => invoke<StatsOut>("get_stats"),
};

export const inTauri = () => typeof (window as any).__TAURI_INTERNALS__ !== "undefined";

/** Brücke zum Android-Hintergrunddienst (nur in der Android-App vorhanden). */
interface PiAndroidBridge {
  start(statusFile: string): void;
  stop(): void;
  requestNotifications(): void;
}
export const android = (): PiAndroidBridge | null => (window as any).PiAndroid ?? null;
