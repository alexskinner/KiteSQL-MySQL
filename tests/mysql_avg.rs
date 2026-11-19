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

//! MySQL result types of AVG: DECIMAL for exact-value arguments, DOUBLE otherwise.

use kite_sql::db::DataBaseBuilder;
use kite_sql::errors::DatabaseError;
use kite_sql::types::value::DataValue;
use kite_sql::types::LogicalType;
use kite_sql_mysql::MySqlBuilderExt;
use tempfile::TempDir;

#[test]
fn test_mysql_avg_result_types_and_scale() -> Result<(), DatabaseError> {
    let temp_dir = TempDir::new().expect("unable to create temporary working directory");
    let mut database = DataBaseBuilder::path(temp_dir.path())
        .with_mysql()?
        .build_lmdb()?;
    database
        .ddl("CREATE TABLE t (id INT PRIMARY KEY, n INT, d DECIMAL(10,2), f DOUBLE, bi BIGINT)")?;
    database
        .run("INSERT INTO t VALUES (1, 1, 1.25, 1.5, 7), (2, 2, 2.50, 2.0, 8), (3, 2, NULL, NULL, NULL)")?
        .done()?;
    let avg = |sql: &str| -> Result<(LogicalType, String), DatabaseError> {
        let mut result = database.run(sql)?;
        let ty = result.schema(|schema| schema.get(0).unwrap().datatype().clone());
        let value = result.next_tuple(|_, row| row[0].to_string())?.unwrap();
        result.done()?;
        Ok((ty, value))
    };
    let decimal = |p, s| LogicalType::Decimal(Some(p), Some(s));

    assert_eq!(
        avg("SELECT AVG(n) FROM t")?,
        (decimal(14, 4), "1.6667".to_string())
    );
    assert_eq!(
        avg("SELECT AVG(d) FROM t")?,
        (decimal(14, 6), "1.875000".to_string())
    );
    assert_eq!(
        avg("SELECT AVG(bi) FROM t")?,
        (decimal(23, 4), "7.5000".to_string())
    );
    assert_eq!(
        avg("SELECT AVG(DISTINCT n) FROM t")?,
        (decimal(14, 4), "1.5000".to_string())
    );
    assert_eq!(
        avg("SELECT AVG(f) FROM t")?,
        (LogicalType::Double, "1.75".to_string())
    );

    let mut result = database.run("SELECT AVG(n) FROM t WHERE id > 10")?;
    let value = result.next_tuple(|_, row| row[0].clone())?.unwrap();
    result.done()?;
    assert_eq!(value, DataValue::Null);
    Ok(())
}
