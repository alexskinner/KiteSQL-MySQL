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

//! Tests for the MySQL function pack installed by `with_mysql()`.

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

fn rows(db: &Database<LmdbStorage>, sql: &str) -> Result<Vec<Vec<DataValue>>, DatabaseError> {
    let mut iter = db.run(sql)?;
    let mut out = Vec::new();
    while let Some(row) = iter.next_tuple(|_, row| row.take_values())? {
        out.push(row);
    }
    iter.done()?;
    Ok(out)
}

fn text(value: &str) -> DataValue {
    DataValue::from(value.to_string())
}

fn scalar(db: &Database<LmdbStorage>, sql: &str) -> Result<DataValue, DatabaseError> {
    Ok(rows(db, sql)?.remove(0).remove(0))
}

#[test]
fn replace_semantics() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(
        scalar(&db, "select replace('a-b-c', '-', '+')")?,
        text("a+b+c")
    );
    assert_eq!(
        scalar(&db, "select replace('aaaa', 'aa', 'b')")?,
        text("bb")
    );
    assert_eq!(
        scalar(&db, "select replace('Abc abc ABC', 'abc', 'x')")?,
        text("Abc x ABC")
    );
    assert_eq!(scalar(&db, "select REPLACE('abc', 'b', 'B')")?, text("aBc"));
    assert_eq!(scalar(&db, "select Replace('abc', 'b', '')")?, text("ac"));
    assert_eq!(scalar(&db, "select replace('abc', '', 'x')")?, text("abc"));
    assert_eq!(scalar(&db, "select replace('abc', 'z', 'x')")?, text("abc"));
    assert_eq!(
        scalar(&db, "select replace(null, 'a', 'b')")?,
        DataValue::Null
    );
    assert_eq!(
        scalar(&db, "select replace('abc', null, 'b')")?,
        DataValue::Null
    );
    assert_eq!(
        scalar(&db, "select replace('abc', 'a', null)")?,
        DataValue::Null
    );
    assert_eq!(
        scalar(
            &db,
            "select replace(replace('{\"link\":\"x\"}', '{\"link\":\"', ''), '\"}', '')"
        )?,
        text("x")
    );
    assert_eq!(
        scalar(&db, "select replace('héllo wörld', 'ö', 'o')")?,
        text("héllo world")
    );
    Ok(())
}

#[test]
fn replace_on_bounded_columns_in_dml() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let mut db = database(&dir)?;
    db.ddl(
        "create table r (id int primary key, v varchar(20) null, c char(5) null, t longtext null)",
    )?;
    db.run("insert into r values (1, 'a.b', 'x.y', 'p.q'), (2, 'c.d', 'z', null)")?
        .done()?;

    assert_eq!(
        rows(
            &db,
            "select replace(v, '.', '/'), replace(c, '.', '/') from r order by id"
        )?,
        vec![vec![text("a/b"), text("x/y")], vec![text("c/d"), text("z")]]
    );
    db.run("update r set v = replace(v, '.', '_') where replace(t, '.', '') = 'pq'")?
        .done()?;
    db.run("insert into r values (3, replace('e.f', '.', '-'), null, null)")?
        .done()?;
    db.run("delete from r where replace(v, '.', '') = 'cd'")?
        .done()?;
    assert_eq!(
        rows(&db, "select id, v from r order by id")?,
        vec![
            vec![DataValue::Int32(1), text("a_b")],
            vec![DataValue::Int32(3), text("e-f")]
        ]
    );

    let mut session = db.session();
    session.run("BEGIN")?.done()?;
    session
        .run("update r set v = replace(v, '_', '+') where id = 1")?
        .done()?;
    session.run("COMMIT")?.done()?;
    drop(session);
    assert_eq!(scalar(&db, "select v from r where id = 1")?, text("a+b"));
    Ok(())
}

#[test]
fn cms_link_backfill_statement() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let mut db = database(&dir)?;
    db.ddl(
        "create table `cms_send_log_activity` ( `id` varchar(35) not null primary key, \
         `link` varchar(2000) null, `extra_data` longtext null )",
    )?;
    db.run(
        "insert into `cms_send_log_activity` values \
         ('a', null, '{\"link\":\"https://example.com/x?y=1\"}'), \
         ('b', 'keep', '{}'), \
         ('c', 'keep', null), \
         ('d', null, '{\"link\":\"\"}')",
    )?
    .done()?;

    db.run(
        "update `cms_send_log_activity`
           set `link` = Replace( Replace( `extra_data`, '{\"link\":\"', '' ), '\"}', '' )
           where `extra_data` is not null
           and `extra_data` != '{}'",
    )?
    .done()?;

    assert_eq!(
        rows(
            &db,
            "select id, link from cms_send_log_activity order by id"
        )?,
        vec![
            vec![text("a"), text("https://example.com/x?y=1")],
            vec![text("b"), text("keep")],
            vec![text("c"), text("keep")],
            vec![text("d"), text("")],
        ]
    );
    Ok(())
}

#[test]
fn length_function() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(scalar(&db, "select length('hello')")?, DataValue::Int32(5));
    assert_eq!(scalar(&db, "select length('héllo')")?, DataValue::Int32(6));
    assert_eq!(scalar(&db, "select length('')")?, DataValue::Int32(0));
    assert_eq!(scalar(&db, "select length(null)")?, DataValue::Null);
    Ok(())
}

#[test]
fn length_in_order_by_on_varchar_column() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let mut db = database(&dir)?;
    db.ddl("create table domains (id int primary key, domain varchar(255))")?;
    db.run(
        "insert into domains values (1, 'example.com'), (2, 'x.co'), (3, 'verylongdomainname.org')",
    )?
    .done()?;

    assert_eq!(
        rows(
            &db,
            "select domain from domains order by Length(domain) desc"
        )?,
        vec![
            vec![text("verylongdomainname.org")],
            vec![text("example.com")],
            vec![text("x.co")]
        ]
    );
    Ok(())
}

#[test]
fn concat_functions() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(scalar(&db, "select concat('a', 'b')")?, text("ab"));
    assert_eq!(scalar(&db, "select concat('a', 'b', 'c')")?, text("abc"));
    assert_eq!(
        scalar(&db, "select concat('a', null, 'c')")?,
        DataValue::Null
    );

    assert_eq!(scalar(&db, "select concat_ws(',', 'a', 'b')")?, text("a,b"));
    assert_eq!(
        scalar(&db, "select concat_ws('-', 'a', 'b', 'c')")?,
        text("a-b-c")
    );
    assert_eq!(
        scalar(&db, "select concat_ws(',', 'a', null, 'c')")?,
        text("a,c")
    );
    assert_eq!(
        scalar(&db, "select concat_ws(null, 'a', 'b')")?,
        DataValue::Null
    );
    Ok(())
}

#[test]
fn substring_index_function() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(
        scalar(&db, "select substring_index('a.b.c.d', '.', 2)")?,
        text("a.b")
    );
    assert_eq!(
        scalar(&db, "select substring_index('a.b.c.d', '.', -2)")?,
        text("c.d")
    );
    assert_eq!(
        scalar(&db, "select substring_index('a.b', '.', 0)")?,
        text("")
    );
    assert_eq!(
        scalar(&db, "select substring_index('test', '.', 5)")?,
        text("test")
    );
    assert_eq!(
        scalar(&db, "select substring_index(null, '.', 1)")?,
        DataValue::Null
    );
    Ok(())
}

#[test]
fn locate_instr_position_functions() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(
        scalar(&db, "select locate('bar', 'foobarbar')")?,
        DataValue::Int32(4)
    );
    assert_eq!(
        scalar(&db, "select locate('bar', 'foobarbar', 5)")?,
        DataValue::Int32(7)
    );
    assert_eq!(
        scalar(&db, "select locate('xyz', 'foobar')")?,
        DataValue::Int32(0)
    );
    assert_eq!(scalar(&db, "select locate(null, 'test')")?, DataValue::Null);

    assert_eq!(
        scalar(&db, "select instr('foobarbar', 'bar')")?,
        DataValue::Int32(4)
    );
    assert_eq!(
        scalar(&db, "select instr('foobar', 'xyz')")?,
        DataValue::Int32(0)
    );

    assert_eq!(
        scalar(&db, "select position('bar' in 'foobarbar')")?,
        DataValue::Int32(4)
    );
    Ok(())
}

#[test]
fn left_right_functions() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(scalar(&db, "select left('foobar', 3)")?, text("foo"));
    assert_eq!(scalar(&db, "select right('foobar', 3)")?, text("bar"));
    assert_eq!(scalar(&db, "select left('test', 10)")?, text("test"));
    assert_eq!(scalar(&db, "select left(null, 3)")?, DataValue::Null);
    Ok(())
}

#[test]
fn lpad_rpad_functions() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(scalar(&db, "select lpad('hi', 5, '?')")?, text("???hi"));
    assert_eq!(scalar(&db, "select rpad('hi', 5, '?')")?, text("hi???"));
    assert_eq!(scalar(&db, "select lpad('hello', 3, 'x')")?, text("hel"));
    assert_eq!(scalar(&db, "select lpad('hi', 4, 'ab')")?, text("abhi"));
    // MySQL 8 returns the string unchanged when there is nothing to pad with (lpad.result 80-85)
    assert_eq!(scalar(&db, "select lpad('test', 5, '')")?, text("test"));
    Ok(())
}

#[test]
fn repeat_reverse_functions() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(scalar(&db, "select repeat('ha', 3)")?, text("hahaha"));
    assert_eq!(scalar(&db, "select repeat('test', 0)")?, text(""));
    assert_eq!(scalar(&db, "select repeat(null, 3)")?, DataValue::Null);

    assert_eq!(scalar(&db, "select reverse('hello')")?, text("olleh"));
    assert_eq!(scalar(&db, "select reverse(null)")?, DataValue::Null);
    Ok(())
}

#[test]
fn ltrim_rtrim_functions() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(scalar(&db, "select ltrim('  hello')")?, text("hello"));
    assert_eq!(scalar(&db, "select rtrim('hello  ')")?, text("hello"));
    assert_eq!(scalar(&db, "select ltrim(null)")?, DataValue::Null);
    Ok(())
}

#[test]
fn hex_function() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(scalar(&db, "select hex('abc')")?, text("616263"));
    assert_eq!(scalar(&db, "select hex('A')")?, text("41"));
    assert_eq!(scalar(&db, "select hex('')")?, text(""));
    assert_eq!(scalar(&db, "select hex(null)")?, DataValue::Null);
    Ok(())
}

#[test]
fn uuid_function() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    let uuid1 = scalar(&db, "select uuid()")?;
    let uuid2 = scalar(&db, "select uuid()")?;
    assert_ne!(uuid1, uuid2);
    assert!(matches!(uuid1, DataValue::Utf8 { .. }));
    Ok(())
}

#[test]
fn greatest_least_functions() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(scalar(&db, "select greatest('a', 'b', 'c')")?, text("c"));
    assert_eq!(scalar(&db, "select greatest('z', 'a')")?, text("z"));
    assert_eq!(
        scalar(&db, "select greatest('a', null, 'c')")?,
        DataValue::Null
    );

    assert_eq!(scalar(&db, "select least('a', 'b', 'c')")?, text("a"));
    assert_eq!(scalar(&db, "select least('z', 'a')")?, text("a"));
    assert_eq!(
        scalar(&db, "select least('a', null, 'c')")?,
        DataValue::Null
    );
    Ok(())
}

#[test]
fn ifnull_function() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(scalar(&db, "select ifnull('a', 'b')")?, text("a"));
    assert_eq!(scalar(&db, "select ifnull(null, 'b')")?, text("b"));
    assert_eq!(scalar(&db, "select ifnull(null, null)")?, DataValue::Null);
    Ok(())
}

#[test]
fn round_function() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let mut db = database(&dir)?;

    db.ddl("create table t (id int primary key, val double)")?;
    db.run("insert into t values (1, 1.5), (2, 1.234), (3, 1.567)")?
        .done()?;

    use ordered_float::OrderedFloat;
    assert_eq!(
        scalar(&db, "select round(val) from t where id = 1")?,
        DataValue::Float64(OrderedFloat(2.0))
    );
    assert_eq!(
        scalar(&db, "select round(val, 2) from t where id = 2")?,
        DataValue::Float64(OrderedFloat(1.23))
    );
    assert_eq!(
        scalar(&db, "select round(val, 1) from t where id = 3")?,
        DataValue::Float64(OrderedFloat(1.6))
    );
    assert_eq!(
        scalar(&db, "select round(cast(null as double))")?,
        DataValue::Null
    );
    Ok(())
}

#[test]
fn greatest_least_common_type() -> Result<(), DatabaseError> {
    use ordered_float::OrderedFloat;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    let dir = TempDir::new().unwrap();
    let mut db = database(&dir)?;

    // literals are DOUBLE in this engine: compared numerically, not as the first argument
    assert_eq!(
        scalar(&db, "select least(12, 1.005, 2.5)")?,
        DataValue::Float64(OrderedFloat(1.005))
    );
    // any decimal widens to Decimal; exact comparison
    let dec = |s: &str| DataValue::Decimal(Decimal::from_str(s).unwrap());
    db.ddl("create table n (id int primary key, d decimal(10,3), i int)")?;
    db.run("insert into n values (1, 1.005, 12)")?.done()?;
    assert_eq!(scalar(&db, "select least(i, d, 3) from n")?, dec("1.005"));
    assert_eq!(scalar(&db, "select greatest(i, d, 3) from n")?, dec("12"));
    // a float wins over decimals
    assert_eq!(
        scalar(&db, "select greatest(i, d, cast(3 as double)) from n")?,
        DataValue::Float64(OrderedFloat(12.0))
    );
    // all strings compare as strings
    assert_eq!(scalar(&db, "select greatest('b', 'a')")?, text("b"));
    assert_eq!(scalar(&db, "select greatest('9', '10')")?, text("9"));
    // a mix of numbers and strings compares as strings (MySQL 8: GREATEST('11', 5, 2) = '5')
    assert_eq!(scalar(&db, "select greatest('10', 9)")?, text("9"));
    assert_eq!(scalar(&db, "select least('b', 1)")?, text("1"));
    assert_eq!(scalar(&db, "select greatest('2.5abc', 2)")?, text("2.5abc"));
    assert_eq!(scalar(&db, "select greatest('11', 5, 2)")?, text("5"));
    assert_eq!(scalar(&db, "select least('11', 5, 2)")?, text("11"));
    // booleans count as 0/1
    assert_eq!(
        scalar(&db, "select greatest(true, 2)")?,
        DataValue::Int32(2)
    );
    assert_eq!(
        scalar(&db, "select least(true, false)")?,
        DataValue::Int8(0)
    );
    // temporal values compare as temporal
    db.ddl("create table d (id int primary key, a date, b date)")?;
    db.run("insert into d values (1, '2024-01-02', '2023-12-31')")?
        .done()?;
    assert_eq!(
        scalar(&db, "select greatest(a, b) from d")?,
        scalar(&db, "select a from d")?
    );
    assert_eq!(
        scalar(&db, "select least(a, b) from d")?,
        scalar(&db, "select b from d")?
    );
    // NULL anywhere gives NULL
    assert_eq!(scalar(&db, "select greatest(1, null)")?, DataValue::Null);
    assert_eq!(scalar(&db, "select least(1.5, null, 2)")?, DataValue::Null);
    Ok(())
}

#[test]
fn greatest_least_temporal_comparison() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let mut db = database(&dir)?;
    let date = |db: &_, s: &str| scalar(db, &format!("select cast('{s}' as date)"));
    let datetime = |db: &_, s: &str| scalar(db, &format!("select cast('{s}' as datetime)"));

    // a DATE argument makes numbers and strings compare as dates, and the result is a DATE
    assert_eq!(
        scalar(
            &db,
            "select least(date '2005-05-05', 20030303, 20010101, 20040404)"
        )?,
        date(&db, "2001-01-01")?
    );
    assert_eq!(
        scalar(
            &db,
            "select least(date '2005-05-05', '20030303', '20010101', '20040404')"
        )?,
        date(&db, "2001-01-01")?
    );
    assert_eq!(
        scalar(
            &db,
            "select greatest(date '1995-05-05', 19910101, 20050505, 19930303)"
        )?,
        date(&db, "2005-05-05")?
    );
    // two-digit years: 70-99 are 19xx, 00-69 are 20xx
    assert_eq!(
        scalar(&db, "select greatest(date '1995-05-05', '10-10-10')")?,
        date(&db, "2010-10-10")?
    );
    assert_eq!(
        scalar(&db, "select least(date '2011-01-01', '95-05-05')")?,
        date(&db, "1995-05-05")?
    );
    // a DATETIME argument gives a DATETIME comparison; 'T' and fractions are accepted
    assert_eq!(
        scalar(
            &db,
            "select greatest(cast('2026-01-02' as datetime), '2026-01-03T12:34:56.1200')"
        )?,
        datetime(&db, "2026-01-03 12:34:56")?
    );
    assert_eq!(
        scalar(
            &db,
            "select greatest(cast('2001-02-03' as datetime), 20101213123456)"
        )?,
        datetime(&db, "2010-12-13 12:34:56")?
    );
    db.ddl("create table td (id int primary key, d date, s varchar(20))")?;
    db.run("insert into td values (1, '2024-01-02', '2024-01-03')")?
        .done()?;
    assert_eq!(
        scalar(&db, "select greatest(d, s) from td")?,
        date(&db, "2024-01-03")?
    );
    assert_eq!(
        scalar(&db, "select least(d, s, null) from td")?,
        DataValue::Null
    );
    Ok(())
}

#[test]
fn wrong_arity_is_an_error_not_a_panic() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    for sql in [
        "select hex()",
        "select hex(1, 2)",
        "select length()",
        "select length('a', 'b')",
        "select round()",
        "select round(1, 2, 3)",
        "select concat()",
        "select concat_ws(',')",
        "select greatest(1)",
        "select least()",
        "select locate('a')",
        "select locate('a', 'b', 1, 2)",
        "select left('a')",
        "select reverse()",
        "select lpad('a')",
        "select substring_index('a', 'b')",
        "select uuid(1)",
        "select instr('a')",
    ] {
        assert!(scalar(&db, sql).is_err(), "{sql} should be an arity error");
    }
    Ok(())
}

#[test]
fn round_return_type_follows_argument() -> Result<(), DatabaseError> {
    use ordered_float::OrderedFloat;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    let dir = TempDir::new().unwrap();
    let mut db = database(&dir)?;
    db.ddl("create table r (id int primary key, i bigint, d decimal(10,3), f double)")?;
    db.run("insert into r values (1, 1234, 1.005, 2.5)")?
        .done()?;

    assert_eq!(
        scalar(&db, "select round(i, -2) from r")?,
        DataValue::Int64(1200)
    );
    assert_eq!(
        scalar(&db, "select round(d, 2) from r")?,
        DataValue::Decimal(Decimal::from_str("1.01").unwrap())
    );
    assert_eq!(
        scalar(&db, "select round(f) from r")?,
        DataValue::Float64(OrderedFloat(3.0))
    );
    // huge or tiny d never gives NaN or infinity
    assert_eq!(
        scalar(&db, "select round(f, 400) from r")?,
        DataValue::Float64(OrderedFloat(2.5))
    );
    assert_eq!(
        scalar(&db, "select round(f, -400) from r")?,
        DataValue::Float64(OrderedFloat(0.0))
    );
    assert_eq!(scalar(&db, "select round(null)")?, DataValue::Null);
    assert_eq!(scalar(&db, "select round(1.5, null)")?, DataValue::Null);
    Ok(())
}

#[test]
fn locate_empty_needle_and_start_bounds() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = database(&dir)?;

    assert_eq!(
        scalar(&db, "select locate('', 'abc')")?,
        DataValue::Int32(1)
    );
    assert_eq!(
        scalar(&db, "select locate('', 'abc', 4)")?,
        DataValue::Int32(4)
    );
    assert_eq!(
        scalar(&db, "select locate('', 'abc', 5)")?,
        DataValue::Int32(0)
    );
    assert_eq!(
        scalar(&db, "select locate('b', 'abc', 0)")?,
        DataValue::Int32(0)
    );
    assert_eq!(
        scalar(&db, "select locate('É', 'héllo')")?,
        DataValue::Int32(2)
    );
    Ok(())
}
