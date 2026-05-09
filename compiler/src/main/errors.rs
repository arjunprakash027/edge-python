use crate::abi::ErrorKind;
use crate::modules::vm::types::VmErr;
use crate::s;
use alloc::string::String;

use super::error_stash;

/* Map a VmErr into the ABI's typed ErrorKind + message. */
pub(super) fn err_to_kind(e: &VmErr) -> ErrorKind {
    match e {
        VmErr::Type(_) | VmErr::TypeMsg(_) => ErrorKind::Type,
        VmErr::Value(_)                    => ErrorKind::Value,
        VmErr::Runtime(_)                  => ErrorKind::Runtime,
        VmErr::Attribute(_) | VmErr::Name(_) => ErrorKind::Attribute,
        VmErr::Raised(s) => {
            if s.starts_with("ValueError")      { ErrorKind::Value }
            else if s.starts_with("IndexError") { ErrorKind::Index }
            else if s.starts_with("KeyError")   { ErrorKind::Key }
            else                                { ErrorKind::Runtime }
        }
        _ => ErrorKind::Runtime,
    }
}

pub(super) fn stash_error(e: VmErr) {
    error_stash().set_typed(err_to_kind(&e), e.render());
}

pub(super) fn error_from_kind(kind: u32, msg: String) -> VmErr {
    match kind {
        0 => VmErr::TypeMsg(msg),
        1 => VmErr::Raised(s!("ValueError: ", str &msg)),
        3 => VmErr::Attribute(msg),
        4 => VmErr::Raised(s!("IndexError: ", str &msg)),
        5 => VmErr::Raised(s!("KeyError: ", str &msg)),
        _ => VmErr::Raised(msg),
    }
}
