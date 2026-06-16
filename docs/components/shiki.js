// Client-side Shiki highlighter (github-light/dark, matches Nextra). createHighlight() returns sync highlight(text) → token-span HTML for CodeJar; re-renders on Shiki ready + .dark toggle.

// Shared HTML-escape (editor/playground import this); `quotes` also encodes `"` for attribute-safe output.
const HTML_ESC = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }
export const escapeHtml = (s, quotes = false) => s.replace(quotes ? /[&<>"]/g : /[&<>]/g, (c) => HTML_ESC[c])

let hlPromise = null
function getHighlighter() {
    if (!hlPromise) {
        // Core + JS RegExp engine instead of the `shiki` bundle: skips the multi-MB Oniguruma WASM engine and every other grammar/theme, shipping only python + the two themes we render. This is the dominant payload of the first Run.
        // Import via shiki's public re-exports, not @shikijs/* transitive deps that break under strict installs.
        hlPromise = Promise.all([
            import('shiki/core'),
            import('shiki/engine/javascript'),
        ]).then(([{ createHighlighterCore }, { createJavaScriptRegexEngine }]) =>
            createHighlighterCore({
                themes: [import('shiki/themes/github-light.mjs'), import('shiki/themes/github-dark.mjs')],
                langs: [import('shiki/langs/python.mjs')],
                // forgiving: skip untranslatable grammar patterns instead of throwing and killing the highlighter.
                engine: createJavaScriptRegexEngine({ forgiving: true }),
            }),
        )
    }
    return hlPromise
}

const isDark = () =>
    typeof document !== 'undefined' && document.documentElement.classList.contains('dark')

export function createHighlight(rerender) {
    let hl = null
    let disposed = false
    getHighlighter().then((h) => {
        if (disposed) return
        hl = h
        rerender?.()
    })

    // Re-highlight when the docs theme toggles (next-themes flips `class` on <html>).
    let observer = null
    if (typeof MutationObserver !== 'undefined') {
        observer = new MutationObserver(() => rerender?.())
        observer.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] })
    }

    const highlight = (code) => {
        if (!hl) return escapeHtml(code) // plain text until Shiki finishes loading
        const html = hl.codeToHtml(code, { lang: 'python', theme: isDark() ? 'github-dark' : 'github-light' })
        // CodeJar sets the return value as the editor's innerHTML, so drop Shiki's <pre><code> wrapper.
        let inner = html.replace(/^<pre[^>]*>\s*<code[^>]*>/, '').replace(/<\/code>\s*<\/pre>\s*$/, '')
        // For code ending in \n, Shiki adds a trailing empty `<span class="line"></span>` the caret can't reach. Drop it so the HTML ends with the \n text node (Prism-style) and stays editable.
        if (code.endsWith('\n')) inner = inner.replace(/<span class="line"><\/span>$/, '')
        return inner
    }
    // Disconnect the <html> observer and ignore a late getHighlighter() resolve, so the editor closure can be GC'd on unmount.
    highlight.dispose = () => { disposed = true; observer?.disconnect() }
    return highlight
}
