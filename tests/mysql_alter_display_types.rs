//! Integration test for MySQL display types with ALTER operations.
//!
//! This test verifies that:
//! 1. Display types are preserved after ADD COLUMN
//! 2. Display types are removed after DROP COLUMN
//! 3. Display types are updated after MODIFY COLUMN
//! 4. Display types are preserved with column rename (CHANGE)
//! 5. Cache is properly refreshed after ALTER operations

use kite_sql::db::DataBaseBuilder;
use kite_sql::errors::DatabaseError;
use kite_sql::storage::lmdb::LmdbStorage;
use kite_sql::types::value::DataValue;
use kite_sql_mysql::MySqlBuilderExt;
use tempfile::TempDir;

#[test]
fn test_mysql_alter_display_types() -> Result<(), DatabaseError> {
        let temp_dir = TempDir::new().expect("unable to create temporary working directory");
        let storage = LmdbStorage::new(temp_dir.path())?;
        let mut database = DataBaseBuilder::path(".")
            .with_mysql()?
            .build_with_storage(storage)?;

        // Create initial table with various MySQL types
        database.ddl(
            "CREATE TABLE test_types (
                id INT PRIMARY KEY,
                flags BIT(1),
                bits BIT(8),
                tiny TINYINT(1),
                description MEDIUMTEXT,
                content LONGTEXT,
                auto_id SERIAL,
                unsigned_val INT UNSIGNED,
                price DECIMAL(10,2)
            )",
        )?;

        // Verify initial types
        let mut result = database.run("DESCRIBE test_types")?;
        
        let id_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(id_row[0], DataValue::from("id".to_string()));
        assert_eq!(id_row[1], DataValue::from("int".to_string()));

        let flags_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(flags_row[0], DataValue::from("flags".to_string()));
        assert_eq!(flags_row[1], DataValue::from("bit(1)".to_string()));

        let bits_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(bits_row[0], DataValue::from("bits".to_string()));
        assert_eq!(bits_row[1], DataValue::from("bit(8)".to_string()));

        let tiny_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(tiny_row[0], DataValue::from("tiny".to_string()));
        assert_eq!(tiny_row[1], DataValue::from("tinyint(1)".to_string()));

        let description_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(description_row[0], DataValue::from("description".to_string()));
        assert_eq!(description_row[1], DataValue::from("mediumtext".to_string()));

        let content_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(content_row[0], DataValue::from("content".to_string()));
        assert_eq!(content_row[1], DataValue::from("longtext".to_string()));

        let auto_id_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(auto_id_row[0], DataValue::from("auto_id".to_string()));
        assert_eq!(auto_id_row[1], DataValue::from("serial".to_string()));

        let unsigned_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(unsigned_row[0], DataValue::from("unsigned_val".to_string()));
        assert_eq!(unsigned_row[1], DataValue::from("int unsigned".to_string()));

        let price_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(price_row[0], DataValue::from("price".to_string()));
        assert_eq!(price_row[1], DataValue::from("decimal(10,2)".to_string()));

        drop(result);

        // Test ADD COLUMN
        database.ddl("ALTER TABLE test_types ADD COLUMN notes TINYTEXT")?;
        
        let mut result = database.run("DESCRIBE test_types")?;
        // Skip to the last column (notes)
        for _ in 0..9 {
            result.next_tuple(|_, _| ())?.unwrap();
        }
        let notes_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(notes_row[0], DataValue::from("notes".to_string()));
        assert_eq!(notes_row[1], DataValue::from("tinytext".to_string()));
        drop(result);

        // Test MODIFY COLUMN (change type)
        database.ddl("ALTER TABLE test_types MODIFY COLUMN notes VARCHAR(255)")?;
        
        let mut result = database.run("DESCRIBE test_types")?;
        for _ in 0..9 {
            result.next_tuple(|_, _| ())?.unwrap();
        }
        let notes_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(notes_row[0], DataValue::from("notes".to_string()));
        assert_eq!(notes_row[1], DataValue::from("varchar(255)".to_string()));
        drop(result);

        // Test CHANGE COLUMN with rename
        database.ddl("ALTER TABLE test_types CHANGE description desc2 LONGTEXT")?;
        
        let mut result = database.run("DESCRIBE test_types")?;
        for _ in 0..4 {
            result.next_tuple(|_, _| ())?.unwrap();
        }
        let desc2_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(desc2_row[0], DataValue::from("desc2".to_string()));
        assert_eq!(desc2_row[1], DataValue::from("longtext".to_string()));
        drop(result);

        // Test DROP COLUMN
        database.ddl("ALTER TABLE test_types DROP COLUMN notes")?;
        
        // Verify the column is gone and other columns still have correct types
        let mut result = database.run("DESCRIBE test_types")?;
        let id_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(id_row[0], DataValue::from("id".to_string()));
        assert_eq!(id_row[1], DataValue::from("int".to_string()));
        
        // Skip to desc2 (originally description, now renamed)
        for _ in 0..3 {
            result.next_tuple(|_, _| ())?.unwrap();
        }
        let desc2_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(desc2_row[0], DataValue::from("desc2".to_string()));
        assert_eq!(desc2_row[1], DataValue::from("longtext".to_string()));
        drop(result);

        // Test persistence: drop and reopen database
        drop(database);
        
        let storage = LmdbStorage::new(temp_dir.path())?;
        let mut database = DataBaseBuilder::path(".")
            .with_mysql()?
            .build_with_storage(storage)?;

        // Verify types are still correct after reopen
        let mut result = database.run("DESCRIBE test_types")?;
        
        let id_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(id_row[0], DataValue::from("id".to_string()));
        assert_eq!(id_row[1], DataValue::from("int".to_string()));

        let flags_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(flags_row[0], DataValue::from("flags".to_string()));
        assert_eq!(flags_row[1], DataValue::from("bit(1)".to_string()));

        // Skip to desc2
        for _ in 0..2 {
            result.next_tuple(|_, _| ())?.unwrap();
        }
        let desc2_row = result.next_tuple(|_, row| row.clone())?.unwrap();
        assert_eq!(desc2_row[0], DataValue::from("desc2".to_string()));
        assert_eq!(desc2_row[1], DataValue::from("longtext".to_string()));

    Ok(())
}
