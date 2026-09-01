#!/usr/bin/env node

import fs from "node:fs/promises";
import JSZip from "jszip";

const [file] = process.argv.slice(2);
if (!file) {
  console.error("Usage: node verify-deck.mjs deck.pptx");
  process.exit(2);
}

const zip = await JSZip.loadAsync(await fs.readFile(file));
const presentation = await zip.file("ppt/presentation.xml")?.async("string");
const size = presentation?.match(/<p:sldSz[^>]*cx="(\d+)"[^>]*cy="(\d+)"/);
if (!size) throw new Error("Presentation slide size is missing");
const width = Number(size[1]);
const height = Number(size[2]);
const tolerance = 12700;
const expectedSlides = 47;

const slideNames = Object.keys(zip.files)
  .filter((name) => /^ppt\/slides\/slide\d+\.xml$/.test(name))
  .sort((left, right) => Number(left.match(/\d+/)?.[0]) - Number(right.match(/\d+/)?.[0]));
const notesNames = Object.keys(zip.files).filter((name) => /^ppt\/notesSlides\/notesSlide\d+\.xml$/.test(name));
const outside = [];

for (const name of slideNames) {
  const xml = await zip.file(name).async("string");
  const transforms = xml.matchAll(/<a:xfrm[^>]*>\s*<a:off[^>]*x="(-?\d+)"[^>]*y="(-?\d+)"[^>]*\/>\s*<a:ext[^>]*cx="(\d+)"[^>]*cy="(\d+)"[^>]*\/>/g);
  let index = 0;
  for (const match of transforms) {
    index += 1;
    const [x, y, cx, cy] = match.slice(1).map(Number);
    if (x < -tolerance || y < -tolerance || x + cx > width + tolerance || y + cy > height + tolerance) {
      outside.push({ slide: name, transform: index, x, y, cx, cy });
    }
  }
}

let sourceBlocks = 0;
for (const name of notesNames) {
  const xml = await zip.file(name).async("string");
  if (xml.includes("[Sources]")) sourceBlocks += 1;
}

const report = {
  schema_version: "leash.talk-deck-verification.v1",
  file,
  slides: slideNames.length,
  notes: notesNames.length,
  source_blocks: sourceBlocks,
  out_of_bounds_transforms: outside,
  passed:
    slideNames.length === expectedSlides &&
    notesNames.length === expectedSlides &&
    sourceBlocks >= 44 &&
    outside.length === 0,
};
console.log(JSON.stringify(report, null, 2));
process.exitCode = report.passed ? 0 : 1;
