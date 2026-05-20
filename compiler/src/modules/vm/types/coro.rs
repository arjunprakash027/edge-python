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
    /// Parked waiting for the host's next render frame; resumed when the embedder calls back.
    WaitingFrame,
    /// Parked in `receive()` with an empty queue; resumed when the host pushes a message.
    WaitingEvent,
    /// Parked mid-`CallExtern`; resumed when the host calls `set_host_result`.
    WaitingHostCall,
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
    // Class where the running method was found and its implicit `self`; consumed by `super()` to walk one level up. `None` for plain function calls.
    pub current_class: Option<Val>,
    pub current_self: Option<Val>,
}

/* ForIter state, consumed one item per `next_item`. */
#[derive(Clone, Debug)]
pub enum IterFrame {
    Seq { items: Vec<Val>, idx: usize },
    Range { cur: i64, end: i64, step: i64 },
    Coroutine(Val),
    // User-defined iterator: holds the value returned by `__iter__`; each step calls its `__next__`.
    UserDefined(Val),
}

impl IterFrame {
    /* Stateless steps only — built-in Seq/Range. User-defined iterators step in `dispatch.rs` because they need the VM to invoke `__next__`. */
    pub fn next_item(&mut self) -> Option<Val> {
        match self {
            Self::Coroutine(_) | Self::UserDefined(_) => None,
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
