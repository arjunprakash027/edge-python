import { useConfig } from 'nextra-theme-docs'
import { Playground } from './components/Playground'

const DEFAULT_DESCRIPTION = 'Edge Python — a sandboxed Python scripting language compiled to WebAssembly for the edge.'

// `head` as a function: emits per-page <title>, description, and favicon from frontmatter.
function Head() {
    const { title, frontMatter } = useConfig()
    return (
        <>
        <title>{title ? `${title} – Edge Python` : 'Edge Python'}</title>
        <meta name="description" content={frontMatter.description || DEFAULT_DESCRIPTION} />
        <link rel="icon" type="image/svg+xml" href="/static/favicon.svg" />
        </>
    )
}

export default {
    head: Head,
    components: { Playground },
    logo: <span style={{ fontWeight: 600 }}>Edge Python</span>,
    project: {
        link: 'https://github.com/dylan-sutton-chavez/edge-python',
    },
    docsRepositoryBase: 'https://github.com/dylan-sutton-chavez/edge-python/tree/main/docs',
    color: {
        hue: { dark: 204, light: 212 },
        saturation: 100,
        lightness: { dark: 55, light: 45 },
    },
    footer: {
        content: 'Edge Python',
    },
}
