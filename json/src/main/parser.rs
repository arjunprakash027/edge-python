/*
Recursive-descent JSON parser. Builds Handles via `Handle::new_dict` / `new_list` / `set_item` and `list.append`; primitives go through `encode`.
*/

use alloc::format;
use wasm_pdk::{encode, Error, Handle, Result, Value};

use super::tokenizer::{JsonError, Token, Tokenizer};

pub fn parse(src: &str) -> Result<Handle> {
    let mut tk = Tokenizer::new(src);
    let value = parse_value(&mut tk)?;
    match tk.next().map_err(to_pdk_err)? {
        Token::Eof => Ok(value),
        _ => Err(value_err(tk.pos(), "trailing data after JSON value")),
    }
}

fn parse_value(tk: &mut Tokenizer) -> Result<Handle> {
    let t = tk.next().map_err(to_pdk_err)?;
    parse_value_with(tk, t)
}

fn parse_value_with(tk: &mut Tokenizer, t: Token) -> Result<Handle> {
    match t {
        Token::Null => encode(Value::None),
        Token::True => encode(Value::Bool(true)),
        Token::False => encode(Value::Bool(false)),
        Token::Int(i) => encode(Value::Int(i)),
        Token::Float(f) => encode(Value::Float(f)),
        Token::Str(s) => encode(Value::Bytes(s.into_bytes())),
        Token::LBracket => parse_array(tk),
        Token::LBrace => parse_object(tk),
        Token::RBracket | Token::RBrace | Token::Comma | Token::Colon => {
            Err(value_err(tk.pos(), "unexpected token"))
        }
        Token::Eof => Err(value_err(tk.pos(), "unexpected end of input")),
    }
}

fn parse_array(tk: &mut Tokenizer) -> Result<Handle> {
    let list = Handle::new_list()?;
    let first = tk.next().map_err(to_pdk_err)?;
    if matches!(first, Token::RBracket) { return Ok(list); }
    let mut current = first;
    loop {
        let item = parse_value_with(tk, current)?;
        let _ = list.call("append", &[item.raw()])?;
        match tk.next().map_err(to_pdk_err)? {
            Token::Comma => current = tk.next().map_err(to_pdk_err)?,
            Token::RBracket => return Ok(list),
            _ => return Err(value_err(tk.pos(), "expected ',' or ']' in array")),
        }
    }
}

fn parse_object(tk: &mut Tokenizer) -> Result<Handle> {
    let dict = Handle::new_dict()?;
    let first = tk.next().map_err(to_pdk_err)?;
    if matches!(first, Token::RBrace) { return Ok(dict); }
    let mut current = first;
    loop {
        let key_str = match current {
            Token::Str(s) => s,
            _ => return Err(value_err(tk.pos(), "object key must be a string")),
        };
        let key = encode(Value::Bytes(key_str.into_bytes()))?;
        match tk.next().map_err(to_pdk_err)? {
            Token::Colon => {}
            _ => return Err(value_err(tk.pos(), "expected ':' after object key")),
        }
        let value = parse_value(tk)?;
        dict.set_item(&key, &value)?;
        match tk.next().map_err(to_pdk_err)? {
            Token::Comma => current = tk.next().map_err(to_pdk_err)?,
            Token::RBrace => return Ok(dict),
            _ => return Err(value_err(tk.pos(), "expected ',' or '}' in object")),
        }
    }
}

fn to_pdk_err(e: JsonError) -> Error {
    Error::Value(format!("{} at byte {}", e.msg, e.pos))
}

fn value_err(pos: usize, msg: &str) -> Error {
    Error::Value(format!("{} at byte {}", msg, pos))
}
