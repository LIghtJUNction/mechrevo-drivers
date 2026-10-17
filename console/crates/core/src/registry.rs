//! Exact identity evidence, independent of marketing names and user presets.
//! A match documents research; it never authorizes an EC or GPU operation.
use crate::Identity;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformMatch {
    pub model_id: String,
    pub title: String,
    pub platform: String,
    pub evidence_level: String,
    pub sources: Vec<String>,
    pub hardware_validated: bool,
    pub firmware_evidence_matches: bool,
    pub matched_fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identification {
    pub status: String,
    pub matches: Vec<PlatformMatch>,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    schema_version: u32,
    rules: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    model_id: String,
    title: String,
    platform: String,
    identity: BTreeMap<String, String>,
    qualified_bios: Vec<String>,
    evidence_level: String,
    sources: Vec<String>,
}

fn field<'a>(identity: &'a Identity, name: &str) -> Option<&'a str> {
    match name {
        "manufacturer" => identity.manufacturer.as_deref(),
        "product_name" => identity.product_name.as_deref(),
        "product_version" => identity.product_version.as_deref(),
        "board_name" => identity.board_name.as_deref(),
        _ => None,
    }
}

fn unknown(reason: impl Into<String>) -> Identification {
    Identification {
        status: "unknown".into(),
        matches: vec![],
        reason: reason.into(),
    }
}

fn identify_from(identity: &Identity, input: &str) -> Identification {
    let registry: Registry = match serde_json::from_str(input) {
        Ok(registry) => registry,
        Err(error) => return unknown(format!("Invalid evidence registry: {error}")),
    };
    if registry.schema_version != 1 {
        return unknown("Unsupported evidence registry schema");
    }
    let mut identifiers = BTreeSet::new();
    for rule in &registry.rules {
        if !identifiers.insert(&rule.model_id)
            || rule.model_id.is_empty()
            || rule.sources.is_empty()
            || rule.identity.len() < 2
            || !rule.identity.contains_key("manufacturer")
            || !rule
                .identity
                .keys()
                .any(|key| key == "product_name" || key == "board_name")
            || rule.identity.iter().any(|(name, value)| {
                !matches!(
                    name.as_str(),
                    "manufacturer" | "product_name" | "product_version" | "board_name"
                ) || value.trim().is_empty()
            })
            || !matches!(
                rule.evidence_level.as_str(),
                "upstream-source" | "hardware-partial"
            )
        {
            return unknown("Invalid or duplicate evidence rule; matching refused");
        }
    }
    let matches: Vec<_> = registry
        .rules
        .into_iter()
        .filter(|rule| {
            rule.identity
                .iter()
                .all(|(name, required)| field(identity, name) == Some(required.as_str()))
        })
        .map(|rule| {
            let firmware_evidence_matches = identity
                .bios_version
                .as_ref()
                .is_some_and(|bios| rule.qualified_bios.contains(bios));
            PlatformMatch {
                model_id: rule.model_id,
                title: rule.title,
                platform: rule.platform,
                evidence_level: rule.evidence_level,
                sources: rule.sources,
                // Partial historical validation is deliberately not full acceptance.
                hardware_validated: false,
                firmware_evidence_matches,
                matched_fields: rule.identity.into_keys().collect(),
            }
        })
        .collect();
    match matches.as_slice() {
        [] => unknown("No exact multi-field identity evidence; marketing names and project-ID prefixes are not fallback authorization"),
        [matched] => Identification {
            status: if matched.evidence_level == "hardware-partial" && matched.firmware_evidence_matches {
                "hardware_partial"
            } else { "identified" }.into(),
            reason: if matched.firmware_evidence_matches {
                "Exact identity and historical BIOS evidence match; only documented partial hardware checks exist"
            } else {
                "Exact identity source match; this BIOS and hardware behavior remain unqualified"
            }.into(),
            matches,
        },
        _ => Identification {
            status: "ambiguous".into(),
            matches,
            reason: "Multiple rules matched; no first-match selection or proprietary writes".into(),
        },
    }
}

// @mark mechrevo.model_registry role=authority contract=exact_identity_evidence
pub fn identify(identity: &Identity) -> Identification {
    identify_from(
        identity,
        include_str!("../../../../data/mechrevo-platform-evidence.json"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn local() -> Identity {
        Identity {
            manufacturer: Some("MECHREVO".into()),
            product_name: Some("Kuangshi16Pro Series GM6PX0X".into()),
            board_name: Some("GM6PX0X".into()),
            bios_version: Some("N.1.09MRO07".into()),
            ..Default::default()
        }
    }
    // @mark mechrevo.model_registry role=verification contract=exact_identity_evidence
    #[test]
    fn exact_identity_retains_partial_scope() {
        let result = identify(&local());
        assert_eq!(result.status, "hardware_partial");
        assert!(result.matches[0].firmware_evidence_matches);
        assert!(!result.matches[0].hardware_validated);
    }
    #[test]
    fn changed_or_missing_bios_does_not_inherit_firmware_evidence() {
        for bios in [None, Some("N.1.10".into())] {
            let mut identity = local();
            identity.bios_version = bios;
            let result = identify(&identity);
            assert_eq!(result.status, "identified");
            assert!(!result.matches[0].firmware_evidence_matches);
        }
    }
    #[test]
    fn marketing_prefix_and_similar_board_do_not_match() {
        let mut identity = local();
        identity.board_name = Some("GM6PX0X-other".into());
        assert_eq!(identify(&identity).status, "unknown");
        identity = Identity {
            manufacturer: Some("MECHREVO".into()),
            ..Default::default()
        };
        assert_eq!(identify(&identity).status, "unknown");
        identity = local();
        identity.manufacturer = Some("Other MECHREVO OEM".into());
        assert_eq!(identify(&identity).status, "unknown");
    }
    #[test]
    fn conflicts_are_explicit_and_invalid_rules_fail_closed() {
        let input = include_str!("../../../../data/mechrevo-platform-evidence.json");
        let mut value: serde_json::Value = serde_json::from_str(input).unwrap();
        let mut other = value["rules"][0].clone();
        other["model_id"] = "different-rule".into();
        value["rules"].as_array_mut().unwrap().push(other);
        assert_eq!(
            identify_from(&local(), &value.to_string()).status,
            "ambiguous"
        );
        value["rules"][0]["identity"]["serial"] = "private".into();
        assert_eq!(
            identify_from(&local(), &value.to_string()).status,
            "unknown"
        );
    }
    #[test]
    fn corrupt_or_user_authorization_fields_cannot_grant_support() {
        assert_eq!(identify_from(&local(), "{}").status, "unknown");
        let mut value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../data/mechrevo-platform-evidence.json"
        ))
        .unwrap();
        value["rules"][0]["mine"] = true.into();
        assert_eq!(
            identify_from(&local(), &value.to_string()).status,
            "unknown"
        );
        value["schema_version"] = 2.into();
        assert_eq!(
            identify_from(&local(), &value.to_string()).status,
            "unknown"
        );
    }
}
