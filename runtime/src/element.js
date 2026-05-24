/**
 * Define the custom element, which is a web component that allows to import the runtime using an HTML tag.
 * https://developer.mozilla.org/en-US/docs/Web/API/Web_components
 */

import { createWorker } from "./index.js";

export class EdgePythonElement extends HTMLElement { 
    async connectedCallback() {
        const file = this.getAttribute('entry');
        const pkg = this.getAttribute('packages');

        // host -> main-thread modules, imports -> worker .py/.wasm modules
        const mainThreadModules = {};
        let imports;
        if (pkg) {
            const base = new URL(pkg, location.href);
            const manifest = await fetch(base).then(r => r.json());
            for (const url of Object.values(manifest.host ?? {})) {
                const { default: _, ...mods } = await import(new URL(url, base).href);
                Object.assign(mainThreadModules, mods);
            }
            if (manifest.imports) {
                imports = {};
                for (const [name, url] of Object.entries(manifest.imports)) imports[name] = new URL(url, base).href;
            }
        }

        const worker = await createWorker({
            wasmUrl: "https://runtime.edgepython.com/js/compiler_lib.wasm",
            mainThreadModules,
            imports,
        });
        const code = await fetch(file).then(r => r.text());
        await worker.run(code);
    }
}

export function defineElement( tag = 'edge-python' ) {
    customElements.define(tag, EdgePythonElement);
}

// In some environment (e.g., cloudflare workers, deno) use: `?setElement=false` to skip auto-defining the element, due to `customElements` doesn't exist in that environment.
const setElement = new URL(import.meta.url).searchParams.get("setElement");
if (setElement != "false") defineElement();
