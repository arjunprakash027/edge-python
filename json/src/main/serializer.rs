/*
Walk a Handle and emit JSON text. Uses `type_of` for dispatch, `iter` + `len` + `get_item` for sequences (`iter_next` is bypassed: wasm-pdk v0.1.0's StopIteration check uses `starts_with` but the host renders the message with a prefix, so it never matches).
*/

use alloc::{format, string::String};
use wasm_pdk::{Error, FromValue, Handle, Result, Value, decode, encode};

pub fn serialize(value: &Handle) -> Result<String> {
    let mut out = String::new();
    serialize_into(value, &mut out)?;
    Ok(out)
}

fn serialize_into(value: &Handle, out: &mut String) -> Result<()> {
    let ty_handle = value.type_of()?;
    let ty = String::from_handle(ty_handle.raw())?;
    match ty.as_str() {
        "NoneType" => { out.push_str("null"); Ok(()) }
        "bool" => {
            match decode(value.raw())? {
                Value::Bool(true) => out.push_str("true"),
                Value::Bool(false) => out.push_str("false"),
                _ => return Err(Error::Type("bool decoded as non-bool".into())),
            }
            Ok(())
        }
        "int" => {
            match decode(value.raw())? {
                Value::Int(i) => out.push_str(&format!("{}", i)),
                _ => return Err(Error::Type("int decoded as non-int".into())),
            }
            Ok(())
        }
        "float" => {
            match decode(value.raw())? {
                Value::Float(f) => {
                    if !f.is_finite() {
                        return Err(Error::Value("float must be finite to serialize as JSON".into()));
                    }
                    out.push_str(&format_float(f));
                }
                _ => return Err(Error::Type("float decoded as non-float".into())),
            }
            Ok(())
        }
        "str" => {
            match decode(value.raw())? {
                Value::Bytes(b) => {
                    let s = String::from_utf8(b).map_err(|e| Error::Value(format!("invalid utf-8 in str: {}", e)))?;
                    escape_string(&s, out);
                }
                _ => return Err(Error::Type("str decoded as non-bytes".into())),
            }
            Ok(())
        }
        "list" | "tuple" | "set" | "frozenset" => serialize_sequence(value, out),
        "dict" => serialize_object(value, out),
        other => Err(Error::Type(format!("'{}' is not JSON-serializable", other))),
    }
}

fn serialize_sequence(value: &Handle, out: &mut String) -> Result<()> {
    out.push('[');
    let it = value.iter()?;
    let n = it.len()?;
    for i in 0..n {
        let idx = encode(Value::Int(i as i128))?;
        let item = it.get_item(&idx)?;
        if i > 0 { out.push(','); }
        serialize_into(&item, out)?;
    }
    out.push(']');
    Ok(())
}

fn serialize_object(value: &Handle, out: &mut String) -> Result<()> {
    out.push('{');
    let keys = value.iter()?;
    let n = keys.len()?;
    for i in 0..n {
        let idx = encode(Value::Int(i as i128))?;
        let key = keys.get_item(&idx)?;
        let key_ty_handle = key.type_of()?;
        let key_ty = String::from_handle(key_ty_handle.raw())?;
        if key_ty != "str" {
            return Err(Error::Type(format!("dict key must be str for JSON, got '{}'", key_ty)));
        }
        let key_str = match decode(key.raw())? {
            Value::Bytes(b) => String::from_utf8(b).map_err(|e| Error::Value(format!("invalid utf-8 in key: {}", e)))?,
            _ => return Err(Error::Type("str key decoded as non-bytes".into())),
        };
        let item = value.get_item(&key)?;
        if i > 0 { out.push(','); }
        escape_string(&key_str, out);
        out.push(':');
        serialize_into(&item, out)?;
    }
    out.push('}');
    Ok(())
}

fn escape_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn format_float(f: f64) -> String {
    // Integer-valued floats keep a trailing ".0" to disambiguate from int (Python's `json.dumps(1.0)` -> `"1.0"`).
    let s = format!("{}", f);
    if s.contains('.') || s.contains('e') || s.contains('E') || s.contains("inf") || s.contains("NaN") {
        s
    } else {
        let mut t = s;
        t.push_str(".0");
        t
    }
}
