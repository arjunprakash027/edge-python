use alloc::vec::Vec;

use super::super::VM;
use super::super::types::*;

impl<'a> VM<'a> {

    /* Resume a coroutine. On yield: persist ip/slots/stack/iters back into it. On return: restore caller stack/iter state. */
    pub fn resume_coroutine(&mut self, callee: Val) -> Result<Val, VmErr> {
        if let HeapObj::Coroutine(ip, saved_slots, saved_stack, fi, saved_iters) = self.heap.get(callee) {
            let (ip, fi) = (*ip, *fi);
            let mut fn_slots = saved_slots.clone();
            let saved_stack_len = self.stack.len();
            let saved_iter_len = self.iter_stack.len();
            self.stack.extend_from_slice(&saved_stack.clone());
            self.iter_stack.extend(saved_iters.clone());
            let saved_yielded = self.yielded;
            self.yielded = false;
            self.depth += 1;
            let (_, body, _, _) = self.functions[fi];
            let result = self.exec_from(body, &mut fn_slots, ip);
            self.depth -= 1;
            let result = result?;
            if self.yielded {
                let resume_ip = self.resume_ip;
                let remaining = self.stack.split_off(saved_stack_len);
                let coro_iters: Vec<IterFrame> = self.iter_stack.drain(saved_iter_len..).collect();
                if let HeapObj::Coroutine(sip, ss, sst, _, si) = self.heap.get_mut(callee) {
                    *sip = resume_ip;
                    *ss = fn_slots;
                    *sst = remaining;
                    *si = coro_iters;
                }
                Ok(result)
            } else {
                self.stack.truncate(saved_stack_len);
                self.iter_stack.truncate(saved_iter_len);
                self.yielded = saved_yielded;
                Ok(result)
            }
        } else {
            Err(cold_type("not a coroutine"))
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
                    self.scheduler.clear();
                    return Err(e);
                }
                _ => {}
            }
        }
        self.scheduler.clear();
        self.push(result);
        Ok(())
    }

    // Drive the scheduler until every handle is terminal (Done / Errored / Cancelled).
    pub(crate) fn run_until_all_done(&mut self) -> Result<(), VmErr> {
        loop {
            let alive = self.scheduler.iter().any(|h| matches!(
                h.state,
                CoroState::Ready
                | CoroState::CancelPending
                | CoroState::Sleeping(_)
            ));
            if !alive { return Ok(()); }
            // Pick a Ready handle; otherwise advance the clock to the earliest wakeup.
            let mut next_ready: Option<usize> = None;
            let mut min_wake: Option<u64> = None;
            for (i, h) in self.scheduler.iter().enumerate() {
                match &h.state {
                    CoroState::Ready => { next_ready = Some(i); break; }
                    CoroState::CancelPending => { next_ready = Some(i); break; }
                    CoroState::Sleeping(w)
                        if min_wake.is_none_or(|m| *w < m) => { min_wake = Some(*w); }
                    _ => {}
                }
            }
            if let Some(i) = next_ready {
                self.scheduler_step(i)?;
                continue;
            }
            match min_wake {
                Some(w) => {
                    let now = self.now_ns();
                    // Real clock: don't busy-wait, but advance virtual_clock_ns so later sleeps are relative.
                    if w > now && self.time_hook.is_none() {
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
                None => return Ok(()),
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
        // Snapshot before resume so a yield during `sleep()` can read it.
        self.pending.sleep_until_ns = None;
        let result = self.resume_coroutine(coro);
        let yielded = self.yielded;
        self.yielded = false;
        let new_state = match result {
            Err(e) => CoroState::Errored(e),
            Ok(v) if yielded => {
                // `sleep()` sets `pending_sleep_until_ns`; `receive()` parks Ready and re-drains the queue.
                if let Some(until) = self.pending.sleep_until_ns.take() {
                    CoroState::Sleeping(until)
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
                    // Coro asked to sleep past our deadline -> time out now.
                    if until >= deadline {
                        self.scheduler[idx].state = CoroState::CancelPending;
                        timed_out = true;
                        self.scheduler_step(idx)?;
                        break;
                    }
                    // Sleep wakes before deadline -> advance clock and wake.
                    if self.time_hook.is_none() && until > self.virtual_clock_ns {
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

    /* Pop the oldest queued message, or yield None to signal "still waiting". */
    pub fn call_receive(&mut self) -> Result<(), VmErr> {
        if !self.event_queue.is_empty() {
            let val = self.event_queue.remove(0);
            self.push(val);
        } else {
            self.push(Val::none());
            self.yielded = true;
        }
        Ok(())
    }
}
