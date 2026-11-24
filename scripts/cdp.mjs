// Testhilfe: steuert die laufende Desktop-App über das DevTools-Protokoll von WebView2.
// Start der App mit: $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9333'
//
//   node scripts/cdp.mjs eval "<js-ausdruck>"
//   node scripts/cdp.mjs shot <datei.png>
//   node scripts/cdp.mjs click <x> <y>
//   node scripts/cdp.mjs key <taste>
//   node scripts/cdp.mjs wheel <x> <y> <deltaY>
//   node scripts/cdp.mjs drag <x0> <y0> <x1> <y1>
import { writeFileSync } from "node:fs";

const port = process.env.CDP_PORT || 9333;
const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = targets.find((t) => t.type === "page");
if (!page) throw new Error("keine Seite gefunden");
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r, j) => {
  ws.onopen = r;
  ws.onerror = j;
});
let id = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id && pending.has(d.id)) {
    pending.get(d.id)(d);
    pending.delete(d.id);
  }
};
const send = (method, params = {}) =>
  new Promise((r) => {
    const i = ++id;
    pending.set(i, r);
    ws.send(JSON.stringify({ id: i, method, params }));
  });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const [cmd, ...args] = process.argv.slice(2);
if (cmd === "eval") {
  const r = await send("Runtime.evaluate", { expression: args.join(" "), awaitPromise: true, returnByValue: true });
  console.log(JSON.stringify(r.result?.result?.value ?? r.result?.exceptionDetails ?? r, null, 1));
} else if (cmd === "shot") {
  const r = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(args[0], Buffer.from(r.result.data, "base64"));
  console.log("ok", args[0]);
} else if (cmd === "shotm") {
  // shotm <breite> <höhe> <datei> [js] – Handy-Ansicht nachstellen
  const [w, h, file, js] = args;
  await send("Emulation.setDeviceMetricsOverride", { width: +w, height: +h, deviceScaleFactor: 2.6, mobile: true });
  await send("Emulation.setTouchEmulationEnabled", { enabled: true });
  await sleep(500);
  if (js) {
    await send("Runtime.evaluate", { expression: js, awaitPromise: true });
    await sleep(2500);
  }
  const r = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(file, Buffer.from(r.result.data, "base64"));
  await send("Emulation.clearDeviceMetricsOverride");
  console.log("ok", file);
} else if (cmd === "click") {
  const [x, y] = args.map(Number);
  for (const type of ["mousePressed", "mouseReleased"])
    await send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1 });
  console.log("ok");
} else if (cmd === "key") {
  const key = args[0];
  await send("Input.dispatchKeyEvent", { type: "keyDown", key, text: key.length === 1 ? key : undefined });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key });
  console.log("ok");
} else if (cmd === "wheel") {
  const [x, y, dy] = args.map(Number);
  await send("Input.dispatchMouseEvent", { type: "mouseWheel", x, y, deltaX: 0, deltaY: dy });
  console.log("ok");
} else if (cmd === "drag") {
  const [x0, y0, x1, y1] = args.map(Number);
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x: x0, y: y0, button: "left", clickCount: 1 });
  for (let i = 1; i <= 12; i++) {
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: x0 + ((x1 - x0) * i) / 12, y: y0 + ((y1 - y0) * i) / 12, button: "left", buttons: 1 });
    await sleep(16);
  }
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: x1, y: y1, button: "left", clickCount: 1 });
  console.log("ok");
}
ws.close();
