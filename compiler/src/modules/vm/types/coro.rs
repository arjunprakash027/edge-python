use alloc::vec::Vec;

use super::Val;
use super::err::VmErr;

/* Cooperative scheduler state for one coroutine living in `vm.scheduler`.
   The scheduler steps each Ready handle once per round-robin pass, swaps
   it to Sleeping/Done/Errored/Cancelled depending on what happened, and
   ends the run when the target handle leaves the Ready/Sleeping pool. */
#[derive(Clone, Debug)]
pub enum CoroState {
    /// Resumable on the next scheduler tick.
    Ready,
    /// Suspended until `until_ns` (per `vm.time_hook`). When all live
    /// handles are Sleeping the scheduler advances the clock to the
    /// minimum `until_ns` rather than spinning.
    Sleeping(u64),
    /// Resumed-on-next-tick state used while a `cancel()` request is
    /// pending: the next resume injects a `CancelledError` raise into the
    /// coroutine instead of advancing it normally.
    CancelPending,
    /// Coroutine returned with this Val.
    Done(Val),
    /// Coroutine raised an exception. Stored verbatim so `gather` /
    /// `with_timeout` can propagate it to the caller.
    Errored(VmErr),
    /// Coroutine was cancelled and its CancelledError was already
    /// observed (or it never ran). Returns `None` to gather()s peers.
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct CoroutineHandle {
    /// The Coroutine HeapObj Val passed by the user.
    pub coro: Val,
    pub state: CoroState,
}

/* One snapshot of a user-function call site, pushed when `exec_call` enters
   a HeapObj::Func and popped on return/error. Carries everything the
   traceback renderer needs without requiring a chunk lookup at error time. */
#[derive(Clone, Debug)]
pub struct CallFrame {
    pub fi: usize,
    pub call_byte_pos: u32,
    pub caller_source: alloc::sync::Arc<alloc::string::String>,
    pub caller_path: alloc::sync::Arc<alloc::string::String>,
}

/* Iterator state for ForIter. Consumed one item at a time. */
#[derive(Clone, Debug)]
pub enum IterFrame {
    Seq { items: Vec<Val>, idx: usize },
    Range { cur: i64, end: i64, step: i64 },
    Coroutine(Val),
}

impl IterFrame {
    pub fn next_item(&mut self) -> Option<Val> {
        match self {
            Self::Coroutine(_) => None,
            Self::Seq { items, idx } => {
                if *idx < items.len() { let v = items[*idx]; *idx += 1; Some(v) } else { None }
            }
            Self::Range { cur, end, step } => {
                let done = if *step > 0 { *cur >= *end } else { *cur <= *end };
                if done { None } else { let v = *cur; *cur += *step; Some(Val::int(v)) }
            }
        }
    }
}
