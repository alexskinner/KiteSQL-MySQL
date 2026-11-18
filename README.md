# kite_sql_mysql

MySQL dialect support for [KiteSQL](https://github.com/KipData/KiteSQL).

## Features

This crate provides MySQL-specific functionality for KiteSQL:

- **MySQL Dialect**: Full MySQL SQL dialect support via sqlparser
- **MySQL Type Mappings**:
  - `TINYTEXT`, `MEDIUMTEXT`, `LONGTEXT` → `VARCHAR(None)`
  - `BIT(1)` → `BOOLEAN`
  - `BIT(n)` (n > 1) → `UBIGINT`
  - `SERIAL` → `UBIGINT` (with TODO for auto-increment, NOT NULL, UNIQUE)
  - `TINYINT(1)` works as expected
- **Easy Integration**: Extension trait for `DataBaseBuilder`

## Usage

Add both `kite_sql` and `kite_sql_mysql` to your dependencies:

```toml
[dependencies]
kite_sql = "0.7"
kite_sql_mysql = "0.7"
```

Then use the `with_mysql()` extension method:

```rust
use kite_sql::db::DataBaseBuilder;
use kite_sql_mysql::MySqlBuilderExt;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut database = DataBaseBuilder::path(".")
        .with_mysql()?
        .build_in_memory()?;

    // Now you can use MySQL-specific types
    database.run("CREATE TABLE users (id SERIAL PRIMARY KEY, bio MEDIUMTEXT)")?;
    database.run("INSERT INTO users VALUES (1, 'Long biography text...')")?;
    
    let mut result = database.run("SELECT * FROM users WHERE id = 1")?;
    while let Some(tuple) = result.next().await? {
        println!("{:?}", tuple);
    }

    Ok(())
}
```

## MySQL Type Support

### Text Types

- `TINYTEXT`: Maps to `VARCHAR` without length limit
- `MEDIUMTEXT`: Maps to `VARCHAR` without length limit  
- `LONGTEXT`: Maps to `VARCHAR` without length limit

### Bit Types

- `BIT(1)`: Maps to `BOOLEAN`
- `BIT(n)` where n > 1: Maps to `UBIGINT`

### Serial Type

- `SERIAL`: Maps to `UBIGINT`
  - **Note**: Auto-increment, NOT NULL, and UNIQUE constraints are not yet implemented automatically

### Integer Types

- `TINYINT(1)`: Works as standard `TINYINT`

## See Also

- [KiteSQL Documentation](https://docs.rs/kite_sql)
- [Writing Custom Dialects](../docs/features.md#sql-dialects)

## License

Apache-2.0
