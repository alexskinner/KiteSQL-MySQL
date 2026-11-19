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

#![doc = include_str!("../README.md")]

//! # Quick Example
//!
//! ```rust
//! use kite_sql::db::DataBaseBuilder;
//! use kite_sql_mysql::MySqlBuilderExt;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut database = DataBaseBuilder::path(".")
//!     .with_mysql()?
//!     .build_in_memory()?;
//!
//! // Use MySQL-specific types
//! database.ddl("CREATE TABLE test (id INT PRIMARY KEY, bio MEDIUMTEXT)")?;
//! database.run("INSERT INTO test VALUES (1, 'text')")?.done()?;
//!
//! let mut result = database.run("SELECT * FROM test WHERE id = 1")?;
//! let tuple = result.next_tuple(|_, t| t.clone())?.expect("row");
//! result.done()?;
//! # Ok(())
//! # }
//! ```

pub mod auto_increment;
pub mod functions;
pub use functions::MySqlFunctions;

use kite_sql::diagnostics::{
    Diagnostic, DiagnosticKind, DiagnosticLevel, DivisionByZero, StatementPolicy,
};
use kite_sql::dialect::{ColumnTypeSpec, SqlDialect};
use kite_sql::errors::DatabaseError;
use kite_sql::expression::BinaryOperator;
use kite_sql::types::value::DataValue;
use kite_sql::types::{CharLengthUnits, LogicalType};
use sqlparser::ast::{
    visit_expressions_mut, DataType, ExactNumberInfo, Expr, ObjectNamePart, Statement, Value,
};
use sqlparser::dialect::{Dialect as SqlParserDialect, MySqlDialect as SqlParserMySqlDialect};
use std::convert::TryFrom;
use std::fmt::Debug;
use std::ops::ControlFlow;
use std::sync::Arc;

pub const DIALECT_MYSQL: &str = "mysql";

/// MySQL's default `div_precision_increment`: the scale digits `/` and AVG add.
const DIV_PRECISION_INCREMENT: u8 = 4;
/// MySQL's largest DECIMAL precision and scale.
const DECIMAL_MAX_PRECISION: u8 = 65;
const DECIMAL_MAX_SCALE: u8 = 30;

/// `AVG(arg)`'s MySQL type: DECIMAL(p + 4, s + 4) for an exact-value argument of precision
/// p and scale s, at most DECIMAL(65,30) (an integer has scale 0 and its type's digit count
/// as precision; a DECIMAL without a declared scale has scale 0, as in MySQL); `None` (DOUBLE)
/// for anything else.
fn mysql_avg_type(arg: &LogicalType) -> Option<LogicalType> {
    let (precision, scale) = match arg {
        // BOOLEAN is TINYINT(1) in MySQL: AVG(b) is DECIMAL(7,4).
        LogicalType::Boolean | LogicalType::Tinyint | LogicalType::UTinyint => (3, 0),
        LogicalType::Smallint | LogicalType::USmallint => (5, 0),
        LogicalType::Integer | LogicalType::UInteger => (10, 0),
        LogicalType::Bigint => (19, 0),
        LogicalType::UBigint => (20, 0),
        LogicalType::Decimal(precision, scale) => {
            let scale = scale.unwrap_or(0);
            (precision.unwrap_or(10).max(scale), scale)
        }
        _ => return None,
    };
    Some(LogicalType::Decimal(
        Some((precision + DIV_PRECISION_INCREMENT).min(DECIMAL_MAX_PRECISION)),
        Some((scale + DIV_PRECISION_INCREMENT).min(DECIMAL_MAX_SCALE)),
    ))
}

/// Precision and scale of an exact-value (integer or DECIMAL) type as MySQL counts them:
/// an integer has scale 0 and its type's digit count as precision (BOOLEAN is TINYINT(1));
/// a DECIMAL keeps its declared precision and scale, precision `None` when undeclared.
/// `None` for FLOAT, DOUBLE, strings and the rest, and for a DECIMAL without a declared
/// scale, whose values carry their own scale.
fn exact_digits(ty: &LogicalType) -> Option<(Option<u8>, u8)> {
    Some(match ty {
        LogicalType::Boolean | LogicalType::Tinyint | LogicalType::UTinyint => (Some(3), 0),
        LogicalType::Smallint | LogicalType::USmallint => (Some(5), 0),
        LogicalType::Integer | LogicalType::UInteger => (Some(10), 0),
        LogicalType::Bigint => (Some(19), 0),
        LogicalType::UBigint => (Some(20), 0),
        LogicalType::Decimal(precision, Some(scale)) => (precision.map(|p| p.max(*scale)), *scale),
        _ => return None,
    })
}

/// MySQL's result type for `left <op> right` on exact values: DECIMAL as soon as one
/// operand is DECIMAL, and always for `/`. Scale: `+ - %` the larger operand scale, `*` the
/// sum of the scales, `/` the dividend's scale plus `div_precision_increment` (4); at most
/// DECIMAL(65,30). Integer `+ - * %` keep the engine's integer types, and an operand that is
/// FLOAT, DOUBLE or a string keeps the engine's (DOUBLE) arithmetic. A DECIMAL operand
/// without a declared scale gives DECIMAL without scale (each value keeps the scale the
/// operation gives it).
fn mysql_arithmetic_type(
    op: &BinaryOperator,
    left: &LogicalType,
    right: &LogicalType,
) -> Option<LogicalType> {
    let is_decimal = |ty: &LogicalType| matches!(ty, LogicalType::Decimal(..));
    let is_exact = |ty: &LogicalType| {
        is_decimal(ty)
            || ty.is_signed_numeric()
            || ty.is_unsigned_numeric()
            || matches!(ty, LogicalType::Boolean)
    };
    if !is_exact(left) || !is_exact(right) {
        return None;
    }
    if !matches!(op, BinaryOperator::Divide) && !is_decimal(left) && !is_decimal(right) {
        return None;
    }
    let (Some((p1, s1)), Some((p2, s2))) = (exact_digits(left), exact_digits(right)) else {
        return Some(LogicalType::Decimal(None, None));
    };
    let (precision, scale) = match op {
        BinaryOperator::Plus | BinaryOperator::Minus => {
            let scale = s1.max(s2);
            let precision = p1
                .zip(p2)
                .map(|(p1, p2)| (p1 - s1).max(p2 - s2) as u16 + scale as u16 + 1);
            (precision, scale as u16)
        }
        BinaryOperator::Multiply => (
            p1.zip(p2).map(|(p1, p2)| p1 as u16 + p2 as u16),
            s1 as u16 + s2 as u16,
        ),
        BinaryOperator::Divide => (
            p1.map(|p1| p1 as u16 + s2 as u16 + DIV_PRECISION_INCREMENT as u16),
            s1 as u16 + DIV_PRECISION_INCREMENT as u16,
        ),
        BinaryOperator::Modulo => (
            p1.zip(p2).map(|(p1, p2)| p1.max(p2) as u16),
            s1.max(s2) as u16,
        ),
        _ => return None,
    };
    let scale = scale.min(DECIMAL_MAX_SCALE as u16) as u8;
    Some(LogicalType::Decimal(
        precision.map(|p| (p.min(DECIMAL_MAX_PRECISION as u16) as u8).max(scale)),
        Some(scale),
    ))
}

/// Largest number of significant digits a DECIMAL value holds here (rust_decimal's 96-bit
/// mantissa); MySQL's DECIMAL goes to 65.
const DECIMAL_VALUE_DIGITS: usize = 28;

/// A MySQL numeric literal: with a decimal point and no exponent it is an exact DECIMAL whose
/// precision and scale are its digits (`0.1`, `00012.340` is DECIMAL(5,3) 12.340); an integer
/// beyond BIGINT is BIGINT UNSIGNED up to 18446744073709551615 and DECIMAL above that.
/// Exponent literals (`1e2`) stay DOUBLE, as do numbers with more digits than a DECIMAL value
/// holds here. `None` leaves the literal to the engine.
fn mysql_number_literal(text: &str) -> Option<Result<(DataValue, LogicalType), DatabaseError>> {
    use std::str::FromStr;

    if text.contains(['e', 'E']) {
        return None;
    }
    let digits = text.trim_start_matches(['-', '+']);
    let (int_part, frac_part) = match digits.split_once('.') {
        Some((int_part, frac_part)) => (int_part, Some(frac_part)),
        None => (digits, None),
    };
    let int_digits = int_part.trim_start_matches('0').len();
    let scale = frac_part.map_or(0, str::len);
    if frac_part.is_none() {
        if text.parse::<i64>().is_ok() {
            return None;
        }
        if let Ok(value) = text.parse::<u64>() {
            return Some(Ok((DataValue::UInt64(value), LogicalType::UBigint)));
        }
    }
    if int_digits + scale > DECIMAL_VALUE_DIGITS {
        return None;
    }
    let normalized = format!(
        "{}{}.{}",
        if text.starts_with('-') { "-" } else { "" },
        if int_part.is_empty() { "0" } else { int_part },
        frac_part.unwrap_or("")
    );
    let normalized = normalized.trim_end_matches('.');
    let value = rust_decimal::Decimal::from_str(normalized).ok()?;
    let precision = (int_digits + scale).max(1) as u8;
    Some(Ok((
        DataValue::Decimal(value),
        LogicalType::Decimal(Some(precision), Some(scale as u8)),
    )))
}

/// MySQL dialect implementation for KiteSQL.
#[derive(Debug)]
pub struct MySqlDialect {
    parser: SqlParserMySqlDialect,
}

impl MySqlDialect {
    pub fn new() -> Self {
        Self {
            parser: SqlParserMySqlDialect {},
        }
    }
}

impl Default for MySqlDialect {
    fn default() -> Self {
        Self::new()
    }
}

impl SqlDialect for MySqlDialect {
    fn name(&self) -> &str {
        DIALECT_MYSQL
    }

    fn parser_dialect(&self) -> &dyn SqlParserDialect {
        &self.parser
    }

    fn rewrite(&self, stmt: &mut Statement) -> Result<(), DatabaseError> {
        if let Statement::CreateTable(create) = stmt {
            auto_increment::rewrite_create_table(create)?;
        }
        let _ = visit_expressions_mut(stmt, |expr| {
            associate_integer_divide(expr);
            ControlFlow::<()>::Continue(())
        });
        let mut err = None;
        let _ = visit_expressions_mut(stmt, |expr| {
            rewrite_integer_arithmetic(expr);
            if let Err(e) = rewrite_like(expr) {
                err = Some(e);
                return ControlFlow::Break(());
            }
            if let Expr::Cast {
                data_type, expr, ..
            } = expr
            {
                if let Err(e) = check_cast_type(data_type, expr) {
                    err = Some(e);
                    return ControlFlow::Break(());
                }
            }
            ControlFlow::Continue(())
        });
        err.map_or(Ok(()), Err)
    }

    /// MySQL's default sql_mode (STRICT_TRANS_TABLES, ERROR_FOR_DIVISION_BY_ZERO): queries
    /// and SET evaluate division by zero to NULL and convert a malformed numeric string by
    /// its leading numeric part, each with a warning; data-changing statements keep both as
    /// errors, as strict mode does.
    fn statement_policy(&self, stmt: &Statement) -> StatementPolicy {
        match stmt {
            Statement::Query(_) | Statement::Set(_) => StatementPolicy {
                division_by_zero: DivisionByZero::NullWithWarning,
                lenient_numeric_strings: true,
            },
            _ => StatementPolicy::default(),
        }
    }

    /// Without `NO_AUTO_VALUE_ON_ZERO` in sql_mode, MySQL generates the next
    /// `AUTO_INCREMENT` value for an explicit 0 as for NULL.
    fn identity_zero_generates(&self) -> bool {
        true
    }

    /// MySQL reads `0.1` as an exact DECIMAL (so `0.1 + 0.2 = 0.3`), keeps `1e2` a DOUBLE and
    /// types integers beyond BIGINT as BIGINT UNSIGNED, then DECIMAL.
    fn number_literal(
        &self,
        text: &str,
    ) -> Option<Result<(DataValue, LogicalType), DatabaseError>> {
        mysql_number_literal(text)
    }

    /// MySQL's exact-value arithmetic: `7 / 2` is DECIMAL 3.5000, `1.10 * 2.5` is 2.750.
    fn arithmetic_return_type(
        &self,
        op: &BinaryOperator,
        left: &LogicalType,
        right: &LogicalType,
    ) -> Option<LogicalType> {
        mysql_arithmetic_type(op, left, right)
    }

    /// MySQL types AVG of an exact-value argument (integer or DECIMAL) as DECIMAL with
    /// `div_precision_increment` (4) more digits of scale and precision than the argument;
    /// FLOAT, DOUBLE and string arguments keep the engine's DOUBLE.
    fn avg_return_type(&self, arg: &LogicalType) -> Option<LogicalType> {
        mysql_avg_type(arg)
    }

    /// MySQL's level, code and text for each core diagnostic.
    fn map_diagnostic(&self, mut diagnostic: Diagnostic) -> Option<Diagnostic> {
        let (level, code, message) = match &diagnostic.kind {
            DiagnosticKind::DivisionByZero => {
                (DiagnosticLevel::Warning, 1365, "Division by 0".to_string())
            }
            DiagnosticKind::TruncatedNumber { value, target } => (
                DiagnosticLevel::Warning,
                1292,
                format!("Truncated incorrect {target} value: '{value}'"),
            ),
            DiagnosticKind::DataTruncated { column, row } => (
                DiagnosticLevel::Note,
                1265,
                format!("Data truncated for column '{column}' at row {row}"),
            ),
            _ => return Some(diagnostic),
        };
        diagnostic.level = level;
        diagnostic.code = code;
        diagnostic.message = message;
        Some(diagnostic)
    }

    fn map_column_type(
        &self,
        data_type: &DataType,
    ) -> Option<Result<ColumnTypeSpec, DatabaseError>> {
        match data_type {
            DataType::Bit(Some(size)) => {
                if *size == 1 {
                    Some(Ok(
                        ColumnTypeSpec::new(LogicalType::Boolean).with_display("bit(1)")
                    ))
                } else {
                    Some(Ok(ColumnTypeSpec::new(LogicalType::UBigint)
                        .with_display(format!("bit({})", size))))
                }
            }
            DataType::TinyText => Some(Ok(ColumnTypeSpec::new(LogicalType::Varchar(
                None,
                CharLengthUnits::Characters,
            ))
            .with_display("tinytext"))),
            DataType::MediumText => Some(Ok(ColumnTypeSpec::new(LogicalType::Varchar(
                None,
                CharLengthUnits::Characters,
            ))
            .with_display("mediumtext"))),
            DataType::LongText => Some(Ok(ColumnTypeSpec::new(LogicalType::Varchar(
                None,
                CharLengthUnits::Characters,
            ))
            .with_display("longtext"))),
            DataType::Custom(name, _modifiers) if name.0.len() == 1 => {
                let type_name = match &name.0[0] {
                    ObjectNamePart::Identifier(ident) => &ident.value,
                    _ => {
                        return Some(Err(DatabaseError::UnsupportedStmt(format!(
                            "unsupported data type: {:?}",
                            data_type
                        ))))
                    }
                };
                let type_name_lower = type_name.to_lowercase();

                if type_name_lower.as_str() == "serial" {
                    // `rewrite` adds SERIAL's NOT NULL UNIQUE AUTO_INCREMENT
                    Some(Ok(
                        ColumnTypeSpec::new(LogicalType::UBigint).with_display("serial")
                    ))
                } else {
                    None
                }
            }
            _ => {
                if let Some(result) = self.map_data_type(data_type) {
                    let display_name = data_type.to_string().to_lowercase();
                    Some(
                        result
                            .map(|logical| ColumnTypeSpec::new(logical).with_display(display_name)),
                    )
                } else {
                    match LogicalType::try_from(data_type.clone()) {
                        Ok(logical) => {
                            let display_name = data_type.to_string().to_lowercase();
                            Some(Ok(ColumnTypeSpec::new(logical).with_display(display_name)))
                        }
                        Err(_) => None,
                    }
                }
            }
        }
    }
}

/// MySQL's largest FLOAT(p) precision, DECIMAL precision and DECIMAL scale.
const MAX_FLOAT_PRECISION: u64 = 53;
const MAX_DECIMAL_PRECISION: u64 = 65;
const MAX_DECIMAL_SCALE: u64 = 30;

/// MySQL `LIKE`: backslash is the default escape character (`'100\\_'` matches `100_`), and the
/// default collation (utf8mb4_0900_ai_ci) compares case-insensitively, so a plain `LIKE` becomes
/// the generic engine's `ILIKE`. Operands marked binary or case-sensitive (`BINARY x`,
/// `CAST(x AS BINARY)`, `COLLATE …_bin` / `…_cs` / `binary`) keep a case-sensitive `LIKE`.
/// Accent folding (`'a' LIKE 'À'`) and per-column collations are not modelled.
/// The SQL parser reads everything after `DIV` as its right operand (`7 DIV 2 + 1` as
/// `7 DIV (2 + 1)`); MySQL gives DIV the precedence of `*` and `/`, left-associative. Moves
/// the `DIV` down the right operand's left spine past every operator that binds no tighter
/// (parenthesised operands are `Expr::Nested` and stay whole).
fn associate_integer_divide(expr: &mut Expr) {
    use sqlparser::ast::BinaryOperator as Op;

    fn attach(left: Expr, right: Expr) -> Expr {
        match right {
            Expr::BinaryOp {
                left: right_left,
                op,
                right: right_right,
            } if matches!(
                op,
                Op::Multiply
                    | Op::Divide
                    | Op::Modulo
                    | Op::MyIntegerDivide
                    | Op::Plus
                    | Op::Minus
                    | Op::StringConcat
                    | Op::Eq
                    | Op::NotEq
                    | Op::Lt
                    | Op::LtEq
                    | Op::Gt
                    | Op::GtEq
                    | Op::Spaceship
                    | Op::And
                    | Op::Or
                    | Op::Xor
                    | Op::BitwiseAnd
                    | Op::BitwiseOr
                    | Op::PGBitwiseShiftLeft
                    | Op::PGBitwiseShiftRight
            ) =>
            {
                Expr::BinaryOp {
                    left: Box::new(attach(left, *right_left)),
                    op,
                    right: right_right,
                }
            }
            Expr::IsNull(inner) => Expr::IsNull(Box::new(attach(left, *inner))),
            Expr::IsNotNull(inner) => Expr::IsNotNull(Box::new(attach(left, *inner))),
            Expr::Between {
                expr: inner,
                negated,
                low,
                high,
            } => Expr::Between {
                expr: Box::new(attach(left, *inner)),
                negated,
                low,
                high,
            },
            Expr::InList {
                expr: inner,
                list,
                negated,
            } => Expr::InList {
                expr: Box::new(attach(left, *inner)),
                list,
                negated,
            },
            right => Expr::BinaryOp {
                left: Box::new(left),
                op: Op::MyIntegerDivide,
                right: Box::new(right),
            },
        }
    }

    if let Expr::BinaryOp {
        left,
        op: Op::MyIntegerDivide,
        right,
    } = expr
    {
        let left = std::mem::replace(left.as_mut(), Expr::value(Value::Null));
        let right = std::mem::replace(right.as_mut(), Expr::value(Value::Null));
        *expr = attach(left, right);
    }
}

/// `a DIV b` becomes the `div(a, b)` function (integer division, see
/// [`functions::IntegerDivide`]) and the `MOD(a, b)` function the `%` operator.
fn rewrite_integer_arithmetic(expr: &mut Expr) {
    use sqlparser::ast::{
        BinaryOperator as SqlBinaryOperator, Function, FunctionArg, FunctionArgExpr,
        FunctionArgumentList, FunctionArguments, Ident, ObjectName,
    };

    match expr {
        Expr::BinaryOp {
            left,
            op: SqlBinaryOperator::MyIntegerDivide,
            right,
        } => {
            let args = [left, right]
                .into_iter()
                .map(|e| {
                    FunctionArg::Unnamed(FunctionArgExpr::Expr(std::mem::replace(
                        e.as_mut(),
                        Expr::value(Value::Null),
                    )))
                })
                .collect();
            *expr = Expr::Function(Function {
                name: ObjectName::from(vec![Ident::new(functions::INTEGER_DIVIDE)]),
                uses_odbc_syntax: false,
                parameters: FunctionArguments::None,
                args: FunctionArguments::List(FunctionArgumentList {
                    duplicate_treatment: None,
                    args,
                    clauses: vec![],
                }),
                filter: None,
                null_treatment: None,
                over: None,
                within_group: vec![],
            });
        }
        Expr::Function(Function {
            name,
            args: FunctionArguments::List(list),
            filter: None,
            over: None,
            ..
        }) if name.0.len() == 1
            && matches!(&name.0[0], ObjectNamePart::Identifier(id) if id.value.eq_ignore_ascii_case("mod"))
            && list.duplicate_treatment.is_none()
            && list.clauses.is_empty()
            && list.args.len() == 2
            && list
                .args
                .iter()
                .all(|a| matches!(a, FunctionArg::Unnamed(FunctionArgExpr::Expr(_)))) =>
        {
            let mut operands: Vec<Box<Expr>> = std::mem::take(&mut list.args)
                .into_iter()
                .filter_map(|a| match a {
                    FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) => Some(Box::new(e)),
                    _ => None,
                })
                .collect();
            let (right, left) = (operands.pop().unwrap(), operands.pop().unwrap());
            *expr = Expr::Nested(Box::new(Expr::BinaryOp {
                left,
                op: SqlBinaryOperator::Modulo,
                right,
            }));
        }
        _ => {}
    }
}

fn rewrite_like(expr: &mut Expr) -> Result<(), DatabaseError> {
    let Expr::Like {
        negated,
        any,
        expr: lhs,
        pattern,
        escape_char,
    } = expr
    else {
        return Ok(());
    };
    // two different explicit collations: MySQL error 1267
    if let (Expr::Collate { collation: l, .. }, Expr::Collate { collation: r, .. }) =
        (lhs.as_ref(), pattern.as_ref())
    {
        let (l, r) = (l.to_string(), r.to_string());
        if !l.eq_ignore_ascii_case(&r) {
            return Err(DatabaseError::InvalidValue(format!(
                "Illegal mix of collations ({l},EXPLICIT) and ({r},EXPLICIT) for operation 'like'"
            )));
        }
    }
    if escape_char.is_none() {
        *escape_char = Some(Value::SingleQuotedString("\\".to_string()));
    }
    let case_sensitive = is_case_sensitive(lhs) || is_case_sensitive(pattern);
    // the engine has no COLLATE; its only effect on LIKE is the case sensitivity decided here
    for operand in [&mut *lhs, &mut *pattern] {
        if let Expr::Collate { expr: inner, .. } = operand.as_mut() {
            *operand = inner.clone();
        }
    }
    if case_sensitive {
        return Ok(());
    }
    *expr = Expr::ILike {
        negated: *negated,
        any: *any,
        expr: lhs.clone(),
        pattern: pattern.clone(),
        escape_char: escape_char.clone(),
    };
    Ok(())
}

fn is_case_sensitive(expr: &Expr) -> bool {
    match expr {
        Expr::Collate { collation, .. } => {
            let name = collation.to_string().to_ascii_lowercase();
            name.ends_with("_bin") || name.ends_with("_cs") || name == "binary"
        }
        Expr::Cast { data_type, .. } => matches!(
            data_type,
            DataType::Binary(_) | DataType::Varbinary(_) | DataType::Blob(_)
        ),
        Expr::Nested(inner) => is_case_sensitive(inner),
        _ => false,
    }
}

/// The target types MySQL's grammar and limits allow in `CAST(expr AS type)`:
/// `FLOAT(p)` with p <= 53 (no `FLOAT(M,D)`), plain `DOUBLE` / `REAL`, and
/// `DECIMAL(M,D)` with D <= M <= 65 and D <= 30. The messages are MySQL's.
fn check_cast_type(data_type: &DataType, expr: &Expr) -> Result<(), DatabaseError> {
    let syntax = |near: String| {
        Err(DatabaseError::InvalidValue(format!(
            "You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near '{near}'"
        )))
    };
    match data_type {
        DataType::Float(ExactNumberInfo::Precision(p)) if *p > MAX_FLOAT_PRECISION => {
            Err(DatabaseError::InvalidValue(format!(
                "Too-big precision {p} specified for 'CAST'. Maximum is {MAX_FLOAT_PRECISION}."
            )))
        }
        DataType::Float(info @ ExactNumberInfo::PrecisionAndScale(..))
        | DataType::Double(
            info @ (ExactNumberInfo::Precision(_) | ExactNumberInfo::PrecisionAndScale(..)),
        ) => syntax(info.to_string()),
        DataType::Decimal(ExactNumberInfo::PrecisionAndScale(p, s))
        | DataType::Numeric(ExactNumberInfo::PrecisionAndScale(p, s))
        | DataType::Dec(ExactNumberInfo::PrecisionAndScale(p, s)) => {
            let (p, s) = (*p, *s as u64);
            if p > MAX_DECIMAL_PRECISION {
                Err(DatabaseError::InvalidValue(format!(
                    "Too-big precision {p} specified for '{expr}'. Maximum is {MAX_DECIMAL_PRECISION}."
                )))
            } else if s > MAX_DECIMAL_SCALE {
                Err(DatabaseError::InvalidValue(format!(
                    "Too big scale {s} specified for '{expr}'. Maximum is {MAX_DECIMAL_SCALE}."
                )))
            } else if s > p {
                Err(DatabaseError::InvalidValue(format!(
                    "For float(M,D), double(M,D) or decimal(M,D), M must be >= D (column '{expr}')."
                )))
            } else {
                Ok(())
            }
        }
        DataType::Decimal(ExactNumberInfo::Precision(p))
        | DataType::Numeric(ExactNumberInfo::Precision(p))
        | DataType::Dec(ExactNumberInfo::Precision(p))
            if *p > MAX_DECIMAL_PRECISION =>
        {
            Err(DatabaseError::InvalidValue(format!(
                "Too-big precision {p} specified for '{expr}'. Maximum is {MAX_DECIMAL_PRECISION}."
            )))
        }
        _ => Ok(()),
    }
}

/// Extension trait for `DataBaseBuilder` to add MySQL support.
pub trait MySqlBuilderExt {
    /// Registers the MySQL dialect and sets it as the default.
    ///
    /// # Example
    ///
    /// ```rust
    /// use kite_sql::db::DataBaseBuilder;
    /// use kite_sql_mysql::MySqlBuilderExt;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut database = DataBaseBuilder::path(".")
    ///     .with_mysql()?
    ///     .build_in_memory()?;
    ///
    /// database.ddl("CREATE TABLE users (id INT PRIMARY KEY, name VARCHAR(50))")?;
    /// # Ok(())
    /// # }
    /// ```
    fn with_mysql(self) -> Result<Self, DatabaseError>
    where
        Self: Sized;
}

impl MySqlBuilderExt for kite_sql::db::DataBaseBuilder {
    fn with_mysql(self) -> Result<Self, DatabaseError> {
        let mysql_dialect = Arc::new(MySqlDialect::new());
        self.register_dialect(mysql_dialect)
            .default_dialect(DIALECT_MYSQL)
            .map(|builder| builder.with_function_pack(Arc::new(MySqlFunctions)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avg_type_adds_div_precision_increment() {
        let decimal = |p, s| Some(LogicalType::Decimal(Some(p), Some(s)));
        // Types as MySQL 8.0 reports them for CREATE TABLE ... AS SELECT AVG(col).
        assert_eq!(mysql_avg_type(&LogicalType::Boolean), decimal(7, 4));
        assert_eq!(mysql_avg_type(&LogicalType::Tinyint), decimal(7, 4));
        assert_eq!(mysql_avg_type(&LogicalType::Smallint), decimal(9, 4));
        assert_eq!(mysql_avg_type(&LogicalType::Integer), decimal(14, 4));
        assert_eq!(mysql_avg_type(&LogicalType::Bigint), decimal(23, 4));
        assert_eq!(mysql_avg_type(&LogicalType::UBigint), decimal(24, 4));
        assert_eq!(
            mysql_avg_type(&LogicalType::Decimal(Some(10), Some(2))),
            decimal(14, 6)
        );
        assert_eq!(
            mysql_avg_type(&LogicalType::Decimal(Some(5), None)),
            decimal(9, 4)
        );
        assert_eq!(
            mysql_avg_type(&LogicalType::Decimal(Some(30), Some(28))),
            decimal(34, 30)
        );
        assert_eq!(
            mysql_avg_type(&LogicalType::Decimal(Some(65), Some(30))),
            decimal(65, 30)
        );
        assert_eq!(mysql_avg_type(&LogicalType::Double), None);
        assert_eq!(mysql_avg_type(&LogicalType::Float), None);
        assert_eq!(
            mysql_avg_type(&LogicalType::Varchar(None, CharLengthUnits::Characters)),
            None
        );
    }

    #[test]
    fn arithmetic_type_follows_mysql() {
        let decimal = |p, s| LogicalType::Decimal(Some(p), Some(s));
        let int = LogicalType::Integer;
        let (d, e) = (decimal(10, 2), decimal(8, 3));
        let ty = |op, l: &LogicalType, r: &LogicalType| mysql_arithmetic_type(&op, l, r);
        // Types as MySQL 8.0 reports them for CREATE TABLE ... AS SELECT with an INT column
        // i, a DECIMAL(10,2) column d and a DECIMAL(8,3) column e.
        assert_eq!(ty(BinaryOperator::Divide, &int, &int), Some(decimal(14, 4)));
        assert_eq!(ty(BinaryOperator::Multiply, &d, &e), Some(decimal(18, 5)));
        assert_eq!(ty(BinaryOperator::Plus, &d, &int), Some(decimal(13, 2)));
        assert_eq!(ty(BinaryOperator::Modulo, &d, &e), Some(decimal(10, 3)));
        assert_eq!(ty(BinaryOperator::Divide, &d, &e), Some(decimal(17, 6)));
        assert_eq!(ty(BinaryOperator::Minus, &d, &e), Some(decimal(12, 3)));
        assert_eq!(ty(BinaryOperator::Modulo, &int, &d), Some(decimal(10, 2)));
        // Integer + - * % and anything involving DOUBLE keep the engine's types.
        assert_eq!(ty(BinaryOperator::Plus, &int, &int), None);
        assert_eq!(ty(BinaryOperator::Modulo, &int, &int), None);
        assert_eq!(ty(BinaryOperator::Divide, &d, &LogicalType::Double), None);
        assert_eq!(
            ty(
                BinaryOperator::Multiply,
                &LogicalType::Decimal(None, None),
                &d
            ),
            Some(LogicalType::Decimal(None, None))
        );
        assert_eq!(
            ty(BinaryOperator::Multiply, &decimal(65, 30), &decimal(65, 30)),
            Some(decimal(65, 30))
        );
    }

    #[test]
    fn number_literals_follow_mysql() {
        let literal = |text: &str| mysql_number_literal(text).map(|r| r.unwrap());
        let decimal = |v: &str, p, s| {
            Some((
                DataValue::Decimal(v.parse().unwrap()),
                LogicalType::Decimal(Some(p), Some(s)),
            ))
        };
        assert_eq!(literal("0.1"), decimal("0.1", 1, 1));
        assert_eq!(literal(".5"), decimal("0.5", 1, 1));
        assert_eq!(literal("5."), decimal("5", 1, 0));
        assert_eq!(literal("00012.340"), decimal("12.340", 5, 3));
        assert_eq!(literal("-1.50"), decimal("-1.50", 3, 2));
        assert_eq!(literal("42"), None);
        assert_eq!(literal("-9223372036854775808"), None);
        assert_eq!(
            literal("18446744073709551615"),
            Some((DataValue::UInt64(u64::MAX), LogicalType::UBigint))
        );
        assert_eq!(literal("1e2"), None);
        assert_eq!(literal("1.5E-3"), None);
        assert_eq!(literal("1.00000000000000000000000000001"), None);
    }
}
