// CodeJar editor (Shiki + auto-pair/Tab/Ctrl+Enter) → Nextra Pre/Code (plain). `code`/`output`: snippet source & default terminal text (base64, remark plugin).

import { useCallback, useEffect, useRef, useState } from 'react'
import { Pre, Code, Button } from 'nextra/components'
import { run } from './runtime'

function fromB64(b64) {
    if (!b64) return ''
    try {
        return new TextDecoder().decode(Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)))
    } catch {
        return ''
    }
}

const PlayIcon = () => (
    <svg viewBox="0 0 24 24" fill="currentColor" height="11.5" aria-hidden="true" focusable="false">
        <path d="M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z" />
    </svg>
)

export function Playground({ code, output }) {
    const edRef = useRef(null)
    const editorRef = useRef(null)
    const defaultText = fromB64(output).replace(/\n$/, '')
    const defaultCode = fromB64(code).replace(/\n$/, '')
    const [result, setResult] = useState(null) // null = showing default; else { text, error, ms }
    const [running, setRunning] = useState(false)

    const runCode = useCallback(async (src) => {
        if (running) return
        setRunning(true)
        // Byte-stream contract: each chunk is raw stdout (print already includes its own `end`); concatenate, don't join.
        let buf = ''
        // Don't clear the terminal up front (collapse + flicker); keep old text until the first chunk replaces it.
        try {
            const res = await run(src, (chunk) => {
                buf += chunk
                setResult({ text: buf, error: '', ms: 0 })
            })
            setResult({ text: buf, error: res.error, ms: res.ms })
        } catch (e) {
            setResult({ text: buf, error: String(e?.message ?? e), ms: 0 })
        } finally {
            setRunning(false)
        }
    }, [running])

    // Mount the CodeJar editor + Shiki highlighter client-side (lazy: keeps codejar/shiki out of SSR).
    useEffect(() => {
        let editor
        let disposed = false
        Promise.all([import('./editor.js'), import('./shiki.js')]).then(([{ createEditor }, { createHighlight }]) => {
            if (disposed || !edRef.current) return
            const highlight = createHighlight(() => editor && editor.setCode(editor.getCode()))
            editor = createEditor({
                ed: edRef.current,
                ln: null,
                defaultCode,
                onRun: (src) => runCode(src),
                highlight,
            })
            editorRef.current = editor
        })
        return () => { disposed = true }
    }, []) // eslint-disable-line react-hooks/exhaustive-deps

    const onRunClick = () => runCode(editorRef.current?.getCode() ?? defaultCode)

    const fmt = (ms) => (ms < 1000 ? `${ms.toFixed(0)}ms` : `${(ms / 1000).toFixed(2)}s`)
    const liveText = result ? result.text.replace(/\n$/, '') : ''
    const differs = result && !result.error && liveText !== defaultText
    const termBody = result ? [liveText, result.error].filter(Boolean).join('\n') : defaultText
    const header = running
        ? 'Output · running…'
        : !result
            ? 'Output · expected'
            : result.error
                ? 'Output · failed'
                : differs
                    ? 'Output · differs'
                    : `Output · ${fmt(result.ms)}`

    return (
        <div className="ep-pg my-5">
            {/* Input: CodeJar editor, framed like a Nextra code block. */}
            <div className="ep-editor overflow-hidden rounded-md border border-gray-300 bg-white text-[.9em] dark:border-neutral-700 dark:bg-black">
                <div ref={edRef} className="ep-ed py-2 font-mono" aria-label="Python source editor"/>
            </div>

            {/* Output: thin header (status + Run) over Nextra's Pre/Code body (plain text, no highlight). */}
            <div className="ep-output mt-3" aria-live="polite">
                <div className="flex items-center justify-between gap-2 rounded-t-md border border-b-0 border-gray-300 bg-gray-100 pl-3 pr-1 py-1 dark:border-neutral-700 dark:bg-neutral-900">
                    <span className="font-mono text-xs text-gray-700 dark:text-gray-200">{header}</span>
                    <Button variant="outline" onClick={onRunClick} className="text-xs flex items-center gap-1.5" disabled={running} title="Run code" aria-label="Run code"><PlayIcon/>Run</Button>
                </div>
                <Pre>
                    <Code>{termBody}</Code>
                </Pre>
            </div>
        </div>
    )
}
