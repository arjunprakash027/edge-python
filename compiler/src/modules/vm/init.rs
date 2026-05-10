use alloc::{string::{String, ToString}, vec::Vec};

use crate::modules::parser::{OpCode, SSAChunk, ssa_strip, ImportKind};

use super::VM;
use super::types::*;

/* Walk a module chunk's top-level instructions for every `StoreName` op
   and read the corresponding slot value. Each unique bare name (after
   SSA strip) becomes one attribute on the resulting Module Val.
   Repeated assignments to the same name post-coalescing share a
   canonical slot; `seen` deduplicates so the latest value wins. */
fn collect_module_attrs(chunk: &SSAChunk, slots: &[Val]) -> Vec<(String, Val)> {
    let mut attrs: Vec<(String, Val)> = Vec::new();
    let mut seen: crate::util::fx::FxHashSet<String> =
        crate::util::fx::FxHashSet::default();
    for ins in &chunk.instructions {
        if !matches!(ins.opcode, OpCode::StoreName) { continue; }
        let Some(name) = chunk.names.get(ins.operand as usize) else { continue; };
        let bare = ssa_strip(name).to_string();
        // Dunders are private: __name__, __main__, etc. exposed as attrs
        // would leak the framing CPython uses. Match Python's `from m
        // import *` semantics that skip names starting with `_`.
        if bare.starts_with('_') { continue; }
        if !seen.insert(bare.clone()) { continue; }
        if let Some(&v) = slots.get(ins.operand as usize)
            && !v.is_undef()
        {
            attrs.push((bare, v));
        }
    }
    attrs
}

impl<'a> VM<'a> {

    /* Recursively flatten nested `def`s into a single global function table,
       depth-first so closures defined inside nested functions still resolve.
       Class bodies are walked too, since they may host method `def`s.

       Also populates two reverse maps used by the call-site propagation to
       distinguish "calling our own lexical-parent's def" (late-binding —
       captures may be overwritten) from "calling a closure created elsewhere"
       (closure semantics — captures must stick):

         function_parents[fi]      -> fi of the def that emitted MakeFunction
                                     for `fi`, None for module-level defs
         body_to_fi[body_chunk_ptr]-> fi whose body that chunk is, used to
                                     resolve the caller's own fi at call time

       Together they let `exec_call` answer: "is the caller the lexical
       parent of the callee?". When yes, propagation overwrites freely
       (Python late-binding); when no, captured slots are protected
       (fixes stacked decorators where each `w` captures its own `f`). */
    pub(crate) fn build_function_table(
        &mut self,
        chunk: &'a SSAChunk,
        parent_fi: Option<usize>,
        module_spec: Option<&str>,
    ) {
        let mut indices = Vec::with_capacity(chunk.functions.len());
        for desc in chunk.functions.iter() {
            let global = self.functions.len() as u32;
            self.functions.push(desc);
            self.function_parents.push(parent_fi);
            self.fn_module.push(module_spec.map(String::from));
            self.body_to_fi.insert(&desc.1 as *const _, global as usize);
            // Resolve the function's bare name from its parent chunk: desc.3
            // is the name slot index into chunk.names. ssa_strip drops the
            // SSA version suffix so the traceback shows `f`, not `f_2`.
            let name = chunk.names.get(desc.3 as usize)
                .map(|n| ssa_strip(n).to_string())
                .unwrap_or_default();
            self.function_names.push(name);
            indices.push(global);
            self.build_function_table(&desc.1, Some(global as usize), module_spec);
        }
        self.fn_index.push((chunk as *const _, indices));

        // Index every SSA name in this chunk by its bare prefix so the
        // call-site free-load fallback can do O(1) lookups instead of
        // re-parsing each name on every miss.
        let mut name_versions: crate::util::fx::FxHashMap<alloc::string::String, alloc::vec::Vec<(i64, usize)>> =
            crate::util::fx::FxHashMap::default();
        for (si, sname) in chunk.names.iter().enumerate() {
            if let Some(p) = sname.rfind('_')
                && let Ok(v) = sname[p+1..].parse::<i64>() {
                    name_versions
                        .entry(sname[..p].to_string())
                        .or_default()
                        .push((v, si));
                }
        }
        self.chunk_name_versions.insert(chunk as *const _, name_versions);
        for class_body in chunk.classes.iter() {
            self.build_function_table(class_body, parent_fi, module_spec);
        }
        // Register imported code-modules so their MakeFunction ops at
        // top-level resolve correctly when `init_modules` runs them.
        // Each imported module's functions carry its spec, so free-load
        // fallback at call time stays inside that module's namespace
        // and cross-module helpers with the same name don't collide.
        for entry in chunk.imports.iter() {
            if let ImportKind::Code(sub) = &entry.kind {
                self.build_function_table(sub, None, Some(&entry.spec));
            }
        }
    }

    pub fn run(&mut self) -> Result<Val, VmErr> {
        self.error_byte_pos = None;
        // Initialise every imported module (top-level runs once) BEFORE
        // user code dispatches. Topological order falls out of recursive
        // descent: a module's dependencies are seen + initialised before
        // its own top-level runs.
        let mut in_progress: crate::util::fx::FxHashSet<String> =
            crate::util::fx::FxHashSet::default();
        self.init_modules(self.chunk, &mut in_progress)?;
        let mut slots = self.fill_builtins(&self.chunk.names);
        self.exec(self.chunk, &mut slots)
    }

    /* Walk every import declared by `chunk` (and transitively by code
       modules), initialising each unique spec exactly once. Code modules
       run their top-level in a fresh slot frame and capture stored
       top-level names as the resulting Module's attrs. Native modules
       skip the run step — their bindings are already concrete. Cycle
       detection: re-entering an in-progress spec errors out cleanly
       rather than looping forever. */
    fn init_modules(
        &mut self,
        chunk: &SSAChunk,
        in_progress: &mut crate::util::fx::FxHashSet<String>,
    ) -> Result<(), VmErr> {
        for entry in &chunk.imports {
            if self.module_table.contains_key(&entry.spec) { continue; }
            if !in_progress.insert(entry.spec.clone()) {
                return Err(VmErr::Runtime("circular import"));
            }
            match &entry.kind {
                ImportKind::Native(bindings) => {
                    let mut attrs: Vec<(String, Val)> = Vec::with_capacity(bindings.len());
                    for b in bindings {
                        let val = self.heap.alloc(HeapObj::Extern(b.clone()))?;
                        attrs.push((b.name.clone(), val));
                    }
                    let val = self.heap.alloc(HeapObj::Module(entry.spec.clone(), attrs))?;
                    self.module_table.insert(entry.spec.clone(), val);
                }
                ImportKind::Code(sub_chunk) => {
                    self.init_modules(sub_chunk, in_progress)?;
                    let mut sub_slots = self.fill_builtins(&sub_chunk.names);
                    // Each module sees its own spec in `__name__`, so
                    // `if __name__ == "__main__":` blocks correctly skip
                    // when a file is imported.
                    let spec_val = self.heap.alloc(HeapObj::Str(entry.spec.clone()))?;
                    for (i, name) in sub_chunk.names.iter().enumerate() {
                        if ssa_strip(name) == "__name__" {
                            sub_slots[i] = spec_val;
                        }
                    }
                    self.exec(sub_chunk, &mut sub_slots)?;
                    let attrs = collect_module_attrs(sub_chunk, &sub_slots);
                    let val = self.heap.alloc(HeapObj::Module(entry.spec.clone(), attrs))?;
                    self.module_table.insert(entry.spec.clone(), val);
                }
            }
            in_progress.remove(&entry.spec);
        }
        Ok(())
    }
}
