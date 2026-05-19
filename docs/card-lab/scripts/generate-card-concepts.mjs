import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const labRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outDir = resolve(labRoot, "cards");
mkdirSync(outDir, { recursive: true });

const W = 1200;
const H = 630;
const font = "Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif";
const mono = "SFMono-Regular, ui-monospace, Consolas, Liberation Mono, monospace";

const cardData = [
  {
    slug: "team-ledger",
    type: "team",
    title: "Team Ledger",
    desc: "A bright scorecard for a hackathon team with aggregate tokens, member coverage, capture mix, and assurance breakdown.",
    concept: "Public scoreboard card for teams that want proof, rank, and fairness at a glance.",
    svg: teamLedger(),
  },
  {
    slug: "team-relay",
    type: "team",
    title: "Team Relay",
    desc: "A network-style team card that highlights multiple members, paths, and proof flow.",
    concept: "Makes the team feel like a coordinated system instead of one shared API key.",
    svg: teamRelay(),
  },
  {
    slug: "team-scoreline",
    type: "team",
    title: "Team Scoreline",
    desc: "A competition-first card for token constraint challenges such as minimum tokens or maximum tokens.",
    concept: "Built for organizers who want challenge formats to be legible from across a room.",
    svg: teamScoreline(),
  },
  {
    slug: "team-terminal",
    type: "team",
    title: "Team Terminal",
    desc: "A compact terminal-inspired team card that feels native in READMEs, CI, and developer profiles.",
    concept: "The least marketing-shaped team card: terse, inspectable, and easy to embed.",
    svg: teamTerminal(),
  },
  {
    slug: "individual-passport",
    type: "individual",
    title: "Individual Passport",
    desc: "A personal proof passport that foregrounds identity, device proof, and current capture posture.",
    concept: "Daily-use card for a hacker who wants a recognizable personal proof object.",
    svg: individualPassport(),
  },
  {
    slug: "individual-signal",
    type: "individual",
    title: "Individual Signal",
    desc: "A live telemetry card focused on active path, latest event, and capture health.",
    concept: "Best for streaming or profile use where the card should feel alive without becoming busy.",
    svg: individualSignal(),
  },
  {
    slug: "individual-tokenstream",
    type: "individual",
    title: "Individual Tokenstream",
    desc: "A brag-card for daily or monthly model activity with model mix and visibility controls.",
    concept: "For people who want to prove that the token burn is real and publicly inspectable.",
    svg: individualTokenstream(),
  },
  {
    slug: "individual-pocket",
    type: "individual",
    title: "Individual Pocket",
    desc: "A small, high-contrast individual card optimized for social bios and tight embed surfaces.",
    concept: "The card that can survive being shrunk, screenshotted, or pasted into a profile.",
    svg: individualPocket(),
  },
];

for (const card of cardData) {
  writeFileSync(resolve(outDir, `${card.slug}.svg`), card.svg);
}

writeFileSync(resolve(labRoot, "index.html"), renderIndex(cardData));

function escapeXml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function svgShell(title, desc, body, defs = "") {
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}" role="img" aria-labelledby="title desc">
  <title id="title">${escapeXml(title)}</title>
  <desc id="desc">${escapeXml(desc)}</desc>
  <defs>${defs}</defs>
${body}
</svg>
`;
}

function text(x, y, value, opts = {}) {
  const {
    size = 24,
    weight = 500,
    fill = "#111",
    family = font,
    anchor = "start",
    opacity = 1,
    transform = "",
  } = opts;
  const attrs = [
    `x="${x}"`,
    `y="${y}"`,
    `fill="${fill}"`,
    `font-family="${family}"`,
    `font-size="${size}"`,
    `font-weight="${weight}"`,
    `text-anchor="${anchor}"`,
  ];
  if (opacity !== 1) attrs.push(`opacity="${opacity}"`);
  if (transform) attrs.push(`transform="${transform}"`);
  return `<text ${attrs.join(" ")}>${escapeXml(value)}</text>`;
}

function rect(x, y, width, height, opts = {}) {
  const {
    fill = "none",
    stroke = "none",
    sw = 1,
    rx = 0,
    opacity = 1,
  } = opts;
  return `<rect x="${x}" y="${y}" width="${width}" height="${height}" rx="${rx}" fill="${fill}" stroke="${stroke}" stroke-width="${sw}" opacity="${opacity}"/>`;
}

function line(x1, y1, x2, y2, opts = {}) {
  const { stroke = "#111", sw = 2, opacity = 1, dash = "" } = opts;
  const dashAttr = dash ? ` stroke-dasharray="${dash}"` : "";
  return `<line x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" stroke="${stroke}" stroke-width="${sw}" opacity="${opacity}"${dashAttr}/>`;
}

function pill(x, y, width, label, opts = {}) {
  const {
    fill = "#111",
    color = "#fff",
    stroke = "none",
    size = 18,
    weight = 700,
  } = opts;
  return `<g>
  ${rect(x, y, width, 34, { fill, stroke, rx: 17 })}
  ${text(x + width / 2, y + 23, label, { size, weight, fill: color, anchor: "middle" })}
</g>`;
}

function metric(x, y, label, value, sub, opts = {}) {
  const {
    fill = "#fff",
    stroke = "#111",
    accent = "#18b7a6",
    valueColor = "#111",
    labelColor = "#4c4c4c",
    width = 238,
  } = opts;
  return `<g>
  ${rect(x, y, width, 120, { fill, stroke, sw: 2, rx: 10 })}
  ${rect(x, y, 12, 120, { fill: accent, rx: 6 })}
  ${text(x + 28, y + 37, label, { size: 17, weight: 750, fill: labelColor, family: mono })}
  ${text(x + 28, y + 82, value, { size: 39, weight: 850, fill: valueColor })}
  ${text(x + 28, y + 106, sub, { size: 16, weight: 600, fill: labelColor })}
</g>`;
}

function miniQr(x, y, size = 116, fg = "#111", bg = "#fff") {
  const cells = [
    [1, 1, 5, 5], [2, 2, 3, 3], [10, 1, 5, 5], [11, 2, 3, 3], [1, 10, 5, 5], [2, 11, 3, 3],
    [7, 7, 1, 3], [9, 7, 2, 1], [6, 11, 3, 1], [10, 11, 1, 3], [13, 8, 2, 1],
    [7, 14, 2, 1], [14, 13, 1, 2], [8, 3, 1, 1], [3, 8, 2, 1], [12, 15, 2, 1],
  ];
  const unit = size / 17;
  return `<g transform="translate(${x} ${y})">
  ${rect(0, 0, size, size, { fill: bg, stroke: fg, sw: 2, rx: 8 })}
  ${cells.map(([cx, cy, cw, ch]) => rect(cx * unit, cy * unit, cw * unit, ch * unit, { fill: fg })).join("\n  ")}
</g>`;
}

function assuranceBar(x, y, width, parts) {
  let cursor = x;
  const total = parts.reduce((sum, part) => sum + part.value, 0);
  return `<g>
  ${parts.map((part, i) => {
    const partWidth = Math.round((part.value / total) * width);
    const segment = rect(cursor, y, i === parts.length - 1 ? x + width - cursor : partWidth, 18, { fill: part.color, rx: i === 0 || i === parts.length - 1 ? 9 : 0 });
    cursor += partWidth;
    return segment;
  }).join("\n  ")}
</g>`;
}

function stackedModelBars(x, y, rows, width = 300) {
  return `<g>
${rows.map((row, i) => {
  const top = y + i * 43;
  return `  ${text(x, top + 18, row.label, { size: 16, weight: 750, fill: row.fg, family: mono })}
  ${rect(x + 92, top + 4, width, 16, { fill: row.track, rx: 8 })}
  ${rect(x + 92, top + 4, Math.round(width * row.value), 16, { fill: row.color, rx: 8 })}
  ${text(x + 92 + width + 18, top + 18, row.caption, { size: 15, weight: 700, fill: row.fg, family: mono })}`;
}).join("\n")}
</g>`;
}

function teamLedger() {
  const body = `
${rect(0, 0, W, H, { fill: "#f6f2e7" })}
${rect(40, 38, 1120, 554, { fill: "#fffdf6", stroke: "#1c1b17", sw: 3, rx: 18 })}
${rect(64, 62, 290, 68, { fill: "#14130f", rx: 10 })}
${text(88, 106, "TEAM RUN CARD", { size: 29, weight: 900, fill: "#fffdf6", family: mono })}
${pill(905, 70, 190, "trust over tokens", { fill: "#f4ff5b", color: "#14130f", stroke: "#14130f", size: 17 })}
${text(76, 178, "Neon Byte Club", { size: 66, weight: 900, fill: "#14130f" })}
${text(80, 221, "HackTX Autonomous Build-Off", { size: 25, weight: 650, fill: "#4e4b42" })}
${rect(878, 154, 220, 142, { fill: "#1fb7a6", stroke: "#14130f", sw: 3, rx: 16 })}
${text(988, 202, "RANK", { size: 20, weight: 850, fill: "#081816", family: mono, anchor: "middle" })}
${text(988, 267, "#02", { size: 70, weight: 950, fill: "#fffdf6", anchor: "middle" })}
${metric(80, 270, "TOKENS", "1.84M", "provider reported", { accent: "#ff5a5f" })}
${metric(343, 270, "CALLS", "212", "gateway + proxy", { accent: "#1fb7a6" })}
${metric(606, 270, "MEMBERS", "5 / 5", "active installs", { accent: "#f4c430" })}
${text(84, 448, "capture mix", { size: 22, weight: 850, fill: "#14130f", family: mono })}
${pill(230, 421, 128, "gateway", { fill: "#14130f", color: "#fffdf6", size: 16 })}
${pill(374, 421, 152, "workbench", { fill: "#1fb7a6", color: "#081816", size: 16 })}
${pill(542, 421, 158, "tee space", { fill: "#f4ff5b", color: "#14130f", stroke: "#14130f", size: 16 })}
${pill(716, 421, 130, "browser", { fill: "#ff8d3a", color: "#14130f", size: 16 })}
${text(84, 505, "assurance", { size: 22, weight: 850, fill: "#14130f", family: mono })}
${assuranceBar(230, 490, 490, [
  { value: 76, color: "#1fb7a6" },
  { value: 19, color: "#f4c430" },
  { value: 5, color: "#ff5a5f" },
])}
${text(738, 506, "76% attested | 19% managed | 5% participant", { size: 18, weight: 700, fill: "#4e4b42" })}
${miniQr(947, 378, 112, "#14130f", "#fffdf6")}
${text(1003, 523, "scan receipt", { size: 17, weight: 850, fill: "#14130f", family: mono, anchor: "middle" })}
${line(64, 562, 1136, 562, { stroke: "#14130f", sw: 2 })}
${text(78, 580, "event log root 9b7c...20af | latest quote 43s ago | prize policy min-tokens", { size: 16, weight: 650, fill: "#4e4b42", family: mono })}
`;
  return svgShell("Team Ledger Runcard", "A team scorecard showing aggregate proof, token totals, capture mix, and assurance distribution.", body);
}

function teamRelay() {
  const body = `
${rect(0, 0, W, H, { fill: "#12110e" })}
${rect(44, 42, 1112, 546, { fill: "#191714", stroke: "#fbf1d6", sw: 2, rx: 22 })}
${text(76, 102, "TEAM RELAY", { size: 27, weight: 900, fill: "#ffcc4d", family: mono })}
${text(75, 171, "Shipshape", { size: 76, weight: 950, fill: "#fbf1d6" })}
${text(78, 213, "Proof graph for 4 builders across 6 agent sessions", { size: 24, weight: 650, fill: "#bfb59a" })}
${line(145, 344, 372, 270, { stroke: "#39d6c6", sw: 8 })}
${line(145, 344, 447, 394, { stroke: "#ff745f", sw: 8 })}
${line(372, 270, 650, 230, { stroke: "#ffcc4d", sw: 8 })}
${line(447, 394, 685, 438, { stroke: "#39d6c6", sw: 8 })}
${line(650, 230, 916, 318, { stroke: "#ff745f", sw: 8 })}
${line(685, 438, 916, 318, { stroke: "#ffcc4d", sw: 8 })}
${memberNode(145, 344, "AL", "#39d6c6")}
${memberNode(372, 270, "JU", "#ffcc4d")}
${memberNode(447, 394, "KE", "#ff745f")}
${memberNode(650, 230, "MO", "#39d6c6")}
${memberNode(685, 438, "AI", "#ffcc4d")}
${memberNode(916, 318, "RC", "#fbf1d6")}
${rect(74, 476, 560, 68, { fill: "#fbf1d6", stroke: "#fbf1d6", rx: 12 })}
${text(102, 519, "active path: managed keys + strict workbench", { size: 24, weight: 900, fill: "#191714", family: mono })}
${rect(706, 82, 366, 140, { fill: "#23211c", stroke: "#fbf1d6", sw: 2, rx: 16 })}
${text(734, 125, "TEAM TOTAL", { size: 19, weight: 900, fill: "#ffcc4d", family: mono })}
${text(734, 181, "684k tokens", { size: 45, weight: 950, fill: "#fbf1d6" })}
${rect(706, 448, 366, 92, { fill: "#23211c", stroke: "#fbf1d6", sw: 2, rx: 16 })}
${text(734, 485, "assurance: attested 61%", { size: 19, weight: 850, fill: "#fbf1d6", family: mono })}
${text(734, 518, "managed keys 39%", { size: 19, weight: 750, fill: "#bfb59a", family: mono })}
${miniQr(1012, 466, 74, "#191714", "#fbf1d6")}
${text(76, 576, "proof over claims | latest event: claude-code call captured by gateway 18s ago", { size: 18, weight: 700, fill: "#bfb59a", family: mono })}
`;
  return svgShell("Team Relay Runcard", "A team card showing multiple participants flowing into one verifiable run card.", body);
}

function memberNode(x, y, label, fill) {
  return `<g>
  <circle cx="${x}" cy="${y}" r="45" fill="${fill}" stroke="#fbf1d6" stroke-width="3"/>
  ${text(x, y + 11, label, { size: 29, weight: 950, fill: "#12110e", family: mono, anchor: "middle" })}
</g>`;
}

function teamScoreline() {
  const body = `
${rect(0, 0, W, H, { fill: "#ffffff" })}
${rect(0, 0, W, 112, { fill: "#e8ff57" })}
${rect(52, 48, 1096, 532, { fill: "#ffffff", stroke: "#101010", sw: 3, rx: 18 })}
${text(82, 98, "MIN TOKENS CHALLENGE", { size: 33, weight: 950, fill: "#101010", family: mono })}
${text(82, 181, "Team: Careful Compiler", { size: 54, weight: 950, fill: "#101010" })}
${pill(826, 142, 214, "proof before prize", { fill: "#101010", color: "#fff", size: 18 })}
${rect(84, 236, 348, 188, { fill: "#101010", rx: 16 })}
${text(112, 284, "score", { size: 22, weight: 850, fill: "#e8ff57", family: mono })}
${text(112, 365, "86,441", { size: 76, weight: 950, fill: "#fff" })}
${text(116, 397, "tokens for accepted build", { size: 20, weight: 700, fill: "#d6d6d6" })}
${rect(462, 236, 300, 188, { fill: "#f5f7fb", stroke: "#101010", sw: 2, rx: 16 })}
${text(492, 286, "members captured", { size: 20, weight: 850, fill: "#101010", family: mono })}
${text(492, 365, "4 / 4", { size: 74, weight: 950, fill: "#101010" })}
${rect(792, 236, 282, 188, { fill: "#fff6f2", stroke: "#101010", sw: 2, rx: 16 })}
${text(822, 286, "rank delta", { size: 20, weight: 850, fill: "#101010", family: mono })}
${text(822, 365, "+7", { size: 78, weight: 950, fill: "#ff4f79" })}
${text(84, 476, "leaderboard row", { size: 20, weight: 900, fill: "#101010", family: mono })}
${rect(286, 452, 612, 42, { fill: "#f5f7fb", stroke: "#101010", sw: 2, rx: 12 })}
${rect(286, 452, 284, 42, { fill: "#36d7b7", rx: 12 })}
${rect(570, 452, 156, 42, { fill: "#ffcf4a" })}
${rect(726, 452, 172, 42, { fill: "#ff7b54", rx: 12 })}
${text(310, 480, "gateway", { size: 18, weight: 900, fill: "#101010", family: mono })}
${text(594, 480, "local", { size: 18, weight: 900, fill: "#101010", family: mono })}
${text(750, 480, "tee", { size: 18, weight: 900, fill: "#101010", family: mono })}
${miniQr(957, 448, 86, "#101010", "#fff")}
${text(84, 548, "enforcement: routed + managed keys | evidence window: May 19 09:00-17:00 UTC", { size: 18, weight: 700, fill: "#333", family: mono })}
`;
  return svgShell("Team Scoreline Runcard", "A competition-oriented team card for token-limited hackathon formats.", body);
}

function teamTerminal() {
  const body = `
${rect(0, 0, W, H, { fill: "#070807" })}
${rect(36, 34, 1128, 560, { fill: "#0d100d", stroke: "#d9f99d", sw: 2, rx: 12 })}
${rect(36, 34, 1128, 56, { fill: "#d9f99d", rx: 12 })}
${text(64, 72, "runcard team status --event sf-agent-jam --team vector-lab", { size: 24, weight: 900, fill: "#0d100d", family: mono })}
${terminalLine(72, 142, "$ team", "Vector Lab", "#fef3c7")}
${terminalLine(72, 190, "$ members", "alex:TEE  bea:managed  cam:TEE", "#a7f3d0")}
${terminalLine(72, 238, "$ capture", "gw=117  proxy=34  wb=8  browser=2", "#bae6fd")}
${terminalLine(72, 286, "$ tokens", "2.40M total | 1.98M provider", "#fda4af")}
${terminalLine(72, 334, "$ latest", "openai.responses via tee-gateway", "#fef08a")}
${rect(74, 388, 600, 96, { fill: "#111811", stroke: "#3f563f", sw: 2, rx: 10 })}
${text(102, 427, "remote attestation", { size: 21, weight: 850, fill: "#d9f99d", family: mono })}
${text(102, 461, "stage0:value-x ok  runtime:tdx ok  tls-spki ok", { size: 23, weight: 850, fill: "#f8fafc", family: mono })}
${rect(724, 132, 346, 352, { fill: "#111811", stroke: "#3f563f", sw: 2, rx: 10 })}
${text(752, 178, "visibility", { size: 20, weight: 850, fill: "#d9f99d", family: mono })}
${text(752, 226, "team total", { size: 23, weight: 800, fill: "#f8fafc", family: mono })}
${text(752, 276, "model mix", { size: 20, weight: 850, fill: "#d9f99d", family: mono })}
${stackedModelBars(752, 304, [
  { label: "gpt", value: 0.55, caption: "55%", color: "#a7f3d0", track: "#233123", fg: "#f8fafc" },
  { label: "claude", value: 0.31, caption: "31%", color: "#bae6fd", track: "#233123", fg: "#f8fafc" },
  { label: "local", value: 0.14, caption: "14%", color: "#fef08a", track: "#233123", fg: "#f8fafc" },
], 178)}
${miniQr(944, 380, 92, "#0d100d", "#d9f99d")}
${text(74, 548, "TRUST OVER TOKENS | receipt root 9d14...ee31 | card policy public aggregate / private prompts", { size: 19, weight: 800, fill: "#9abf9a", family: mono })}
`;
  return svgShell("Team Terminal Runcard", "A terminal-native team card for README and CI surfaces.", body);
}

function terminalLine(x, y, key, value, color) {
  return `<g>
  ${text(x, y, key, { size: 24, weight: 850, fill: "#8fbc8f", family: mono })}
  ${text(x + 150, y, value, { size: 26, weight: 850, fill: color, family: mono })}
</g>`;
}

function individualPassport() {
  const body = `
${rect(0, 0, W, H, { fill: "#eef7f4" })}
${rect(54, 46, 1092, 538, { fill: "#fff8e8", stroke: "#11211d", sw: 3, rx: 20 })}
${rect(86, 80, 255, 396, { fill: "#c8efe8", stroke: "#11211d", sw: 3, rx: 12 })}
${rect(116, 112, 195, 180, { fill: "#11211d", rx: 12 })}
${text(213, 221, "MP", { size: 76, weight: 950, fill: "#fff8e8", family: mono, anchor: "middle" })}
${text(116, 341, "participant", { size: 19, weight: 850, fill: "#11211d", family: mono })}
${text(116, 386, "@mina.builds", { size: 30, weight: 900, fill: "#11211d", family: mono })}
${pill(116, 416, 158, "public daily", { fill: "#ffcf4a", color: "#11211d", stroke: "#11211d", size: 16 })}
${text(392, 124, "INDIVIDUAL RUN CARD", { size: 25, weight: 900, fill: "#116b63", family: mono })}
${text(390, 188, "Mina Park", { size: 70, weight: 950, fill: "#11211d" })}
${text(394, 230, "trust over tokens | proof passport", { size: 26, weight: 700, fill: "#515b55" })}
${metric(392, 278, "TODAY", "318k", "captured tokens", { width: 218, accent: "#116b63", fill: "#fffdf5" })}
${metric(632, 278, "CALLS", "54", "all routed", { width: 188, accent: "#ff7b54", fill: "#fffdf5" })}
${metric(842, 278, "STREAK", "19d", "visible proof", { width: 188, accent: "#ffcf4a", fill: "#fffdf5" })}
${rect(392, 436, 410, 56, { fill: "#11211d", rx: 12 })}
${text(418, 472, "active path: tee gateway on macOS", { size: 22, weight: 850, fill: "#fff8e8", family: mono })}
${rect(830, 430, 178, 72, { fill: "#fffdf5", stroke: "#11211d", sw: 2, rx: 12 })}
${text(854, 461, "assurance", { size: 17, weight: 850, fill: "#116b63", family: mono })}
${text(854, 489, "attested", { size: 25, weight: 950, fill: "#11211d", family: mono })}
${miniQr(1006, 88, 104, "#11211d", "#fffdf5")}
${text(1008, 226, "verify", { size: 17, weight: 850, fill: "#11211d", family: mono })}
${line(388, 532, 1110, 532, { stroke: "#11211d", sw: 2 })}
${text(394, 560, "device key 5f1c...92a0 | latest event: local proxy usage receipt 12s ago", { size: 18, weight: 700, fill: "#515b55", family: mono })}
`;
  return svgShell("Individual Passport Runcard", "A personal proof card for an individual participant with device identity and daily stats.", body);
}

function individualSignal() {
  const body = `
${rect(0, 0, W, H, { fill: "#06201c" })}
${rect(44, 42, 1112, 546, { fill: "#092b25", stroke: "#d8fff6", sw: 2, rx: 24 })}
${text(76, 104, "LIVE RUNCARD", { size: 26, weight: 900, fill: "#59ffd6", family: mono })}
${text(76, 174, "Nova Chen", { size: 74, weight: 950, fill: "#f7fff4" })}
${pill(756, 84, 128, "LIVE", { fill: "#59ffd6", color: "#06201c", size: 18 })}
${pill(902, 84, 170, "attested", { fill: "#ffd166", color: "#06201c", size: 18 })}
${waveform(78, 252)}
${rect(80, 416, 258, 92, { fill: "#0d3932", stroke: "#23685c", sw: 2, rx: 12 })}
${text(104, 453, "active path", { size: 18, weight: 850, fill: "#59ffd6", family: mono })}
${text(104, 486, "local proxy", { size: 30, weight: 950, fill: "#f7fff4", family: mono })}
${rect(368, 416, 258, 92, { fill: "#0d3932", stroke: "#23685c", sw: 2, rx: 12 })}
${text(392, 453, "latest event", { size: 18, weight: 850, fill: "#59ffd6", family: mono })}
${text(392, 486, "11s ago", { size: 30, weight: 950, fill: "#f7fff4", family: mono })}
${rect(656, 416, 258, 92, { fill: "#0d3932", stroke: "#23685c", sw: 2, rx: 12 })}
${text(680, 453, "today", { size: 18, weight: 850, fill: "#59ffd6", family: mono })}
${text(680, 486, "92 calls", { size: 30, weight: 950, fill: "#f7fff4", family: mono })}
${miniQr(960, 398, 112, "#06201c", "#d8fff6")}
${text(80, 560, "capture health: browser connected | gateway ok | workbench idle | 0 actionable failures", { size: 19, weight: 800, fill: "#a8d9ce", family: mono })}
`;
  return svgShell("Individual Signal Runcard", "A live individual card showing current capture path and latest event.", body);
}

function waveform(x, y) {
  const points = [
    [0, 54], [48, 18], [96, 82], [144, 33], [192, 99], [240, 26], [288, 72], [336, 42],
    [384, 118], [432, 34], [480, 65], [528, 23], [576, 90], [624, 50], [672, 75],
    [720, 28], [768, 102], [816, 56], [864, 44], [912, 82],
  ];
  const d = points.map(([px, py], i) => `${i === 0 ? "M" : "L"}${x + px} ${y + py}`).join(" ");
  return `<g>
  ${line(x, y + 62, x + 912, y + 62, { stroke: "#23685c", sw: 2, dash: "10 12" })}
  <path d="${d}" fill="none" stroke="#59ffd6" stroke-width="8" stroke-linejoin="round"/>
  <path d="${d}" fill="none" stroke="#ffd166" stroke-width="2" stroke-linejoin="round"/>
  ${text(x, y + 148, "signal: prompts hidden | usage receipts public | model names public", { size: 21, weight: 800, fill: "#d8fff6", family: mono })}
</g>`;
}

function individualTokenstream() {
  const body = `
${rect(0, 0, W, H, { fill: "#fafafa" })}
${rect(46, 44, 1108, 542, { fill: "#ffffff", stroke: "#151515", sw: 3, rx: 20 })}
${text(80, 84, "INDIVIDUAL TOKENSTREAM", { size: 26, weight: 950, fill: "#1d4ed8", family: mono })}
${text(78, 196, "41.2M", { size: 104, weight: 950, fill: "#151515" })}
${text(86, 238, "tokens this month", { size: 31, weight: 800, fill: "#4a4a4a" })}
${text(80, 304, "Trust over tokens. Public receipts.", { size: 28, weight: 750, fill: "#151515" })}
${text(80, 336, "Private prompts.", { size: 28, weight: 750, fill: "#151515" })}
${stackedModelBars(86, 372, [
  { label: "openai", value: 0.49, caption: "49%", color: "#2dd4bf", track: "#ececec", fg: "#151515" },
  { label: "anth", value: 0.27, caption: "27%", color: "#f59e0b", track: "#ececec", fg: "#151515" },
  { label: "local", value: 0.24, caption: "24%", color: "#ef4444", track: "#ececec", fg: "#151515" },
], 360)}
${rect(688, 112, 338, 132, { fill: "#eff6ff", stroke: "#151515", sw: 2, rx: 16 })}
${text(718, 158, "@max-context", { size: 34, weight: 950, fill: "#151515", family: mono })}
${text(720, 198, "visibility: public aggregate", { size: 21, weight: 750, fill: "#4a4a4a", family: mono })}
${rect(688, 282, 338, 132, { fill: "#fff7ed", stroke: "#151515", sw: 2, rx: 16 })}
${text(718, 326, "assurance", { size: 20, weight: 850, fill: "#9a3412", family: mono })}
${text(718, 374, "managed + attested", { size: 29, weight: 950, fill: "#151515", family: mono })}
${miniQr(895, 444, 94, "#151515", "#fff")}
${pill(688, 462, 174, "daily card", { fill: "#151515", color: "#fff", size: 17 })}
${text(80, 552, "receipt API: /@max-context/card.json | token source: provider + local estimates", { size: 18, weight: 750, fill: "#4a4a4a", family: mono })}
`;
  return svgShell("Individual Tokenstream Runcard", "A public individual card focused on token volume and model mix.", body);
}

function individualPocket() {
  const body = `
${rect(0, 0, W, H, { fill: "#ffd84d" })}
${rect(64, 58, 1072, 514, { fill: "#111111", stroke: "#111111", sw: 2, rx: 24 })}
${rect(92, 86, 284, 458, { fill: "#ffd84d", rx: 16 })}
${text(234, 238, "RC", { size: 118, weight: 950, fill: "#111111", family: mono, anchor: "middle" })}
${text(234, 302, "individual", { size: 28, weight: 950, fill: "#111111", family: mono, anchor: "middle" })}
${miniQr(180, 350, 108, "#111111", "#ffd84d")}
${text(420, 137, "POCKET RUNCARD", { size: 27, weight: 950, fill: "#80ffea", family: mono })}
${text(418, 230, "Ari Vale", { size: 80, weight: 950, fill: "#fffaf0" })}
${text(424, 280, "proof over claims", { size: 31, weight: 750, fill: "#ffd84d" })}
${rect(424, 338, 184, 70, { fill: "#80ffea", rx: 12 })}
${text(448, 382, "TEE", { size: 35, weight: 950, fill: "#111111", family: mono })}
${rect(636, 338, 184, 70, { fill: "#fffaf0", rx: 12 })}
${text(660, 382, "18k", { size: 35, weight: 950, fill: "#111111", family: mono })}
${rect(848, 338, 184, 70, { fill: "#ff7b54", rx: 12 })}
${text(872, 382, "12d", { size: 35, weight: 950, fill: "#111111", family: mono })}
${text(426, 464, "active: strict workbench | latest: 27s ago | public aggregate", { size: 22, weight: 850, fill: "#fffaf0", family: mono })}
${text(426, 512, "runcard.dev/@arivale", { size: 26, weight: 950, fill: "#80ffea", family: mono })}
`;
  return svgShell("Individual Pocket Runcard", "A small high-contrast individual card for social profiles.", body);
}

function renderIndex(cards) {
  const teamCards = cards.filter((card) => card.type === "team");
  const individualCards = cards.filter((card) => card.type === "individual");
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Runcard Card Lab</title>
  <style>
    :root {
      color-scheme: light;
      --ink: #151515;
      --muted: #5d625b;
      --paper: #f7f4ea;
      --line: #252525;
      --aqua: #18b7a6;
      --gold: #f4c430;
      --coral: #ff745f;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      min-width: 320px;
      background: var(--paper);
      color: var(--ink);
      font-family: ${font};
    }
    header, main, footer {
      width: min(1180px, calc(100vw - 32px));
      margin: 0 auto;
    }
    header {
      padding: 34px 0 22px;
      border-bottom: 2px solid var(--line);
    }
    .eyebrow {
      display: inline-flex;
      align-items: center;
      min-height: 34px;
      padding: 0 14px;
      color: #fff;
      background: var(--ink);
      border-radius: 999px;
      font-family: ${mono};
      font-size: 15px;
      font-weight: 800;
    }
    h1 {
      max-width: 820px;
      margin: 18px 0 10px;
      font-size: clamp(44px, 7vw, 84px);
      line-height: 0.95;
    }
    p {
      max-width: 760px;
      color: var(--muted);
      font-size: 19px;
      line-height: 1.5;
    }
    section {
      padding: 40px 0;
      border-bottom: 2px solid var(--line);
    }
    h2 {
      margin: 0 0 18px;
      font-size: 34px;
      line-height: 1.1;
    }
    .grid {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 22px;
    }
    article {
      min-width: 0;
      padding: 14px;
      background: #fffdf6;
      border: 2px solid var(--line);
      border-radius: 12px;
    }
    article img {
      display: block;
      width: 100%;
      height: auto;
      border: 1px solid rgba(0, 0, 0, 0.18);
      border-radius: 8px;
      background: #fff;
    }
    article h3 {
      display: flex;
      justify-content: space-between;
      gap: 12px;
      margin: 12px 0 6px;
      font-size: 22px;
      line-height: 1.1;
    }
    article h3 span {
      flex: 0 0 auto;
      color: var(--muted);
      font-family: ${mono};
      font-size: 14px;
      font-weight: 800;
    }
    article p {
      margin: 0 0 8px;
      font-size: 15px;
    }
    article a {
      color: var(--ink);
      font-family: ${mono};
      font-weight: 800;
    }
    .notes {
      display: grid;
      grid-template-columns: repeat(3, minmax(0, 1fr));
      gap: 16px;
      margin-top: 18px;
    }
    .note {
      padding: 16px;
      background: #fffdf6;
      border: 2px solid var(--line);
      border-radius: 12px;
    }
    .note strong {
      display: block;
      margin-bottom: 8px;
      font-family: ${mono};
      font-size: 17px;
    }
    footer {
      padding: 30px 0 44px;
      color: var(--muted);
      font-family: ${mono};
      font-weight: 700;
    }
    @media (max-width: 860px) {
      .grid, .notes {
        grid-template-columns: 1fr;
      }
      h1 {
        font-size: 46px;
      }
    }
  </style>
</head>
<body>
  <header>
    <span class="eyebrow">Runcard card lab</span>
    <h1>Team cards for competitions. Individual cards for proof culture.</h1>
    <p>Eight SVG concepts for the LLM Attested surface. The cards intentionally share a narrow data vocabulary so the renderer can stay small while the skins stay expressive.</p>
  </header>
  <main>
    <section>
      <h2>Team Cards</h2>
      <div class="grid">
        ${teamCards.map(renderCardArticle).join("\n        ")}
      </div>
    </section>
    <section>
      <h2>Individual Cards</h2>
      <div class="grid">
        ${individualCards.map(renderCardArticle).join("\n        ")}
      </div>
    </section>
    <section>
      <h2>Design Rules</h2>
      <p>The useful split is not just visual. Team cards aggregate fairness signals for judging. Individual cards make daily attestation and usage feel worth showing off.</p>
      <div class="notes">
        <div class="note"><strong>Small vocabulary</strong>Every skin uses subject, capture path, assurance, enforcement, token source, latest event, and receipt URL.</div>
        <div class="note"><strong>Honest labels</strong>Cards say provider-reported, estimated, participant-controlled, managed, or attested instead of collapsing everything into verified.</div>
        <div class="note"><strong>Embeddable first</strong>Each concept is 1200x630 SVG, readable when scaled down, and suitable for social cards, dashboards, and README embeds.</div>
      </div>
    </section>
  </main>
  <footer>Generated from docs/card-lab/scripts/generate-card-concepts.mjs</footer>
</body>
</html>
`;
}

function renderCardArticle(card) {
  return `<article>
          <img src="cards/${card.slug}.svg" alt="${escapeXml(card.title)} card preview">
          <h3>${escapeXml(card.title)} <span>${escapeXml(card.type)}</span></h3>
          <p>${escapeXml(card.concept)}</p>
          <a href="cards/${card.slug}.svg">open svg</a>
        </article>`;
}
