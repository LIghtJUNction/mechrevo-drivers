//! Portable policy used by the native installation verifier. Filesystem,
//! owner/ACL, file identity and signature checks remain native and mandatory.
use crate::ControlError;
use serde::Deserialize;

pub(crate) const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub(crate) const IMAGE_NAMES: [&str; 3] = [
    "mechrevo-console.exe",
    "mechrevo-console-gui.exe",
    "mechrevo-console-oem-helper.exe",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trust {
    DevelopmentCandidate,
    Authenticode,
}

impl<'de> Deserialize<'de> for Trust {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Unit-enum derives also accept externally tagged objects. The manifest
        // contract requires the exact string representation instead.
        match String::deserialize(deserializer)?.as_str() {
            "development_candidate" => Ok(Self::DevelopmentCandidate),
            "authenticode" => Ok(Self::Authenticode),
            _ => Err(serde::de::Error::custom("Unsupported release trust string")),
        }
    }
}

// Every object has a complete fixed schema. Derived struct deserialization
// rejects duplicate decoded keys as well as unknown/missing fields; no Value
// conversion that could silently overwrite duplicate keys precedes this parse.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) schema_version: u32,
    pub(crate) release_id: String,
    pub(crate) source_commit: String,
    pub(crate) architecture: String,
    pub(crate) trust: Trust,
    pub(crate) files: ManifestFiles,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestFiles {
    #[serde(rename = "mechrevo-console.exe")]
    pub(crate) cli: String,
    #[serde(rename = "mechrevo-console-gui.exe")]
    pub(crate) gui: String,
    #[serde(rename = "mechrevo-console-oem-helper.exe")]
    pub(crate) helper: String,
}

fn reject(message: impl Into<String>) -> ControlError {
    ControlError::new("windows_oem_installation", message)
}

fn valid_release_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || matches!(v, b'.' | b'_' | b'-'))
}

fn lower_hex(value: &str, count: usize) -> bool {
    value.len() == count
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}

pub(crate) fn parse_sha256(value: &str) -> Result<[u8; 32], ControlError> {
    if !lower_hex(value, 64) {
        return Err(reject(
            "Every executable hash must be exactly 64 lowercase hexadecimal digits",
        ));
    }
    fn digit(value: u8) -> u8 {
        match value {
            b'0'..=b'9' => value - b'0',
            b'a'..=b'f' => value - b'a' + 10,
            _ => unreachable!("validated lowercase hexadecimal digest"),
        }
    }
    let mut hash = [0u8; 32];
    for (output, pair) in hash.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *output = digit(pair[0]) * 16 + digit(pair[1]);
    }
    Ok(hash)
}

/// Compile-time release pins are passed by native code using option_env!, never
/// accepted from a request or a runtime environment override.
pub(crate) fn parse_manifest(
    bytes: &[u8],
    compiled_release: Option<&str>,
    compiled_commit: Option<&str>,
) -> Result<Manifest, ControlError> {
    if bytes.is_empty() || bytes.len() > MAX_MANIFEST_BYTES {
        return Err(reject(
            "Release manifest is empty or exceeds the fixed byte bound",
        ));
    }
    let manifest: Manifest = serde_json::from_slice(bytes)
        .map_err(|error| reject(format!("Invalid strict release manifest: {error}")))?;
    validate_manifest_identity(&manifest, compiled_release, compiled_commit)?;
    Ok(manifest)
}

pub(crate) fn validate_manifest_identity(
    manifest: &Manifest,
    compiled_release: Option<&str>,
    compiled_commit: Option<&str>,
) -> Result<(), ControlError> {
    if manifest.schema_version != 1
        || manifest.architecture != "x86_64"
        || !valid_release_id(&manifest.release_id)
        || !lower_hex(&manifest.source_commit, 40)
    {
        return Err(reject(
            "Invalid release manifest schema, architecture or release identity",
        ));
    }
    let release = compiled_release
        .filter(|v| valid_release_id(v))
        .ok_or_else(|| reject("This executable has no valid compiled Windows release ID"))?;
    let commit = compiled_commit
        .filter(|v| lower_hex(v, 40))
        .ok_or_else(|| reject("This executable has no valid compiled Windows source commit"))?;
    if manifest.release_id != release || manifest.source_commit != commit {
        return Err(reject(
            "The release manifest does not match this executable's compiled release identity",
        ));
    }
    for hash in [
        &manifest.files.cli,
        &manifest.files.gui,
        &manifest.files.helper,
    ] {
        parse_sha256(hash)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const RELEASE: &str = "candidate.test-1";
    fn document() -> Value {
        json!({"schema_version":1,"release_id":RELEASE,"source_commit":"a".repeat(40),
            "architecture":"x86_64","trust":"development_candidate","files":{
                "mechrevo-console.exe":"1".repeat(64),"mechrevo-console-gui.exe":"2".repeat(64),
                "mechrevo-console-oem-helper.exe":"3".repeat(64)}})
    }
    fn parse(value: &Value) -> Result<Manifest, ControlError> {
        parse_manifest(
            &serde_json::to_vec(value).unwrap(),
            Some(RELEASE),
            Some(&"a".repeat(40)),
        )
    }

    #[test]
    fn production_parser_accepts_only_the_exact_three_file_release_and_trust_modes() {
        let value = document();
        let parsed = parse(&value).unwrap();
        assert_eq!(parsed.release_id, RELEASE);
        assert_eq!(parsed.trust, Trust::DevelopmentCandidate);
        assert_eq!(parsed.files.cli, "1".repeat(64));
        assert_eq!(parsed.files.gui, "2".repeat(64));
        assert_eq!(parsed.files.helper, "3".repeat(64));
        let mut signed = value;
        signed["trust"] = json!("authenticode");
        assert_eq!(parse(&signed).unwrap().trust, Trust::Authenticode);
        // This policy classification is not a signature verification result.
        assert_eq!(
            IMAGE_NAMES,
            [
                "mechrevo-console.exe",
                "mechrevo-console-gui.exe",
                "mechrevo-console-oem-helper.exe"
            ]
        );
    }

    #[test]
    fn duplicate_unknown_missing_and_escaped_duplicate_fields_are_rejected() {
        let serialized = document().to_string();
        for encoded in [
            serialized.replacen(
                "\"schema_version\":1",
                "\"schema_version\":1,\"schema_version\":1",
                1,
            ),
            serialized.replacen(
                "\"schema_version\":1",
                "\"schema_version\":1,\"\\u0073chema_version\":1",
                1,
            ),
            serialized.replacen(
                "\"mechrevo-console.exe\":",
                "\"mechrevo-console.exe\":\"0\",\"mechrevo-console.exe\":",
                1,
            ),
            format!("{serialized}{{}}"),
        ] {
            assert!(
                parse_manifest(encoded.as_bytes(), Some(RELEASE), Some(&"a".repeat(40))).is_err()
            );
        }
        for parent in ["root", "files"] {
            let mut invalid = document();
            let object = if parent == "root" {
                &mut invalid
            } else {
                &mut invalid["files"]
            };
            object
                .as_object_mut()
                .unwrap()
                .insert("arbitrary_path".into(), json!("C:\\bad"));
            assert!(parse(&invalid).is_err());
        }
        for field in [
            "schema_version",
            "release_id",
            "source_commit",
            "architecture",
            "trust",
            "files",
        ] {
            let mut missing = document();
            missing.as_object_mut().unwrap().remove(field);
            assert!(parse(&missing).is_err(), "{field}");
        }
        for field in IMAGE_NAMES {
            let mut missing = document();
            missing["files"].as_object_mut().unwrap().remove(field);
            assert!(parse(&missing).is_err(), "{field}");
        }
    }

    #[test]
    fn numeric_schema_representation_and_unsupported_trust_are_rejected() {
        let serialized = document().to_string();
        for number in [
            "0",
            "2",
            "4294967295",
            "4294967296",
            "-1",
            "1.0",
            "1e0",
            "true",
            "\"1\"",
            "null",
        ] {
            let encoded = serialized.replacen(
                "\"schema_version\":1",
                &format!("\"schema_version\":{number}"),
                1,
            );
            assert!(
                parse_manifest(encoded.as_bytes(), Some(RELEASE), Some(&"a".repeat(40))).is_err(),
                "{number}"
            );
        }
        for trust in [
            json!("ignore-signature"),
            Value::Null,
            json!(true),
            json!({"development_candidate":true}),
            json!({"development_candidate":null}),
        ] {
            let mut invalid = document();
            invalid["trust"] = trust;
            assert!(parse(&invalid).is_err());
        }
    }

    #[test]
    fn manifest_requires_present_valid_matching_compiled_pins() {
        let value = serde_json::to_vec(&document()).unwrap();
        let commit = "a".repeat(40);
        for (release, source) in [
            (None, Some(commit.as_str())),
            (Some(RELEASE), None),
            (Some("other-release"), Some(commit.as_str())),
            (Some(RELEASE), Some("bad-commit")),
            (Some("bad release"), Some(commit.as_str())),
            (
                Some(RELEASE),
                Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            ),
        ] {
            assert!(parse_manifest(&value, release, source).is_err());
        }
        let other = "b".repeat(40);
        assert!(parse_manifest(&value, Some(RELEASE), Some(&other)).is_err());
    }

    #[test]
    fn fixed_architecture_release_names_and_every_executable_digest_are_enforced() {
        for (key, value) in [
            ("architecture", json!("arm64")),
            ("release_id", json!("")),
            ("release_id", json!("bad release")),
            ("release_id", json!("x".repeat(129))),
            ("source_commit", json!("A".repeat(40))),
            ("source_commit", json!("g".repeat(40))),
        ] {
            let mut invalid = document();
            invalid[key] = value;
            assert!(parse(&invalid).is_err());
        }
        assert_eq!(parse_sha256(&"ff".repeat(32)).unwrap(), [255; 32]);
        assert_eq!(parse_sha256(&"00".repeat(32)).unwrap(), [0; 32]);
        for name in IMAGE_NAMES {
            for digest in [
                "a".repeat(63),
                "A".repeat(64),
                "g".repeat(64),
                "0".repeat(65),
            ] {
                let mut invalid = document();
                invalid["files"][name] = json!(digest);
                assert!(parse(&invalid).is_err(), "{name}");
            }
        }
    }

    #[test]
    fn byte_boundary_invalid_utf8_and_nested_non_strings_use_the_production_parser() {
        let mut encoded = serde_json::to_vec(&document()).unwrap();
        encoded.resize(MAX_MANIFEST_BYTES, b' ');
        assert!(parse_manifest(&encoded, Some(RELEASE), Some(&"a".repeat(40))).is_ok());
        encoded.push(b' ');
        assert!(parse_manifest(&encoded, Some(RELEASE), Some(&"a".repeat(40))).is_err());
        assert!(parse_manifest(b"\xff", Some(RELEASE), Some(&"a".repeat(40))).is_err());
        assert!(parse_manifest(b"", Some(RELEASE), Some(&"a".repeat(40))).is_err());
        let mut nested = Value::Null;
        for _ in 0..32 {
            nested = Value::Array(vec![nested]);
        }
        let mut invalid = document();
        invalid["files"]["mechrevo-console.exe"] = nested;
        assert!(parse(&invalid).is_err());
    }
}
