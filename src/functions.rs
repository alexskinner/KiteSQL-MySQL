// Copyright 2024 KipData/KiteSQL
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! MySQL-specific scalar functions, installed by [`MySqlFunctions`].

use kite_sql::errors::DatabaseError;
use kite_sql::expression::function::scala::ScalarFunctionImpl;
use kite_sql::expression::function::{FunctionSummary, PersistencePolicy};
use kite_sql::expression::TypeCast;
use kite_sql::planner::{ExprRef, MetaArena};
use kite_sql::types::tuple::Input;
use kite_sql::types::value::DataValue;
use kite_sql::types::{CharLengthUnits, LogicalType};
use kite_sql::{FunctionPack, FunctionRegistry};
use std::sync::Arc;

const TEXT: LogicalType = LogicalType::Varchar(None, CharLengthUnits::Characters);

/// The MySQL function pack. `with_mysql()` installs it.
#[derive(Debug, Default)]
pub struct MySqlFunctions;

impl FunctionPack for MySqlFunctions {
    fn name(&self) -> &str {
        "mysql"
    }

    fn install(&self, registry: &mut FunctionRegistry) -> Result<(), DatabaseError> {
        registry.register_scalar(Replace::new())?;

        // LENGTH: accepts any type (TEXT signature, converts numerics to string)
        registry.register_scalar(Length::new(TEXT))?;

        // HEX: accepts any type (TEXT signature, handles numerics specially)
        registry.register_scalar(Hex::new(TEXT))?;

        // CONCAT: truly variadic with runtime arity check (minimum 1 arg)
        registry.register_scalar(Concat::new())?;

        // CONCAT_WS: truly variadic with runtime arity check (minimum 2 args)
        registry.register_scalar(ConcatWs::new())?;

        registry.register_scalar(SubstringIndex::new())?;
        registry.register_scalar(Locate::new(2))?;
        registry.register_scalar(Locate::new(3))?;
        registry.register_scalar(Instr::new())?;
        registry.register_scalar(FindInSet::new())?;
        // POSITION is handled by core evaluator (PR #38), don't register here
        registry.register_scalar(Left::new())?;
        registry.register_scalar(Right::new())?;
        registry.register_scalar(Lpad::new())?;
        registry.register_scalar(Rpad::new())?;
        registry.register_scalar(Repeat::new())?;
        registry.register_scalar(Reverse::new())?;
        registry.register_scalar(Ltrim::new())?;
        registry.register_scalar(Rtrim::new())?;
        registry.register_scalar(Uuid::new())?;

        // GREATEST/LEAST: truly variadic with runtime arity check (minimum 2 args)
        registry.register_scalar(Greatest::new())?;
        registry.register_scalar(Least::new())?;

        // ROUND: single registration handles both 1 and 2 args via accepts_any_args
        registry.register_scalar(Round::new_variadic())?;
        registry.register_scalar(IntegerDivide::new())?;

        // ISNULL(expr): 1 when expr is NULL, else 0 (an integer, never NULL).
        registry.register_scalar(IsNull::new())?;

        // LAST_INSERT_ID() and LAST_INSERT_ID(expr); a host that registered its own keeps it.
        for with_arg in [false, true] {
            match registry.register_scalar(crate::auto_increment::LastInsertId::new(with_arg)) {
                Ok(()) | Err(DatabaseError::FunctionAlreadyExists(_)) => {}
                Err(err) => return Err(err),
            }
        }

        Ok(())
    }
}

/// MySQL's default `max_allowed_packet` (64 MiB): string functions return NULL instead of a
/// longer result.
const MAX_ALLOWED_PACKET: usize = 64 * 1024 * 1024;

fn eval_text(
    arg: ExprRef,
    arena: &(dyn MetaArena + '_),
    tuple: Option<&dyn Input>,
) -> Result<Option<String>, DatabaseError> {
    let value = arena.expression(arg).eval(arena, tuple)?;
    if value.is_null() {
        return Ok(None);
    }
    let value = value.into_owned().cast(&TEXT)?;
    Ok(value.utf8().map(str::to_string))
}

/// Evaluate an argument as text, converting numbers to their string representation (MySQL formatting).
/// Booleans convert to "1" or "0".
fn eval_value_as_text(
    arg: ExprRef,
    arena: &(dyn MetaArena + '_),
    tuple: Option<&dyn Input>,
) -> Result<Option<String>, DatabaseError> {
    let value = arena.expression(arg).eval(arena, tuple)?;
    if value.is_null() {
        return Ok(None);
    }
    match value.as_ref() {
        DataValue::Boolean(b) => Ok(Some(if *b { "1" } else { "0" }.to_string())),
        DataValue::Int8(i) => Ok(Some(i.to_string())),
        DataValue::Int16(i) => Ok(Some(i.to_string())),
        DataValue::Int32(i) => Ok(Some(i.to_string())),
        DataValue::Int64(i) => Ok(Some(i.to_string())),
        DataValue::UInt8(i) => Ok(Some(i.to_string())),
        DataValue::UInt16(i) => Ok(Some(i.to_string())),
        DataValue::UInt32(i) => Ok(Some(i.to_string())),
        DataValue::UInt64(i) => Ok(Some(i.to_string())),
        DataValue::Float32(f) => Ok(Some(f.0.to_string())),
        DataValue::Float64(f) => Ok(Some(f.0.to_string())),
        DataValue::Decimal(d) => Ok(Some(d.to_string())),
        _ => {
            let value = value.into_owned().cast(&TEXT)?;
            Ok(value.utf8().map(str::to_string))
        }
    }
}

/// `REPLACE(str, from_str, to_str)`: every occurrence of `from_str` in `str`, matched
/// case-sensitively, is replaced with `to_str`. Returns NULL if any argument is NULL, and `str`
/// unchanged when `from_str` is empty.
#[derive(Debug)]
pub struct Replace {
    summary: FunctionSummary,
}

impl Replace {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "replace".into(),
                arg_types: vec![TEXT, TEXT, TEXT],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Replace {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        let mut texts = Vec::with_capacity(3);
        for arg in args {
            match eval_text(*arg, arena, tuple)? {
                Some(text) => texts.push(text),
                None => return Ok(DataValue::Null),
            }
        }
        let [input, from, to] = <[String; 3]>::try_from(texts)
            .map_err(|_| DatabaseError::MisMatch("number of replace() parameters", "3"))?;
        if from.is_empty() {
            return Ok(DataValue::from(input));
        }
        Ok(DataValue::from(input.replace(&from, &to)))
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// `LENGTH(str)`: returns the byte length of `str` in UTF-8 encoding. NULL if argument is NULL.
/// Accepts numeric arguments (converted to string first).
#[derive(Debug)]
pub struct Length {
    summary: FunctionSummary,
}

impl Length {
    pub fn new(arg_type: LogicalType) -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "length".into(),
                arg_types: vec![arg_type],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Length {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        if args.len() != 1 {
            return Err(DatabaseError::InvalidValue(format!(
                "LENGTH expects 1 argument, got {}",
                args.len()
            )));
        }
        match eval_value_as_text(args[0], arena, tuple)? {
            Some(s) => Ok(DataValue::Int32(s.len() as i32)),
            None => Ok(DataValue::Null),
        }
    }

    fn return_type(&self) -> &LogicalType {
        &LogicalType::Integer
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }

    fn accepts_any_args(&self) -> bool {
        true
    }
}

/// `CONCAT(str1, str2, ...)`: concatenates all arguments. Returns NULL if any argument is NULL.
#[derive(Debug)]
pub struct Concat {
    summary: FunctionSummary,
}

impl Concat {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "concat".into(),
                arg_types: vec![TEXT], // Placeholder, actual count checked at runtime
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Concat {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        if args.is_empty() {
            return Err(DatabaseError::InvalidValue(
                "CONCAT requires at least 1 argument".into(),
            ));
        }
        let mut result = String::new();
        for arg in args {
            match eval_value_as_text(*arg, arena, tuple)? {
                Some(s) => result.push_str(&s),
                None => return Ok(DataValue::Null),
            }
        }
        Ok(DataValue::from(result))
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }

    fn accepts_any_args(&self) -> bool {
        true
    }

    fn accepts_arity(&self, n: usize) -> bool {
        n >= 1
    }
}

/// `CONCAT_WS(separator, str1, str2, ...)`: concatenates strings with separator. Skips NULL values.
/// Returns NULL if separator is NULL.
#[derive(Debug)]
pub struct ConcatWs {
    summary: FunctionSummary,
}

impl ConcatWs {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "concat_ws".into(),
                arg_types: vec![TEXT, TEXT], // Placeholder, actual count checked at runtime
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for ConcatWs {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        if args.len() < 2 {
            return Err(DatabaseError::InvalidValue(
                "CONCAT_WS requires at least 2 arguments".into(),
            ));
        }
        let separator = eval_text(args[0], arena, tuple)?;
        if separator.is_none() {
            return Ok(DataValue::Null);
        }
        let sep = separator.unwrap();

        let mut parts = Vec::new();
        for arg in &args[1..] {
            if let Some(s) = eval_value_as_text(*arg, arena, tuple)? {
                parts.push(s);
            }
        }
        Ok(DataValue::from(parts.join(&sep)))
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }

    fn accepts_any_args(&self) -> bool {
        true
    }

    fn accepts_arity(&self, n: usize) -> bool {
        n >= 2
    }
}

/// `SUBSTRING_INDEX(str, delim, count)`: returns substring before count occurrences of delim.
#[derive(Debug)]
pub struct SubstringIndex {
    summary: FunctionSummary,
}

impl SubstringIndex {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "substring_index".into(),
                arg_types: vec![TEXT, TEXT, LogicalType::Integer],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for SubstringIndex {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        let str_val = eval_text(args[0], arena, tuple)?;
        let delim = eval_text(args[1], arena, tuple)?;
        let count = arena.expression(args[2]).eval(arena, tuple)?;

        if str_val.is_none() || delim.is_none() || count.is_null() {
            return Ok(DataValue::Null);
        }

        let s = str_val.unwrap();
        let d = delim.unwrap();
        let cnt = match count.as_ref() {
            DataValue::Int32(c) => *c,
            _ => return Err(DatabaseError::InvalidValue("count must be integer".into())),
        };

        if cnt == 0 || d.is_empty() {
            return Ok(DataValue::from(String::new()));
        }

        let result = if cnt > 0 {
            let mut positions = Vec::new();
            let mut start = 0;
            while let Some(pos) = s[start..].find(&d) {
                positions.push(start + pos);
                if positions.len() >= cnt as usize {
                    break;
                }
                start += pos + d.len();
            }
            if positions.len() < cnt as usize {
                s.clone()
            } else {
                s[..positions[cnt as usize - 1]].to_string()
            }
        } else {
            // MySQL searches backwards from the end, each match ending before the previous one
            // starts: substring_index('aaaaaaaaa1', 'aa', -2) is 'aa1', not 'aaa1'.
            let abs_cnt = cnt.unsigned_abs() as usize;
            let mut end = s.len();
            let mut last = None;
            for _ in 0..abs_cnt {
                match s[..end].rfind(&d) {
                    Some(pos) => {
                        last = Some(pos);
                        end = pos;
                    }
                    None => {
                        last = None;
                        break;
                    }
                }
            }
            match last {
                Some(pos) => s[pos + d.len()..].to_string(),
                None => s.clone(),
            }
        };

        Ok(DataValue::from(result))
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// `LOCATE(substr, str[, pos])`: returns 1-based character position of substr in str, or 0 if not found.
/// Case-insensitive comparison following MySQL's default collation.
#[derive(Debug)]
pub struct Locate {
    summary: FunctionSummary,
}

impl Locate {
    pub fn new(arg_count: usize) -> Arc<Self> {
        let arg_types = if arg_count == 2 {
            vec![TEXT, TEXT]
        } else {
            vec![TEXT, TEXT, LogicalType::Integer]
        };
        Arc::new(Self {
            summary: FunctionSummary {
                name: "locate".into(),
                arg_types,
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Locate {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        let substr = eval_text(args[0], arena, tuple)?;
        let str_val = eval_text(args[1], arena, tuple)?;

        if substr.is_none() || str_val.is_none() {
            return Ok(DataValue::Null);
        }

        // Case-insensitive, character by character, so positions map 1:1 onto characters.
        let fold = |text: &str| -> Vec<char> {
            text.chars()
                .map(|c| c.to_lowercase().next().unwrap_or(c))
                .collect()
        };
        let sub = fold(&substr.unwrap());
        let hay = fold(&str_val.unwrap());

        let start = if args.len() == 3 {
            let pos = arena.expression(args[2]).eval(arena, tuple)?;
            if pos.is_null() {
                return Ok(DataValue::Null);
            }
            match pos.into_owned().cast(&LogicalType::Bigint)? {
                DataValue::Int64(p) if p >= 1 => p as usize,
                DataValue::Int64(_) => return Ok(DataValue::Int32(0)),
                _ => {
                    return Err(DatabaseError::InvalidValue(
                        "position must be integer".into(),
                    ))
                }
            }
        } else {
            1
        };

        // A start past the end (one past it for an empty needle) never matches.
        if start > hay.len() + 1 {
            return Ok(DataValue::Int32(0));
        }
        if sub.is_empty() {
            return Ok(DataValue::Int32(start as i32));
        }
        let found = hay[start - 1..]
            .windows(sub.len())
            .position(|window| window == sub.as_slice());
        Ok(DataValue::Int32(found.map_or(0, |i| (start + i) as i32)))
    }

    fn return_type(&self) -> &LogicalType {
        &LogicalType::Integer
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// `INSTR(str, substr)`: returns 1-based character position of substr in str, or 0 if not found.
/// Case-insensitive comparison following MySQL's default collation.
#[derive(Debug)]
pub struct Instr {
    summary: FunctionSummary,
}

impl Instr {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "instr".into(),
                arg_types: vec![TEXT, TEXT],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Instr {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        let str_val = eval_text(args[0], arena, tuple)?;
        let substr = eval_text(args[1], arena, tuple)?;

        if str_val.is_none() || substr.is_none() {
            return Ok(DataValue::Null);
        }

        let s = str_val.unwrap();
        let s_lower = s.to_lowercase();
        let sub = substr.unwrap().to_lowercase();

        if let Some(byte_pos) = s_lower.find(&sub) {
            let char_pos = s[..byte_pos].chars().count() + 1;
            Ok(DataValue::Int32(char_pos as i32))
        } else {
            Ok(DataValue::Int32(0))
        }
    }

    fn return_type(&self) -> &LogicalType {
        &LogicalType::Integer
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// `FIND_IN_SET(str, strlist)`: the 1-based position of `str` among the comma-separated
/// items of `strlist`, compared by the statement's collation; 0 when absent, NULL with a NULL
/// argument.
#[derive(Debug)]
pub struct FindInSet {
    summary: FunctionSummary,
}

impl FindInSet {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "find_in_set".into(),
                arg_types: vec![TEXT, TEXT],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for FindInSet {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        let (Some(needle), Some(list)) = (
            eval_text(args[0], arena, tuple)?,
            eval_text(args[1], arena, tuple)?,
        ) else {
            return Ok(DataValue::Null);
        };
        if list.is_empty() {
            return Ok(DataValue::Int32(0));
        }
        let position = list
            .split(',')
            .position(|item| kite_sql::types::collation::text_eq(item, &needle));
        Ok(DataValue::Int32(position.map_or(0, |i| i as i32 + 1)))
    }

    fn return_type(&self) -> &LogicalType {
        &LogicalType::Integer
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

#[derive(Debug)]
pub struct Left {
    summary: FunctionSummary,
}

impl Left {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "left".into(),
                arg_types: vec![TEXT, LogicalType::Integer],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Left {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        let str_val = eval_text(args[0], arena, tuple)?;
        let len = arena.expression(args[1]).eval(arena, tuple)?;

        if str_val.is_none() || len.is_null() {
            return Ok(DataValue::Null);
        }

        let s = str_val.unwrap();
        let n = match len.as_ref() {
            DataValue::Int32(l) => *l.max(&0) as usize,
            _ => return Err(DatabaseError::InvalidValue("length must be integer".into())),
        };

        let chars: Vec<char> = s.chars().collect();
        let result: String = chars.iter().take(n).collect();
        Ok(DataValue::from(result))
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// `RIGHT(str, len)`: returns rightmost len characters.
#[derive(Debug)]
pub struct Right {
    summary: FunctionSummary,
}

impl Right {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "right".into(),
                arg_types: vec![TEXT, LogicalType::Integer],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Right {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        let str_val = eval_text(args[0], arena, tuple)?;
        let len = arena.expression(args[1]).eval(arena, tuple)?;

        if str_val.is_none() || len.is_null() {
            return Ok(DataValue::Null);
        }

        let s = str_val.unwrap();
        let n = match len.as_ref() {
            DataValue::Int32(l) => *l.max(&0) as usize,
            _ => return Err(DatabaseError::InvalidValue("length must be integer".into())),
        };

        let chars: Vec<char> = s.chars().collect();
        let start = if chars.len() > n { chars.len() - n } else { 0 };
        let result: String = chars[start..].iter().collect();
        Ok(DataValue::from(result))
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// `LPAD(str, len, padstr)`: left-pads str to len characters with padstr.
#[derive(Debug)]
pub struct Lpad {
    summary: FunctionSummary,
}

impl Lpad {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "lpad".into(),
                arg_types: vec![TEXT, LogicalType::Integer, TEXT],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Lpad {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        let str_val = eval_text(args[0], arena, tuple)?;
        let len = arena.expression(args[1]).eval(arena, tuple)?;
        let pad = eval_text(args[2], arena, tuple)?;

        if str_val.is_none() || len.is_null() || pad.is_none() {
            return Ok(DataValue::Null);
        }

        let s = str_val.unwrap();
        let target_len = match len.as_ref() {
            DataValue::Int32(l) if *l >= 0 => *l as usize,
            _ => return Ok(DataValue::Null),
        };
        let padstr = pad.unwrap();
        if target_len > MAX_ALLOWED_PACKET {
            return Ok(DataValue::Null);
        }

        let chars: Vec<char> = s.chars().collect();
        if chars.len() >= target_len {
            let result: String = chars.iter().take(target_len).collect();
            return Ok(DataValue::from(result));
        }
        // nothing to pad with: MySQL returns the string as it is
        if padstr.is_empty() {
            return Ok(DataValue::from(s));
        }

        let pad_chars: Vec<char> = padstr.chars().collect();
        let pad_needed = target_len - chars.len();
        let mut result = String::new();
        for i in 0..pad_needed {
            result.push(pad_chars[i % pad_chars.len()]);
        }
        result.push_str(&s);
        Ok(DataValue::from(result))
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// `RPAD(str, len, padstr)`: right-pads str to len characters with padstr.
#[derive(Debug)]
pub struct Rpad {
    summary: FunctionSummary,
}

impl Rpad {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "rpad".into(),
                arg_types: vec![TEXT, LogicalType::Integer, TEXT],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Rpad {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        let str_val = eval_value_as_text(args[0], arena, tuple)?;
        let len = arena.expression(args[1]).eval(arena, tuple)?;
        let pad = eval_value_as_text(args[2], arena, tuple)?;

        if str_val.is_none() || len.is_null() || pad.is_none() {
            return Ok(DataValue::Null);
        }

        let s = str_val.unwrap();
        let target_len = match len.as_ref() {
            DataValue::Int32(l) if *l >= 0 => *l as usize,
            _ => return Ok(DataValue::Null),
        };
        let padstr = pad.unwrap();
        if target_len > MAX_ALLOWED_PACKET {
            return Ok(DataValue::Null);
        }

        let chars: Vec<char> = s.chars().collect();
        if chars.len() >= target_len {
            let result: String = chars.iter().take(target_len).collect();
            return Ok(DataValue::from(result));
        }
        // nothing to pad with: MySQL returns the string as it is
        if padstr.is_empty() {
            return Ok(DataValue::from(s));
        }

        let pad_chars: Vec<char> = padstr.chars().collect();
        let pad_needed = target_len - chars.len();
        let mut result = s.clone();
        for i in 0..pad_needed {
            result.push(pad_chars[i % pad_chars.len()]);
        }
        Ok(DataValue::from(result))
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// `REPEAT(str, count)`: repeats str count times.
#[derive(Debug)]
pub struct Repeat {
    summary: FunctionSummary,
}

impl Repeat {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "repeat".into(),
                arg_types: vec![TEXT, LogicalType::Integer],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Repeat {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        let str_val = eval_text(args[0], arena, tuple)?;
        let count = arena.expression(args[1]).eval(arena, tuple)?;

        if str_val.is_none() || count.is_null() {
            return Ok(DataValue::Null);
        }

        let s = str_val.unwrap();
        let n = match count.as_ref() {
            DataValue::Int32(c) if *c >= 0 => *c as usize,
            _ => 0,
        };
        // MySQL returns NULL (warning 1301) rather than build a result over max_allowed_packet
        if s.len()
            .checked_mul(n)
            .is_none_or(|len| len > MAX_ALLOWED_PACKET)
        {
            return Ok(DataValue::Null);
        }

        Ok(DataValue::from(s.repeat(n)))
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// `REVERSE(str)`: reverses the characters in str.
#[derive(Debug)]
pub struct Reverse {
    summary: FunctionSummary,
}

impl Reverse {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "reverse".into(),
                arg_types: vec![TEXT],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Reverse {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        match eval_text(args[0], arena, tuple)? {
            Some(s) => Ok(DataValue::from(s.chars().rev().collect::<String>())),
            None => Ok(DataValue::Null),
        }
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// `LTRIM(str)`: removes leading whitespace from str.
#[derive(Debug)]
pub struct Ltrim {
    summary: FunctionSummary,
}

impl Ltrim {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "ltrim".into(),
                arg_types: vec![TEXT],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Ltrim {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        match eval_text(args[0], arena, tuple)? {
            Some(s) => Ok(DataValue::from(s.trim_start().to_string())),
            None => Ok(DataValue::Null),
        }
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// `RTRIM(str)`: removes trailing whitespace from str.
#[derive(Debug)]
pub struct Rtrim {
    summary: FunctionSummary,
}

impl Rtrim {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "rtrim".into(),
                arg_types: vec![TEXT],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Rtrim {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        match eval_text(args[0], arena, tuple)? {
            Some(s) => Ok(DataValue::from(s.trim_end().to_string())),
            None => Ok(DataValue::Null),
        }
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// `HEX(str_or_num)`: returns the hexadecimal representation.
/// For strings, converts bytes to hex. For integers, returns the hex value (negatives in two's complement).
#[derive(Debug)]
pub struct Hex {
    summary: FunctionSummary,
}

impl Hex {
    pub fn new(arg_type: LogicalType) -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "hex".into(),
                arg_types: vec![arg_type],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Hex {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        if args.len() != 1 {
            return Err(DatabaseError::InvalidValue(format!(
                "HEX expects 1 argument, got {}",
                args.len()
            )));
        }
        let value = arena.expression(args[0]).eval(arena, tuple)?;
        if value.is_null() {
            return Ok(DataValue::Null);
        }

        let hex = match value.as_ref() {
            DataValue::Boolean(b) => format!("{:X}", if *b { 1 } else { 0 }),
            DataValue::Int8(i) => format!("{:X}", *i as i64 as u64),
            DataValue::Int16(i) => format!("{:X}", *i as i64 as u64),
            DataValue::Int32(i) => format!("{:X}", *i as i64 as u64),
            DataValue::Int64(i) => format!("{:X}", *i as u64),
            DataValue::UInt8(i) => format!("{:X}", *i as u64),
            DataValue::UInt16(i) => format!("{:X}", *i as u64),
            DataValue::UInt32(i) => format!("{:X}", *i as u64),
            DataValue::UInt64(i) => format!("{:X}", *i),
            DataValue::Float32(f) => format!("{:X}", f.0.round() as i64 as u64),
            DataValue::Float64(f) => format!("{:X}", f.0.round() as i64 as u64),
            DataValue::Decimal(d) => {
                let rounded = d.round_dp_with_strategy(
                    0,
                    rust_decimal::RoundingStrategy::MidpointAwayFromZero,
                );
                let bits = rust_decimal::prelude::ToPrimitive::to_i64(&rounded)
                    .map(|i| i as u64)
                    .or_else(|| rust_decimal::prelude::ToPrimitive::to_u64(&rounded))
                    .unwrap_or(u64::MAX);
                format!("{:X}", bits)
            }
            _ => match eval_value_as_text(args[0], arena, tuple)? {
                Some(s) => s.bytes().map(|b| format!("{:02X}", b)).collect(),
                None => return Ok(DataValue::Null),
            },
        };
        Ok(DataValue::from(hex))
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }

    fn accepts_any_args(&self) -> bool {
        true
    }
}

/// `UUID()`: generates a version 4 UUID.
#[derive(Debug)]
pub struct Uuid {
    summary: FunctionSummary,
}

impl Uuid {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "uuid".into(),
                arg_types: vec![],
                persistence: PersistencePolicy::Transient,
            },
        })
    }
}

impl ScalarFunctionImpl for Uuid {
    fn eval(
        &self,
        _args: &[ExprRef],
        _arena: &(dyn MetaArena + '_),
        _tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        let uuid = uuid::Uuid::new_v4();
        Ok(DataValue::from(uuid.to_string()))
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// Common comparison type of GREATEST/LEAST arguments, following MySQL 8:
/// - a DATE/DATETIME/TIMESTAMP argument makes the comparison temporal: strings and numbers are
///   read as dates (`'95-05-05'`, `'20010101'`, `20050505`, `20101213123456`). The type is DATE
///   when every temporal argument is a DATE, otherwise DATETIME;
/// - otherwise any string makes it a string comparison, also when numbers are mixed in
///   (`GREATEST('11', 5, 2)` is `'5'`);
/// - otherwise any float gives DOUBLE, any decimal DECIMAL, and booleans and integers compare as
///   the widest integer.
fn common_type(arg_types: &[LogicalType]) -> LogicalType {
    use LogicalType::*;
    let types: Vec<&LogicalType> = arg_types.iter().filter(|t| **t != SqlNull).collect();
    if types.is_empty() {
        return TEXT;
    }
    let is_temporal = |t: &LogicalType| matches!(t, Date | DateTime | Time(_) | TimeStamp(..));
    if types.iter().all(|t| is_temporal(t)) && types.iter().all(|t| **t == *types[0]) {
        return types[0].clone();
    }
    let is_text = |t: &LogicalType| matches!(t, Char(..) | Varchar(..));
    let is_numeric = |t: &LogicalType| {
        matches!(
            t,
            Boolean
                | Tinyint
                | UTinyint
                | Smallint
                | USmallint
                | Integer
                | UInteger
                | Bigint
                | UBigint
                | Float
                | Double
                | Decimal(..)
        )
    };
    if types.iter().any(|t| is_temporal(t)) {
        if types.iter().any(|t| matches!(t, Time(_)))
            || !types
                .iter()
                .all(|t| is_temporal(t) || is_text(t) || is_numeric(t))
        {
            return TEXT;
        }
        return if types.iter().filter(|t| is_temporal(t)).all(|t| **t == Date) {
            Date
        } else {
            DateTime
        };
    }
    if types.iter().any(|t| is_text(t)) {
        // Strings, alone or mixed with numbers, compare as strings.
        return TEXT;
    }
    if !types.iter().all(|t| is_numeric(t)) {
        return TEXT;
    }
    if types.iter().any(|t| matches!(t, Float | Double)) {
        Double
    } else if types.iter().any(|t| matches!(t, Decimal(..))) {
        Decimal(None, None)
    } else {
        // Widest integer among the arguments; booleans count as TINYINT 0/1.
        let rank = |t: &LogicalType| match t {
            Boolean | Tinyint | UTinyint => 1,
            Smallint | USmallint => 2,
            Integer | UInteger => 3,
            _ => 4,
        };
        let width = types.iter().map(|t| rank(t)).max().unwrap_or(4);
        if types
            .iter()
            .all(|t| matches!(t, UBigint | UInteger | USmallint | UTinyint))
        {
            [UTinyint, USmallint, UInteger, UBigint][width - 1].clone()
        } else {
            [Tinyint, Smallint, Integer, Bigint][width - 1].clone()
        }
    }
}

/// Numeric value of a string the way MySQL converts it in a numeric context: the longest numeric
/// prefix after leading whitespace, otherwise 0.
fn lenient_f64(text: &str) -> f64 {
    let t = text.trim_start();
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits = |mut j: usize| {
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        j
    };
    let int_end = digits(i);
    let mut end = int_end;
    let mut has_digits = int_end > i;
    if end < b.len() && b[end] == b'.' {
        let frac_end = digits(end + 1);
        if frac_end > end + 1 {
            has_digits = true;
        }
        end = frac_end;
    }
    if !has_digits {
        return 0.0;
    }
    if end < b.len() && (b[end] == b'e' || b[end] == b'E') {
        let mut j = end + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_end = digits(j);
        if exp_end > j {
            end = exp_end;
        }
    }
    t[..end].parse().unwrap_or(0.0)
}

/// Reads the digits of a MySQL numeric date (`[YY]YYMMDD[hhmmss]`) as `YYYY-MM-DD[ hh:mm:ss]`.
/// An odd digit count has lost a leading zero (`10101` is `010101`).
fn compact_date(digits: &str) -> Option<String> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let digits = if digits.len() % 2 == 1 {
        format!("0{digits}")
    } else {
        digits.to_string()
    };
    let (year, rest) = match digits.len() {
        6 | 12 => (two_digit_year(&digits[..2])?, &digits[2..]),
        8 | 14 => (digits[..4].to_string(), &digits[4..]),
        _ => return None,
    };
    let mut text = format!("{year}-{}-{}", &rest[..2], &rest[2..4]);
    if rest.len() == 10 {
        text.push_str(&format!(
            " {}:{}:{}",
            &rest[4..6],
            &rest[6..8],
            &rest[8..10]
        ));
    }
    Some(text)
}

/// MySQL's two-digit years: 70-99 are 1970-1999, 00-69 are 2000-2069.
fn two_digit_year(year: &str) -> Option<String> {
    let year: u32 = year.parse().ok()?;
    Some(format!(
        "{}",
        if year < 70 { 2000 + year } else { 1900 + year }
    ))
}

/// Normalises a MySQL date string to `YYYY-MM-DD[ hh:mm:ss]`: two-digit years, any punctuation
/// as delimiter, `T` between date and time, a fractional second (dropped), or bare digits.
fn relaxed_date(text: &str) -> Option<String> {
    let text = text.trim();
    let whole = text.split('.').next().unwrap_or(text);
    if let Some(date) = compact_date(whole) {
        return Some(date);
    }
    let (date, time) = match text.find([' ', 'T']) {
        Some(i) => (&text[..i], Some(text[i + 1..].trim())),
        None => (text, None),
    };
    let date: Vec<&str> = date
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .collect();
    let [year, month, day] = date.as_slice() else {
        return None;
    };
    let year = if year.len() <= 2 {
        two_digit_year(year)?
    } else {
        year.to_string()
    };
    let mut normalised = format!("{year:0>4}-{month:0>2}-{day:0>2}");
    if let Some(time) = time {
        let time = time.split('.').next().unwrap_or(time);
        let parts: Vec<&str> = time
            .split(|c: char| !c.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .collect();
        if !parts.is_empty() {
            let part = |i: usize| parts.get(i).copied().unwrap_or("0");
            normalised.push_str(&format!(" {:0>2}:{:0>2}:{:0>2}", part(0), part(1), part(2)));
        }
    }
    Some(normalised)
}

/// Converts a GREATEST/LEAST argument for a temporal comparison: temporal values are cast,
/// strings are read leniently and numbers as `[YY]YYMMDD[hhmmss]`, as MySQL does.
fn to_temporal(value: DataValue, ty: &LogicalType) -> Result<DataValue, DatabaseError> {
    let text = match &value {
        DataValue::Utf8 { value: text, .. } => relaxed_date(text.as_ref()),
        DataValue::Int8(_)
        | DataValue::Int16(_)
        | DataValue::Int32(_)
        | DataValue::Int64(_)
        | DataValue::UInt8(_)
        | DataValue::UInt16(_)
        | DataValue::UInt32(_)
        | DataValue::UInt64(_) => compact_date(&value.to_string()),
        DataValue::Decimal(v) => compact_date(&v.trunc().to_string()),
        DataValue::Float32(v) => compact_date(&format!("{}", v.0.trunc())),
        DataValue::Float64(v) => compact_date(&format!("{}", v.0.trunc())),
        _ => None,
    };
    match text {
        // A DATE comparison reads only the date part of a string or number.
        Some(mut text) => {
            if *ty == LogicalType::Date {
                text.truncate(10);
            }
            DataValue::from(text).cast(ty)
        }
        None => value.cast(ty),
    }
}

/// Shared GREATEST/LEAST evaluation: NULL if any argument is NULL, otherwise every argument is
/// converted to the common type and the extreme value is returned.
fn extremum(
    name: &str,
    greatest: bool,
    args: &[ExprRef],
    arena: &(dyn MetaArena + '_),
    tuple: Option<&dyn Input>,
) -> Result<DataValue, DatabaseError> {
    if args.len() < 2 {
        return Err(DatabaseError::InvalidValue(format!(
            "{name} requires at least 2 arguments, got {}",
            args.len()
        )));
    }
    let arg_types: Vec<LogicalType> = args
        .iter()
        .map(|arg| arena.expression(*arg).return_type(arena).into_owned())
        .collect();
    let ty = common_type(&arg_types);
    let mut best: Option<DataValue> = None;
    let mut saw_null = false;
    for arg in args {
        let value = arena.expression(*arg).eval(arena, tuple)?.into_owned();
        if value.is_null() {
            saw_null = true;
            continue;
        }
        let value = match value {
            DataValue::Boolean(b) => DataValue::Int8(b as i8),
            other => other,
        };
        let value = if matches!(ty, LogicalType::Date | LogicalType::DateTime) {
            to_temporal(value, &ty)?
        } else if ty == LogicalType::Double {
            match value {
                DataValue::Utf8 { value, .. } => {
                    DataValue::Float64(ordered_float::OrderedFloat(lenient_f64(value.as_ref())))
                }
                other => other.cast(&ty)?,
            }
        } else if ty == TEXT {
            match value {
                DataValue::Float32(f) => DataValue::from(f.0.to_string()),
                DataValue::Float64(f) => DataValue::from(f.0.to_string()),
                other => other.cast(&ty)?,
            }
        } else {
            value.cast(&ty)?
        };
        best = Some(match best {
            None => value,
            Some(current) => {
                let replace = match value.partial_cmp(&current) {
                    Some(std::cmp::Ordering::Greater) => greatest,
                    Some(std::cmp::Ordering::Less) => !greatest,
                    _ => false,
                };
                if replace {
                    value
                } else {
                    current
                }
            }
        });
    }
    if saw_null {
        return Ok(DataValue::Null);
    }
    Ok(best.unwrap_or(DataValue::Null))
}

macro_rules! extremum_function {
    ($ty:ident, $name:literal, $greatest:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug)]
        pub struct $ty {
            summary: FunctionSummary,
        }

        impl $ty {
            pub fn new() -> Arc<Self> {
                Arc::new(Self {
                    summary: FunctionSummary {
                        name: $name.into(),
                        arg_types: vec![TEXT, TEXT], // Placeholder; see `accepts_arity`
                        persistence: PersistencePolicy::Durable,
                    },
                })
            }
        }

        impl ScalarFunctionImpl for $ty {
            fn eval(
                &self,
                args: &[ExprRef],
                arena: &(dyn MetaArena + '_),
                tuple: Option<&dyn Input>,
            ) -> Result<DataValue, DatabaseError> {
                extremum($name, $greatest, args, arena, tuple)
            }

            fn return_type(&self) -> &LogicalType {
                &TEXT
            }

            fn return_type_for(&self, arg_types: &[LogicalType]) -> LogicalType {
                common_type(arg_types)
            }

            fn summary(&self) -> &FunctionSummary {
                &self.summary
            }

            fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
                None
            }

            fn accepts_any_args(&self) -> bool {
                true
            }

            fn accepts_arity(&self, n: usize) -> bool {
                n >= 2
            }
        }
    };
}

extremum_function!(
    Greatest,
    "greatest",
    true,
    "`GREATEST(val1, val2, ...)`: the largest argument, compared in the common type of all \
     arguments. NULL if any argument is NULL."
);
extremum_function!(
    Least,
    "least",
    false,
    "`LEAST(val1, val2, ...)`: the smallest argument, compared in the common type of all \
     arguments. NULL if any argument is NULL."
);

/// `IFNULL(expr1, expr2)`: returns expr1 if not NULL, otherwise expr2.
#[derive(Debug)]
pub struct Ifnull {
    summary: FunctionSummary,
}

impl Ifnull {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "ifnull".into(),
                arg_types: vec![TEXT, TEXT],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for Ifnull {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        match eval_text(args[0], arena, tuple)? {
            Some(v) => Ok(DataValue::from(v)),
            None => match eval_text(args[1], arena, tuple)? {
                Some(v) => Ok(DataValue::from(v)),
                None => Ok(DataValue::Null),
            },
        }
    }

    fn return_type(&self) -> &LogicalType {
        &TEXT
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }
}

/// `ROUND(x[, d])`: rounds x to d decimal places (default 0, negative d rounds left of the
/// decimal point), half away from zero. The result has the type of `x`: integers stay integers,
/// DECIMAL stays DECIMAL, FLOAT/DOUBLE stay floating; other arguments are rounded as DOUBLE.
///
/// A literal such as `1.005` is an exact DECIMAL (the dialect reads it so), so `ROUND(1.005, 2)`
/// is 1.01 as in MySQL; DOUBLE values, `1.005e0` included, are rounded as binary values.
#[derive(Debug)]
pub struct Round {
    summary: FunctionSummary,
}

impl Round {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "round".into(),
                arg_types: vec![LogicalType::Double], // Placeholder; see `accepts_arity`
                persistence: PersistencePolicy::Durable,
            },
        })
    }

    pub fn new_variadic() -> Arc<Self> {
        Self::new()
    }
}

fn round_integer(value: i128, d: i64) -> Result<i128, DatabaseError> {
    if d >= 0 {
        return Ok(value);
    }
    let Some(divisor) = u32::try_from(-d)
        .ok()
        .and_then(|exp| 10_i128.checked_pow(exp))
    else {
        // 10^|d| exceeds every supported integer, so everything rounds to zero.
        return Ok(0);
    };
    let (quotient, remainder) = (value / divisor, value % divisor);
    let quotient = if remainder.unsigned_abs() * 2 >= divisor.unsigned_abs() {
        quotient + value.signum()
    } else {
        quotient
    };
    quotient.checked_mul(divisor).ok_or(DatabaseError::OverFlow)
}

fn round_float(x: f64, d: i64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let exp = d.clamp(-400, 400) as i32;
    let result = if exp >= 0 {
        let multiplier = 10_f64.powi(exp);
        if !multiplier.is_finite() || !(x * multiplier).is_finite() {
            return x;
        }
        (x * multiplier).round() / multiplier
    } else {
        let divisor = 10_f64.powi(-exp);
        if !divisor.is_finite() {
            return 0.0;
        }
        (x / divisor).round() * divisor
    };
    if result.is_finite() {
        result
    } else {
        x
    }
}

fn round_decimal(
    value: rust_decimal::Decimal,
    d: i64,
) -> Result<rust_decimal::Decimal, DatabaseError> {
    use rust_decimal::{Decimal, RoundingStrategy};
    if d >= 0 {
        return Ok(
            value.round_dp_with_strategy(d.min(28) as u32, RoundingStrategy::MidpointAwayFromZero)
        );
    }
    if d < -28 {
        return Ok(Decimal::ZERO);
    }
    let scale_factor = Decimal::from_i128_with_scale(10_i128.pow((-d) as u32), 0);
    (value / scale_factor)
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
        .checked_mul(scale_factor)
        .ok_or(DatabaseError::OverFlow)
}

impl ScalarFunctionImpl for Round {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        use ordered_float::OrderedFloat;

        if args.is_empty() || args.len() > 2 {
            return Err(DatabaseError::InvalidValue(format!(
                "ROUND expects 1 or 2 arguments, got {}",
                args.len()
            )));
        }
        let x = arena.expression(args[0]).eval(arena, tuple)?.into_owned();
        let d = if args.len() == 2 {
            let d = arena.expression(args[1]).eval(arena, tuple)?.into_owned();
            if d.is_null() {
                return Ok(DataValue::Null);
            }
            match d.cast(&LogicalType::Bigint)? {
                DataValue::Int64(d) => d,
                _ => {
                    return Err(DatabaseError::InvalidValue(
                        "decimals must be integer".into(),
                    ))
                }
            }
        } else {
            0
        };
        if x.is_null() {
            return Ok(DataValue::Null);
        }
        macro_rules! int {
            ($v:expr, $variant:ident) => {{
                let rounded = round_integer(i128::from($v), d)?;
                DataValue::$variant(rounded.try_into().map_err(|_| DatabaseError::OverFlow)?)
            }};
        }
        Ok(match x {
            DataValue::Boolean(b) => DataValue::Int64(round_integer(b as i128, d)? as i64),
            DataValue::Int8(v) => int!(v, Int8),
            DataValue::Int16(v) => int!(v, Int16),
            DataValue::Int32(v) => int!(v, Int32),
            DataValue::Int64(v) => int!(v, Int64),
            DataValue::UInt8(v) => int!(v, UInt8),
            DataValue::UInt16(v) => int!(v, UInt16),
            DataValue::UInt32(v) => int!(v, UInt32),
            DataValue::UInt64(v) => int!(v, UInt64),
            DataValue::Decimal(v) => DataValue::Decimal(round_decimal(v, d)?),
            DataValue::Float32(v) => {
                DataValue::Float32(OrderedFloat(round_float(v.0 as f64, d) as f32))
            }
            DataValue::Float64(v) => DataValue::Float64(OrderedFloat(round_float(v.0, d))),
            other => match other.cast(&LogicalType::Double)? {
                DataValue::Float64(v) => DataValue::Float64(OrderedFloat(round_float(v.0, d))),
                _ => {
                    return Err(DatabaseError::InvalidValue(
                        "ROUND expects numeric argument".into(),
                    ))
                }
            },
        })
    }

    fn return_type(&self) -> &LogicalType {
        &LogicalType::Double
    }

    fn return_type_for(&self, arg_types: &[LogicalType]) -> LogicalType {
        use LogicalType::*;
        match arg_types.first() {
            Some(Boolean) => Bigint,
            Some(
                t @ (Tinyint | UTinyint | Smallint | USmallint | Integer | UInteger | Bigint
                | UBigint | Float | Double),
            ) => t.clone(),
            // The result scale depends on the rounding digits, which may be per row
            // (`ROUND(d, 2)` of a DECIMAL(10,3) is 1.01, not 1.010): leave it unset so the
            // rounded value keeps the scale ROUND gave it.
            Some(Decimal(precision, _)) => Decimal(*precision, None),
            _ => Double,
        }
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }

    fn accepts_any_args(&self) -> bool {
        true
    }

    fn accepts_arity(&self, n: usize) -> bool {
        (1..=2).contains(&n)
    }
}

/// `a DIV b` (the MySQL dialect rewrites the operator to this function): integer division,
/// truncated toward zero. Integers divide exactly, a DECIMAL operand makes it an exact
/// DECIMAL division and FLOAT, DOUBLE or string operands divide as DOUBLE. The result is
/// BIGINT, or BIGINT UNSIGNED when an operand is unsigned; out of range is an error. Division
/// by zero follows the statement's policy (NULL with a warning in queries).
#[derive(Debug)]
pub struct IntegerDivide {
    summary: FunctionSummary,
}

/// Name the MySQL dialect gives `DIV`.
pub(crate) const INTEGER_DIVIDE: &str = "div";

impl IntegerDivide {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: INTEGER_DIVIDE.into(),
                arg_types: vec![LogicalType::Bigint, LogicalType::Bigint], // Placeholder
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for IntegerDivide {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        use rust_decimal::prelude::ToPrimitive;
        use rust_decimal::Decimal;

        let a = arena.expression(args[0]).eval(arena, tuple)?.into_owned();
        let b = arena.expression(args[1]).eval(arena, tuple)?.into_owned();
        if a.is_null() || b.is_null() {
            return Ok(DataValue::Null);
        }
        let integer = |v: &DataValue| -> Option<i128> {
            Some(match v {
                DataValue::Boolean(v) => *v as i128,
                DataValue::Int8(v) => *v as i128,
                DataValue::Int16(v) => *v as i128,
                DataValue::Int32(v) => *v as i128,
                DataValue::Int64(v) => *v as i128,
                DataValue::UInt8(v) => *v as i128,
                DataValue::UInt16(v) => *v as i128,
                DataValue::UInt32(v) => *v as i128,
                DataValue::UInt64(v) => *v as i128,
                _ => return None,
            })
        };
        let exact = |v: &DataValue| -> Option<Decimal> {
            match v {
                DataValue::Decimal(d) => Some(*d),
                v => integer(v).and_then(|i| Decimal::try_from_i128_with_scale(i, 0).ok()),
            }
        };
        let quotient: Option<i128> = if let (Some(x), Some(y)) = (integer(&a), integer(&b)) {
            (y != 0).then(|| x / y)
        } else if let (Some(x), Some(y)) = (exact(&a), exact(&b)) {
            if y.is_zero() {
                None
            } else {
                Some(
                    x.checked_div(y)
                        .ok_or(DatabaseError::OverFlow)?
                        .trunc()
                        .to_i128()
                        .ok_or(DatabaseError::OverFlow)?,
                )
            }
        } else {
            let double = |v: DataValue| match v.cast(&LogicalType::Double)? {
                DataValue::Float64(f) => Ok(f.0),
                _ => Err(DatabaseError::InvalidValue("DIV expects numbers".into())),
            };
            let (x, y) = (double(a.clone())?, double(b.clone())?);
            if y == 0.0 {
                None
            } else {
                let q = (x / y).trunc();
                if !q.is_finite() || q.abs() >= 1e38 {
                    return Err(DatabaseError::OverFlow);
                }
                Some(q as i128)
            }
        };
        let Some(quotient) = quotient else {
            kite_sql::diagnostics::division_by_zero()?;
            return Ok(DataValue::Null);
        };
        let unsigned = [&a, &b].iter().any(|v| {
            matches!(
                v,
                DataValue::UInt8(_)
                    | DataValue::UInt16(_)
                    | DataValue::UInt32(_)
                    | DataValue::UInt64(_)
            )
        });
        Ok(if unsigned {
            DataValue::UInt64(u64::try_from(quotient).map_err(|_| DatabaseError::OverFlow)?)
        } else {
            DataValue::Int64(i64::try_from(quotient).map_err(|_| DatabaseError::OverFlow)?)
        })
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }

    fn return_type(&self) -> &LogicalType {
        &LogicalType::Bigint
    }

    fn return_type_for(&self, arg_types: &[LogicalType]) -> LogicalType {
        if arg_types.iter().any(LogicalType::is_unsigned_numeric) {
            LogicalType::UBigint
        } else {
            LogicalType::Bigint
        }
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn accepts_any_args(&self) -> bool {
        true
    }
}

/// `ISNULL(expr)`: MySQL's NULL test function. Returns the integer 1 when `expr` is NULL and 0
/// otherwise; the result itself is never NULL. Any argument type is accepted.
#[derive(Debug)]
pub struct IsNull {
    summary: FunctionSummary,
}

impl IsNull {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            summary: FunctionSummary {
                name: "isnull".into(),
                arg_types: vec![TEXT],
                persistence: PersistencePolicy::Durable,
            },
        })
    }
}

impl ScalarFunctionImpl for IsNull {
    fn eval(
        &self,
        args: &[ExprRef],
        arena: &(dyn MetaArena + '_),
        tuple: Option<&dyn Input>,
    ) -> Result<DataValue, DatabaseError> {
        if args.len() != 1 {
            return Err(DatabaseError::InvalidValue(format!(
                "ISNULL expects 1 argument, got {}",
                args.len()
            )));
        }
        let value = arena.expression(args[0]).eval(arena, tuple)?;
        Ok(DataValue::Int64(i64::from(value.is_null())))
    }

    fn return_type(&self) -> &LogicalType {
        &LogicalType::Bigint
    }

    fn summary(&self) -> &FunctionSummary {
        &self.summary
    }

    fn monotonicity(&self) -> Option<Vec<Option<bool>>> {
        None
    }

    fn accepts_any_args(&self) -> bool {
        true
    }
}
