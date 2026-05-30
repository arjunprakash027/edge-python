/*
 * Post-build: derive sitemap.xml from the exported pages in out/.
 * Runs automatically after `next build` (npm `postbuild` hook).
 */

import { readdirSync, statSync, writeFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const BASE = 'https://edgepython.com'
const OUT = fileURLToPath(new URL('../out', import.meta.url))

function walk(dir) {
  const files = []
  for (const entry of readdirSync(dir)) {
    const p = join(dir, entry)
    if (statSync(p).isDirectory()) files.push(...walk(p))
    else if (entry.endsWith('.html') && entry !== '404.html') files.push(p)
  }
  return files
}

const urls = walk(OUT)
  .map((f) => '/' + relative(OUT, f).replaceAll('\\', '/').replace(/\.html$/, ''))
  .map((u) => (u === '/index' ? '/' : u))
  .sort()

const xml = '<?xml version="1.0" encoding="UTF-8"?>\n' + '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n' + urls.map((u) => `  <url><loc>${BASE}${u}</loc></url>`).join('\n') + '\n</urlset>\n'

writeFileSync(join(OUT, 'sitemap.xml'), xml)
console.log(`sitemap.xml: ${urls.length} urls`)
