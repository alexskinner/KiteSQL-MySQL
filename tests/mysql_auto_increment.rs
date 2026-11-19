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

//! MySQL AUTO_INCREMENT: the counter persists across a restart, and LAST_INSERT_ID() is per
//! connection (thread).

use kite_sql::db::{CatalogKind, DataBaseBuilder};
use kite_sql::errors::DatabaseError;
use kite_sql::storage::Storage;
use kite_sql_mysql::MySqlBuilderExt;
use tempfile::TempDir;

fn ints<S: Storage>(
    database: &kite_sql::db::Database<S>,
    sql: &str,
) -> Result<Vec<String>, DatabaseError> {
    let mut result = database.run(sql)?;
    let mut out = Vec::new();
    while let Some(value) = result.next_tuple(|_, row| row[0].to_string())? {
        out.push(value);
    }
    result.done()?;
    Ok(out)
}

#[test]
fn test_auto_increment_counter_survives_restart() -> Result<(), DatabaseError> {
    let temp_dir = TempDir::new().expect("unable to create temporary working directory");
    {
        let mut database = DataBaseBuilder::path(temp_dir.path())
            .with_mysql()?
            .build_lmdb()?;
        database.ddl("CREATE TABLE t (id INT AUTO_INCREMENT PRIMARY KEY, v INT)")?;
        database
            .run("INSERT INTO t (v) VALUES (1), (2), (3)")?
            .done()?;
        database.run("DELETE FROM t WHERE id = 3")?.done()?;
        database.ddl(
            "CREATE TABLE s (id INT AUTO_INCREMENT PRIMARY KEY, v INT) AUTO_INCREMENT = 500",
        )?;
    }
    let mut database = DataBaseBuilder::path(temp_dir.path())
        .with_mysql()?
        .build_lmdb()?;
    database.load(CatalogKind::Table("t".into()))?;
    database.load(CatalogKind::Table("s".into()))?;
    // MySQL 8 persists the counter: the deleted 3 is not handed out again.
    database.run("INSERT INTO t (v) VALUES (4)")?.done()?;
    assert_eq!(
        ints(&database, "SELECT id FROM t ORDER BY id")?,
        ["1", "2", "4"]
    );
    assert_eq!(ints(&database, "SELECT LAST_INSERT_ID()")?, ["4"]);
    database.run("INSERT INTO s (v) VALUES (1)")?.done()?;
    assert_eq!(ints(&database, "SELECT id FROM s")?, ["500"]);
    Ok(())
}

#[test]
fn test_last_insert_id_is_per_connection() -> Result<(), DatabaseError> {
    let temp_dir = TempDir::new().expect("unable to create temporary working directory");
    let mut database = DataBaseBuilder::path(temp_dir.path())
        .with_mysql()?
        .build_lmdb()?;
    database.ddl("CREATE TABLE t (id BIGINT AUTO_INCREMENT PRIMARY KEY, v INT)")?;
    database.run("INSERT INTO t (v) VALUES (1), (2)")?.done()?;
    assert_eq!(ints(&database, "SELECT LAST_INSERT_ID()")?, ["1"]);

    let other = std::thread::scope(|scope| {
        scope
            .spawn(|| -> Result<_, DatabaseError> {
                let before = ints(&database, "SELECT LAST_INSERT_ID()")?;
                database.run("INSERT INTO t (v) VALUES (3)")?.done()?;
                Ok((before, ints(&database, "SELECT LAST_INSERT_ID()")?))
            })
            .join()
            .unwrap()
    })?;
    assert_eq!(other, (vec!["0".to_string()], vec!["3".to_string()]));
    // this connection still sees its own insert
    assert_eq!(ints(&database, "SELECT LAST_INSERT_ID()")?, ["1"]);
    Ok(())
}
