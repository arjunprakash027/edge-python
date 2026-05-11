use super::VM;
use super::types::*;

impl<'a> VM<'a> {

    /* Mark all reachable roots then sweep; non-heap Vals are no-op to mark. */
    pub(crate) fn collect(&mut self, current_slots: &[Val]) {
        for &v in &self.stack { self.heap.mark(v); }
        for &v in &self.with_stack { self.heap.mark(v); }
        for &v in &self.yields { self.heap.mark(v); }
        for &v in &self.event_queue { self.heap.mark(v); }
        for &v in current_slots { self.heap.mark(v); }
        for &v in &self.live_slots { self.heap.mark(v); }
        for tpl in &self.slot_templates {
            for &v in tpl { self.heap.mark(v); }
        }
        for &v in self.globals.values() { self.heap.mark(v); }
        for frame in &self.iter_stack {
            match frame {
                IterFrame::Seq { items, .. } => {
                    for &v in items { self.heap.mark(v); }
                }
                IterFrame::Coroutine(v) => self.heap.mark(*v),
                IterFrame::Range { .. } => {}
            }
        }
        for cache in self.opcode_caches.values() {
            if let Some(consts) = cache.const_vals_opt() {
                for &v in consts { self.heap.mark(v); }
            }
        }
        // SAFETY: each ptr is live for its exec() frame and the Vec's alloc is move-stable.
        for i in 0..self.active_const_pools.len() {
            let consts: &[Val] = unsafe { &*self.active_const_pools[i] };
            for &v in consts { self.heap.mark(v); }
        }
        self.templates.mark_all(&mut self.heap);
        self.heap.sweep();
    }
}
