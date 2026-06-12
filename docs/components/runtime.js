// Shared Edge Python runtime for doc playgrounds. One Web Worker per page (lazy init): loads CDN runtime + compiler.wasm, streams stdout to the active block.
const WASM_URL = 'https://cdn.edgepython.com/compiler.wasm'
const RUNTIME_URL = 'https://cdn.edgepython.com/runtime/src/index.js'

let workerPromise = null
let workerReady = false // flips true once the worker+wasm are loaded; gates the cold-start phases
let activeSink = null // raw-stdout-chunk handler of the block currently running
let runChain = Promise.resolve() // serializes runs: one shared worker + one global activeSink can't host two blocks at once

// `onPhase` only matters for the first (cold) call: 'runtime' (downloading the ESM) then 'worker' (spawn + wasm fetch/instantiate).
function getWorker(onPhase) {
	if (workerPromise) return workerPromise
	workerPromise = (async () => {
		onPhase?.('runtime')
		// webpackIgnore keeps Next from bundling the cross-origin ESM; it loads in the browser at runtime.
		const { createWorker } = await import(/* webpackIgnore: true */ RUNTIME_URL)
		onPhase?.('worker')
		const worker = await createWorker({ wasmUrl: WASM_URL, integrity: true })
		worker.onOutput((chunk) => { if (activeSink) activeSink(chunk) })
		workerReady = true
		return worker
	})()
	return workerPromise
}

/* Run `src`, streaming each raw stdout chunk to `onChunk`. `onPhase` reports 'runtime'/'worker' (cold start only) then 'running'. Resolves with the error text (empty on success) and elapsed ms. */
export async function run(src, onChunk, onPhase) {
	const exec = async () => {
		// Only the run that triggers the cold start drives the load phases; warm runs go straight to 'running'.
		const worker = await getWorker(workerReady ? undefined : onPhase)
		onPhase?.('running')
		activeSink = onChunk
		try {
			const { out, ms } = await worker.run(src, { baseUrl: location.href })
			return { error: out || '', ms }
		} finally {
			activeSink = null
		}
	}
	// Queue behind any in-flight run (success or failure) so a second block never overwrites the first's activeSink mid-stream.
	const result = runChain.then(exec, exec)
	runChain = result.catch(() => {})
	return result
}
