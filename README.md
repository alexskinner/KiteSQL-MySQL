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
- **Exact-value arithmetic** as in MySQL 8:
  - A number with a decimal point and no exponent (`0.1`, `.5`, `2.50`) is an exact `DECIMAL`, so `0.1 + 0.2 = 0.3`; with an exponent (`1e2`) it stays `DOUBLE`. Integers above `BIGINT` up to 18446744073709551615 are `BIGINT UNSIGNED`.
  - `/` on exact values gives `DECIMAL` with the dividend's scale plus `div_precision_increment` (4): `7 / 2 = 3.5000`, `1.0 / 3 = 0.33333`.
  - `*` adds the operand scales (`1.10 * 2.5 = 2.750`); `+`, `-` and `%` keep the larger one. `%` and `MOD()` take the dividend's sign.
  - `DIV` is integer division, truncating toward zero, with the precedence of `*` and `/`.
  - Division or modulo by zero gives `NULL` with warning 1365.
  - Limits: values hold up to 28 significant digits (longer literals are read as `DOUBLE`); MySQL keeps division intermediates truncated at nine decimals, here a chain is exact and rounded once at the end, so results differ in the last digit for chains like `2 / 3 * 1000000000`.
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
