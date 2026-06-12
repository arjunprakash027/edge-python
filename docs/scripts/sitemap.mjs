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

// Shallow clones collapse all per-file dates to HEAD — omit lastmod, don't lie.
let shallow = true
try {
    shallow = execFileSync('git', ['rev-parse', '--is-shallow-repository'], { encoding: 'utf8' }).trim() !== 'false'
} catch {
    // git missing: treat as shallow so we omit lastmod instead of guessing.
}

function walk(dir) {
    const files = []
    for (const entry of readdirSync(dir)) {
        const p = join(dir, entry)
        if (statSync(p).isDirectory()) files.push(...walk(p))
        else if (entry.endsWith('.html') && entry !== '404.html') files.push(p)
    }
    return files
}

// Source's last-commit date (YYYY-MM-DD), or '' — a false lastmod is worse than none.
function lastmod(url) {
    if (shallow) return ''
    const src = [join(CONTENT, url + '.md'), join(CONTENT, url + '.mdx')].find(existsSync)
    if (!src) return ''
    try {
        return execFileSync('git', ['log', '-1', '--format=%cs', '--', src], { encoding: 'utf8' }).trim()
    } catch {
        return ''
    }
}

const urls = walk(OUT)
    .map((f) => '/' + relative(OUT, f).replaceAll('\\', '/').replace(/\.html$/, ''))
    .map((u) => (u === '/index' ? '/' : u))
    .sort()

const xml = '<?xml version="1.0" encoding="UTF-8"?>\n' + '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n' + urls
    .map((u) => {
        const day = lastmod(u)
        return `  <url><loc>${BASE}${u}</loc>${day ? `<lastmod>${day}</lastmod>` : ''}</url>`
    })
    .join('\n') + '\n</urlset>\n'

writeFileSync(join(OUT, 'sitemap.xml'), xml)
console.log(`sitemap.xml: ${urls.length} urls`)
