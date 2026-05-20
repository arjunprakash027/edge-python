use alloc::vec::Vec;

use crate::modules::parser::SSAChunk;
use super::super::VM;
use super::super::types::*;

impl<'a> VM<'a> {

    // Resume coroutine: persist state on yield, restore caller on return. Suspended sync sub-frames run innermost-first, each pushing its result onto the next frame's stack at the Call site.
    pub fn resume_coroutine(&mut self, callee: Val) -> Result<Val, VmErr> {
        let (outer_ip, mut outer_slots, outer_stack, outer_body, outer_iters, mut sync_frames) =
            if let HeapObj::Coroutine(ip, slots, stack, body, iters, sf) = self.heap.get(callee) {
                (*ip, slots.clone(), stack.clone(), *body, iters.clone(), sf.clone())
            } else {
                return Err(cold_type("not a coroutine"));
            };

        let saved_stack_len = self.stack.len();
        let saved_iter_len = self.iter_stack.len();
        self.stack.extend_from_slice(&outer_stack);
        self.iter_stack.extend(outer_iters);
        let saved_yielded = self.yielded;
        self.yielded = false;
        self.depth += 1;

        // Walk frames inside-out, then the outer. `outer_ran` tracks whether `outer_ip` should be overwritten by `resume_ip` on save — a re-yield inside a sync frame leaves the outer pristine.
        let mut outer_ran = false;
        let result: Result<Val, VmErr> = 'drive: loop {
            if let Some(frame) = sync_frames.pop() {
                let SyncFrame { ip, fi, mut slots, stack_delta, iter_delta } = frame;
                let frame_stack_base = self.stack.len();
                let frame_iter_base = self.iter_stack.len();
                self.stack.extend(stack_delta);
                self.iter_stack.extend(iter_delta);
                let (_, body, _, _) = self.functions[fi];
                match self.exec_from(body, &mut slots, ip) {
                    Err(e) => break 'drive Err(e),
                    Ok(val) if self.yielded => {
                        let new_stack = if self.stack.len() > frame_stack_base { self.stack.split_off(frame_stack_base) } else { Vec::new() };
                        let new_iter: Vec<IterFrame> = if self.iter_stack.len() > frame_iter_base { self.iter_stack.drain(frame_iter_base..).collect() } else { Vec::new() };
                        sync_frames.push(SyncFrame {
                            ip: self.resume_ip, fi, slots,
                            stack_delta: new_stack, iter_delta: new_iter,
                        });
                        // Any deeper sync calls suspended during this exec come back via the VM-level buffer; chain them on top (still innermost-last).
                        let newer = core::mem::take(&mut self.pending_sync_frames);
                        sync_frames.extend(newer);
                        break 'drive Ok(val);
                    }
                    Ok(val) => {
                        // Frame completed; its return value feeds whatever frame (or outer) was waiting at the Call site.
                        self.push(val);
                    }
                }
            } else {
                let body: &SSAChunk = match outer_body {
                    BodyRef::Fn(fi) => &self.functions[fi].1,
                    BodyRef::Module => self.chunk,
                };
                outer_ran = true;
                match self.exec_from(body, &mut outer_slots, outer_ip) {
                    Err(e) => break 'drive Err(e),
                    Ok(val) => {
                        if self.yielded {
                            let newer = core::mem::take(&mut self.pending_sync_frames);
                            sync_frames.extend(newer);
                        }
                        break 'drive Ok(val);
                    }
                }
            }
        };

        self.depth -= 1;
        let result = result?;

        if self.yielded {
            let resume_ip = if outer_ran { self.resume_ip } else { outer_ip };
            let remaining = self.stack.split_off(saved_stack_len);
            let coro_iters: Vec<IterFrame> = self.iter_stack.drain(saved_iter_len..).collect();
            if let HeapObj::Coroutine(sip, ss, sst, _, si, sf) = self.heap.get_mut(callee) {
                *sip = resume_ip;
                *ss = outer_slots;
                *sst = remaining;
                *si = coro_iters;
                *sf = sync_frames;
            }
            Ok(result)
        } else {
            self.stack.truncate(saved_stack_len);
            self.iter_stack.truncate(saved_iter_len);
            self.yielded = saved_yielded;
            Ok(result)
        }
    }

    /* `run(*coros)` — drive the scheduler until the *first* arg finishes; the rest still drain to completion (matches `gather` semantics). Returns the first result. */
    pub fn call_run(&mut self, argc: u16) -> Result<(), VmErr> {
        let tasks = self.pop_n(argc as usize)?;
        if tasks.is_empty() {
            self.push(Val::none());
            return Ok(());
        }
        let target = tasks[0];
        // Reset per-run virtual clock so deterministic tests don't drift.
        if self.time_hook.is_none() { self.virtual_clock_ns = 0; }
        for v in &tasks {
            if v.is_heap() && matches!(self.heap.get(*v), HeapObj::Coroutine(..)) {
                self.scheduler.push(CoroutineHandle {
                    coro: *v,
                    state: CoroState::Ready,
                });
            }
        }
        // Drain everything so concurrent coroutines all run to completion; remember the target handle's result for the return value.
        self.run_until_all_done()?;
        let mut result = Val::none();
        if let Some(h) = self.scheduler.iter().find(|h| h.coro == target) {
            match &h.state {
                CoroState::Done(v) => result = *v,
                CoroState::Errored(e) => {
                    let e = e.clone();
                    // Only drop the handles we pushed; an enclosing driver (e.g. the implicit module-body coro) may still own scheduler entries.
                    self.scheduler.retain(|h| !tasks.contains(&h.coro));
                    return Err(e);
                }
                _ => {}
            }
        }
        self.scheduler.retain(|h| !tasks.contains(&h.coro));
        self.push(result);
        Ok(())
    }

    // Drive the scheduler until every handle is terminal (Done / Errored / Cancelled). Skip currently-executing handles — `executing_coros` is non-empty when a coro called `run(...)` mid-body and we're driving its children from a nested invocation.
    pub(crate) fn run_until_all_done(&mut self) -> Result<(), VmErr> {
        loop {
            let alive = self.scheduler.iter().any(|h| !self.executing_coros.contains(&h.coro) && matches!(
                h.state,
                CoroState::Ready
                | CoroState::CancelPending
                | CoroState::Sleeping(_)
                | CoroState::WaitingFrame
                | CoroState::WaitingEvent
                | CoroState::WaitingHostCall
            ));
            if !alive { return Ok(()); }
            // Pick a Ready handle; otherwise inspect parked coros to decide which kind of host
            // wake-up to request.
            let mut next_ready: Option<usize> = None;
            let mut min_wake: Option<u64> = None;
            let mut any_frame = false;
            let mut any_event = false;
            let mut any_host_call = false;
            for (i, h) in self.scheduler.iter().enumerate() {
                if self.executing_coros.contains(&h.coro) { continue; }
                match &h.state {
                    CoroState::Ready => { next_ready = Some(i); break; }
                    CoroState::CancelPending => { next_ready = Some(i); break; }
                    CoroState::Sleeping(w)
                        if min_wake.is_none_or(|m| *w < m) => { min_wake = Some(*w); }
                    CoroState::WaitingFrame => { any_frame = true; }
                    CoroState::WaitingEvent => { any_event = true; }
                    CoroState::WaitingHostCall => { any_host_call = true; }
                    _ => {}
                }
            }
            if let Some(i) = next_ready {
                self.scheduler_step(i)?;
                continue;
            }
            // Yield priority: frame tick (~16ms) > sleep deadline > host call > open-ended event push.
            if any_frame {
                return Err(VmErr::HostYield(SchedulerStatus::PendingFrame));
            }
            match min_wake {
                Some(w) => {
                    let now = self.now_ns();
                    if w > now {
                        if self.time_hook.is_some() {
                            // Real clock: yield; embedder arms `w - now` timer and calls `run_resume`.
                            return Err(VmErr::HostYield(SchedulerStatus::PendingTimer(w)));
                        }
                        // No host clock: virtual jump so deterministic tests still progress.
                        self.virtual_clock_ns = w;
                    }
                    let now = self.now_ns();
                    for h in self.scheduler.iter_mut() {
                        if let CoroState::Sleeping(w) = h.state
                            && w <= now
                        {
                            h.state = CoroState::Ready;
                        }
                    }
                }
                None => {
                    // Host-call precedence: a deferred DOM op resolves before unrelated `receive()` waits.
                    if any_host_call {
                        return Err(VmErr::HostYield(SchedulerStatus::PendingHostCall));
                    }
                    if any_event {
                        return Err(VmErr::HostYield(SchedulerStatus::PendingEvent));
                    }
                    return Ok(());
                }
            }
        }
    }

    fn scheduler_step(&mut self, idx: usize) -> Result<(), VmErr> {
        let coro = self.scheduler[idx].coro;
        // CancelPending -> inject a CancelledError raise instead of resuming.
        if matches!(self.scheduler[idx].state, CoroState::CancelPending) {
            self.scheduler[idx].state = CoroState::Cancelled;
            return Ok(());
        }
        // Snapshot before resume so a yield during `sleep` / `frame` / `receive` can read it.
        self.pending.sleep_until_ns = None;
        self.pending.host_frame_request = false;
        self.pending.event_wait_request = false;
        self.pending.host_call_request = false;
        // Park the running coro on the executing stack so nested `run(...)` drivers skip it.
        self.executing_coros.push(coro);
        let result = self.resume_coroutine(coro);
        self.executing_coros.pop();
        let yielded = self.yielded;
        self.yielded = false;
        let new_state = match result {
            // Nested host yield bubbles up unchanged; this coro stays Ready so the outer driver re-picks it after `run_resume`.
            Err(VmErr::HostYield(s)) => return Err(VmErr::HostYield(s)),
            Err(e) => CoroState::Errored(e),
            Ok(v) if yielded => {
                // Suspension precedence: sleep > frame > receive > host-call > bare yield.
                if let Some(until) = self.pending.sleep_until_ns.take() {
                    CoroState::Sleeping(until)
                } else if core::mem::replace(&mut self.pending.host_frame_request, false) {
                    CoroState::WaitingFrame
                } else if core::mem::replace(&mut self.pending.event_wait_request, false) {
                    CoroState::WaitingEvent
                } else if core::mem::replace(&mut self.pending.host_call_request, false) {
                    CoroState::WaitingHostCall
                } else {
                    let _ = v;
                    CoroState::Ready
                }
            }
            Ok(v) => CoroState::Done(v),
        };
        self.scheduler[idx].state = new_state;
        Ok(())
    }

    /* Suspend until the host's next render frame; browsers hook `requestAnimationFrame`. */
    pub fn call_frame(&mut self) -> Result<(), VmErr> {
        self.pending.host_frame_request = true;
        self.push(Val::none());
        self.yielded = true;
        Ok(())
    }

    /* Suspend until `s` real seconds elapse. */
    pub fn call_sleep(&mut self) -> Result<(), VmErr> {
        let n = self.pop()?;
        let secs: f64 = if n.is_int() { n.as_int() as f64 }
            else if n.is_float() { n.as_float() }
            else if n.is_bool() { n.as_bool() as i64 as f64 }
            else { 0.0 };
        let secs = if secs < 0.0 { 0.0 } else { secs };
        let until = self.now_ns().saturating_add((secs * 1_000_000_000.0) as u64);
        self.pending.sleep_until_ns = Some(until);
        // Push None as the yield value; the scheduler ignores it.
        self.push(Val::none());
        self.yielded = true;
        Ok(())
    }

    /* `gather(*coros)` — concurrent fan-out. */
    pub fn call_gather(&mut self, argc: u16) -> Result<(), VmErr> {
        let tasks = self.pop_n(argc as usize)?;
        let coros: Vec<Val> = tasks.into_iter()
            .filter(|v| v.is_heap() && matches!(self.heap.get(*v), HeapObj::Coroutine(..)))
            .collect();
        for v in &coros {
            self.scheduler.push(CoroutineHandle {
                coro: *v,
                state: CoroState::Ready,
            });
        }
        self.run_until_all_done()?;
        // Cancel-rest-and-raise on first error.
        let mut first_err: Option<VmErr> = None;
        for v in &coros {
            if let Some(h) = self.scheduler.iter().find(|h| h.coro == *v)
                && let CoroState::Errored(e) = &h.state
            {
                first_err = Some(e.clone());
                break;
            }
        }
        let mut results = Vec::with_capacity(coros.len());
        for v in &coros {
            let res = self.scheduler.iter().find(|h| h.coro == *v)
                .map(|h| match &h.state {
                    CoroState::Done(r) => *r,
                    _ => Val::none(),
                }).unwrap_or(Val::none());
            results.push(res);
        }
        // Drop only the gather'd handles — leave any unrelated scheduler entries (set up by an outer `run()`) alone.
        self.scheduler.retain(|h| !coros.contains(&h.coro));
        if let Some(e) = first_err { return Err(e); }
        self.alloc_and_push_list(results)
    }

    /* `with_timeout(seconds, coro)` — drain `coro` until terminal or `seconds` elapse. On timeout: cancel the coroutine and raise TimeoutError. */
    pub fn call_with_timeout(&mut self) -> Result<(), VmErr> {
        let coro = self.pop()?;
        let secs_v = self.pop()?;
        if !(coro.is_heap() && matches!(self.heap.get(coro), HeapObj::Coroutine(..))) {
            return Err(cold_type("with_timeout() requires a coroutine"));
        }
        let secs: f64 = if secs_v.is_int() { secs_v.as_int() as f64 }
            else if secs_v.is_float() { secs_v.as_float() }
            else { return Err(cold_type("with_timeout() seconds must be a number")); };
        let deadline = self.now_ns().saturating_add((secs.max(0.0) * 1_000_000_000.0) as u64);
        self.scheduler.push(CoroutineHandle {
            coro, state: CoroState::Ready,
        });
        // Drive one step at a time so the deadline check stays tight.
        let mut timed_out = false;
        while let Some(idx) = self.scheduler.iter().position(|h| h.coro == coro) {
            match self.scheduler[idx].state.clone() {
                CoroState::Done(_)
                | CoroState::Errored(_)
                | CoroState::Cancelled => break,
                CoroState::Sleeping(until) => {
                    // Sleep past our deadline -> time out now.
                    if until >= deadline {
                        self.scheduler[idx].state = CoroState::CancelPending;
                        timed_out = true;
                        self.scheduler_step(idx)?;
                        break;
                    }
                    // Sleep wakes before deadline: host clock yields to embedder; else virtual-jump.
                    if self.time_hook.is_some() {
                        return Err(VmErr::HostYield(SchedulerStatus::PendingTimer(until)));
                    }
                    if until > self.virtual_clock_ns {
                        self.virtual_clock_ns = until;
                    }
                    self.scheduler[idx].state = CoroState::Ready;
                }
                _ => {}
            }
            if self.now_ns() >= deadline {
                self.scheduler[idx].state = CoroState::CancelPending;
                timed_out = true;
                self.scheduler_step(idx)?;
                break;
            }
            self.scheduler_step(idx)?;
        }
        let result = self.scheduler.iter().find(|h| h.coro == coro)
            .map(|h| match &h.state {
                CoroState::Done(v) => Ok(*v),
                CoroState::Errored(e) => Err(e.clone()),
                _ => Ok(Val::none()),
            }).unwrap_or(Ok(Val::none()));
        self.scheduler.retain(|h| h.coro != coro);
        if timed_out { return Err(VmErr::Raised("TimeoutError".into())); }
        let v = result?;
        self.push(v); Ok(())
    }

    /* cancel(coro) — flag the coroutine for cancellation. */
    pub fn call_cancel(&mut self) -> Result<(), VmErr> {
        let coro = self.pop()?;
        if let Some(h) = self.scheduler.iter_mut().find(|h| h.coro == coro) {
            h.state = CoroState::CancelPending;
        }
        self.push(Val::none()); Ok(())
    }

    /* Pop oldest queued message; if empty, park in `WaitingEvent` until `run_push_event`. */
    pub fn call_receive(&mut self) -> Result<(), VmErr> {
        if !self.event_queue.is_empty() {
            let val = self.event_queue.remove(0);
            self.push(val);
        } else {
            self.pending.event_wait_request = true;
            self.push(Val::none());
            self.yielded = true;
        }
        Ok(())
    }
}
