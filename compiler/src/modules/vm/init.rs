use alloc::{string::{String, ToString}, vec::Vec};

use crate::modules::parser::{OpCode, SSAChunk, ssa_strip, ImportKind};

use super::VM;
use super::types::*;

/* Collect top-level StoreName bindings as module attrs; `seen` keeps the latest per bare name. */
fn collect_module_attrs(chunk: &SSAChunk, slots: &[Val]) -> Vec<(String, Val)> {
    let mut attrs: Vec<(String, Val)> = Vec::new();
    let mut seen: crate::util::fx::FxHashSet<String> = crate::util::fx::FxHashSet::default();
    for ins in &chunk.instructions {
        if !matches!(ins.opcode, OpCode::StoreName) { continue; }
        let Some(name) = chunk.names.get(ins.operand as usize) else { continue; };
        let bare = ssa_strip(name).to_string();
        // Skip `_`-prefixed names, mirroring `from m import *` semantics.
        if bare.starts_with('_') { continue; }
        if !seen.insert(bare.clone()) { continue; }
        if let Some(&v) = slots.get(ins.operand as usize) && !v.is_undef()
        {
            attrs.push((bare, v));
        }
    }
    attrs
}

impl<'a> VM<'a> {

    /* Flatten nested defs into one table (DFS); also build parent/body-pointer maps so `exec_call` can tell lexical-parent calls (late-bind) from foreign closures (captures stick). */
    pub(crate) fn build_function_table(&mut self, chunk: &'a SSAChunk, parent_fi: Option<usize>, module_spec: Option<&str>) {
        let mut indices = Vec::with_capacity(chunk.functions.len());
        for desc in chunk.functions.iter() {
            let global = self.functions.len() as u32;
            self.functions.push(desc);
            self.function_parents.push(parent_fi);
            self.fn_module.push(module_spec.map(String::from));
            self.body_to_fi.insert(&desc.1 as *const _, global as usize);
            // Bare function name (SSA suffix stripped) for tracebacks.
            let name = chunk.names.get(desc.3 as usize).map(|n| ssa_strip(n).to_string()).unwrap_or_default();
            self.function_names.push(name);
            indices.push(global);
            self.build_function_table(&desc.1, Some(global as usize), module_spec);
        }
        self.fn_index.push((chunk as *const _, indices));

        // Bare-name index so the free-load fallback is O(1) instead of re-parsing per miss.
        let mut name_versions: super::NameVersionIndex = crate::util::fx::FxHashMap::default();
        for (si, sname) in chunk.names.iter().enumerate() {
            if let Some(parsed) = crate::modules::parser::SsaName::parse(sname) {
                name_versions
                    .entry(parsed.bare.to_string())
                    .or_default()
                    .push((parsed.version as i64, si));
            }
        }
        self.chunk_name_versions.insert(chunk as *const _, name_versions);
        for class_body in chunk.classes.iter() {
            self.build_function_table(class_body, parent_fi, module_spec);
        }
        // Recurse into code-module imports; each fn carries its spec so namespaces stay separate.
        for entry in chunk.imports.iter() {
            if let ImportKind::Code(sub) = &entry.kind {
                self.build_function_table(sub, None, Some(&entry.spec));
            }
        }
    }

    pub fn run(&mut self) -> Result<Val, VmErr> {
        self.error_byte_pos = None;
        // Initialise imports before user code; DFS gives topological order naturally.
        let mut in_progress: crate::util::fx::FxHashSet<String> = crate::util::fx::FxHashSet::default();
        self.init_modules(self.chunk, &mut in_progress)?;
        let mut slots = self.fill_builtins(&self.chunk.names);
        self.exec(self.chunk, &mut slots)
    }

    /* Init each unique import once; code modules run their top-level, native ones just bind. `in_progress` catches cycles cleanly. */
    fn init_modules(&mut self, chunk: &SSAChunk, in_progress: &mut crate::util::fx::FxHashSet<String>) -> Result<(), VmErr> {
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
                    // Set `__name__` to the module spec so `if __name__ == "__main__":` works.
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
