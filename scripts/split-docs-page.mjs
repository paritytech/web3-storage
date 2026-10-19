// Moves the parts of an archify page that are the same on every page into
// shared files, and replaces them in the page with <script src> tags.
// build-docs-site.sh runs it on each delivered page.
//
// Usage: node scripts/split-docs-page.mjs <page.html> <assets-dir>
//
// Writes into <assets-dir>:
//   theme.js  sets the theme before first paint (from the inline <head> script)
//   style.js  adds archify's CSS as a <style> element
//   i18n.js   adds archify's i18n data as the #archify-i18n-data element
//   viewer.js archify's viewer script (from the inline script at the end of <body>)
//
// The CSS is added by a script, not linked as a .css file: archify's SVG and
// PNG export reads document.styleSheets[].cssRules, which browsers block for
// a linked stylesheet on a file:// page.
//
// Fails when the page does not have exactly one of each part, or when a file
// in <assets-dir> already exists with other content (two pages built by
// different archify output).

import { createHash } from 'node:crypto';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const [pagePath, assetsDir] = process.argv.slice(2);
if (!pagePath || !assetsDir) {
  console.error('usage: node scripts/split-docs-page.mjs <page.html> <assets-dir>');
  process.exit(1);
}

let html = readFileSync(pagePath, 'utf8');

function takeOne(name, re) {
  const matches = [...html.matchAll(re)];
  if (matches.length !== 1) {
    console.error(`${pagePath}: expected 1 ${name}, found ${matches.length}`);
    process.exit(1);
  }
  return matches[0];
}

function writeAsset(file, content) {
  const path = join(assetsDir, file);
  if (existsSync(path) && readFileSync(path, 'utf8') !== content) {
    console.error(`${pagePath}: ${file} differs from the copy written for another page`);
    process.exit(1);
  }
  writeFileSync(path, content);
  const version = createHash('sha256').update(content).digest('hex').slice(0, 12);
  return `<script src="assets/archify/${file}?v=${version}"></script>`;
}

const theme = takeOne('inline <head> script', /<script>([\s\S]*?)<\/script>(?=[\s\S]*<\/head>)/g);
const style = takeOne('<style> element', /<style>([\s\S]*?)<\/style>/g);
const i18n = takeOne(
  '#archify-i18n-data element',
  /<script id="archify-i18n-data" type="application\/json">([\s\S]*?)<\/script>/g,
);
const viewer = takeOne('inline script after </head>', /(?<=<\/head>[\s\S]*)<script>([\s\S]*?)<\/script>/g);

const css = style[1];
if (css.includes('`') || css.includes('${') || css.endsWith('\\')) {
  console.error(`${pagePath}: the CSS cannot be put in a template literal`);
  process.exit(1);
}

const insertAfterScript = (build) => `(function () {
  var el = ${build};
  document.currentScript.after(el);
})();
`;

const replacements = [
  [theme[0], writeAsset('theme.js', theme[1].trim() + '\n')],
  [
    style[0],
    writeAsset(
      'style.js',
      insertAfterScript(`document.createElement('style');
  el.textContent = String.raw\`${css}\``),
    ),
  ],
  [
    i18n[0],
    writeAsset(
      'i18n.js',
      insertAfterScript(`document.createElement('script');
  el.type = 'application/json';
  el.id = 'archify-i18n-data';
  el.textContent = ${JSON.stringify(i18n[1])}`),
    ),
  ],
  [viewer[0], writeAsset('viewer.js', viewer[1].trim() + '\n')],
];

for (const [from, to] of replacements) html = html.replace(from, () => to);
writeFileSync(pagePath, html);
