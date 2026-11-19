// Tests for the MySQL function fixes

use kite_sql::db::{DataBaseBuilder, Database};
use kite_sql::errors::DatabaseError;
use kite_sql::storage::lmdb::LmdbStorage;
use kite_sql::types::value::DataValue;
use kite_sql_mysql::MySqlBuilderExt;
use tempfile::TempDir;

fn database(dir: &TempDir) -> Result<Database<LmdbStorage>, DatabaseError> {
    let storage = LmdbStorage::new(dir.path())?;
    DataBaseBuilder::path(".")
        .with_mysql()?
        .build_with_storage(storage)
}

fn scalar(db: &Database<LmdbStorage>, sql: &str) -> Result<DataValue, DatabaseError> {
    let mut iter = db.run(sql)?;
    let mut rows = Vec::new();
    while let Some(row) = iter.next_tuple(|_, row| row.take_values())? {
        rows.push(row);
    }
    iter.done()?;
    Ok(rows.remove(0).remove(0))
}

#[test]
fn locate_multibyte_crash_fix() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    // This used to panic with "byte index 2 is not a char boundary"
    assert_eq!(
        scalar(&db, "select locate('l', 'héllo', 3)")?,
        DataValue::Int32(3)
    );
    assert_eq!(
        scalar(&db, "select locate('o', 'héllo', 4)")?,
        DataValue::Int32(5)
    );
    Ok(())
}

#[test]
fn locate_case_insensitive() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    // MySQL's default collation is case-insensitive
    assert_eq!(
        scalar(&db, "select locate('BAR', 'foobarbar')")?,
        DataValue::Int32(4)
    );
    assert_eq!(
        scalar(&db, "select instr('foobarbar', 'BAR')")?,
        DataValue::Int32(4)
    );
    Ok(())
}

#[test]
fn locate_character_positions_not_bytes() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    // Character position (3), not byte position (4)
    assert_eq!(
        scalar(&db, "select locate('l', 'héllo')")?,
        DataValue::Int32(3)
    );
    assert_eq!(
        scalar(&db, "select instr('héllo', 'l')")?,
        DataValue::Int32(3)
    );
    Ok(())
}

#[test]
fn round_with_decimal() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let mut db = database(&dir)?;

    db.ddl("create table t (id int primary key, val decimal(10,3))")?;
    db.run("insert into t values (1, 1.005), (2, 1.234), (3, 1.567)")?
        .done()?;

    // MySQL rounds 1.005 to 1.01 with half-away-from-zero
    let rows = db.run("select id, round(val, 2) from t order by id")?;
    let mut results = Vec::new();
    let mut iter = rows;
    while let Some(row) = iter.next_tuple(|_, row| row.take_values())? {
        results.push(row);
    }
    iter.done()?;

    use rust_decimal::Decimal;
    assert_eq!(results[0][1], DataValue::Decimal(Decimal::new(101, 2))); // 1.01
    assert_eq!(results[1][1], DataValue::Decimal(Decimal::new(123, 2))); // 1.23
    assert_eq!(results[2][1], DataValue::Decimal(Decimal::new(157, 2))); // 1.57

    Ok(())
}

#[test]
fn round_with_integer() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let mut db = database(&dir)?;

    db.ddl("create table t (id int primary key, val int)")?;
    db.run("insert into t values (1, 123), (2, 456)")?.done()?;

    // Integer with d >= 0 returns the integer unchanged
    let rows = db.run("select round(val, 2) from t order by id")?;
    let mut results = Vec::new();
    let mut iter = rows;
    while let Some(row) = iter.next_tuple(|_, row| row.take_values())? {
        results.push(row);
    }
    iter.done()?;

    assert_eq!(results[0][0], DataValue::Int32(123));
    assert_eq!(results[1][0], DataValue::Int32(456));

    Ok(())
}

#[test]
fn hex_with_integers() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(
        scalar(&db, "select hex(255)")?,
        DataValue::from("FF".to_string())
    );
    // -1 in two's complement is FFFFFFFFFFFFFFFF
    assert_eq!(
        scalar(&db, "select hex(-1)")?,
        DataValue::from("FFFFFFFFFFFFFFFF".to_string())
    );
    Ok(())
}

#[test]
fn hex_with_bigint_column() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let mut db = database(&dir)?;

    db.ddl("create table t (id int primary key, val bigint)")?;
    db.run("insert into t values (1, 255), (2, -1)")?.done()?;

    let rows = db.run("select hex(val) from t order by id")?;
    let mut results = Vec::new();
    let mut iter = rows;
    while let Some(row) = iter.next_tuple(|_, row| row.take_values())? {
        results.push(row);
    }
    iter.done()?;

    assert_eq!(results[0][0], DataValue::from("FF".to_string()));
    assert_eq!(
        results[1][0],
        DataValue::from("FFFFFFFFFFFFFFFF".to_string())
    );

    Ok(())
}

#[test]
fn length_with_int_column() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let mut db = database(&dir)?;

    db.ddl("create table t (id int primary key, val int)")?;
    db.run("insert into t values (1, 12345), (2, -999)")?
        .done()?;

    let rows = db.run("select length(val) from t order by id")?;
    let mut results = Vec::new();
    let mut iter = rows;
    while let Some(row) = iter.next_tuple(|_, row| row.take_values())? {
        results.push(row);
    }
    iter.done()?;

    assert_eq!(results[0][0], DataValue::Int32(5)); // "12345" has 5 bytes
    assert_eq!(results[1][0], DataValue::Int32(4)); // "-999" has 4 bytes

    Ok(())
}

#[test]
fn concat_with_numeric_arguments() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(
        scalar(&db, "select concat('a', 1, 2.5)")?,
        DataValue::from("a12.5".to_string())
    );
    Ok(())
}

#[test]
fn concat_variadic_1_to_6_args() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(
        scalar(&db, "select concat('a')")?,
        DataValue::from("a".to_string())
    );
    assert_eq!(
        scalar(&db, "select concat('a', 'b', 'c', 'd', 'e', 'f')")?,
        DataValue::from("abcdef".to_string())
    );
    Ok(())
}

#[test]
fn greatest_least_with_integers() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(
        scalar(&db, "select greatest(1, 5, 3)")?,
        DataValue::Int32(5)
    );
    assert_eq!(scalar(&db, "select least(1, 5, 3)")?, DataValue::Int32(1));
    Ok(())
}

#[test]
fn greatest_least_with_null() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(scalar(&db, "select greatest(1, null, 3)")?, DataValue::Null);
    assert_eq!(scalar(&db, "select least(1, null, 3)")?, DataValue::Null);
    Ok(())
}

#[test]
fn greatest_least_more_than_4_args() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(
        scalar(&db, "select greatest('a', 'b', 'c', 'd', 'e', 'f')")?,
        DataValue::from("f".to_string())
    );
    assert_eq!(
        scalar(&db, "select least('a', 'b', 'c', 'd', 'e', 'f')")?,
        DataValue::from("a".to_string())
    );
    Ok(())
}

// `POSITION(x IN y)` is evaluated by the core engine, not by this function pack. These cases need
// character-based positions (PR #38) and MySQL's case-insensitive default collation.
#[test]
#[ignore = "core POSITION is byte-based and case-sensitive; see PR #38"]
fn position_in_syntax_matches_mysql() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(
        scalar(&db, "select position('BAR' in 'foobarbar')")?,
        DataValue::Int32(4)
    );
    assert_eq!(
        scalar(&db, "select position('l' in 'héllo')")?,
        DataValue::Int32(3)
    );
    Ok(())
}
