use crate::{
    config::C,
    error::Error,
    tigergraph::{
        edge::{EdgeUnion, FromWithParams as EdgeFromWithParams, HoldRecord, ProofRecord},
        vertex::{FromWithParams, Vertex, VertexRecord},
        Attribute, BaseResponse, OpCode, Transfer, IDENTITY_GRAPH,
    },
    upstream::{vec_string_to_vec_datasource, DataSource, Platform},
    util::{
        naive_datetime_from_string, naive_datetime_to_string, naive_now,
        option_naive_datetime_from_string, option_naive_datetime_to_string, parse_body,
    },
};

use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use http::uri::InvalidUri;
use hyper::Method;
use hyper::{client::HttpConnector, Body, Client};
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::{from_value, json, to_value, Value};
use std::collections::HashMap;
use std::fmt;
use tracing::{debug, error};
use uuid::Uuid;

pub const VERTEX_NAME: &str = "Identities";

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    /// For `ethereum`, this is the reversed ENS name set by user.
    pub display_name: Option<String>,
    /// URL to target identity profile page on `platform` (if any).
    pub profile_url: Option<String>,
    /// URL to avatar (if any is recorded and given by target platform).
    pub avatar_url: Option<String>,
    /// Account / identity creation time ON TARGET PLATFORM.
    /// This is not necessarily the same as the creation time of the record in the database.
    /// Since `created_at` may not be recorded or given by target platform.
    /// e.g. `Twitter` has a `created_at` in the user profile API.
    /// but `Ethereum` is obviously no such thing.
    #[serde(deserialize_with = "option_naive_datetime_from_string")]
    #[serde(serialize_with = "option_naive_datetime_to_string")]
    pub created_at: Option<NaiveDateTime>,
    /// When this Identity is added into this database. Generated by us.
    #[serde(deserialize_with = "naive_datetime_from_string")]
    #[serde(serialize_with = "naive_datetime_to_string")]
    pub added_at: NaiveDateTime,
    /// When it is updated (re-fetched) by us RelationService. Managed by us.
    #[serde(deserialize_with = "naive_datetime_from_string")]
    #[serde(serialize_with = "naive_datetime_to_string")]
    pub updated_at: NaiveDateTime,
}

#[async_trait]
impl Vertex for Identity {
    fn primary_key(&self) -> String {
        // self.0.v_id.clone()
        format!("{},{}", self.platform, self.identity)
    }

    fn vertex_type(&self) -> String {
        VERTEX_NAME.to_string()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IdentityRecord(VertexRecord<Identity>);

impl FromWithParams<Identity> for IdentityRecord {
    fn from_with_params(v_type: String, v_id: String, attributes: Identity) -> Self {
        IdentityRecord(VertexRecord {
            v_type,
            v_id,
            attributes,
        })
    }
}

impl From<VertexRecord<Identity>> for IdentityRecord {
    fn from(record: VertexRecord<Identity>) -> Self {
        IdentityRecord(record)
    }
}

impl std::ops::Deref for IdentityRecord {
    type Target = VertexRecord<Identity>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for IdentityRecord {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IdentityAttribute(HashMap<String, Attribute>);

// Implement `Transfer` trait for converting `Identity` into a `HashMap<String, Attribute>`.
impl Transfer for Identity {
    fn to_attributes_map(&self) -> HashMap<String, Attribute> {
        let mut attributes_map = HashMap::new();

        attributes_map.insert(
            "id".to_string(),
            Attribute {
                value: json!(self.primary_key()),
                op: Some(OpCode::IgnoreIfExists),
            },
        );
        if let Some(uuid) = self.uuid {
            attributes_map.insert(
                "uuid".to_string(),
                Attribute {
                    value: json!(uuid),
                    op: Some(OpCode::IgnoreIfExists),
                },
            );
        }
        attributes_map.insert(
            "platform".to_string(),
            Attribute {
                value: json!(self.platform),
                op: Some(OpCode::IgnoreIfExists),
            },
        );
        attributes_map.insert(
            "identity".to_string(),
            Attribute {
                value: json!(&self.identity),
                op: Some(OpCode::IgnoreIfExists),
            },
        );
        if let Some(display_name) = self.display_name.clone() {
            attributes_map.insert(
                "display_name".to_string(),
                Attribute {
                    value: json!(display_name),
                    op: None,
                },
            );
        }
        if let Some(profile_url) = self.profile_url.clone() {
            attributes_map.insert(
                "profile_url".to_string(),
                Attribute {
                    value: json!(profile_url),
                    op: None,
                },
            );
        }
        if let Some(avatar_url) = self.avatar_url.clone() {
            attributes_map.insert(
                "avatar_url".to_string(),
                Attribute {
                    value: json!(avatar_url),
                    op: None,
                },
            );
        }
        if let Some(created_at) = self.created_at {
            attributes_map.insert(
                "created_at".to_string(),
                Attribute {
                    value: json!(created_at),
                    op: Some(OpCode::IgnoreIfExists),
                },
            );
        }

        attributes_map.insert(
            "added_at".to_string(),
            Attribute {
                value: json!(self.added_at),
                op: None,
            },
        );
        attributes_map.insert(
            "updated_at".to_string(),
            Attribute {
                value: json!(self.updated_at),
                op: Some(OpCode::Max),
            },
        );
        attributes_map
    }
}

impl Default for Identity {
    fn default() -> Self {
        Self {
            uuid: Default::default(),
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NeighborsWithSource {
    #[serde(flatten)]
    base: BaseResponse,
    // results: Option<Vec<IdentityWithSource>>,
    results: Option<Vec<VertexWithSource>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VertexWithSource {
    vertices: Vec<IdentityWithSource>,
}

#[derive(Clone, Serialize, Debug)]
pub struct IdentityWithSource {
    pub identity: IdentityRecord,
    pub sources: Vec<DataSource>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NeighborsResponse {
    #[serde(flatten)]
    base: BaseResponse,
    results: Option<Vec<EdgeUnions>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct EdgeUnions {
    edges: Vec<EdgeUnion>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VertexResponse {
    #[serde(flatten)]
    base: BaseResponse,
    results: Option<Vec<IdentityRecord>>,
}

impl<'de> Deserialize<'de> for IdentityWithSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct IdentityWithSourceVisitor;
        impl<'de> Visitor<'de> for IdentityWithSourceVisitor {
            type Value = IdentityWithSource;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct IdentityWithSource")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut v_type: Option<String> = None;
                let mut v_id: Option<String> = None;
                let mut attributes: Option<serde_json::Map<String, serde_json::Value>> = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        "v_type" => v_type = Some(map.next_value()?),
                        "v_id" => v_id = Some(map.next_value()?),
                        "attributes" => attributes = Some(map.next_value()?),
                        _ => {}
                    }
                }

                let mut attributes =
                    attributes.ok_or_else(|| de::Error::missing_field("attributes"))?;

                let source_list = attributes
                    .remove("@source_list")
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(de::Error::custom)?;

                let attributes = serde_json::from_value(serde_json::Value::Object(attributes))
                    .map_err(de::Error::custom)?;
                let v_type = v_type.ok_or_else(|| de::Error::missing_field("v_type"))?;
                let v_id = v_id.ok_or_else(|| de::Error::missing_field("v_id"))?;
                let source_list =
                    source_list.ok_or_else(|| de::Error::missing_field("@source_list"))?;
                let sources =
                    vec_string_to_vec_datasource(source_list).map_err(de::Error::custom)?;

                Ok(IdentityWithSource {
                    identity: IdentityRecord(VertexRecord {
                        v_type,
                        v_id,
                        attributes,
                    }),
                    sources,
                })
            }
        }

        deserializer.deserialize_map(IdentityWithSourceVisitor)
    }
}

impl Identity {
    pub fn uuid(&self) -> Option<Uuid> {
        self.uuid
    }

    /// Judge if this record is outdated and should be refetched.
    fn is_outdated(&self) -> bool {
        let outdated_in = Duration::hours(1);
        self.updated_at
            .checked_add_signed(outdated_in)
            .unwrap()
            .lt(&naive_now())
    }

    /// Find `IdentityRecord` by given UUID.
    pub async fn find_by_uuid(
        client: &Client<HttpConnector>,
        uuid: Uuid,
    ) -> Result<Option<IdentityRecord>, Error> {
        // Builtins: http://server:9000/graph/{GraphName}/vertices/{VertexName}/filter=field1="a",field2="b"
        let uri: http::Uri = format!(
            "{}/graph/{}/vertices/{}?filter=uuid=%22{}%22",
            C.tdb.host,
            IDENTITY_GRAPH,
            VERTEX_NAME,
            uuid.to_string(),
        )
        .parse()
        .map_err(|_err: InvalidUri| Error::ParamError(format!("Uri format Error {}", _err)))?;
        let req = hyper::Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(
                "Authorization",
                format!("Bearer {}", C.tdb.identity_graph_token),
            )
            .body(Body::empty())
            .map_err(|_err| Error::ParamError(format!("ParamError Error {}", _err)))?;

        let mut resp = client.request(req).await.map_err(|err| {
            Error::ManualHttpClientError(format!(
                "query filter error | Fail to request: {:?}",
                err.to_string()
            ))
        })?;
        match parse_body::<VertexResponse>(&mut resp).await {
            Ok(r) => {
                if r.base.error {
                    let err_message = format!(
                        "TigerGraph query filter error | Code: {:?}, Message: {:?}",
                        r.base.code, r.base.message
                    );
                    error!(err_message);
                    return Err(Error::General(err_message, resp.status()));
                }
                if let Some(results) = r.results {
                    Ok(results.first().unwrap().to_owned().into())
                } else {
                    Ok(None)
                }
            }
            Err(err) => {
                let err_message = format!("TigerGraph query filter parse_body error: {:?}", err);
                error!(err_message);
                return Err(err);
            }
        }
    }
    /// Find `IdentityRecord` by given platform and identity.
    pub async fn find_by_platform_identity(
        client: &Client<HttpConnector>,
        platform: &Platform,
        identity: &str,
    ) -> Result<Option<IdentityRecord>, Error> {
        // Builtins: http://server:9000/graph/{GraphName}/vertices/{VertexName}/filter=field1="a",field2="b"
        let uri: http::Uri = format!(
            "{}/graph/{}/vertices/{}?filter=platform=%22{}%22,identity=%22{}%22",
            C.tdb.host,
            IDENTITY_GRAPH,
            VERTEX_NAME,
            platform.to_string(),
            identity.to_string(),
        )
        .parse()
        .map_err(|_err: InvalidUri| Error::ParamError(format!("Uri format Error | {}", _err)))?;
        let req = hyper::Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(
                "Authorization",
                format!("Bearer {}", C.tdb.identity_graph_token),
            )
            .body(Body::empty())
            .map_err(|_err| Error::ParamError(format!("ParamError Error | {}", _err)))?;

        let mut resp = client.request(req).await.map_err(|err| {
            Error::ManualHttpClientError(format!(
                "query filter error | Fail to request: {:?}",
                err.to_string()
            ))
        })?;
        match parse_body::<VertexResponse>(&mut resp).await {
            Ok(r) => {
                if r.base.error {
                    let err_message = format!(
                        "TigerGraph query filter error | Code: {:?}, Message: {:?}",
                        r.base.code, r.base.message
                    );
                    error!(err_message);
                    return Err(Error::General(err_message, resp.status()));
                }
                if let Some(results) = r.results {
                    Ok(results.first().unwrap().to_owned().into())
                } else {
                    Ok(None)
                }
            }
            Err(err) => {
                let err_message = format!("TigerGraph query filter parse_body error: {:?}", err);
                error!(err_message);
                return Err(err);
            }
        }
    }
}

impl IdentityRecord {
    pub async fn neighbors(
        &self,
        client: &Client<HttpConnector>,
        depth: u16,
    ) -> Result<Vec<IdentityWithSource>, Error> {
        // query see in Solution: CREATE QUERY neighbors_with_source(VERTEX<Identities> p, INT depth) FOR GRAPH IdentityGraph
        let uri: http::Uri = format!(
            "{}/query/IdentityGraph/neighbors_with_source?p={}&depth={}",
            C.tdb.host, self.v_id, depth,
        )
        .parse()
        .map_err(|_err: InvalidUri| Error::ParamError(format!("Uri format Error {}", _err)))?;

        let req = hyper::Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(
                "Authorization",
                format!("Bearer {}", C.tdb.identity_graph_token),
            )
            .body(Body::empty())
            .map_err(|_err| Error::ParamError(format!("ParamError Error {}", _err)))?;
        let mut resp = client.request(req).await.map_err(|err| {
            Error::ManualHttpClientError(format!(
                "query neighbors_with_source | Fail to request: {:?}",
                err.to_string()
            ))
        })?;

        match parse_body::<NeighborsWithSource>(&mut resp).await {
            Ok(r) => {
                if r.base.error {
                    let err_message = format!(
                        "TigerGraph query neighbors_with_source error | Code: {:?}, Message: {:?}",
                        r.base.code, r.base.message
                    );
                    error!(err_message);
                    return Err(Error::General(err_message, resp.status()));
                }
                if let Some(results) = r.results {
                    if results.len() > 0 {
                        let res: VertexWithSource = results.first().unwrap().to_owned().into();
                        // filter out self::node_id
                        Ok(res
                            .vertices
                            .into_iter()
                            .filter(|target| target.identity.v_id != self.v_id)
                            .collect())
                    } else {
                        Ok(vec![])
                    }
                } else {
                    Ok(vec![])
                }
            }
            Err(err) => {
                let err_message = format!(
                    "TigerGraph neighbors_with_source parse_body error: {:?}",
                    err
                );
                error!(err_message);
                return Err(err);
            }
        }
    }
    // Return all neighbors of this identity with traversal paths.
    pub async fn neighbors_with_traversal(
        &self,
        client: &Client<HttpConnector>,
        depth: u16,
    ) -> Result<Vec<EdgeUnion>, Error> {
        // query see in Solution: CREATE QUERY neighbors(VERTEX<Identities> p, INT depth) FOR GRAPH IdentityGraph
        let uri: http::Uri = format!(
            "{}/query/IdentityGraph/neighbors?p={}&depth={}",
            C.tdb.host, self.v_id, depth,
        )
        .parse()
        .map_err(|_err: InvalidUri| Error::ParamError(format!("Uri format Error {}", _err)))?;
        let req = hyper::Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(
                "Authorization",
                format!("Bearer {}", C.tdb.identity_graph_token),
            )
            .body(Body::empty())
            .map_err(|_err| Error::ParamError(format!("ParamError Error {}", _err)))?;
        let mut resp = client.request(req).await.map_err(|err| {
            Error::ManualHttpClientError(format!(
                "query neighbors | Fail to request: {:?}",
                err.to_string()
            ))
        })?;
        match parse_body::<NeighborsResponse>(&mut resp).await {
            Ok(r) => {
                if r.base.error {
                    let err_message = format!(
                        "TigerGraph query neighbors error | Code: {:?}, Message: {:?}",
                        r.base.code, r.base.message
                    );
                    error!(err_message);
                    return Err(Error::General(err_message, resp.status()));
                }

                if let Some(results) = r.results {
                    if results.len() > 0 {
                        let res: EdgeUnions = results.first().unwrap().to_owned().into();
                        Ok(res.edges)
                    } else {
                        Ok(vec![])
                    }
                } else {
                    Ok(vec![])
                }
            }
            Err(err) => {
                let err_message = format!("TigerGraph query neighbors parse_body error: {:?}", err);
                error!(err_message);
                return Err(err);
            }
        }
    }
}
