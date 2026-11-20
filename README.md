# kite_sql_mysql

MySQL dialect support for [KiteSQL](https://github.com/KipData/KiteSQL)[^trademark].

[^trademark]: See the [Trademark Notice](#trademark-notice) section below.

## Overview

This crate provides MySQL-specific functionality for KiteSQL through its dialect registration hooks. It enables KiteSQL to parse and execute MySQL-specific SQL syntax, type mappings, functions, and collation rules.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
kite_sql = { git = "https://github.com/alexskinner/KiteSQL", rev = "806503632a84aecd15bbc5d31723b125671ee224" }
kite_sql_mysql = { git = "https://github.com/alexskinner/KiteSQL-MySQL" }
```

## Usage

Use the `with_mysql()` extension method to register the MySQL dialect with your database:

```rust
use kite_sql::db::DataBaseBuilder;
use kite_sql_mysql::MySqlBuilderExt;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut database = DataBaseBuilder::path(".")
        .with_mysql()?
        .build_in_memory()?;

    // Now you can use MySQL-specific types and syntax
    database.ddl("CREATE TABLE users (
        id INT AUTO_INCREMENT PRIMARY KEY,
        name VARCHAR(100),
        bio MEDIUMTEXT
    )")?;
    
    database.run("INSERT INTO users (name, bio) VALUES ('Alice', 'Software developer')")?
        .done()?;
    
    let mut result = database.run("SELECT * FROM users WHERE id = 1")?;
    if let Some(tuple) = result.next_tuple(|_, tuple| tuple.clone())? {
        println!("{:?}", tuple);
    }
    result.done()?;

    Ok(())
}
```

## Features

### MySQL Type Mappings

- **Text Types**: `TINYTEXT`, `MEDIUMTEXT`, `LONGTEXT` → `VARCHAR` without length limit
- **Bit Types**: 
  - `BIT(1)` → `BOOLEAN`
  - `BIT(n)` (n > 1) → `UBIGINT`
- **Serial Type**: `SERIAL` → `UBIGINT NOT NULL AUTO_INCREMENT UNIQUE`
- **Integer Display Width**: `TINYINT(1)` and other display widths work as expected

### AUTO_INCREMENT and LAST_INSERT_ID()

Identity columns with `AUTO_INCREMENT` are supported, including:
- The `AUTO_INCREMENT = n` table option
- Explicit 0 values generate new IDs like NULL
- `LAST_INSERT_ID()` returns the last generated ID
- `LAST_INSERT_ID(expr)` sets and returns a value

### Exact-Value Arithmetic (MySQL 8 Compatibility)

- **Exact Decimal Literals**: Numbers with a decimal point and no exponent (`0.1`, `.5`, `2.50`) are treated as exact `DECIMAL` values, so `0.1 + 0.2 = 0.3` exactly
- **Division**: `/` on exact values produces `DECIMAL` with scale = dividend scale + 4, so `7 / 2 = 3.5000` and `1.0 / 3 = 0.33333`
- **Arithmetic Operations**:
  - `*` adds operand scales (`1.10 * 2.5 = 2.750`)
  - `+`, `-`, `%` keep the larger scale
  - `%` and `MOD()` take the dividend's sign
- **Integer Division**: `DIV` performs integer division, truncating toward zero, with the precedence of `*` and `/`
- **Division by Zero**: Returns `NULL` with warning code 1365
- **Precision Limits**: Values hold up to 28 significant digits

### MySQL Collation (utf8mb4_0900_ai_ci)

Text comparison follows MySQL 8's default collation: case- and accent-insensitive, NO PAD:
- `'Smith' = 'SMITH'`, `'é' = 'e'`, `'ß' = 'ss'`
- `'a' <> 'a '` (trailing spaces matter)

Applies to: `=`, `<>`, `<`, `>`, `IN`, `BETWEEN`, `CASE`, `NULLIF`, `STRCMP`, `FIELD`, `FIND_IN_SET`, `ORDER BY`, `GROUP BY`, `DISTINCT`, `UNION`, joins, `MIN`/`MAX`, and to primary and unique keys.

**Byte-wise comparison** can be forced with:
- `BINARY x`, `CAST(x AS BINARY)`
- `COLLATE …_bin`, `…_cs`, or `binary`
- Columns declared with these collations

**LIKE** is case-insensitive by default unless an operand is marked binary, and supports:
- `ESCAPE` clause for custom escape characters
- Standard `%` and `_` wildcards

### MySQL Functions

Includes MySQL-specific implementations of:
- String functions: `REPLACE`, `CONCAT`, `SUBSTRING`, `LENGTH`, and others
- Aggregate functions: `GROUP_CONCAT` with `ORDER BY` and `SEPARATOR` support
- Conditional functions: `ISNULL()`, `IFNULL()`, `IF()`
- Comparison functions: `GREATEST`, `LEAST` with MySQL type compatibility

### NULL Ordering

Results order NULLs as MySQL does: NULLs first when ascending, NULLs last when descending.

### Warning Codes

Division by zero and numeric truncation generate MySQL-compatible warning codes (1365).

### Type System Compatibility

Proper type comparison rules for:
- Boolean values
- Signed vs unsigned integers
- Temporal types
- Decimal precision and scale

## Limitations

Not yet modeled:
- Full Unicode Collation Algorithm for punctuation/symbol ordering
- Some collation edge cases in `GROUP BY`, `DISTINCT`, `UNION` on `_bin`/`_cs` columns
- Accent folding in `LIKE`
- Per-column collation metadata beyond "byte-wise or not"
- Error 1267 (illegal mix of collations) outside `LIKE`

## See Also

- [KiteSQL](https://github.com/KipData/KiteSQL)
- [KiteSQL API documentation](https://docs.rs/kite_sql)
- [SQL dialects in KiteSQL, including writing custom dialects](https://github.com/alexskinner/KiteSQL/blob/main/docs/features.md#sql-dialects)

## Trademark Notice

**MySQL is a registered trademark of Oracle Corporation and/or its affiliates.** KiteSQL-MySQL is an independent open source project and is not affiliated with, sponsored by, or endorsed by Oracle Corporation. The name "MySQL" is used only to describe the SQL dialect and syntax this crate aims to be compatible with.

## License

Licensed under the Apache License, Version 2.0. See the LICENSE file for details.

This software builds upon KiteSQL, which is also licensed under Apache 2.0 and is Copyright (c) 2024 KipData and contributors.
