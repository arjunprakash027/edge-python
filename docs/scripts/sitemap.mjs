/*
 * Post-build: derive sitemap.xml from the exported pages in out/.
 * Runs automatically after `next build` (npm `postbuild` hook).
 */

import { execFileSync } from 'node:child_process'
import { existsSync, readdirSync, statSync, writeFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const BASE = 'https://edgepython.com'
const OUT = fileURLToPath(new URL('../out', import.meta.url))
const CONTENT = fileURLToPath(new URL('../content', import.meta.url))

function walk(dir) {
  const files = []
  for (const entry of readdirSync(dir)) {
    const p = join(dir, entry)
    if (statSync(p).isDirectory()) files.push(...walk(p))
    else if (entry.endsWith('.html') && entry !== '404.html') files.push(p)
  }
  return files
}

// W3C date (YYYY-MM-DD) of the route's source .md/.mdx: last commit, else its mtime, else today. Build mtimes reset on CI checkout, so git is the only stable signal.
function lastmod(url) {
  const src = [join(CONTENT, url + '.md'), join(CONTENT, url + '.mdx')].find(existsSync)
  if (!src) return new Date().toISOString().slice(0, 10)
  try {
    const day = execFileSync('git', ['log', '-1', '--format=%cs', '--', src], { encoding: 'utf8' }).trim()
    if (day) return day
  } catch {
    // git missing or file untracked: fall through to mtime.
  }
  return statSync(src).mtime.toISOString().slice(0, 10)
}

const urls = walk(OUT)
  .map((f) => '/' + relative(OUT, f).replaceAll('\\', '/').replace(/\.html$/, ''))
  .map((u) => (u === '/index' ? '/' : u))
  .sort()

const xml =
  '<?xml version="1.0" encoding="UTF-8"?>\n' +
  '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n' +
  urls.map((u) => `  <url><loc>${BASE}${u}</loc><lastmod>${lastmod(u)}</lastmod></url>`).join('\n') +
  '\n</urlset>\n'

writeFileSync(join(OUT, 'sitemap.xml'), xml)
console.log(`sitemap.xml: ${urls.length} urls`)
