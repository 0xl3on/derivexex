#[derive(Debug, Clone, serde::Deserialize)]
pub struct GenesisData {
    #[serde(rename = "genesis_time")]
    #[serde(with = "alloy_serde::quantity")]
    pub genesis_time: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GenesisResponse {
    pub data: GenesisData,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SpecData {
    #[serde(rename = "SECONDS_PER_SLOT")]
    #[serde(with = "alloy_serde::quantity")]
    pub seconds_per_slot: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SpecResponse {
    pub data: SpecData,
}
