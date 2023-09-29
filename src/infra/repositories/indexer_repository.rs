use std::str::FromStr;

use axum::async_trait;
use diesel::{ExpressionMethods, Insertable, QueryDsl, Queryable, Selectable, SelectableHelper};
use diesel_async::pooled_connection::deadpool::Pool;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde::{Deserialize, Serialize};
use strum::ParseError;
use uuid::Uuid;

use crate::domain::models::indexer::{IndexerModel, IndexerStatus, IndexerType};
use crate::infra::db::schema::indexers;
use crate::infra::errors::InfraError;

#[derive(Serialize, Queryable, Selectable)]
#[diesel(table_name = indexers)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct IndexerDb {
    pub id: Uuid,
    pub status: String,
    pub indexer_type: String,
    pub process_id: Option<i64>,
    pub target_url: String,
}

#[derive(Deserialize)]
pub struct IndexerFilter {
    pub status: Option<String>,
}

#[derive(Deserialize, Insertable)]
#[diesel(table_name = indexers)]
pub struct NewIndexerDb {
    pub id: Uuid,
    pub status: String,
    pub indexer_type: String,
    pub target_url: String,
}

#[derive(Deserialize, Insertable)]
#[diesel(table_name = indexers)]
pub struct UpdateIndexerStatusDb {
    pub id: Uuid,
    pub status: String,
}

#[derive(Deserialize, Insertable)]
#[diesel(table_name = indexers)]
pub struct UpdateIndexerStatusAndProcessIdDb {
    pub id: Uuid,
    pub status: String,
    pub process_id: i64,
}

#[async_trait]
pub trait Repository {
    async fn insert(&mut self, new_indexer: NewIndexerDb) -> Result<IndexerModel, InfraError>;
    async fn get(&self, id: Uuid) -> Result<IndexerModel, InfraError>;
    async fn get_all(&self, filter: IndexerFilter) -> Result<Vec<IndexerModel>, InfraError>;
    async fn update_status(&mut self, indexer: UpdateIndexerStatusDb) -> Result<IndexerModel, InfraError>;
    async fn update_status_and_process_id(
        &mut self,
        indexer: UpdateIndexerStatusAndProcessIdDb,
    ) -> Result<IndexerModel, InfraError>;
}

pub struct IndexerRepository<'a> {
    pool: &'a Pool<AsyncPgConnection>,
}

impl IndexerRepository<'_> {
    pub fn new(pool: &Pool<AsyncPgConnection>) -> IndexerRepository {
        IndexerRepository { pool }
    }
}

#[async_trait]
impl Repository for IndexerRepository<'_> {
    async fn insert(&mut self, new_indexer: NewIndexerDb) -> Result<IndexerModel, InfraError> {
        _insert(self.pool, new_indexer).await
    }

    async fn get(&self, id: Uuid) -> Result<IndexerModel, InfraError> {
        get(self.pool, id).await
    }

    async fn get_all(&self, filter: IndexerFilter) -> Result<Vec<IndexerModel>, InfraError> {
        get_all(self.pool, filter).await
    }

    async fn update_status(&mut self, indexer: UpdateIndexerStatusDb) -> Result<IndexerModel, InfraError> {
        update_status(self.pool, indexer).await
    }

    async fn update_status_and_process_id(
        &mut self,
        indexer: UpdateIndexerStatusAndProcessIdDb,
    ) -> Result<IndexerModel, InfraError> {
        update_status_and_process_id(self.pool, indexer).await
    }
}

async fn _insert(pool: &Pool<AsyncPgConnection>, new_indexer: NewIndexerDb) -> Result<IndexerModel, InfraError> {
    let mut conn = pool.get().await?;
    let res = diesel::insert_into(indexers::table)
        .values(new_indexer)
        .returning(IndexerDb::as_returning())
        .get_result(&mut conn)
        .await?
        .try_into()
        .map_err(InfraError::ParseError)?;

    Ok(res)
}

async fn get(pool: &Pool<AsyncPgConnection>, id: Uuid) -> Result<IndexerModel, InfraError> {
    let mut conn = pool.get().await?;
    let res = indexers::table
        .filter(indexers::id.eq(id))
        .select(IndexerDb::as_select())
        .get_result::<IndexerDb>(&mut conn)
        .await?
        .try_into()
        .map_err(InfraError::ParseError)?;

    Ok(res)
}

async fn get_all(pool: &Pool<AsyncPgConnection>, filter: IndexerFilter) -> Result<Vec<IndexerModel>, InfraError> {
    let mut conn = pool.get().await?;
    let mut query = indexers::table.into_boxed::<diesel::pg::Pg>();
    if let Some(status) = filter.status {
        query = query.filter(indexers::status.eq(status));
    }
    let res: Vec<IndexerDb> = query.select(IndexerDb::as_select()).load::<IndexerDb>(&mut conn).await?;

    let posts: Vec<IndexerModel> = res
        .into_iter()
        .map(|indexer_db| indexer_db.try_into())
        .collect::<Result<Vec<IndexerModel>, ParseError>>()
        .map_err(InfraError::ParseError)?;

    Ok(posts)
}

async fn update_status(
    pool: &Pool<AsyncPgConnection>,
    indexer: UpdateIndexerStatusDb,
) -> Result<IndexerModel, InfraError> {
    let mut conn = pool.get().await?;
    let res = diesel::update(indexers::table)
        .filter(indexers::id.eq(indexer.id))
        .set(indexers::status.eq(indexer.status))
        .get_result::<IndexerDb>(&mut conn)
        .await?
        .try_into()
        .map_err(InfraError::ParseError)?;

    Ok(res)
}

async fn update_status_and_process_id(
    pool: &Pool<AsyncPgConnection>,
    indexer: UpdateIndexerStatusAndProcessIdDb,
) -> Result<IndexerModel, InfraError> {
    let mut conn = pool.get().await?;
    let res = diesel::update(indexers::table)
        .filter(indexers::id.eq(indexer.id))
        .set((indexers::status.eq(indexer.status), indexers::process_id.eq(indexer.process_id)))
        .get_result::<IndexerDb>(&mut conn)
        .await?
        .try_into()
        .map_err(InfraError::ParseError)?;

    Ok(res)
}

impl TryFrom<NewIndexerDb> for IndexerModel {
    type Error = ParseError;
    fn try_from(value: NewIndexerDb) -> Result<Self, Self::Error> {
        let model = IndexerDb {
            id: value.id,
            status: value.status,
            indexer_type: value.indexer_type,
            target_url: value.target_url,
            process_id: None,
        }
        .try_into()?;
        Ok(model)
    }
}

impl TryFrom<IndexerDb> for IndexerModel {
    type Error = ParseError;
    fn try_from(value: IndexerDb) -> Result<Self, Self::Error> {
        let model = IndexerModel {
            id: value.id,
            status: IndexerStatus::from_str(value.status.as_str())?,
            process_id: value.process_id,
            indexer_type: IndexerType::from_str(value.indexer_type.as_str())?,
            target_url: value.target_url,
        };
        Ok(model)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_from_indexer_db_to_indexer_model() {
        let id = Uuid::new_v4();
        let indexer_db = IndexerDb {
            id,
            status: "Created".to_string(),
            indexer_type: "Webhook".to_string(),
            process_id: Some(1234),
            target_url: "http://example.com".to_string(),
        };

        let indexer_model: IndexerModel = indexer_db.try_into().unwrap();

        assert_eq!(indexer_model.id, id);
        assert_eq!(indexer_model.status, IndexerStatus::from_str("Created").unwrap());
        assert_eq!(indexer_model.indexer_type, IndexerType::from_str("Webhook").unwrap());
        assert_eq!(indexer_model.process_id, Some(1234));
        assert_eq!(indexer_model.target_url, "http://example.com".to_string());
    }

    // You can add more tests, for example, to handle cases where the status or indexer_type strings are
    // invalid. This will test the unwrapping and ensure that the conversion panics as expected.
    #[test]
    #[should_panic(expected = "VariantNotFound")]
    fn test_invalid_status_conversion() {
        let indexer_db = IndexerDb {
            id: Uuid::new_v4(),
            status: "InvalidStatus".to_string(),
            indexer_type: "Webhook".to_string(),
            process_id: Some(1234),
            target_url: "http://example.com".to_string(),
        };

        let _: IndexerModel = indexer_db.try_into().unwrap();
    }

    #[test]
    #[should_panic(expected = "VariantNotFound")]
    fn test_invalid_type_conversion() {
        let indexer_db = IndexerDb {
            id: Uuid::new_v4(),
            status: "Created".to_string(),
            indexer_type: "InvalidType".to_string(),
            process_id: Some(1234),
            target_url: "http://example.com".to_string(),
        };

        let _: IndexerModel = indexer_db.try_into().unwrap();
    }
}
