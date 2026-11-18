//! Infrastructure for resource-scoped snapshots of every catalogued owner.
//! Domain roots stay in content/runtime. A snapshot owns the maintenance guard.
use crate::utils::object_store::{catalog::RootCatalog, StoreMaintenance};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Sqlite, SqlitePool, Transaction,
};
use std::{collections::HashMap, fs, sync::Arc};

pub(crate) struct LockedDatabases {
    pub(crate) transactions: Vec<Transaction<'static, Sqlite>>,
    pools: Vec<SqlitePool>,
    _maintenance: Arc<StoreMaintenance>,
}

impl LockedDatabases {
    pub(crate) async fn open(maintenance: Arc<StoreMaintenance>) -> Result<Self, sqlx::Error> {
        let owners = RootCatalog::owners(&maintenance)?;
        let mut result = Self {
            transactions: Vec::new(),
            pools: Vec::new(),
            _maintenance: maintenance,
        };
        let mut paths = HashMap::new();
        for owner in owners {
            if let Some(previous) = paths.insert(owner.path.clone(), owner.identity.clone()) {
                if previous != owner.identity {
                    return Err(sqlx::Error::Protocol(
                        "database replaced in retention catalogue".into(),
                    ));
                }
                continue;
            }
            if !fs::symlink_metadata(&owner.path)?.is_file()
                || fs::canonicalize(&owner.path)? != owner.path
            {
                return Err(sqlx::Error::Protocol(
                    "catalogued database is missing or replaced by a link".into(),
                ));
            }
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(
                    SqliteConnectOptions::new()
                        .filename(&owner.path)
                        .create_if_missing(false)
                        .foreign_keys(true),
                )
                .await?;
            let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
            let identity: String =
                sqlx::query_scalar("SELECT identity FROM retention_identity WHERE singleton=1")
                    .fetch_one(&mut *transaction)
                    .await?;
            if identity != owner.identity {
                return Err(sqlx::Error::Protocol(
                    "retention database identity changed".into(),
                ));
            }
            result.pools.push(pool);
            result.transactions.push(transaction);
        }
        Ok(result)
    }

    pub(crate) async fn close(self) {
        drop(self.transactions);
        for pool in self.pools {
            pool.close().await;
        }
    }
}
