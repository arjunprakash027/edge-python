import { useMDXComponents as getThemeComponents } from 'nextra-theme-docs'
import { Playground } from './components/Playground'

// Nextra 4 replaces theme.config's `components` map with this root hook.
const themeComponents = getThemeComponents()

export function useMDXComponents(components) {
    return {
        ...themeComponents,
        // Runnable python snippets injected by lib/remark-playground.mjs.
        Playground,
        ...components,
    }
}
