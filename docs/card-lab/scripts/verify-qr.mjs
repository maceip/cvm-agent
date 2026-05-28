import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import sharp from "sharp";
import jsQR from "jsqr";

const cardsDir = resolve(dirname(fileURLToPath(import.meta.url)), "..", "cards");

// file -> { qr: [x, y, size] on the 1200x630 canvas, url: expected payload }
const expected = {
  "team-ledger.svg": { qr: [947, 378, 112], url: "https://runcard.dev/e/hacktx/neon-byte-club" },
  "team-relay.svg": { qr: [1012, 466, 74], url: "https://runcard.dev/e/agent-jam/shipshape" },
  "team-scoreline.svg": { qr: [957, 448, 86], url: "https://runcard.dev/e/min-tokens/careful-compiler" },
  "team-terminal.svg": { qr: [944, 380, 92], url: "https://runcard.dev/e/sf-agent-jam/vector-lab" },
  "individual-passport.svg": { qr: [1006, 88, 104], url: "https://runcard.dev/@mina.builds" },
  "individual-signal.svg": { qr: [960, 398, 112], url: "https://runcard.dev/@nova/signal" },
  "individual-tokenstream.svg": { qr: [895, 444, 94], url: "https://runcard.dev/@max-context" },
  "individual-pocket.svg": { qr: [180, 350, 108], url: "https://runcard.dev/@arivale" },
};

// jsQR is a single-shot decoder and is sensitive to module pixel size, so we
// rasterize at several scales and accept the first that decodes. A QR that
// decodes at any reasonable scale is a valid, scannable code.
const SCALES = [8, 6, 5, 10];
const QUIET = 64; // white border so the decoder sees a quiet zone

async function decodeQr(svg, [x, y, size]) {
  for (const scale of SCALES) {
    const { data, info } = await sharp(svg, { density: 72 * scale })
      .extract({
        left: Math.round(x * scale),
        top: Math.round(y * scale),
        width: Math.round(size * scale),
        height: Math.round(size * scale),
      })
      .flatten({ background: "#ffffff" })
      .extend({ top: QUIET, bottom: QUIET, left: QUIET, right: QUIET, background: "#ffffff" })
      .ensureAlpha()
      .raw()
      .toBuffer({ resolveWithObject: true });
    const result = jsQR(new Uint8ClampedArray(data), info.width, info.height);
    if (result?.data) return result.data;
  }
  return null;
}

let failures = 0;
for (const [file, { qr, url }] of Object.entries(expected)) {
  const svg = readFileSync(resolve(cardsDir, file));
  const decoded = await decodeQr(svg, qr);
  const ok = decoded === url;
  if (!ok) failures += 1;
  console.log(`${ok ? "PASS" : "FAIL"}  ${file}  ->  ${decoded ?? "(no QR found)"}`);
}

console.log(failures === 0 ? "\nAll QR codes decode to their receipt URL." : `\n${failures} card(s) failed.`);
process.exit(failures === 0 ? 0 : 1);
