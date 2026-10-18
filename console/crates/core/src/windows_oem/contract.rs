//! Versioned release-owned contracts. Client metadata never supplies a contract.
use crate::ControlError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CONTROL_ID: &str = "fn-lock";
pub const CONTROL_LABEL: &str = "Fn lock";
pub const SOURCE_BACKEND: &str = "windows_uwacpi_fnlock";
pub const SOURCE_PATH: &str = r"\\.\ACPIDriver";
pub const READBACK_SCOPE: &str =
    "Complete EC byte at fixed Fn-lock register; not physical key behavior or reboot persistence";
pub(crate) const FN_MASK: u8 = 0x10;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SupportStage {
    Candidate,
    RuntimeAccepted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SmbiosIdentity {
    pub system_vendor: String,
    pub product_name: String,
    pub board_vendor: Option<String>,
    pub board_name: String,
    pub bios_vendor: Option<String>,
    pub bios_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriverContract {
    pub hardware_id: String,
    pub service: String,
    pub inf_version: String,
    pub driver_version: String,
    pub disk_file_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeAcceptance {
    pub id: String,
    pub receipt_sha256: String,
    pub architecture: String,
    pub minimum_windows_build: u32,
    pub maximum_windows_build: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportContract {
    pub id: String,
    pub stage: SupportStage,
    pub smbios: SmbiosIdentity,
    pub raw_dsdt_sha256: String,
    pub driver: DriverContract,
    pub architecture: String,
    pub buffer_contract_version: u32,
    pub runtime_acceptance: Option<RuntimeAcceptance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportCatalog {
    pub schema_version: u32,
    pub contracts: Vec<SupportContract>,
}

fn error(message: impl Into<String>) -> ControlError {
    ControlError::new("windows_fnlock_contract_invalid", message)
}

pub(crate) fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl SupportContract {
    pub(crate) fn validate(&self) -> Result<(), ControlError> {
        if self.id.is_empty()
            || self.id.len() > 128
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            || !is_sha256(&self.raw_dsdt_sha256)
            || !is_sha256(&self.driver.disk_file_sha256)
            || self.driver.hardware_id != r"ACPI\INOU0000"
            || self.driver.service != "UWACPIDriver"
            || self.driver.inf_version != "10/28/2025,10.48.11.331"
            || self.driver.driver_version != "10.48.11.331"
            || self.driver.disk_file_sha256
                != "6e360ee4a0c42b2a4eaf46208c1592c87f8cd71aca2a6c52831a2ad356e61235"
            || self.architecture != "x86_64"
            || self.buffer_contract_version != 1
        {
            return Err(error(
                "The release contract does not describe the fixed Fn-lock ABI",
            ));
        }
        for field in [
            &self.smbios.system_vendor,
            &self.smbios.product_name,
            &self.smbios.board_name,
            &self.smbios.bios_version,
        ] {
            if field.is_empty() || field.len() > 256 || field.chars().any(char::is_control) {
                return Err(error(
                    "The release contract has an invalid exact SMBIOS field",
                ));
            }
        }
        for field in [&self.smbios.board_vendor, &self.smbios.bios_vendor]
            .into_iter()
            .flatten()
        {
            if field.is_empty() || field.len() > 256 || field.chars().any(char::is_control) {
                return Err(error("An optional exact SMBIOS field is malformed"));
            }
        }
        match (&self.stage, &self.runtime_acceptance) {
            (SupportStage::Candidate, None) => Ok(()),
            (SupportStage::RuntimeAccepted, Some(a))
                if !a.id.is_empty()
                    && is_sha256(&a.receipt_sha256)
                    && a.architecture == self.architecture
                    && a.minimum_windows_build > 0
                    && a.maximum_windows_build >= a.minimum_windows_build =>
            {
                Ok(())
            }
            _ => Err(error("Candidate and runtime acceptance metadata disagree")),
        }
    }

    /// Candidate enablement is a build decision, never a request field.
    pub(crate) fn permits_current_build(&self, windows_build: u32) -> bool {
        match &self.runtime_acceptance {
            Some(a) if self.stage == SupportStage::RuntimeAccepted => {
                (a.minimum_windows_build..=a.maximum_windows_build).contains(&windows_build)
            }
            None if self.stage == SupportStage::Candidate => {
                cfg!(feature = "windows-fnlock-candidate")
            }
            _ => false,
        }
    }
}

/// Only helper-owned native state can construct this identity. It is not a DTO
/// and does not deserialize client-provided qualification claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionIdentity {
    contract_id: String,
    lifecycle_digest: [u8; 32],
}

impl SessionIdentity {
    pub(crate) fn new(contract_id: &str, lifecycle_material: &[u8]) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"mechrevo.windows_fnlock.session.v1\0");
        digest.update((contract_id.len() as u64).to_le_bytes());
        digest.update(contract_id.as_bytes());
        digest.update((lifecycle_material.len() as u64).to_le_bytes());
        digest.update(lifecycle_material);
        Self {
            contract_id: contract_id.into(),
            lifecycle_digest: digest.finalize().into(),
        }
    }

    pub(crate) fn binding(&self, full_byte: u8) -> String {
        let mut digest = Sha256::new();
        digest.update(b"mechrevo.windows_fnlock.binding.v1\0");
        digest.update((self.contract_id.len() as u64).to_le_bytes());
        digest.update(self.contract_id.as_bytes());
        digest.update(self.lifecycle_digest);
        digest.update([full_byte]);
        format!("{:x}", digest.finalize())
    }
}

pub(crate) fn parse_catalog(input: &str) -> Result<SupportCatalog, ControlError> {
    let catalog: SupportCatalog = serde_json::from_str(input).map_err(|e| error(e.to_string()))?;
    if catalog.schema_version != 1 || catalog.contracts.len() > 16 {
        return Err(error("Unsupported contract catalog version or size"));
    }
    let mut ids = std::collections::HashSet::new();
    for contract in &catalog.contracts {
        contract.validate()?;
        if !ids.insert(&contract.id) {
            return Err(error("Duplicate release contract ID"));
        }
    }
    Ok(catalog)
}

pub(crate) fn bundled_catalog() -> Result<SupportCatalog, ControlError> {
    parse_catalog(include_str!(
        "../../../../resources/windows-fnlock-support.json"
    ))
}
