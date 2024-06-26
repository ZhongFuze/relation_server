#[cfg(test)]
mod tests;

use crate::config::C;
use crate::error::Error;
use crate::tigergraph::edge::{Hold, HyperEdge, Wrapper, HOLD_CONTRACT, HYPER_EDGE};
use crate::tigergraph::upsert::create_identity_to_contract_hold_record;
use crate::tigergraph::vertex::{Contract, IdentitiesGraph, Identity};
use crate::tigergraph::{EdgeList, EdgeWrapperEnum};
use crate::upstream::{
    Chain, ContractCategory, DataSource, Fetcher, Platform, Target, TargetProcessedList,
};
use crate::util::{
    make_client, make_http_client, naive_now, parse_body, request_with_timeout, utc_to_naive,
};
use async_trait::async_trait;
use futures::future::join_all;
use http::uri::InvalidUri;
use hyper::{Body, Method};
use serde::Deserialize;
use std::str::FromStr;
use tracing::{error, info};
use uuid::Uuid;

use super::DataFetcher;

#[derive(Deserialize, Debug, Clone)]
pub struct Rss3Response {
    pub total: i64,
    pub cursor: Option<String>,
    pub result: Vec<ResultItem>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ResultItem {
    pub timestamp: String,
    #[serde(default)]
    pub hash: String,
    pub owner: String,
    pub address_from: String,
    #[serde(default)]
    pub address_to: String,
    pub network: String,
    pub platform: Option<String>,
    pub tag: String,
    #[serde(rename = "type")]
    pub tag_type: String,
    pub success: bool,
    pub actions: Vec<ActionItem>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ActionItem {
    pub tag: String,
    #[serde(rename = "type")]
    pub tag_type: String,
    #[serde(default)]
    pub hash: String,
    pub index: i64,
    pub address_from: String,
    #[serde(default)]
    pub address_to: String,
    pub metadata: MetaData,
    #[serde(default)]
    pub related_urls: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MetaData {
    pub id: Option<String>,
    pub name: Option<String>,
    pub image: Option<String>,
    pub value: Option<String>,
    pub symbol: Option<String>,
    pub standard: Option<String>,
    pub contract_address: Option<String>,
    pub handle: Option<String>,
}

const PAGE_LIMIT: i64 = 500;
pub struct Rss3 {}

#[async_trait]
impl Fetcher for Rss3 {
    async fn fetch(target: &Target) -> Result<TargetProcessedList, Error> {
        if !Self::can_fetch(target) {
            return Ok(vec![]);
        }

        match target {
            Target::Identity(platform, identity) => fetch_nfts_by_account(platform, identity).await,
            Target::NFT(_, _, _, _) => todo!(),
        }
    }

    async fn batch_fetch(target: &Target) -> Result<(TargetProcessedList, EdgeList), Error> {
        if !Self::can_fetch(target) {
            return Ok((vec![], vec![]));
        }

        match target.platform()? {
            Platform::Ethereum => batch_fetch_nfts(target).await,
            _ => Ok((vec![], vec![])),
        }
    }

    fn can_fetch(target: &Target) -> bool {
        target.in_platform_supported(vec![Platform::Ethereum])
    }
}

async fn batch_fetch_nfts(target: &Target) -> Result<(TargetProcessedList, EdgeList), Error> {
    let client = make_client();
    let address = target.identity()?.to_lowercase();
    let mut cursor = String::from("");

    let mut edges = EdgeList::new();
    let hv = IdentitiesGraph::default();

    loop {
        let uri: http::Uri;
        if cursor.len() == 0 {
            uri = format!(
                "{}/{}?tag=collectible&include_poap=true&refresh=true",
                C.upstream.rss3_service.url, address
            )
            .parse()
            .map_err(|_err: InvalidUri| Error::ParamError(format!("Uri format Error {}", _err)))?;
        } else {
            uri = format!(
                "{}/{}?tag=collectible&include_poap=true&refresh=true&cursor={}",
                C.upstream.rss3_service.url, address, cursor
            )
            .parse()
            .map_err(|_err: InvalidUri| Error::ParamError(format!("Uri format Error {}", _err)))?;
        }

        let req = hyper::Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .map_err(|_err| Error::ParamError(format!("Rss3 Build Request Error {}", _err)))?;

        let mut resp = request_with_timeout(&client, req, None)
            .await
            .map_err(|err| {
                Error::ManualHttpClientError(format!(
                    "Rss3 fetch fetch | error: {:?}",
                    err.to_string()
                ))
            })?;

        let body: Rss3Response = parse_body(&mut resp).await?;
        if body.total == 0 {
            info!("Rss3 Response result is empty");
            // break;
        }

        let result: Vec<ResultItem> = body
            .result
            .into_iter()
            .filter(|p| p.owner == address)
            .collect();

        for p in result.into_iter() {
            if p.actions.len() == 0 {
                continue;
            }

            let found = p
                .actions
                .iter()
                // collectible (transfer, mint, burn) share the same UMS, but approve/revoke not.
                // we need to record is the `hold` relation, so burn is excluded
                .filter(|a| {
                    (a.tag_type == "transfer" && p.tag_type == "transfer")
                        || (a.tag_type == "mint" && p.tag_type == "mint")
                })
                .find(|a| (p.tag == "collectible" && a.tag == "collectible"));

            if found.is_none() {
                continue;
            }
            let real_action = found.unwrap();

            if real_action.metadata.symbol.is_none()
                || real_action.metadata.symbol.as_ref().unwrap() == &String::from("ENS")
            {
                continue;
            }

            let mut nft_category = ContractCategory::Unknown;
            let standard = real_action.metadata.standard.clone();
            if let Some(standard) = standard {
                if standard == "ERC-721".to_string() {
                    nft_category = ContractCategory::ERC721;
                } else if standard == "ERC-1155".to_string() {
                    nft_category = ContractCategory::ERC1155;
                }
            }
            if real_action.tag_type == "poap".to_string() {
                nft_category = ContractCategory::POAP;
            }

            let created_at_naive = match p.timestamp.as_ref() {
                "" => None,
                timestamp => match utc_to_naive(timestamp.to_string()) {
                    Ok(naive_dt) => Some(naive_dt),
                    Err(_) => None, // You may want to handle this error differently
                },
            };

            let from: Identity = Identity {
                uuid: Some(Uuid::new_v4()),
                platform: Platform::Ethereum,
                identity: p.owner.to_lowercase(),
                uid: None,
                created_at: created_at_naive,
                // Don't use ETH's wallet as display_name, use ENS reversed lookup instead.
                display_name: None,
                added_at: naive_now(),
                avatar_url: None,
                profile_url: None,
                updated_at: naive_now(),
                expired_at: None,
                reverse: Some(false),
            };

            let chain = Chain::from_str(p.network.as_str()).unwrap_or_default();
            if chain == Chain::Unknown {
                error!("Rss3 Fetch data | Unknown Chain, original data: {:?}", p);
                continue;
            }
            let contract_addr = real_action
                .metadata
                .contract_address
                .as_ref()
                .unwrap()
                .to_lowercase();
            let nft_id = real_action.metadata.id.as_ref().unwrap();

            let to: Contract = Contract {
                uuid: Uuid::new_v4(),
                category: nft_category,
                address: contract_addr.clone(),
                chain,
                symbol: Some(real_action.metadata.symbol.as_ref().unwrap().clone()),
                updated_at: naive_now(),
            };

            let hold: Hold = Hold {
                uuid: Uuid::new_v4(),
                source: DataSource::Rss3,
                transaction: Some(p.hash),
                id: nft_id.clone(),
                created_at: created_at_naive,
                updated_at: naive_now(),
                fetcher: DataFetcher::RelationService,
                expired_at: None,
            };

            edges.push(EdgeWrapperEnum::new_hyper_edge(
                HyperEdge {}.wrapper(&hv, &from, HYPER_EDGE),
            ));
            let hdc = hold.wrapper(&from, &to, HOLD_CONTRACT);
            edges.push(EdgeWrapperEnum::new_hold_contract(hdc));
        }
        if body.cursor.is_none() || body.total < PAGE_LIMIT {
            break;
        } else {
            cursor = body.cursor.unwrap();
        }
    }
    Ok((vec![], edges))
}

async fn fetch_nfts_by_account(
    _platform: &Platform,
    identity: &str,
) -> Result<TargetProcessedList, Error> {
    let mut cursor = String::from("");
    let client = make_client();
    let mut next_targets = Vec::new();

    loop {
        let uri: http::Uri;
        if cursor.len() == 0 {
            uri = format!(
                "{}/{}?tag=collectible&include_poap=true&refresh=true",
                C.upstream.rss3_service.url, identity
            )
            .parse()
            .map_err(|_err: InvalidUri| Error::ParamError(format!("Uri format Error {}", _err)))?;
        } else {
            uri = format!(
                "{}/{}?tag=collectible&include_poap=true&refresh=true&cursor={}",
                C.upstream.rss3_service.url, identity, cursor
            )
            .parse()
            .map_err(|_err: InvalidUri| Error::ParamError(format!("Uri format Error {}", _err)))?;
        }

        let req = hyper::Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .map_err(|_err| Error::ParamError(format!("Rss3 Build Request Error {}", _err)))?;

        let mut resp = request_with_timeout(&client, req, None)
            .await
            .map_err(|err| {
                Error::ManualHttpClientError(format!(
                    "Rss3 fetch fetch | error: {:?}",
                    err.to_string()
                ))
            })?;

        let body: Rss3Response = parse_body(&mut resp).await?;
        if body.total == 0 {
            info!("Rss3 Response result is empty");
            break;
        }

        let futures: Vec<_> = body
            .result
            .into_iter()
            .filter(|p| p.owner == identity.to_lowercase())
            .map(save_item)
            .collect();

        let targets: TargetProcessedList = join_all(futures)
            .await
            .into_iter()
            .flat_map(|result| result.unwrap_or_default())
            .collect();

        next_targets.extend(targets);
        if body.cursor.is_none() || body.total < PAGE_LIMIT {
            break;
        } else {
            cursor = body.cursor.unwrap();
        }
    }

    Ok(next_targets)
}

async fn save_item(p: ResultItem) -> Result<TargetProcessedList, Error> {
    // let creataed_at = DateTime::parse_from_rfc3339(&p.timestamp).unwrap();
    // let created_at_naive = NaiveDateTime::from_timestamp_opt(creataed_at.timestamp(), 0);
    let created_at_naive = match p.timestamp.as_ref() {
        "" => None,
        timestamp => match utc_to_naive(timestamp.to_string()) {
            Ok(naive_dt) => Some(naive_dt),
            Err(_) => None, // You may want to handle this error differently
        },
    };
    let cli = make_http_client();

    let from: Identity = Identity {
        uuid: Some(Uuid::new_v4()),
        platform: Platform::Ethereum,
        identity: p.owner.to_lowercase(),
        uid: None,
        created_at: created_at_naive,
        // Don't use ETH's wallet as display_name, use ENS reversed lookup instead.
        display_name: None,
        added_at: naive_now(),
        avatar_url: None,
        profile_url: None,
        updated_at: naive_now(),
        expired_at: None,
        reverse: Some(false),
    };

    if p.actions.len() == 0 {
        return Ok(vec![]);
    }

    let found = p
        .actions
        .iter()
        // collectible (transfer, mint, burn) share the same UMS, but approve/revoke not.
        // we need to record is the `hold` relation, so burn is excluded
        .filter(|a| {
            (a.tag_type == "transfer" && p.tag_type == "transfer")
                || (a.tag_type == "mint" && p.tag_type == "mint")
        })
        .find(|a| (p.tag == "collectible" && a.tag == "collectible"));
    if found.is_none() {
        return Ok(vec![]);
    }
    let real_action = found.unwrap();

    if real_action.metadata.symbol.is_none()
        || real_action.metadata.symbol.as_ref().unwrap() == &String::from("ENS")
    {
        return Ok(vec![]);
    }
    let mut nft_category = ContractCategory::Unknown;
    let standard = real_action.metadata.standard.clone();
    if let Some(standard) = standard {
        if standard == "ERC-721".to_string() {
            nft_category = ContractCategory::ERC721;
        } else if standard == "ERC-1155".to_string() {
            nft_category = ContractCategory::ERC1155;
        }
    }

    // let mut nft_category = ContractCategory::from_str(
    //     real_action
    //         .metadata
    //         .standard
    //         .as_ref()
    //         .unwrap()
    //         .to_lowercase()
    //         .as_str(),
    // )
    // .unwrap_or_default();

    if real_action.tag_type == "poap".to_string() {
        nft_category = ContractCategory::POAP;
    }

    let chain = Chain::from_str(p.network.as_str()).unwrap_or_default();
    if chain == Chain::Unknown {
        error!("Rss3 Fetch data | Unknown Chain, original data: {:?}", p);
        return Ok(vec![]);
    }
    let contract_addr = real_action
        .metadata
        .contract_address
        .as_ref()
        .unwrap()
        .to_lowercase();
    let nft_id = real_action.metadata.id.as_ref().unwrap();

    let to: Contract = Contract {
        uuid: Uuid::new_v4(),
        category: nft_category,
        address: contract_addr.clone(),
        chain,
        symbol: Some(real_action.metadata.symbol.as_ref().unwrap().clone()),
        updated_at: naive_now(),
    };

    let hold: Hold = Hold {
        uuid: Uuid::new_v4(),
        source: DataSource::Rss3,
        transaction: Some(p.hash),
        id: nft_id.clone(),
        created_at: created_at_naive,
        updated_at: naive_now(),
        fetcher: DataFetcher::RelationService,
        expired_at: None,
    };
    create_identity_to_contract_hold_record(&cli, &from, &to, &hold).await?;

    Ok(vec![Target::NFT(
        chain,
        nft_category,
        contract_addr.clone(),
        nft_id.clone(),
    )])
}
