// Tests for issue fixes in PR #34

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

// Issue 2: ROUND no longer panics with large negative d
#[test]
fn round_no_panic_on_overflow() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    // These used to panic. A divisor beyond every integer rounds to 0.
    assert_eq!(
        scalar(&db, "select round(cast(123 as bigint), -10)")?,
        DataValue::Int64(0)
    );
    assert_eq!(
        scalar(&db, "select round(cast(2147483647 as bigint), -1)")?,
        DataValue::Int64(2147483650)
    );
    // A result outside the argument's type is a clean error, not a panic, wrap or 0.
    assert!(scalar(&db, "select round(2147483647, -1)").is_err());
    assert!(scalar(&db, "select round(9223372036854775807, -1)").is_err());
    Ok(())
}

// Issue 3: HEX formatting (no zero-padding)
#[test]
fn hex_no_zero_padding() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(
        scalar(&db, "select hex(255)")?,
        DataValue::from("FF".to_string())
    );
    assert_eq!(
        scalar(&db, "select hex(1)")?,
        DataValue::from("1".to_string())
    );
    assert_eq!(
        scalar(&db, "select hex(0)")?,
        DataValue::from("0".to_string())
    );
    Ok(())
}

#[test]
fn hex_rounds_floats() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let mut db = database(&dir)?;

    db.ddl("create table t (id int primary key, val double)")?;
    db.run("insert into t values (1, 2.5), (2, 2.4)")?.done()?;

    assert_eq!(
        scalar(&db, "select hex(val) from t where id = 1")?,
        DataValue::from("3".to_string())
    );
    assert_eq!(
        scalar(&db, "select hex(val) from t where id = 2")?,
        DataValue::from("2".to_string())
    );
    Ok(())
}

#[test]
fn hex_boolean() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let mut db = database(&dir)?;

    db.ddl("create table t (id int primary key, val boolean)")?;
    db.run("insert into t values (1, true), (2, false)")?
        .done()?;

    assert_eq!(
        scalar(&db, "select hex(val) from t where id = 1")?,
        DataValue::from("1".to_string())
    );
    assert_eq!(
        scalar(&db, "select hex(val) from t where id = 2")?,
        DataValue::from("0".to_string())
    );
    Ok(())
}

// Issue 4: Truly variadic (11+ arguments work)
#[test]
fn concat_variadic() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    // 11 arguments (used to be capped at 10)
    assert_eq!(
        scalar(
            &db,
            "select concat('a','b','c','d','e','f','g','h','i','j','k')"
        )?,
        DataValue::from("abcdefghijk".to_string())
    );

    // Even more arguments
    assert_eq!(
        scalar(
            &db,
            "select concat('a','b','c','d','e','f','g','h','i','j','k','l','m','n','o')"
        )?,
        DataValue::from("abcdefghijklmno".to_string())
    );
    Ok(())
}

// Issue 8: locate with pos < 1 returns 0
#[test]
fn locate_pos_less_than_1() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(
        scalar(&db, "select locate('a', 'abc', 0)")?,
        DataValue::Int32(0)
    );
    assert_eq!(
        scalar(&db, "select locate('a', 'abc', -1)")?,
        DataValue::Int32(0)
    );
    assert_eq!(
        scalar(&db, "select locate('a', 'abc', 1)")?,
        DataValue::Int32(1)
    ); // Normal case
    Ok(())
}

// Issue: NULL handling in ROUND
#[test]
fn round_null_handling() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(
        scalar(&db, "select round(cast(null as double))")?,
        DataValue::Null
    );
    assert_eq!(
        scalar(&db, "select round(cast(1.5 as double), null)")?,
        DataValue::Null
    );
    Ok(())
}
