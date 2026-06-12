// Client-side Shiki highlighter (github-light/dark, matches Nextra). createHighlight() returns sync highlight(text) → token-span HTML for CodeJar; re-renders on Shiki ready + .dark toggle.

const HTML_ESC = { '&': '&amp;', '<': '&lt;', '>': '&gt;' }
const escapeHtml = (s) => s.replace(/[&<>]/g, (c) => HTML_ESC[c])

let hlPromise = null
function getHighlighter() {
    if (!hlPromise) {
        hlPromise = import('shiki').then(({ createHighlighter }) =>
            createHighlighter({ themes: ['github-light', 'github-dark'], langs: ['python'] }),
        )
    }
    return hlPromise
}

const isDark = () =>
    typeof document !== 'undefined' && document.documentElement.classList.contains('dark')

export function createHighlight(rerender) {
    let hl = null
    getHighlighter().then((h) => {
        hl = h
        rerender?.()
    })

    // Re-highlight when the docs theme toggles (next-themes flips `class` on <html>).
    if (typeof MutationObserver !== 'undefined') {
        new MutationObserver(() => rerender?.()).observe(document.documentElement, {
            attributes: true,
            attributeFilter: ['class'],
        })
    }

    return (code) => {
        if (!hl) return escapeHtml(code) // plain text until Shiki finishes loading
        const html = hl.codeToHtml(code, { lang: 'python', theme: isDark() ? 'github-dark' : 'github-light' })
        // CodeJar sets the return value as the editor's innerHTML, so drop Shiki's <pre><code> wrapper.
        let inner = html.replace(/^<pre[^>]*>\s*<code[^>]*>/, '').replace(/<\/code>\s*<\/pre>\s*$/, '')
        // For code ending in \n, Shiki adds a trailing empty `<span class="line"></span>` the caret can't reach. Drop it so the HTML ends with the \n text node (Prism-style) and stays editable.
        if (code.endsWith('\n')) inner = inner.replace(/<span class="line"><\/span>$/, '')
        return inner
    }
}
