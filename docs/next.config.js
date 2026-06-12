import nextra from 'nextra'
import remarkPlayground from './lib/remark-playground.mjs'

const withNextra = nextra({
    // Nextra 4 (App Router) moved theme/themeConfig to app/layout.jsx; remark wraps python blocks into runnable <Playground>.
    mdxOptions: { remarkPlugins: [remarkPlayground] },
})

export default withNextra({
    // Static export for Cloudflare Pages Direct Upload (-> out/).
    output: 'export',
    images: { unoptimized: true },
    // Dev convenience only: redirect the root to the first page. `output: 'export'` ignores this (Cloudflare serves the redirect via public/_redirects in prod).
    async redirects() {
        return [
        {
            source: '/',
            destination: '/getting-started/introduction',
            permanent: false,
        },
        ]
    },
})
