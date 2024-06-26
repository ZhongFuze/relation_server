mod contract;
mod hold;
mod identity;
mod identity_graph;
mod proof;
mod relation;
mod resolve;

use self::{hold::HoldQuery, identity::IdentityQuery, proof::ProofQuery, resolve::ResolveQuery};
use async_graphql::{MergedObject, Object};
const API_VERSION: &str = "0.1";

/// Base struct of GraphQL query request.
#[derive(MergedObject, Default)]
pub struct Query(
    GeneralQuery,
    IdentityQuery,
    ResolveQuery,
    ProofQuery,
    HoldQuery,
);

#[derive(Default)]
pub struct GeneralQuery;

#[Object]
impl GeneralQuery {
    async fn ping(&self) -> &'static str {
        "Pong!"
    }

    async fn api_version(&self) -> &'static str {
        API_VERSION
    }
}
