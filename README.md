# kite_sql_mysql

MySQL dialect support for [KiteSQL](https://github.com/KipData/KiteSQL).

## Features

This crate provides MySQL-specific functionality for KiteSQL:

- **MySQL Dialect**: Full MySQL SQL dialect support via sqlparser
- **MySQL Type Mappings**:
  - `TINYTEXT`, `MEDIUMTEXT`, `LONGTEXT` → `VARCHAR(None)`
  - `BIT(1)` → `BOOLEAN`
  - `BIT(n)` (n > 1) → `UBIGINT`
  - `SERIAL` → `UBIGINT NOT NULL AUTO_INCREMENT UNIQUE`
- **AUTO_INCREMENT**: columns and the `AUTO_INCREMENT = n` table option, on the core identity
  column; an explicit 0 generates like NULL; `LAST_INSERT_ID()` / `LAST_INSERT_ID(expr)`
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

    // Now you can use MySQL-specific types like MEDIUMTEXT
    database.ddl("CREATE TABLE users (id INT PRIMARY KEY, bio MEDIUMTEXT)")?;
    database.run("INSERT INTO users VALUES (1, 'Long biography text...')")?.done()?;
    
    let mut result = database.run("SELECT * FROM users WHERE id = 1")?;
    let tuple = result.next_tuple(|_, tuple| tuple.clone())?.expect("expected a row");
    println!("{:?}", tuple);
    result.done()?;

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

- `SERIAL`: Maps to `UBIGINT NOT NULL AUTO_INCREMENT UNIQUE`

### Integer Types

- `TINYINT(1)`: Works as standard `TINYINT`

## See Also

- [KiteSQL Documentation](https://docs.rs/kite_sql)
- [Writing Custom Dialects](../docs/features.md#sql-dialects)

## License

Apache-2.0
