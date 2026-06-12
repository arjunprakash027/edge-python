// Shared Edge Python runtime for doc playgrounds. One Web Worker per page (lazy init): loads CDN runtime + compiler.wasm, streams stdout to the active block.
const WASM_URL = 'https://cdn.edgepython.com/compiler.wasm'
const RUNTIME_URL = 'https://cdn.edgepython.com/runtime/src/index.js'

let workerPromise = null
let activeSink = null // raw-stdout-chunk handler of the block currently running

function getWorker() {
	if (workerPromise) return workerPromise
	workerPromise = (async () => {
		// webpackIgnore keeps Next from bundling the cross-origin ESM; it loads in the browser at runtime.
		const { createWorker } = await import(/* webpackIgnore: true */ RUNTIME_URL)
		const worker = await createWorker({ wasmUrl: WASM_URL, integrity: true })
		worker.onOutput((chunk) => { if (activeSink) activeSink(chunk) })
		return worker
	})()
	return workerPromise
}

/* Run `src`, streaming each raw stdout chunk to `onChunk`. Resolves with the error text (empty on success) and elapsed ms. */
export async function run(src, onChunk) {
	const worker = await getWorker()
	activeSink = onChunk
	try {
		const { out, ms } = await worker.run(src, { baseUrl: location.href })
		return { error: out || '', ms }
	} finally {
		activeSink = null
	}
}
