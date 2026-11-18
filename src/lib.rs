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

use kite_sql::dialect::SqlDialect;
use kite_sql::errors::DatabaseError;
use kite_sql::types::{CharLengthUnits, LogicalType};
use sqlparser::ast::{DataType, ObjectNamePart};
use sqlparser::dialect::{Dialect as SqlParserDialect, MySqlDialect as SqlParserMySqlDialect};
use std::fmt::Debug;
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

    fn map_data_type(&self, data_type: &DataType) -> Option<Result<LogicalType, DatabaseError>> {
        match data_type {
            DataType::Bit(Some(size)) => {
                if *size == 1 {
                    Some(Ok(LogicalType::Boolean))
                } else {
                    Some(Ok(LogicalType::UBigint))
                }
            }
            DataType::TinyText => Some(Ok(LogicalType::Varchar(None, CharLengthUnits::Characters))),
            DataType::MediumText => {
                Some(Ok(LogicalType::Varchar(None, CharLengthUnits::Characters)))
            }
            DataType::LongText => Some(Ok(LogicalType::Varchar(None, CharLengthUnits::Characters))),
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
                    Some(Ok(LogicalType::UBigint))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Extension trait for `DataBaseBuilder` to add MySQL support.
pub trait MySqlBuilderExt {
    /// Registers the MySQL dialect and sets it as the default.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use kite_sql::db::DataBaseBuilder;
    /// use kite_sql_mysql::MySqlBuilderExt;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let database = DataBaseBuilder::path(".")
    ///     .with_mysql()?
    ///     .build_in_memory()?;
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
    }
}
