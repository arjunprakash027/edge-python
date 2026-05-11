use alloc::vec::Vec;

use super::Val;
use super::err::VmErr;

/* Scheduler state per coroutine; stepped round-robin until the target leaves Ready/Sleeping. */
#[derive(Clone, Debug)]
pub enum CoroState {
    /// Resumable on next tick.
    Ready,
    /// Suspended until `until_ns`; the scheduler fast-forwards when all are Sleeping.
    Sleeping(u64),
    /// Next resume injects a `CancelledError` raise.
    CancelPending,
    /// Returned with this Val.
    Done(Val),
    /// Raised; stored verbatim for `gather` / `with_timeout`.
    Errored(VmErr),
    /// Cancellation already observed; yields `None` to gather() peers.
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct CoroutineHandle {
    /// User-provided Coroutine HeapObj.
    pub coro: Val,
    pub state: CoroState,
}

/* Call-site snapshot for traceback rendering; pushed by `exec_call`, popped on return/error. */
#[derive(Clone, Debug)]
pub struct CallFrame {
    pub fi: usize,
    pub call_byte_pos: u32,
    pub caller_source: alloc::sync::Arc<alloc::string::String>,
    pub caller_path: alloc::sync::Arc<alloc::string::String>,
}

/* ForIter state, consumed one item per `next_item`. */
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
