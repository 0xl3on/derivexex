pub mod beacon;
pub mod beacon_api_types;

pub use beacon::{BeaconBlobProvider, BeaconError};
pub use beacon_api_types::{GenesisResponse, SpecResponse};
