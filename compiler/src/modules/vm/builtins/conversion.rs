use super::super::VM;
use super::super::types::*;

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
        let name = self.type_name(o); // interned, so shares the `set`/`int` singleton
        let t = self.heap.alloc(HeapObj::Type(name.to_string()))?;
        self.push(t);
        Ok(())
    }
}
