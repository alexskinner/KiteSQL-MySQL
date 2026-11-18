use kite_sql::db::{DataBaseBuilder, ResultIter};
use kite_sql::errors::DatabaseError;
use kite_sql_mysql::MySqlBuilderExt;
use tempfile::TempDir;

fn rows<I: ResultIter>(mut it: I) -> Result<Vec<String>, DatabaseError> {
    let mut out = vec![];
    while let Some(row) = it.next_tuple(|_, r| {
        r.iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("|")
    })? {
        out.push(row);
    }
    it.done()?;
    Ok(out)
}

fn describe<I: ResultIter>(it: I) -> Result<Vec<String>, DatabaseError> {
    // column name | declared type
    Ok(rows(it)?
        .into_iter()
        .map(|r| r.split('|').take(2).collect::<Vec<_>>().join(" "))
        .collect())
}

#[test]
fn session_ddl_with_display_types_commits_and_rolls_back() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let db = DataBaseBuilder::path(dir.path())
        .with_mysql()?
        .build_lmdb()?;
    {
        let mut s = db.session();
        s.run("BEGIN")?.done()?;
        s.run("CREATE TABLE t (id INT PRIMARY KEY, u INT UNIQUE, a BIT(1), c MEDIUMTEXT, h INT UNSIGNED)")?.done()?;
        // same-transaction visibility incl. declared types
        assert_eq!(
            describe(s.run("DESCRIBE t")?)?,
            [
                "id int",
                "u int",
                "a bit(1)",
                "c mediumtext",
                "h int unsigned"
            ]
        );
        s.run("INSERT INTO t VALUES (1, 10, 1, 'x', 5)")?.done()?;
        // failing statement: duplicate unique key midway; rolls back alone
        assert!(s
            .run("INSERT INTO t VALUES (2, 20, 0, 'y', 6), (3, 10, 0, 'z', 7)")?
            .done()
            .is_err());
        assert_eq!(rows(s.run("SELECT id FROM t")?)?, ["1"]);
        assert_eq!(describe(s.run("DESCRIBE t")?)?.len(), 5);
        s.run("COMMIT")?;
    }
    // published, with display types, no load and no reopen
    assert_eq!(
        describe(db.run("DESCRIBE t")?)?,
        [
            "id int",
            "u int",
            "a bit(1)",
            "c mediumtext",
            "h int unsigned"
        ]
    );

    // ALTER in a transaction: visible in-session, then on the database after COMMIT
    {
        let mut s = db.session();
        s.run("BEGIN")?.done()?;
        for sql in [
            "ALTER TABLE t ADD COLUMN n LONGTEXT",
            "ALTER TABLE t MODIFY COLUMN c VARCHAR(10)",
            "ALTER TABLE t RENAME COLUMN a TO a2",
            "ALTER TABLE t ALTER COLUMN h SET DEFAULT 5",
        ] {
            s.run(sql)?.done()?;
        }
        let expect = [
            "id int",
            "u int",
            "a2 bit(1)",
            "c varchar(10)",
            "h int unsigned",
            "n longtext",
        ];
        assert_eq!(describe(s.run("DESCRIBE t")?)?, expect);
        // other sessions and the database still see the committed shape
        assert_eq!(
            describe(db.run("DESCRIBE t")?)?,
            [
                "id int",
                "u int",
                "a bit(1)",
                "c mediumtext",
                "h int unsigned"
            ]
        );
        s.run("COMMIT")?;
        assert_eq!(describe(db.run("DESCRIBE t")?)?, expect);
    }

    // ALTER then ROLLBACK: nothing leaks to the database or to the session afterwards
    {
        let mut s = db.session();
        s.run("BEGIN")?.done()?;
        s.run("ALTER TABLE t DROP COLUMN n")?.done()?;
        s.run("ALTER TABLE t MODIFY COLUMN c MEDIUMTEXT")?.done()?;
        s.run("ROLLBACK")?.done()?;
        let expect = [
            "id int",
            "u int",
            "a2 bit(1)",
            "c varchar(10)",
            "h int unsigned",
            "n longtext",
        ];
        assert_eq!(describe(s.run("DESCRIBE t")?)?, expect);
        assert_eq!(describe(db.run("DESCRIBE t")?)?, expect);
    }
    Ok(())
}

#[test]
fn reopen_matches_published_catalog() -> Result<(), DatabaseError> {
    let dir = TempDir::new().unwrap();
    let expect = ["id int", "j tinyint(1)", "k serial"];
    {
        let db = DataBaseBuilder::path(dir.path())
            .with_mysql()?
            .build_lmdb()?;
        let mut s = db.session();
        s.run("BEGIN")?.done()?;
        s.run("CREATE TABLE t (id INT PRIMARY KEY, j TINYINT(1), k SERIAL)")?
            .done()?;
        s.run("COMMIT")?;
        assert_eq!(describe(db.run("DESCRIBE t")?)?, expect);
    }
    let mut db = DataBaseBuilder::path(dir.path())
        .with_mysql()?
        .build_lmdb()?;
    db.load(kite_sql::db::CatalogKind::Table("t".into()))?;
    assert_eq!(describe(db.run("DESCRIBE t")?)?, expect);
    Ok(())
}
