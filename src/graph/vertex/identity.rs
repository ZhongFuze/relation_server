use crate::{
    error::Error,
    graph::{edge::Proof, vertex::Vertex},
    upstream::{DataSource, Platform},
    util::naive_now,
};
use aragog::{
    query::{Comparison, Filter, Query, QueryResult},
    DatabaseConnection, DatabaseRecord, Record,
};
use async_trait::async_trait;
use chrono::{Duration, NaiveDateTime};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize, Serialize, Record)]
#[collection_name = "Identities"]
pub struct Identity {
    /// UUID of this record. Generated by us to provide a better
    /// global-uniqueness for future P2P-network data exchange
    /// scenario.
    pub uuid: Option<Uuid>,
    /// Platform.
    pub platform: Platform,
    /// Identity on target platform.
    /// Username or database primary key (prefer, usually digits).
    /// e.g. `Twitter` has this digits-like user ID thing.
    pub identity: String,
    /// Usually user-friendly screen name.
    /// e.g. for `Twitter`, this is the user's `screen_name`.
    pub display_name: String,
    /// URL to target identity profile page on `platform` (if any).
    pub profile_url: Option<String>,
    /// URL to avatar (if any is recorded and given by target platform).
    pub avatar_url: Option<String>,
    /// Account / identity creation time ON TARGET PLATFORM.
    /// This is not necessarily the same as the creation time of the record in the database.
    /// Since `created_at` may not be recorded or given by target platform.
    /// e.g. `Twitter` has a `created_at` in the user profile API.
    /// but `Ethereum` is obviously no such thing.
    pub created_at: Option<NaiveDateTime>,
    /// When this Identity is added into this database. Generated by us.
    pub added_at: NaiveDateTime,
    /// When it is updated (re-fetched) by us RelationService. Managed by us.
    pub updated_at: NaiveDateTime,
}

impl Default for Identity {
    fn default() -> Self {
        Self {
            uuid: None,
            platform: Platform::Twitter,
            identity: Default::default(),
            display_name: Default::default(),
            profile_url: None,
            avatar_url: None,
            created_at: None,
            added_at: naive_now(),
            updated_at: naive_now(),
        }
    }
}

impl PartialEq for Identity {
    fn eq(&self, other: &Self) -> bool {
        self.uuid.is_some() && other.uuid.is_some() && self.uuid == other.uuid
    }
}

impl Identity {
    /// Find record by given platform and identity.
    pub async fn find_by_platform_identity(
        db: &DatabaseConnection,
        platform: &Platform,
        identity: &str,
    ) -> Result<Option<IdentityRecord>, Error> {
        let query = Self::query().filter(
            Filter::new(Comparison::field("platform").equals_str(platform))
                .and(Comparison::field("identity").equals_str(identity)),
        );
        let query_result = Self::get(&query, db).await?;

        if query_result.len() == 0 {
            Ok(None)
        } else {
            Ok(Some(query_result.first().unwrap().to_owned().into()))
        }
    }
}

#[async_trait]
impl Vertex<IdentityRecord> for Identity {
    fn uuid(&self) -> Option<Uuid> {
        self.uuid
    }

    /// Do create / update side-effect.
    /// Used by upstream crawler.
    async fn create_or_update(&self, db: &DatabaseConnection) -> Result<IdentityRecord, Error> {
        // Find first
        let found = Self::find_by_platform_identity(db, &self.platform, &self.identity).await?;
        match found {
            None => {
                // Create
                let mut to_be_created = self.clone();
                to_be_created.uuid = to_be_created.uuid.or(Some(Uuid::new_v4()));
                to_be_created.added_at = naive_now();
                to_be_created.updated_at = naive_now();
                let created = DatabaseRecord::create(to_be_created, db).await?;
                Ok(created.into())
            }
            Some(mut found) => {
                // Update
                found.display_name = self.display_name.clone();
                found.profile_url = self.profile_url.clone();
                found.avatar_url = self.avatar_url.clone();
                found.created_at = self.created_at.or(found.created_at.clone());
                found.updated_at = naive_now();

                found.save(db).await?;
                Ok(found.into())
            }
        }
    }

    async fn find_by_uuid(
        db: &DatabaseConnection,
        uuid: Uuid,
    ) -> Result<Option<IdentityRecord>, Error> {
        let query = Identity::query().filter(Comparison::field("uuid").equals_str(uuid).into());
        let query_result = Identity::get(&query, db).await?;
        if query_result.len() == 0 {
            Ok(None)
        } else {
            Ok(Some(query_result.first().unwrap().to_owned().into()))
        }
    }

    /// Judge if this record is outdated and should be refetched.
    fn is_outdated(&self) -> bool {
        let outdated_in = Duration::hours(1);
        self.updated_at
            .clone()
            .checked_add_signed(outdated_in)
            .unwrap()
            .lt(&naive_now())
    }
}

/// Result struct queried from graph database.
/// Useful by GraphQL side to wrap more function / traits.
#[derive(Clone, Deserialize, Serialize, Default, Debug)]
pub struct IdentityRecord(pub DatabaseRecord<Identity>);

impl std::ops::Deref for IdentityRecord {
    type Target = DatabaseRecord<Identity>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for IdentityRecord {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<DatabaseRecord<Identity>> for IdentityRecord {
    fn from(record: DatabaseRecord<Identity>) -> Self {
        Self(record)
    }
}

impl IdentityRecord {
    /// Returns all neighbors of this identity. Depth and upstream data souce can be specified.
    pub async fn neighbors(
        &self,
        db: &DatabaseConnection,
        depth: u16,
        _source: Option<DataSource>,
    ) -> Result<Vec<Self>, Error> {
        // TODO: make `source` filter work.
        // let proof_query = match source {
        //     None => Proof::query(),
        //     Some(source) => Proof::query().filter(
        //         Comparison::field("source") // Don't know why this won't work
        //             .equals_str(source.to_string())
        //             .into(),
        //     ).distinct(),
        // };

        let result: QueryResult<Identity> = Query::any(1, depth, Proof::COLLECTION_NAME, self.id())
            .call(db)
            .await?;
        Ok(result.iter().map(|r| r.to_owned().into()).collect())
    }
}

#[cfg(test)]
mod tests {
    use aragog::DatabaseConnection;
    use fake::{Dummy, Fake, Faker};
    use uuid::Uuid;

    use super::{Identity, IdentityRecord};
    use crate::{
        error::Error,
        graph::{edge::Proof, new_db_connection, Edge, Vertex},
        upstream::Platform,
        util::naive_now,
    };

    impl Identity {
        /// Create test dummy data in database.
        pub async fn create_dummy(db: &DatabaseConnection) -> Result<IdentityRecord, Error> {
            let identity: Identity = Faker.fake();
            Ok(identity.create_or_update(db).await?.into())
        }
    }

    impl Dummy<Faker> for Identity {
        fn dummy_with_rng<R: rand::Rng + ?Sized>(config: &Faker, _rng: &mut R) -> Self {
            Self {
                uuid: Some(Uuid::new_v4()),
                platform: Platform::Twitter,
                identity: config.fake(),
                display_name: config.fake(),
                profile_url: Some(config.fake()),
                avatar_url: Some(config.fake()),
                created_at: Some(config.fake()),
                added_at: naive_now(),
                updated_at: naive_now(),
            }
        }
    }

    #[tokio::test]
    async fn test_create() -> Result<(), Error> {
        let identity: Identity = Faker.fake();
        let db = new_db_connection().await?;
        let result = identity.create_or_update(&db).await?;
        assert!(result.uuid.is_some());
        assert!(result.key().len() > 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_update() -> Result<(), Error> {
        let db = new_db_connection().await?;

        let mut identity: Identity = Faker.fake();
        let created = identity.create_or_update(&db).await?;

        // Change some of data
        identity.avatar_url = Some(Faker.fake());
        identity.profile_url = Some(Faker.fake());
        let updated = identity.create_or_update(&db).await?;

        assert_eq!(created.uuid, updated.uuid);
        assert_eq!(created.key(), updated.key());
        assert_ne!(created.avatar_url, updated.avatar_url);
        assert_ne!(created.profile_url, updated.profile_url);

        Ok(())
    }

    #[tokio::test]
    async fn test_find_by_uuid() -> Result<(), Error> {
        let db = new_db_connection().await?;
        let created = Identity::create_dummy(&db).await?;
        let uuid = created.uuid.unwrap();

        let found = Identity::find_by_uuid(&db, uuid).await?;
        assert_eq!(found.unwrap().uuid, created.uuid);

        Ok(())
    }

    #[tokio::test]
    async fn test_find_by_platform_identity() -> Result<(), Error> {
        let db = new_db_connection().await?;
        let created = Identity::create_dummy(&db).await?;

        let found = Identity::find_by_platform_identity(&db, &created.platform, &created.identity)
            .await?
            .expect("Record not found");
        assert_eq!(found.uuid, created.uuid);

        Ok(())
    }

    #[tokio::test]
    async fn test_neighbors() -> Result<(), Error> {
        let db = new_db_connection().await?;
        // ID2 <--Proof1-- ID1 --Proof2--> ID3
        let id1 = Identity::create_dummy(&db).await?;
        let id2 = Identity::create_dummy(&db).await?;
        let id3 = Identity::create_dummy(&db).await?;
        let proof1_raw: Proof = Faker.fake();
        let proof2_raw: Proof = Faker.fake();
        proof1_raw.connect(&db, &id1, &id2).await?;
        proof2_raw.connect(&db, &id1, &id3).await?;

        let neighbors = id1.neighbors(&db, 2, None).await?;
        assert_eq!(2, neighbors.len());
        assert!(neighbors
            .iter()
            .all(|i| i.uuid == id2.uuid || i.uuid == id3.uuid));
        Ok(())
    }
}
