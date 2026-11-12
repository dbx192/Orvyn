//! MongoDB adapter crate backed by the official `mongodb` driver.
//!
//! MongoDB speaks the wire protocol over a client-server connection. The driver
//! maintains its own connection pool internally, so each operation simply
//! builds a [`mongodb::Client`] from the profile connection string and runs the
//! command. MongoDB has no client-side catalogs to pre-fetch; databases and
//! collections are enumerated on demand.

use async_trait::async_trait;
use db_common::{
    AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    DocumentCollection, DocumentConnector, DocumentDatabase, ErrorKind, Result,
};
use mongodb::bson::Document;
use std::time::Instant;

#[derive(Debug, Clone, Copy, Default)]
pub struct MongoDbConnectionTester;

#[derive(Debug, Clone, Copy, Default)]
pub struct MongoDbDocumentConnector;

/// Assemble a MongoDB connection string from the profile. Username and password
/// are percent-encoded; a default `authSource=admin` is applied unless the
/// profile overrides it via the `mongo_auth_source` extra parameter.
fn mongo_uri(profile: &ConnectionProfile, password: Option<&str>) -> Result<String> {
    let host = profile.required_host()?;
    let port = profile.required_port()?;

    let username = profile
        .username
        .as_deref()
        .filter(|value| !value.is_empty());
    let creds = match (username, password.filter(|value| !value.is_empty())) {
        (Some(user), Some(pass)) => format!(
            "{}:{}@",
            urlencoding::encode(user),
            urlencoding::encode(pass)
        ),
        (Some(user), None) => format!("{}@", urlencoding::encode(user)),
        _ => String::new(),
    };

    let auth_source = profile
        .extra_param("mongo_auth_source")
        .filter(|value| !value.is_empty())
        .unwrap_or("admin");

    Ok(format!(
        "mongodb://{creds}{host}:{port}/?directConnection=true&authSource={}",
        urlencoding::encode(auth_source)
    ))
}

async fn mongo_client(
    profile: &ConnectionProfile,
    password: Option<&str>,
) -> Result<mongodb::Client> {
    use sha2::{Digest, Sha256};
    type Cache = std::collections::HashMap<db_common::ConnectionId, ([u8; 32], mongodb::Client)>;
    static CLIENTS: std::sync::OnceLock<tokio::sync::Mutex<Cache>> = std::sync::OnceLock::new();
    let mut uri = mongo_uri(profile, password)?;
    uri.push_str("&serverSelectionTimeoutMS=10000&connectTimeoutMS=5000");
    if profile.security.ssl_mode != db_common::SslMode::Disable {
        uri.push_str("&tls=true");
        if let Some(ca) = profile.extra_param("tls_ca_path") {
            uri.push_str("&tlsCAFile=");
            uri.push_str(&urlencoding::encode(ca));
        }
    }
    let fingerprint: [u8; 32] = Sha256::digest(uri.as_bytes()).into();
    let clients = CLIENTS.get_or_init(Default::default);
    {
        let cache = clients.lock().await;
        if let Some((key, client)) = cache.get(&profile.id) {
            if key == &fingerprint {
                return Ok(client.clone());
            }
        }
    }
    let client = mongodb::Client::with_uri_str(&uri).await.map_err(|_| {
        AppError::new(
            ErrorKind::Connection,
            "invalid MongoDB connection configuration",
        )
    })?;
    let mut cache = clients.lock().await;
    if cache.len() >= 64 {
        cache.clear();
    }
    cache.insert(profile.id, (fingerprint, client.clone()));
    Ok(client)
}

/// Convert a BSON document to a JSON value. Binary and other non-JSON BSON
/// kinds (e.g. ObjectId, dates) surface in their standard `$...` forms.
fn bson_to_json(document: &Document) -> Result<serde_json::Value> {
    let bson = mongodb::bson::to_bson(document).map_err(|error| {
        AppError::new(
            ErrorKind::Serialization,
            "failed to encode MongoDB document",
        )
        .with_detail(error.to_string())
    })?;
    serde_json::to_value(&bson).map_err(|error| {
        AppError::new(
            ErrorKind::Serialization,
            "failed to serialize MongoDB document",
        )
        .with_detail(error.to_string())
    })
}

fn connection_error(error: mongodb::error::Error) -> AppError {
    AppError::new(ErrorKind::Connection, "MongoDB operation failed").with_detail(error.to_string())
}

async fn ping_db(client: &mongodb::Client) -> Result<()> {
    client
        .database("admin")
        .run_command(mongodb::bson::doc! { "ping": 1 })
        .await
        .map_err(connection_error)?;
    Ok(())
}

#[async_trait]
impl ConnectionTester for MongoDbConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::MongoDb
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;
        let started_at = Instant::now();
        let client = mongo_client(profile, password).await?;
        ping_db(&client).await?;
        Ok(ConnectionTestReport::success(
            DataSourceKind::MongoDb,
            started_at.elapsed().as_millis(),
        ))
    }
}

#[async_trait]
impl DocumentConnector for MongoDbDocumentConnector {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::MongoDb
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<()> {
        profile.validate()?;
        let client = mongo_client(profile, password).await?;
        ping_db(&client).await
    }

    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<DocumentDatabase>> {
        let client = mongo_client(profile, password).await?;
        let databases = client.list_databases().await.map_err(connection_error)?;
        Ok(databases
            .into_iter()
            .filter(|database| !database.empty)
            .map(|database| DocumentDatabase {
                name: database.name,
                size_on_disk: Some(database.size_on_disk),
            })
            .collect())
    }

    async fn list_collections(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: &str,
    ) -> Result<Vec<DocumentCollection>> {
        let client = mongo_client(profile, password).await?;
        let names = client
            .database(database)
            .list_collection_names()
            .await
            .map_err(connection_error)?;
        Ok(names
            .into_iter()
            .map(|name| DocumentCollection {
                name,
                database: database.to_string(),
                document_count: None,
            })
            .collect())
    }

    async fn find_documents(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: &str,
        collection: &str,
        filter: Option<&serde_json::Value>,
        limit: u32,
    ) -> Result<Vec<serde_json::Value>> {
        let client = mongo_client(profile, password).await?;
        let collection: mongodb::Collection<Document> =
            client.database(database).collection(collection);

        let filter = match filter {
            Some(value) => mongodb::bson::to_document(value).map_err(|error| {
                AppError::new(ErrorKind::Validation, "invalid MongoDB filter document")
                    .with_detail(error.to_string())
            })?,
            None => Document::new(),
        };

        let mut cursor = collection
            .find(filter)
            .limit(limit.max(1) as i64)
            .await
            .map_err(connection_error)?;

        let mut documents = Vec::new();
        while let Some(item) = futures_util::StreamExt::next(&mut cursor).await {
            let document = item.map_err(connection_error)?;
            documents.push(bson_to_json(&document)?);
        }
        Ok(documents)
    }

    async fn execute_document_query(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: &str,
        collection: &str,
        query: &serde_json::Value,
        limit: u32,
        offset: u64,
    ) -> Result<serde_json::Value> {
        use mongodb::bson::doc;
        if !query.is_object() {
            return Err(AppError::validation("MongoDB query must be an object"));
        }
        let operation = query
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("find");
        if profile.security.read_only
            && (matches!(operation, "insert" | "update" | "delete") || contains_write_stage(query))
        {
            return Err(AppError::new(
                ErrorKind::Permission,
                "connection is read-only",
            ));
        }
        if operation == "aggregate" && contains_write_stage(query) {
            return Err(AppError::validation(
                "Paginated aggregation does not support $out or $merge",
            ));
        }
        let limit = limit.clamp(1, 10000);
        let client = mongo_client(profile, password).await?;
        let collection: mongodb::Collection<Document> =
            client.database(database).collection(collection);
        let document = |key: &str| -> Result<Document> {
            match query.get(key) {
                Some(value) => json_document(value),
                None => Ok(Document::new()),
            }
        };
        let mut documents = Vec::new();
        match operation {
            "find" => {
                let filter = if query.get("operation").is_some() {
                    document("filter")?
                } else {
                    json_document(query)?
                };
                let mut action = collection
                    .find(filter)
                    .limit(i64::from(limit))
                    .skip(offset)
                    .max_time(std::time::Duration::from_secs(30));
                if query.get("operation").is_some() && query.get("projection").is_some() {
                    action = action.projection(document("projection")?);
                }
                let mut sort = if query.get("operation").is_some() {
                    document("sort")?
                } else {
                    Document::new()
                };
                // A unique tiebreaker keeps equal sort values stable across pages.
                if !sort.contains_key("_id") {
                    sort.insert("_id", 1);
                }
                action = action.sort(sort);
                let mut cursor = action.await.map_err(connection_error)?;
                while let Some(item) = futures_util::StreamExt::next(&mut cursor).await {
                    documents.push(bson_to_json(&item.map_err(connection_error)?)?);
                }
            }
            "aggregate" => {
                let values = query
                    .get("pipeline")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| AppError::validation("aggregate requires a pipeline array"))?;
                let mut pipeline = values
                    .iter()
                    .map(json_document)
                    .collect::<Result<Vec<_>>>()?;
                if offset > i64::MAX as u64 {
                    return Err(AppError::validation("offset is too large"));
                }
                if offset > 0 {
                    pipeline.push(doc! { "$skip": offset as i64 });
                }
                pipeline.push(doc! { "$limit": i64::from(limit) });
                let mut cursor = collection
                    .aggregate(pipeline)
                    .max_time(std::time::Duration::from_secs(30))
                    .await
                    .map_err(connection_error)?;
                while let Some(item) = futures_util::StreamExt::next(&mut cursor).await {
                    documents.push(bson_to_json(&item.map_err(connection_error)?)?);
                }
            }
            "insert" => {
                if !query.get("document").is_some_and(|v| v.is_object()) {
                    return Err(AppError::validation("insert requires a document object"));
                }
                let result = collection
                    .insert_one(document("document")?)
                    .await
                    .map_err(connection_error)?;
                return Ok(serde_json::json!({ "inserted_id": result.inserted_id }));
            }
            "update" | "delete" => {
                if !query
                    .get("filter")
                    .is_some_and(|v| v.is_object() && !v.as_object().unwrap().is_empty())
                {
                    return Err(AppError::validation(
                        "update/delete requires a non-empty filter",
                    ));
                }
                if operation == "delete" {
                    let result = collection
                        .delete_one(document("filter")?)
                        .await
                        .map_err(connection_error)?;
                    return Ok(serde_json::json!({ "deleted_count": result.deleted_count }));
                }
                let update = document("update")?;
                if update.is_empty() || !update.keys().all(|key| key.starts_with('$')) {
                    return Err(AppError::validation(
                        "update requires update operators such as $set",
                    ));
                }
                let result = collection
                    .update_one(document("filter")?, update)
                    .await
                    .map_err(connection_error)?;
                return Ok(
                    serde_json::json!({ "matched_count": result.matched_count, "modified_count": result.modified_count }),
                );
            }
            _ => return Err(AppError::validation("unknown document operation")),
        }
        Ok(serde_json::json!({ "documents": documents, "offset": offset, "limit": limit }))
    }

    async fn delete_document(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: &str,
        collection: &str,
        document_id: &serde_json::Value,
    ) -> Result<u64> {
        if profile.security.read_only {
            return Err(AppError::new(
                ErrorKind::Permission,
                "connection is read-only",
            ));
        }
        let client = mongo_client(profile, password).await?;
        let collection: mongodb::Collection<Document> =
            client.database(database).collection(collection);

        let id = mongodb::bson::Bson::try_from(document_id.clone()).map_err(|error| {
            AppError::new(ErrorKind::Validation, "invalid MongoDB document id")
                .with_detail(error.to_string())
        })?;
        let mut query = Document::new();
        query.insert("_id", id);

        let result = collection
            .delete_one(query)
            .await
            .map_err(connection_error)?;
        Ok(result.deleted_count)
    }
}

/// Decode canonical Extended JSON as BSON (including ObjectId/date/binary).
fn json_document(value: &serde_json::Value) -> Result<Document> {
    let bson = mongodb::bson::Bson::try_from(value.clone())
        .map_err(|e| AppError::validation("invalid Extended JSON").with_detail(e.to_string()))?;
    bson.as_document()
        .cloned()
        .ok_or_else(|| AppError::validation("expected a JSON object"))
}

fn contains_write_stage(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, value)| {
            matches!(key.as_str(), "$out" | "$merge") || contains_write_stage(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(contains_write_stage),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_extended_json_ids_and_rejects_write_pipelines() {
        let doc =
            json_document(&serde_json::json!({"_id":{"$oid":"507f1f77bcf86cd799439011"}})).unwrap();
        assert!(doc.get_object_id("_id").is_ok());
        assert!(contains_write_stage(
            &serde_json::json!({"pipeline":[{"$facet":{"a":[{"$merge":"other"}]}}]})
        ));
        assert!(!contains_write_stage(
            &serde_json::json!({"pipeline":[{"$match":{"x":1}}]})
        ));
    }
}
