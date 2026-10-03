//! Scoped knowledge queries shared by read-only readers and publishing stores.
use chaosbox_store::StoreError;
use typedb_driver::TypeDBDriver;
use crate::{
    common::{col_string, read_rows},
    encode::str_lit,
};

pub(crate) async fn current(
    driver: &TypeDBDriver,
    database: &str,
    scope: &str,
) -> Result<Option<(String, String)>, StoreError> {
    let rows = read_rows(driver, database, &format!(
        "match $p isa session-knowledge-pointer, has memory-scope {}, has memory-build-id $id; $b isa session-knowledge-build, has memory-build-id $id, has memory-scope {}, has raw-envelope $body; select $id, $body;",
        str_lit(scope), str_lit(scope)), &["id", "body"]).await?;
    rows.first()
        .map(|r| Ok((col_string(r, "id")?, col_string(r, "body")?)))
        .transpose()
}

pub(crate) async fn at(
    driver: &TypeDBDriver,
    database: &str,
    scope: &str,
    id: &str,
) -> Result<Option<String>, StoreError> {
    let rows = read_rows(driver, database, &format!(
        "match $b isa session-knowledge-build, has memory-build-id {}, has memory-scope {}, has raw-envelope $body; select $body;",
        str_lit(id), str_lit(scope)), &["body"]).await?;
    rows.first().map(|r| col_string(r, "body")).transpose()
}
