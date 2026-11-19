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

//! MySQL warning codes for the generic statement diagnostics channel.

use kite_sql::db::DataBaseBuilder;
use kite_sql::errors::DatabaseError;
use kite_sql::types::value::DataValue;
use kite_sql_mysql::MySqlBuilderExt;
use tempfile::TempDir;

type Warnings = Vec<(kite_sql::diagnostics::DiagnosticLevel, u32, String)>;

#[test]
fn test_mysql_warnings_codes_and_strict_writes() -> Result<(), DatabaseError> {
    use kite_sql::diagnostics::DiagnosticLevel;
    let temp_dir = TempDir::new().expect("unable to create temporary working directory");
    let mut database = DataBaseBuilder::path(temp_dir.path())
        .with_mysql()?
        .build_lmdb()?;
    database.ddl("CREATE TABLE t (id INT PRIMARY KEY, d DECIMAL(4, 2), n INT)")?;
    let scalar = |sql: &str| -> Result<(DataValue, Warnings), DatabaseError> {
        let mut result = database.run(sql)?;
        let row = result.next_tuple(|_, row| row[0].clone())?.unwrap();
        result.done()?;
        let warnings = database
            .take_diagnostics()
            .into_iter()
            .map(|d| (d.level, d.code, d.message))
            .collect();
        Ok((row, warnings))
    };
    let w = |code: u32, msg: &str| (DiagnosticLevel::Warning, code, msg.to_string());

    assert_eq!(
        scalar("SELECT 1 / 0")?,
        (DataValue::Null, vec![w(1365, "Division by 0")])
    );
    assert_eq!(scalar("SELECT 5 % 0")?.1, vec![w(1365, "Division by 0")]);
    assert_eq!(
        scalar("SELECT CAST('10x' AS UNSIGNED)")?,
        (
            DataValue::UInt32(10),
            vec![w(1292, "Truncated incorrect INTEGER value: '10x'")]
        )
    );
    assert_eq!(
        scalar("SELECT CAST('10 ' AS UNSIGNED)")?,
        (DataValue::UInt32(10), vec![])
    );
    assert_eq!(
        scalar("SELECT CAST('a' AS DECIMAL)")?.1,
        vec![w(1292, "Truncated incorrect DECIMAL value: 'a'")]
    );
    assert_eq!(
        scalar("SELECT CAST('1.5e1x' AS DOUBLE)")?,
        (
            DataValue::Float64(15.0.into()),
            vec![w(1292, "Truncated incorrect DOUBLE value: '1.5e1x'")]
        )
    );

    // strict mode: writes keep the errors
    assert!(database
        .run("INSERT INTO t VALUES (1, 1, 1 / 0)")
        .and_then(|r| r.done())
        .is_err());
    assert!(database
        .run("INSERT INTO t VALUES (1, 1, '10x')")
        .and_then(|r| r.done())
        .is_err());
    // DECIMAL scale rounding is a Note 1265 per row, and the row is stored
    database
        .run("INSERT INTO t VALUES (1, 1.000001, 0), (2, 1.5, 0)")?
        .done()?;
    let notes: Vec<_> = database
        .take_diagnostics()
        .into_iter()
        .map(|d| (d.level, d.code, d.message))
        .collect();
    assert_eq!(
        notes,
        vec![(
            DiagnosticLevel::Note,
            1265,
            "Data truncated for column 'd' at row 1".to_string()
        )]
    );
    Ok(())
}
