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
    let uri = mongo_uri(profile, password)?;
    mongodb::Client::with_uri_str(&uri).await.map_err(|error| {
        AppError::new(ErrorKind::Connection, "failed to connect to MongoDB").with_detail(format!(
            "{error} (URI host/port from profile; credentials are not included)"
        ))
    })
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

    async fn delete_document(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: &str,
        collection: &str,
        document_id: &serde_json::Value,
    ) -> Result<u64> {
        let client = mongo_client(profile, password).await?;
        let collection: mongodb::Collection<Document> =
            client.database(database).collection(collection);

        let id = mongodb::bson::to_bson(document_id).map_err(|error| {
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
