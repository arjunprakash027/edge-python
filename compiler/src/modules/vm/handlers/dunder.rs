/* 
Dunder dispatch protocol: probe an instance method, invoke with `self` prepended, treat `NotImplemented` as a miss so reflected ops / generic fallbacks take over. 
*/

use super::*;
use super::methods::AttrLookup;

impl<'a> VM<'a> {
    /* `recv.<name>(*args)`: `Some(v)` on return, `None` on miss / `NotImplemented`, `Err` only on a raised dunder. */
    #[allow(dead_code)] // Consumed by per-operator handlers in the next phase.
    pub(crate) fn try_call_dunder(&mut self, recv: Val, name: &str, args: &[Val], chunk: &SSAChunk, slots: &mut [Val]) -> Result<Option<Val>, VmErr> {
        // Built-in types route through their native handlers; dunder dispatch only fires on user instances.
        if !recv.is_heap() { return Ok(None); }
        if !matches!(self.heap.get(recv), HeapObj::Instance(..)) { return Ok(None); }

        let Some(AttrLookup::InstanceMethod { recv, func }) = self.resolve_attr_silent(recv, name)? else { return Ok(None); };

        // Mirror `__init__` dispatch: depth guard before pushing so a recursive blow-up leaves no half-built frame.
        if self.depth >= self.max_calls { return Err(cold_depth()); }

        self.push(func);
        self.push(recv);
        for &a in args { self.push(a); }
        let argc = (1 + args.len()) as u16;
        self.exec_call(argc, chunk, slots)?;

        let result = self.pop()?;
        if self.heap.is_not_implemented(result) { return Ok(None); }
        Ok(Some(result))
    }
}
