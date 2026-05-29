use super::super::VM;
use super::super::types::*;
use crate::alloc::string::ToString;

impl<'a> VM<'a> {

    pub fn call_str(&mut self, chunk: &crate::modules::parser::SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        let o = self.pop()?;
        let s = self.display_op(o, chunk, slots)?;
        self.alloc_and_push_str(s)
    }

    pub fn call_bool(&mut self, chunk: &crate::modules::parser::SSAChunk, slots: &mut [Val]) -> Result<(), VmErr> {
        let o = self.pop()?;
        let t = self.truthy_op(o, chunk, slots)?;
        self.push(Val::bool(t));
        Ok(())
    }

    pub fn call_type(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        // Exception instances report their concrete class (e.g. `ZeroDivisionError`), not the generic `exception`.
        let name = match o.is_heap().then(|| self.heap.get(o)) {
            Some(HeapObj::ExcInstance(n, _)) => n.clone(),
            _ => self.type_name(o).to_string(), // interned, so shares the `set`/`int` singleton
        };
        let t = self.heap.alloc(HeapObj::Type(name))?;
        self.push(t);
        Ok(())
    }
}
