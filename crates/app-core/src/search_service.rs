//! Search / document / HTTP-SQL facade for the UI.
//!
//! Unlike [`crate::redis_service::RedisService`], the Phase-5 connectors
//! (`SearchConnector`, `DocumentConnector`, `HttpSqlConnector`) are stateless:
//! they derive their endpoint from a [`ConnectionProfile`] on every call. This
//! service resolves a profile (with its runtime password) from the
//! [`ConnectionManager`] and dispatches to the right connector via the shared
//! [`DriverRegistry`], so the UI does not have to branch on `DataSourceKind`.

use crate::{connection::ConnectionManager, registry::DriverRegistry};
use db_common::{ConnectionId, QueryResult, Result, SearchResult};
use std::sync::Arc;

#[derive(Clone)]
pub struct SearchService {
    connections: ConnectionManager,
    registry: Arc<DriverRegistry>,
}

impl std::fmt::Debug for SearchService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SearchService { .. }")
    }
}

impl SearchService {
    pub fn new(connections: ConnectionManager, registry: Arc<DriverRegistry>) -> Self {
        Self {
            connections,
            registry,
        }
    }

    async fn load(
        &self,
        connection_id: ConnectionId,
        runtime_password: Option<&str>,
    ) -> Result<(db_common::ConnectionProfile, Option<String>)> {
        self.connections
            .profile_with_password(connection_id, runtime_password)
            .await
    }

    /// List search indices for a search-service connection.
    pub async fn list_indices(
        &self,
        connection_id: ConnectionId,
        runtime_password: Option<&str>,
    ) -> Result<Vec<db_common::SearchIndex>> {
        let (profile, password) = self.load(connection_id, runtime_password).await?;
        let connector = self.registry.search(profile.kind)?;
        connector.list_indices(&profile, password.as_deref()).await
    }

    /// Return the mapping / settings document for a search index.
    pub async fn get_mapping(
        &self,
        connection_id: ConnectionId,
        runtime_password: Option<&str>,
        index: &str,
    ) -> Result<serde_json::Value> {
        let (profile, password) = self.load(connection_id, runtime_password).await?;
        let connector = self.registry.search(profile.kind)?;
        connector
            .get_mapping(&profile, password.as_deref(), index)
            .await
    }

    pub async fn open_point_in_time(&self, id: ConnectionId, index: &str) -> Result<String> {
        let (profile, password) = self.load(id, None).await?;
        self.registry
            .search(profile.kind)?
            .open_point_in_time(&profile, password.as_deref(), index)
            .await
    }
    pub async fn close_point_in_time(&self, id: ConnectionId, pit: &str) -> Result<()> {
        let (profile, password) = self.load(id, None).await?;
        self.registry
            .search(profile.kind)?
            .close_point_in_time(&profile, password.as_deref(), pit)
            .await
    }

    /// Run a search query (JSON DSL) against a search index.
    pub async fn search(
        &self,
        connection_id: ConnectionId,
        runtime_password: Option<&str>,
        index: &str,
        query: &serde_json::Value,
        limit: u32,
    ) -> Result<SearchResult> {
        let (profile, password) = self.load(connection_id, runtime_password).await?;
        let connector = self.registry.search(profile.kind)?;
        connector
            .search(&profile, password.as_deref(), index, query, limit)
            .await
    }

    /// List collections in a document-store database.
    pub async fn list_collections(
        &self,
        connection_id: ConnectionId,
        runtime_password: Option<&str>,
        database: &str,
    ) -> Result<Vec<db_common::DocumentCollection>> {
        let (profile, password) = self.load(connection_id, runtime_password).await?;
        let connector = self.registry.document(profile.kind)?;
        connector
            .list_collections(&profile, password.as_deref(), database)
            .await
    }

    /// Find documents in a document-store collection.
    pub async fn find_documents(
        &self,
        connection_id: ConnectionId,
        runtime_password: Option<&str>,
        database: &str,
        collection: &str,
        filter: Option<&serde_json::Value>,
        limit: u32,
    ) -> Result<Vec<serde_json::Value>> {
        let (profile, password) = self.load(connection_id, runtime_password).await?;
        let connector = self.registry.document(profile.kind)?;
        connector
            .find_documents(
                &profile,
                password.as_deref(),
                database,
                collection,
                filter,
                limit,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_document_query(
        &self,
        connection_id: ConnectionId,
        runtime_password: Option<&str>,
        database: &str,
        collection: &str,
        query: &serde_json::Value,
        limit: u32,
        offset: u64,
    ) -> Result<serde_json::Value> {
        let (profile, password) = self.load(connection_id, runtime_password).await?;
        self.registry
            .document(profile.kind)?
            .execute_document_query(
                &profile,
                password.as_deref(),
                database,
                collection,
                query,
                limit,
                offset,
            )
            .await
    }

    /// Execute a SQL statement against an HTTP-SQL database (Cloudflare D1).
    pub async fn execute_sql(
        &self,
        connection_id: ConnectionId,
        runtime_password: Option<&str>,
        _database: Option<&str>,
        sql: &str,
    ) -> Result<Vec<QueryResult>> {
        let (profile, password) = self.load(connection_id, runtime_password).await?;
        if profile.security.read_only
            && db_common::split_statements(sql)
                .iter()
                .any(|statement| !db_common::is_read_only_statement(statement))
        {
            return Err(db_common::AppError::validation("connection is read-only"));
        }
        let connector = self.registry.http_sql(profile.kind)?;
        let mut results = connector
            .execute(&profile, password.as_deref(), sql)
            .await?;
        for result in &mut results {
            if result.statement.is_empty() {
                result.statement = sql.to_string();
            }
        }
        Ok(results)
    }
}
