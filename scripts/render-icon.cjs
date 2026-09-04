// Run with Node.js and sharp installed (NODE_PATH may point to a shared install).
const sharp = require('sharp');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
async function main() {
  for (const name of ['portal-icon', 'portal-icon-foreground', 'portal-icon-monochrome']) {
    await sharp(path.join(root, 'assets', name + '.svg')).resize(1024, 1024).png().toFile(path.join(root, 'assets', name + '.png'));
  }
  // Android's 108dp layer has a guaranteed 66dp circular safe area.
  const {data, info} = await sharp(path.join(root, 'assets/portal-icon-foreground.png')).ensureAlpha().raw().toBuffer({resolveWithObject: true});
  let radius = 0;
  for (let y = 0; y < info.height; y++) for (let x = 0; x < info.width; x++) {
    if (data[(y * info.width + x) * 4 + 3] > 0) radius = Math.max(radius, Math.hypot(x + 0.5 - 512, y + 0.5 - 512));
  }
  if (radius > 1024 * 33 / 108) throw new Error('Foreground exceeds Android safe circle');
  console.log(`Foreground radius ${radius.toFixed(1)}px; safe radius ${(1024 * 33 / 108).toFixed(1)}px`);
  if (process.argv[2]) {
    // Model Android's 108dp-to-72dp crop, rather than masking the entire layer.
    const layer = await sharp(path.join(root, 'assets/portal-icon-foreground.png')).resize(324, 324).extract({left:54, top:54, width:216, height:216}).toBuffer();
    const tiles = [];
    for (const [index, radius] of [108, 58, 32].entries()) {
      const mask = Buffer.from(`<svg width="216" height="216"><rect width="216" height="216" rx="${radius}" fill="white"/></svg>`);
      const tile = await sharp({create:{width:216,height:216,channels:4,background:'#191B1C'}}).composite([{input:layer}]).png().toBuffer();
      tiles.push({input:await sharp(tile).composite([{input:mask,blend:'dest-in'}]).png().toBuffer(),left:48+index*264,top:48});
    }
    await sharp({create:{width:840,height:312,channels:4,background:'#E3DED3'}}).composite(tiles).png().toFile(process.argv[2]);
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
