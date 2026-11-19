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

pub mod functions;
pub use functions::MySqlFunctions;

use kite_sql::diagnostics::{
    Diagnostic, DiagnosticKind, DiagnosticLevel, DivisionByZero, StatementPolicy,
};
use kite_sql::dialect::{ColumnTypeSpec, SqlDialect};
use kite_sql::errors::DatabaseError;
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
        let mut err = None;
        let _ = visit_expressions_mut(stmt, |expr| {
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
                    // TODO: serial should also apply NOT NULL UNIQUE AUTO_INCREMENT constraints
                    // Currently only the type mapping (BIGINT UNSIGNED) is implemented
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
