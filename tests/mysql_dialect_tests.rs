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
fn test_mysql_dialect_selection() -> Result<(), DatabaseError> {
    let temp_dir = TempDir::new().expect("unable to create temporary working directory");
    let storage = LmdbStorage::new(temp_dir.path())?;

    // Test that .with_mysql() successfully registers the dialect
    let builder = DataBaseBuilder::path(".").with_mysql()?;
    let mut database = builder.build_with_storage(storage)?;

    // MySQL dialect should accept backtick identifiers
    database.ddl("CREATE TABLE `test` (`id` INT PRIMARY KEY)")?;
    database.run("INSERT INTO `test` VALUES (1)")?.done()?;

    let mut result = database.run("SELECT `id` FROM `test`")?;
    let row = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(row[0], DataValue::Int32(1));
    result.done()?;

    Ok(())
}

#[test]
fn test_mysql_comment_syntax() -> Result<(), DatabaseError> {
    let temp_dir = TempDir::new().expect("unable to create temporary working directory");
    let storage = LmdbStorage::new(temp_dir.path())?;

    let database_mysql = DataBaseBuilder::path(".")
        .with_mysql()?
        .build_with_storage(storage)?;

    // MySQL dialect should accept `#` comments
    let mut result = database_mysql.run("SELECT 1 # MySQL comment")?;
    let row = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(row[0], DataValue::Int32(1));
    result.done()?;

    Ok(())
}

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

#[test]
fn test_mysql_display_types() -> Result<(), DatabaseError> {
    let temp_dir = TempDir::new().expect("unable to create temporary working directory");
    let storage = LmdbStorage::new(temp_dir.path())?;
    let mut database = DataBaseBuilder::path(".")
        .with_mysql()?
        .build_with_storage(storage)?;

    database.ddl(
        "CREATE TABLE type_test (
            id INT PRIMARY KEY,
            tiny_text TINYTEXT,
            medium_text MEDIUMTEXT,
            long_text LONGTEXT,
            serial_col SERIAL,
            bool_bit BIT(1)
        )",
    )?;

    let mut result = database.run("DESCRIBE type_test")?;

    let id_row = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(id_row[0], DataValue::from("id".to_string()));
    assert_eq!(id_row[1], DataValue::from("int".to_string()));

    let tiny_text_row = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(tiny_text_row[0], DataValue::from("tiny_text".to_string()));
    assert_eq!(tiny_text_row[1], DataValue::from("tinytext".to_string()));

    let medium_text_row = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(
        medium_text_row[0],
        DataValue::from("medium_text".to_string())
    );
    assert_eq!(
        medium_text_row[1],
        DataValue::from("mediumtext".to_string())
    );

    let long_text_row = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(long_text_row[0], DataValue::from("long_text".to_string()));
    assert_eq!(long_text_row[1], DataValue::from("longtext".to_string()));

    let serial_row = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(serial_row[0], DataValue::from("serial_col".to_string()));
    assert_eq!(serial_row[1], DataValue::from("serial".to_string()));

    let bool_bit_row = result.next_tuple(|_, row| row.clone())?.unwrap();
    assert_eq!(bool_bit_row[0], DataValue::from("bool_bit".to_string()));
    assert_eq!(bool_bit_row[1], DataValue::from("bit(1)".to_string()));

    result.done()?;

    Ok(())
}

#[test]
fn test_mysql_alter_display_types() -> Result<(), DatabaseError> {
    let temp_dir = TempDir::new().expect("unable to create temporary working directory");
    let storage = LmdbStorage::new(temp_dir.path())?;

    let mut database = DataBaseBuilder::path(".")
        .with_mysql()?
        .build_with_storage(storage)?;

    // Create a table with various MySQL display types
    database.ddl(
        "CREATE TABLE test_alter (
            id INT PRIMARY KEY,
            a BIT(1),
            b BIT(8),
            c TINYINT(1),
            d MEDIUMTEXT,
            e LONGTEXT,
            f SERIAL,
            g INT UNSIGNED,
            h DECIMAL(10,2)
        )",
    )?;

    // Helper to check a column's display type
    let check_display = |db: &mut kite_sql::db::Database<LmdbStorage>,
                         col: &str,
                         expected: &str|
     -> Result<(), DatabaseError> {
        let mut result = db.run("DESCRIBE test_alter")?;
        while let Some(row) = result.next_tuple(|_, row| row.clone())? {
            let col_name = row[0].to_string();
            if col_name == col {
                let dtype = row[1].to_string();
                assert_eq!(
                    dtype, expected,
                    "Column {} should be {}, got {}",
                    col, expected, dtype
                );
                result.done()?;
                return Ok(());
            }
        }
        result.done()?;
        panic!("Column {} not found in DESCRIBE", col);
    };

    // Check initial display types
    check_display(&mut database, "a", "bit(1)")?;
    check_display(&mut database, "b", "bit(8)")?;
    check_display(&mut database, "c", "tinyint(1)")?;
    check_display(&mut database, "d", "mediumtext")?;
    check_display(&mut database, "e", "longtext")?;
    check_display(&mut database, "f", "serial")?;
    check_display(&mut database, "g", "int unsigned")?;
    check_display(&mut database, "h", "decimal(10,2)")?;

    // Test ADD COLUMN
    database.ddl("ALTER TABLE test_alter ADD COLUMN i TINYTEXT")?;
    check_display(&mut database, "i", "tinytext")?;

    // Test DROP COLUMN
    database.ddl("ALTER TABLE test_alter DROP COLUMN i")?;

    // Test MODIFY COLUMN (change type to one with display mapping)
    database.ddl("ALTER TABLE test_alter MODIFY COLUMN d TINYTEXT")?;
    check_display(&mut database, "d", "tinytext")?;

    // Test MODIFY COLUMN (change type to one without display mapping)
    database.ddl("ALTER TABLE test_alter MODIFY COLUMN d VARCHAR(100)")?;
    check_display(&mut database, "d", "varchar(100)")?;

    // Test CHANGE COLUMN with rename
    database.ddl("ALTER TABLE test_alter CHANGE COLUMN e e2 LONGTEXT")?;
    check_display(&mut database, "e2", "longtext")?;

    // Test RENAME COLUMN (should preserve display type)
    database.ddl("ALTER TABLE test_alter RENAME COLUMN f TO f2")?;
    check_display(&mut database, "f2", "serial")?;

    // Test SET DEFAULT (should preserve display type)
    database.ddl("ALTER TABLE test_alter ALTER COLUMN g SET DEFAULT 42")?;
    check_display(&mut database, "g", "int unsigned")?;

    // Test DROP DEFAULT (should preserve display type)
    database.ddl("ALTER TABLE test_alter ALTER COLUMN g DROP DEFAULT")?;
    check_display(&mut database, "g", "int unsigned")?;

    // Test SET NOT NULL (should preserve display type)
    database.ddl("ALTER TABLE test_alter ALTER COLUMN h SET NOT NULL")?;
    check_display(&mut database, "h", "decimal(10,2)")?;

    // Test DROP NOT NULL (should preserve display type)
    database.ddl("ALTER TABLE test_alter ALTER COLUMN h DROP NOT NULL")?;
    check_display(&mut database, "h", "decimal(10,2)")?;

    // Drop database and reopen to test persistence
    drop(database);

    let storage = LmdbStorage::new(temp_dir.path())?;
    let mut database = DataBaseBuilder::path(".")
        .with_mysql()?
        .build_with_storage(storage)?;
    database.load(kite_sql::db::CatalogKind::Table("test_alter".into()))?;

    // Check display types after reopening
    check_display(&mut database, "a", "bit(1)")?;
    check_display(&mut database, "b", "bit(8)")?;
    check_display(&mut database, "c", "tinyint(1)")?;
    check_display(&mut database, "d", "varchar(100)")?;
    check_display(&mut database, "e2", "longtext")?;
    check_display(&mut database, "f2", "serial")?;
    check_display(&mut database, "g", "int unsigned")?;
    check_display(&mut database, "h", "decimal(10,2)")?;

    Ok(())
}

#[test]
fn test_mysql_cast_target_type_limits() -> Result<(), DatabaseError> {
    let temp_dir = TempDir::new().expect("unable to create temporary working directory");
    let storage = LmdbStorage::new(temp_dir.path())?;
    let mut database = DataBaseBuilder::path(".")
        .with_mysql()?
        .build_with_storage(storage)?;

    for (sql, message) in [
        (
            "SELECT CAST(1/3 AS FLOAT(54))",
            "Too-big precision 54 specified for 'CAST'. Maximum is 53.",
        ),
        ("SELECT CAST(1/3 AS DOUBLE(52))", "near '(52)'"),
        ("SELECT CAST(999.00009 AS FLOAT(7,4))", "near '(7,4)'"),
        ("SELECT CAST(999.00009 AS DOUBLE(7,4))", "near '(7,4)'"),
        (
            "SELECT CAST(12.1 AS DECIMAL(3, 4))",
            "M must be >= D (column '12.1')",
        ),
        (
            "SELECT CAST(12.1 AS DECIMAL(66, 2))",
            "Too-big precision 66 specified for '12.1'. Maximum is 65.",
        ),
        (
            "SELECT CAST(1 AS DECIMAL(40, 31))",
            "Too big scale 31 specified for '1'. Maximum is 30.",
        ),
        (
            "SELECT 1 FROM (SELECT CAST(1 AS FLOAT(60)) AS x) AS t",
            "Too-big precision 60",
        ),
    ] {
        let err = database
            .run(sql)
            .and_then(|iter| iter.done())
            .expect_err(sql);
        assert!(err.to_string().contains(message), "{sql}: {err}");
    }

    let mut result = database.run("SELECT CAST(1.5 AS FLOAT(53)), CAST(12.1 AS DECIMAL(5, 2))")?;
    let row = result.next_tuple(|_, row| row.clone())?.unwrap();
    // "12.1" or "12.10" depending on whether the engine pads DECIMAL scale
    assert!(row[1].to_string().starts_with("12.1"), "{}", row[1]);
    result.done()?;

    database.ddl("CREATE TABLE t_cast_ddl (id INT PRIMARY KEY, f FLOAT(7,4), d DOUBLE(7,4))")?;

    Ok(())
}
