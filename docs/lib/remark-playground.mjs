// Converts `python` + `text Output` block pairs into runnable <Playground> components (base64 `code`/`output` attrs for CodeJar). Bare python blocks and standalone Output blocks are left untouched.

const isPython = (n) => n?.type === 'code' && n.lang === 'python'
const isOutput = (n) => n?.type === 'code' && /(^|\s)output\b/i.test(n.meta || '')
const b64 = (s) => Buffer.from(s, 'utf8').toString('base64')

function transform(parent) {
    const kids = parent.children
    if (!Array.isArray(kids)) return
    const out = []
    for (let i = 0; i < kids.length; i++) {
        const node = kids[i]
        if (node.children) transform(node) // recurse into containers first
        const after = kids[i + 1]
        // Only a python block paired with an Output block is runnable; everything else stays static.
        if (!isPython(node) || !isOutput(after)) {
            out.push(node)
            continue
        }
        i++ // consume the Output block
        out.push({
            type: 'mdxJsxFlowElement',
            name: 'Playground',
            attributes: [
                { type: 'mdxJsxAttribute', name: 'code', value: b64(node.value) },
                { type: 'mdxJsxAttribute', name: 'output', value: b64(after.value) },
            ],
            children: [],
        })
    }
    parent.children = out
}

export default function remarkPlayground() {
    return (tree) => transform(tree)
}
