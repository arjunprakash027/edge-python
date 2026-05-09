use alloc::string::String;

use super::super::VM;
use super::super::types::*;

impl<'a> VM<'a> {

    /* Pops N args, joins with single spaces. Calls `print_hook` if set (streaming),
       otherwise buffers into `output`. */
    pub fn call_print(&mut self, op: u16) -> Result<(), VmErr> {
        let args = self.pop_n(op as usize)?;
        let mut out = String::new();
        for (i, v) in args.iter().enumerate() {
            if i > 0 { out.push(' '); }
            out.push_str(&self.display(*v));
        }
        match self.print_hook {
            Some(hook) => hook(&out),
            None       => self.output.push(out),
        }
        Ok(())
    }

    /* Returns empty string in sandbox; no stdin access in WASM. */
    pub fn call_input(&mut self) -> Result<(), VmErr> {
        let s = if !self.input_buffer.is_empty() {
            self.input_buffer.remove(0)
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let mut line = String::new();
                let _ = std::io::stdin().read_line(&mut line);
                while line.ends_with('\n') || line.ends_with('\r') { line.pop(); }
                line
            }
            #[cfg(target_arch = "wasm32")]
            { return Err(VmErr::Runtime("input() requires host data in WASM (use set_input)")); }
        };
        let val = self.heap.alloc(HeapObj::Str(s))?;
        self.push(val); Ok(())
    }

    // format(value [, spec]).
    pub fn call_format(&mut self, op: u16) -> Result<(), VmErr> {
        if op != 1 && op != 2 {
            return Err(cold_type("format() takes 1 or 2 arguments"));
        }
        let spec_val = if op == 2 { Some(self.pop()?) } else { None };
        let val = self.pop()?;
        let result = match spec_val {
            Some(sv) => {
                let spec = match self.heap.get(sv) {
                    HeapObj::Str(s) => s.clone(),
                    _ => return Err(cold_type("format() spec must be a string")),
                };
                super::super::handlers::format::format_value(val, &spec, &self.heap)
                    .map_err(cold_value)?
            }
            None => self.display(val),
        };
        self.alloc_and_push_str(result)
    }
}
