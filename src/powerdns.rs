use std::fmt::{Display, Formatter};
use super::{config, Result};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use clap::Args;
use reqwest::{Client, ClientBuilder, StatusCode, Url};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use tracing::field::debug;

const BASE_PATH: &'static str = "api/v1/";

#[derive(Debug, Clone, Args)]
pub(crate) struct PowerDnsCliOpts {
    /// Base URL for the PowerDNS server (e.g., http://localhost:8081)
    #[arg(long="power-dns-url", visible_alias="pdnsu", env)]
    pub(crate) url: String,
    /// PowerDNS server - the default is "localhost" unless another server was explicitly created
    #[arg(long="power-dns-server", visible_alias="pdnss", env)]
    pub(crate) server: String,
    /// API Key for PowerDNS. Set as the `api-key` property in the PowerDNS config.
    #[arg(long="power-dns-api-key", visible_alias="pdnsak", env)]
    pub(crate) api_key: String,
}

pub(crate) struct PowerDnsClient {
    url: Url,
    server: String,
    api_key: String,
    client: Client,
}

impl PowerDnsClient {
    pub(crate) fn new(url: Url, server: String, api_key: String) -> Result<Self> {
        let client = ClientBuilder::new().build()?;

        Ok(PowerDnsClient {
            url,
            server,
            api_key,
            client,
        })
    }

    pub(crate) async fn list_zone(&self, zone_id: &str) -> Result<Option<PowerDnsApiZone>> {
        if !zone_id.ends_with(".") {
            return Err(format!("zone_id {zone_id} must end with a dot - e.g., [{zone_id}.]").into())
        }

        let request = self.client.get(self.url
            .join(BASE_PATH)?
            .join("servers/")?
            .join(&format!("{}/", self.server))?
            .join("zones/")?
            .join(zone_id)?
        ).header("X-API-Key", &self.api_key).build()?;

        let response = self.client.execute(request).await?;

        match response.status() {
            StatusCode::OK => {
                let zone_response: PowerDnsApiZone = response.json().await?;
                Ok(Some(zone_response))
            },
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
                let api_error: PowerDnsApiError = response.json().await?;
                Err(format!(
                    "malformed request passed to PowerDNS, Error Message [{}], Error Codes [{}]",
                    api_error.error,
                    api_error.errors.unwrap_or_default().join(","),
                ).into())
            },
            StatusCode::INTERNAL_SERVER_ERROR => {
                let api_error: PowerDnsApiError = response.json().await?;
                Err(format!(
                    "PowerDNS return an internal error, Error Message [{}], Error Codes [{}]",
                    api_error.error,
                    api_error.errors.unwrap_or_default().join(","),
                ).into())
            },
            StatusCode::NOT_FOUND => {
                Ok(None)
            },
            s @ _ => {
                Err(format!(
                    "unexpected {} error calling API: {}",
                    s.as_str(),
                    response.text().await.unwrap_or("unexpected error fetching error response content".to_string()),
                ).into())
            }
        }
    }

    pub(crate) async fn update_rrsets(&self, zone_id: &str, rrsets: PowerDnsApiRRSets) -> Result<()> {
        if !zone_id.ends_with(".") {
            return Err(format!("zone_id {zone_id} must end with a dot - e.g., [{zone_id}.]").into())
        }

        info!(zone_id, url=self.url.as_str(), BASE_PATH, server=self.server, rrset_count=rrsets.rrsets.len(), "updating rrset(s)");

        let request = self.client.patch(
            self.url
                .join(BASE_PATH)?
                .join("servers/")?
                .join(&format!("{}/", self.server))?
                .join("zones/")?
                .join(zone_id)?
        ).header("X-API-Key", &self.api_key)
            .json(&rrsets)
            .build()?;

        let response = self.client.execute(request).await?;

        match response.status() {
            StatusCode::NO_CONTENT => {
                Ok(())
            },
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
                let api_error: PowerDnsApiError = response.json().await?;
                Err(format!(
                    "malformed request passed to PowerDNS, Error Message [{}], Error Codes [{}]",
                    api_error.error,
                    api_error.errors.unwrap_or_default().join(","),
                ).into())
            },
            StatusCode::INTERNAL_SERVER_ERROR => {
                let api_error: PowerDnsApiError = response.json().await?;
                Err(format!(
                    "PowerDNS return an internal error, Error Message [{}], Error Codes [{}]",
                    api_error.error,
                    api_error.errors.unwrap_or_default().join(","),
                ).into())
            },
            s @ _ => {
                Err(format!(
                    "unexpected {} error calling API: {}",
                    s.as_str(),
                    response.text().await.unwrap_or("unexpected error fetching error response content".to_string()),
                ).into())
            }
        }
    }

    pub(crate) async fn create_rrset_record(&self, zone_id: &str, rrset_id: &str, rrset_type: RRSetType, record: PowerDnsApiRecord) -> Result<()> {
        // if rrset.change_type.is_none() || matches!(rrset.change_type, Some(RRSetChangeType::DELETE)) {
        //     return Err("change_type must be set to REPLACE when creating an RRset".into());
        // }

        if !zone_id.ends_with(".") {
            return Err(format!("zone_id {zone_id} must end with a dot - e.g., [{zone_id}.]").into())
        }

        if !rrset_id.ends_with(".") {
            return Err(format!("RRset name {rrset_id} must end with a dot - e.g., [{rrset_id}.]").into())
        }

        let zone = match self.list_zone(zone_id).await? {
            Some(zone) => zone,
            None => return Err(format!("zone {zone_id} not found").into()),
        };

        let mut rrset = if let Some(rrsets) = zone.rrsets {
            if let Some(mut rrset) = rrsets.into_iter().find(|r| r.name == rrset_id && r.record_type == rrset_type) {
                if let Some(ref mut records) = rrset.records {
                    records.push(record);
                } else {
                    rrset.records = Some(vec![record]);
                }

                rrset
            } else {
                return Err(format!("zone {zone_id} has no RRset for {rrset_id}/{rrset_type}, cannot add single record").into());
            }
        } else {
            return Err(format!("zone {zone_id} has no existing RRSets, cannot add single record").into());
        };

        rrset.change_type = Some(RRSetChangeType::REPLACE);

        self.update_rrsets(zone_id, PowerDnsApiRRSets { rrsets: vec![rrset] }).await
    }

    pub(crate) async fn delete_rrset_record(&self, zone_id: &str, rrset_id: &str, rrset_type: RRSetType, record: PowerDnsApiRecord) -> Result<()> {
        // if rrset.change_type.is_none() || matches!(rrset.change_type, Some(RRSetChangeType::DELETE)) {
        //     return Err("change_type must be set to REPLACE when creating an RRset".into());
        // }

        if !zone_id.ends_with(".") {
            return Err(format!("zone_id {zone_id} must end with a dot - e.g., [{zone_id}.]").into())
        }

        if !rrset_id.ends_with(".") {
            return Err(format!("RRset name {rrset_id} must end with a dot - e.g., [{rrset_id}.]").into())
        }

        let zone = match self.list_zone(zone_id).await? {
            Some(zone) => zone,
            None => return Err(format!("zone {zone_id} not found").into()),
        };

        let mut rrset = if let Some(rrsets) = zone.rrsets {
            if let Some(mut rrset) = rrsets.into_iter().find(|r| r.name == rrset_id && r.record_type == rrset_type) {
                if let Some(ref mut records) = rrset.records {
                    if records.contains(&record) {
                        records.retain(|r| r != &record);
                    } else {
                        return Err(format!("record {record} does not exist for {rrset_id}/{rrset_type}").into())
                    }
                } else {
                    return Err(format!("record {record} does not exist for {rrset_id}/{rrset_type}").into())
                }

                rrset
            } else {
                return Err(format!("zone {zone_id} has no RRset for {rrset_id}/{rrset_type}, cannot remove single record").into());
            }
        } else {
            return Err(format!("zone {zone_id} has no existing RRSets, cannot remove single record").into());
        };

        rrset.change_type = Some(RRSetChangeType::REPLACE);

        self.update_rrsets(zone_id, PowerDnsApiRRSets { rrsets: vec![rrset] }).await
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct PowerDnsApiError {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    errors: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize, PartialOrd, PartialEq)]
pub(crate) enum RRSetType {
    A,
    AAAA,
    PTR,
    MX,
}

impl Display for RRSetType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

#[derive(Debug, Deserialize, Serialize, PartialOrd, PartialEq)]
pub(crate) enum RRSetChangeType {
    REPLACE,
    DELETE,
}

#[derive(Debug, Deserialize, Serialize, PartialOrd, PartialEq)]
pub(crate) enum ZoneType {
    #[serde(rename="Zone")]
    ZONE,
}

#[derive(Debug, Deserialize, Serialize, PartialOrd, PartialEq)]
#[serde(rename_all="PascalCase")]
pub(crate) enum ZoneKind {
    NATIVE,
    MASTER,
    SLAVE,
    PRODUCER,
    CONSUMER,
}

#[derive(Debug, Deserialize, Serialize, PartialOrd, PartialEq)]
pub(crate) struct PowerDnsApiZone {
    id: String,
    name: String,
    #[serde(rename="type")]
    zone_type: ZoneType,
    url: String,
    kind: ZoneKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    rrsets: Option<Vec<PowerDnsApiRRSet>>,
    serial: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    masters: Option<Vec<IpAddr>>,
    dnssec: bool,
    nsec3param: String,
    nsec3narrow: bool,
    presigned: bool,
    soa_edit: String,
    soa_edit_api: String,
    api_rectify: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    zone: Option<String>,
    catalog: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nameservers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    master_tsig_key_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slave_tsig_key_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize, PartialOrd, PartialEq)]
pub(crate) struct PowerDnsApiRRSets {
    pub(crate) rrsets: Vec<PowerDnsApiRRSet>,
}

#[derive(Debug, Deserialize, Serialize, PartialOrd, PartialEq)]
pub(crate) struct PowerDnsApiRRSet {
    pub(crate) name: String,
    #[serde(rename="type")]
    pub(crate) record_type: RRSetType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ttl: Option<f64>,
    #[serde(rename="changetype", skip_serializing_if = "Option::is_none")]
    pub(crate) change_type: Option<RRSetChangeType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) records: Option<Vec<PowerDnsApiRecord>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) comments: Option<Vec<PowerDnsApiComment>>,
}

impl PowerDnsApiRRSet {
    pub(crate) fn new_ipv4(host: &str, domain: &str, ipv4addr: &Ipv4Addr) -> Self {
        PowerDnsApiRRSet {
            name: format!("{}.{}.", host, domain),
            record_type: RRSetType::A,
            ttl: Some(300.0),
            change_type: Some(RRSetChangeType::REPLACE),
            records: Some(
                vec![
                    PowerDnsApiRecord {
                        content: ipv4addr.to_string(),
                        disabled: false,
                    }
                ]
            ),
            comments: None,
        }
    }

    pub(crate) fn delete_ipv4(host: &str, domain: &str) -> Self {
        PowerDnsApiRRSet {
            name: format!("{}.{}.", host, domain),
            record_type: RRSetType::A,
            ttl: Some(300.0),
            change_type: Some(RRSetChangeType::DELETE),
            records: None,
            comments: None,
        }
    }

    pub(crate) fn new_ipv6(host: &str, domain: &str, ipv6addr: &Ipv6Addr) -> Self {
        PowerDnsApiRRSet {
            name: format!("{}.{}.", host, domain),
            record_type: RRSetType::AAAA,
            ttl: Some(300.0),
            change_type: Some(RRSetChangeType::REPLACE),
            records: Some(
                vec![
                    PowerDnsApiRecord {
                        content: ipv6addr.to_string(),
                        disabled: false,
                    }
                ]
            ),
            comments: None,
        }
    }

    pub(crate) fn delete_ipv6(host: &str, domain: &str) -> Self {
        PowerDnsApiRRSet {
            name: format!("{}.{}.", host, domain),
            record_type: RRSetType::AAAA,
            ttl: Some(300.0),
            change_type: Some(RRSetChangeType::DELETE),
            records: None,
            comments: None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, PartialOrd, PartialEq)]
pub(crate) struct PowerDnsApiRecord {
    pub(crate) content: String,
    pub(crate) disabled: bool,
}

impl Display for PowerDnsApiRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "content=\"{}\", disabled={}", self.content, self.disabled)
    }
}

#[derive(Debug, Deserialize, Serialize, PartialOrd, PartialEq)]
pub(crate) struct PowerDnsApiComment {
    content: String,
    account: String,
    modified_at: f64,
}