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

//! Tests for MySQL dialect functionality.

use kite_sql::db::DataBaseBuilder;
use kite_sql::errors::DatabaseError;
use kite_sql::storage::lmdb::LmdbStorage;
use kite_sql::types::value::DataValue;
use kite_sql_mysql::MySqlBuilderExt;
use tempfile::TempDir;

#[test]
fn test_mysql_backtick_identifiers() -> Result<(), DatabaseError> {
    let temp_dir = TempDir::new().expect("unable to create temporary working directory");
    let storage = LmdbStorage::new(temp_dir.path())?;
    let mut database_mysql = DataBaseBuilder::path(".")
        .with_mysql()?
        .build_with_storage(storage)?;

    database_mysql.ddl("CREATE TABLE `my_table` (`id` INT PRIMARY KEY, `name` VARCHAR(50))")?;
    database_mysql
        .run("INSERT INTO `my_table` VALUES (1, 'test')")?
        .done()?;

    let mut result = database_mysql.run("SELECT `id`, `name` FROM `my_table`")?;
    let row = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(row[0], DataValue::Int32(1));
    assert_eq!(row[1], DataValue::from("test".to_string()));
    result.done()?;

    Ok(())
}

#[test]
fn test_mysql_limit_offset_comma_syntax() -> Result<(), DatabaseError> {
    let temp_dir = TempDir::new().expect("unable to create temporary working directory");
    let storage = LmdbStorage::new(temp_dir.path())?;
    let mut database_mysql = DataBaseBuilder::path(".")
        .with_mysql()?
        .build_with_storage(storage)?;

    database_mysql.ddl("CREATE TABLE test_table (id INT PRIMARY KEY, value INT)")?;
    for i in 1..=10 {
        database_mysql
            .run(format!("INSERT INTO test_table VALUES ({}, {})", i, i * 10))?
            .done()?;
    }

    let mut result = database_mysql.run("SELECT id FROM test_table ORDER BY id LIMIT 2, 3")?;
    let row1 = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(row1[0], DataValue::Int32(3));
    let row2 = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(row2[0], DataValue::Int32(4));
    let row3 = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(row3[0], DataValue::Int32(5));
    assert!(result.next_tuple(|_, _| ())?.is_none());
    result.done()?;

    Ok(())
}

#[test]
fn test_mysql_run_statement_with_preparsed() -> Result<(), DatabaseError> {
    use kite_sql::parser::parse_sql_with_dialect;
    use sqlparser::dialect::MySqlDialect;

    let temp_dir = TempDir::new().expect("unable to create temporary working directory");
    let storage = LmdbStorage::new(temp_dir.path())?;
    let mut database = DataBaseBuilder::path(".")
        .with_mysql()?
        .build_with_storage(storage)?;

    database.ddl("CREATE TABLE `test_preparsed` (`id` INT PRIMARY KEY, `val` INT)")?;
    database
        .run("INSERT INTO `test_preparsed` VALUES (1, 100), (2, 200)")?
        .done()?;

    let dialect = MySqlDialect {};
    let statements = parse_sql_with_dialect(
        "SELECT `id`, `val` FROM `test_preparsed` WHERE `id` = 1",
        &dialect,
    )?;
    let statement = statements.into_iter().next().unwrap();

    let mut result = database.run_statement(statement)?;
    let row = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(row[0], DataValue::Int32(1));
    assert_eq!(row[1], DataValue::Int32(100));
    result.done()?;

    Ok(())
}

#[test]
fn test_mysql_run_mut_statement_with_ddl() -> Result<(), DatabaseError> {
    use kite_sql::parser::parse_sql_with_dialect;
    use sqlparser::dialect::MySqlDialect;

    let temp_dir = TempDir::new().expect("unable to create temporary working directory");
    let storage = LmdbStorage::new(temp_dir.path())?;
    let mut database = DataBaseBuilder::path(".")
        .with_mysql()?
        .build_with_storage(storage)?;

    let dialect = MySqlDialect {};
    let statements =
        parse_sql_with_dialect("CREATE TABLE `ddl_test` (`id` INT PRIMARY KEY)", &dialect)?;
    let statement = statements.into_iter().next().unwrap();

    database.run_mut_statement(statement, ())?;

    database.run("INSERT INTO `ddl_test` VALUES (1)")?.done()?;

    let mut result = database.run("SELECT `id` FROM `ddl_test`")?;
    let row = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(row[0], DataValue::Int32(1));
    result.done()?;

    Ok(())
}
