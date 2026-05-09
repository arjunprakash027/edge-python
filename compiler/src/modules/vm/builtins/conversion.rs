use crate::s;

use super::super::VM;
use super::super::types::*;

impl<'a> VM<'a> {

    pub fn call_str(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        self.alloc_and_push_str(self.display(o))
    }

    pub fn call_bool(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?; self.push(Val::bool(self.truthy(o))); Ok(())
    }

    pub fn call_type(&mut self) -> Result<(), VmErr> {
        let o = self.pop()?;
        let s = self.type_name(o);
        self.alloc_and_push_str(s!("<class '", str s, "'>"))
    }
}
