import fs from "node:fs/promises";
import path from "node:path";
import sharp from "sharp";

const root = process.cwd();
const outputDir = path.join(root, "design-assets", "ttv-icon-concepts");
const concepts = [
  { file: "ttv-prism-play", label: "01  Prism Play", note: "Brand continuity" },
  { file: "ttv-cinema-frame", label: "02  Cinema Frame", note: "Desktop cinema" },
  { file: "ttv-stream-orbit", label: "03  Stream Orbit", note: "Multi-source streaming" },
  { file: "ttv-light-curtain", label: "04  Light Curtain", note: "Picture enhancement" },
];

await fs.mkdir(outputDir, { recursive: true });

for (const concept of concepts) {
  await sharp(path.join(outputDir, `${concept.file}.svg`))
    .resize(1024, 1024)
    .png({ compressionLevel: 9, palette: false })
    .toFile(path.join(outputDir, `${concept.file}.png`));
}

const width = 1600;
const height = 560;
const tileSize = 300;
const gap = 56;
const left = 58;
const top = 104;

const background = Buffer.from(`
  <svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}">
    <defs>
      <radialGradient id="bg" cx="0" cy="0" r="1" gradientUnits="userSpaceOnUse" gradientTransform="translate(320 60) rotate(25) scale(1500 720)">
        <stop stop-color="#182338"/>
        <stop offset="0.46" stop-color="#0B111C"/>
        <stop offset="1" stop-color="#06080D"/>
      </radialGradient>
      <pattern id="grid" width="48" height="48" patternUnits="userSpaceOnUse">
        <path d="M48 0H0V48" fill="none" stroke="#FFFFFF" stroke-opacity="0.025"/>
      </pattern>
    </defs>
    <rect width="1600" height="560" rx="28" fill="url(#bg)"/>
    <rect width="1600" height="560" rx="28" fill="url(#grid)"/>
    <text x="58" y="58" fill="#F5F7FC" font-family="Inter, Segoe UI, sans-serif" font-size="26" font-weight="700">TTV Box / application icon concepts</text>
    <text x="1542" y="56" text-anchor="end" fill="#8792A7" font-family="Inter, Segoe UI, sans-serif" font-size="15">cinema glass / cyan / indigo / violet</text>
    ${concepts.map((concept, index) => {
      const x = left + index * (tileSize + gap);
      return `
        <rect x="${x - 12}" y="${top - 12}" width="${tileSize + 24}" height="${tileSize + 24}" rx="72" fill="#FFFFFF" fill-opacity="0.025" stroke="#FFFFFF" stroke-opacity="0.07"/>
        <text x="${x}" y="${top + tileSize + 48}" fill="#EFF3FB" font-family="Inter, Segoe UI, sans-serif" font-size="19" font-weight="700">${concept.label}</text>
        <text x="${x}" y="${top + tileSize + 76}" fill="#7F8BA0" font-family="Inter, Segoe UI, sans-serif" font-size="15">${concept.note}</text>
      `;
    }).join("")}
  </svg>
`);

const composites = await Promise.all(concepts.map(async (concept, index) => ({
  input: await sharp(path.join(outputDir, `${concept.file}.png`)).resize(tileSize, tileSize).toBuffer(),
  left: left + index * (tileSize + gap),
  top,
})));

await sharp(background)
  .composite(composites)
  .png({ compressionLevel: 9 })
  .toFile(path.join(outputDir, "ttv-icon-concepts-preview.png"));

const sizes = [64, 48, 32];
const checkWidth = 820;
const checkHeight = 330;
const checkBackground = Buffer.from(`
  <svg xmlns="http://www.w3.org/2000/svg" width="${checkWidth}" height="${checkHeight}" viewBox="0 0 ${checkWidth} ${checkHeight}">
    <defs>
      <pattern id="checker" width="16" height="16" patternUnits="userSpaceOnUse">
        <rect width="16" height="16" fill="#111827"/>
        <rect width="8" height="8" fill="#1B2638"/>
        <rect x="8" y="8" width="8" height="8" fill="#1B2638"/>
      </pattern>
    </defs>
    <rect width="820" height="330" rx="22" fill="#080B11"/>
    <text x="34" y="46" fill="#F4F7FC" font-family="Inter, Segoe UI, sans-serif" font-size="21" font-weight="700">Small-size legibility check</text>
    ${sizes.map((size, row) => {
      const y = 86 + row * 76;
      return `
        <text x="34" y="${y + 38}" fill="#8792A7" font-family="Inter, Segoe UI, sans-serif" font-size="15" font-weight="700">${size}px</text>
        <rect x="96" y="${y}" width="690" height="${size}" rx="10" fill="url(#checker)"/>
      `;
    }).join("")}
  </svg>
`);

const sizeCheckComposites = [];
for (let row = 0; row < sizes.length; row += 1) {
  const size = sizes[row];
  for (let column = 0; column < concepts.length; column += 1) {
    sizeCheckComposites.push({
      input: await sharp(path.join(outputDir, `${concepts[column].file}.png`)).resize(size, size).toBuffer(),
      left: 116 + column * 160,
      top: 86 + row * 76,
    });
  }
}

await sharp(checkBackground)
  .composite(sizeCheckComposites)
  .png({ compressionLevel: 9 })
  .toFile(path.join(outputDir, "ttv-icon-size-check.png"));

console.log(`Rendered ${concepts.length} icon concepts to ${outputDir}`);
