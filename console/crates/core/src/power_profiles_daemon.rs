//! A system-bus power-profile request, not an OEM performance or wattage claim.
//! Changing ActiveProfile can cancel other applications' holds before profile
//! activation fails. No failure path retries a setter or restores old policy.
//! Observation and Set are separate operations, not an atomic compare-and-swap.
#[cfg(any(target_os = "linux", test))]
use crate::{ApplyResult, PowerPolicy, ProfilePlan};
use crate::{ControlError, PowerProfile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const BACKEND: &str = "linux_power_profiles_daemon";
pub const SCOPE: &str = "System power profile (power-profiles-daemon)";
pub const PROFILE_RECEIPT_MARKER: &str = "profile_apply_receipt=";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileDaemonIdentity {
    pub bus_id: String,
    pub unique_owner: String,
    pub bus_name: String,
    pub object_path: String,
    pub interface: String,
    pub unix_user: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileHold {
    pub application_id: String,
    pub profile: PowerProfile,
    pub reason: String,
    /// Additional string variants are retained in the binding, not interpreted.
    pub attributes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdvertisedProfile {
    pub profile: PowerProfile,
    pub attributes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileDetails {
    pub daemon: ProfileDaemonIdentity,
    /// The bounded original API value; unknown degradation reasons stay visible.
    pub performance_degraded: String,
    pub holds: Vec<ProfileHold>,
    pub advertised_profiles: Vec<AdvertisedProfile>,
    /// Preview intent only. False for observations and same-profile no-ops.
    pub holds_will_be_released: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileApplyOutcome {
    Confirmed,
    NotSubmitted,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileApplyStage {
    BeforeWrite,
    Setter,
    Readback,
    Complete,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileHoldEffect {
    Unchanged,
    /// Holds were present before and absent in the independent observation.
    /// This is an observation, not proof of which actor released each hold.
    ReleasedObserved,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileApplyReceipt {
    pub outcome: ProfileApplyOutcome,
    pub stage: ProfileApplyStage,
    pub submitted: bool,
    pub acknowledged: Option<bool>,
    /// Waiting for busctl to exit does not fence a daemon request whose reply
    /// was lost. Later observations never erase this historical uncertainty.
    pub writer_may_continue: bool,
    pub readback: Option<String>,
    pub authority_unchanged: Option<bool>,
    pub holds_before: Vec<ProfileHold>,
    pub holds_after: Option<Vec<ProfileHold>>,
    pub hold_effect: ProfileHoldEffect,
    pub primary_error: Option<ControlError>,
    pub readback_error: Option<ControlError>,
}

impl Default for ProfileApplyReceipt {
    fn default() -> Self {
        Self {
            outcome: ProfileApplyOutcome::NotSubmitted,
            stage: ProfileApplyStage::BeforeWrite,
            submitted: false,
            acknowledged: None,
            writer_may_continue: false,
            readback: None,
            authority_unchanged: None,
            holds_before: vec![],
            holds_after: None,
            hold_effect: ProfileHoldEffect::Unchanged,
            primary_error: None,
            readback_error: None,
        }
    }
}

pub fn error_receipt(error: &ControlError) -> Option<ProfileApplyReceipt> {
    let mut markers = error
        .details
        .iter()
        .filter_map(|detail| detail.strip_prefix(PROFILE_RECEIPT_MARKER));
    let encoded = markers.next()?;
    if encoded.len() > 512 * 1024 || markers.next().is_some() {
        return None;
    }
    serde_json::from_str(encoded).ok()
}

#[cfg(any(target_os = "linux", test))]
pub(crate) enum Discovery {
    Absent,
    Present(Result<Box<PowerPolicy>, ControlError>),
}

#[cfg(target_os = "linux")]
pub(crate) fn discover() -> Discovery {
    backend::discover_with(&mut backend::Native)
}

#[cfg(target_os = "linux")]
pub(crate) fn plan(requested: PowerProfile) -> Result<ProfilePlan, ControlError> {
    backend::plan_with(&mut backend::Native, requested)
}

#[cfg(target_os = "linux")]
pub(crate) fn apply(plan: &ProfilePlan) -> Result<ApplyResult, ControlError> {
    backend::apply_with(&mut backend::Native, plan)
}

#[cfg(any(target_os = "linux", test))]
mod backend {
    use super::*;
    use crate::{command::CommandOutput, EvidenceSource};
    use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
    use serde_json::{Map, Value};
    use sha2::{Digest, Sha256};
    use std::{fmt, str::FromStr};

    const LIMIT: usize = 64 * 1024;
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Endpoint {
        Modern,
        Legacy,
    }
    impl Endpoint {
        fn name(self) -> &'static str {
            match self {
                Self::Modern => "org.freedesktop.UPower.PowerProfiles",
                Self::Legacy => "net.hadess.PowerProfiles",
            }
        }
        fn path(self) -> &'static str {
            match self {
                Self::Modern => "/org/freedesktop/UPower/PowerProfiles",
                Self::Legacy => "/net/hadess/PowerProfiles",
            }
        }
    }
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) enum Query {
        BusId,
        HasOwner(Endpoint),
        Owner(Endpoint),
        User(String),
        Properties(String, Endpoint),
    }
    pub(super) enum Submission {
        NotSubmitted(ControlError),
        Submitted(Result<CommandOutput, ControlError>),
    }
    pub(super) trait Io {
        fn query(&mut self, query: Query) -> Result<CommandOutput, ControlError>;
        fn set(&mut self, identity: &ProfileDaemonIdentity, target: &str) -> Submission;
    }
    fn failure(code: &str, message: impl Into<String>) -> ControlError {
        ControlError::new(code, message)
    }
    fn invalid(message: impl Into<String>) -> ControlError {
        failure("profile_daemon_invalid_reply", message)
    }

    // serde_json::Value normally overwrites duplicate object keys. The actual
    // wire parser rejects decoded duplicates at every nesting level instead.
    struct Unique(usize);
    impl<'de> DeserializeSeed<'de> for Unique {
        type Value = Value;
        fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<Value, D::Error> {
            if self.0 > 16 {
                return Err(serde::de::Error::custom(
                    "Power-profile reply nesting is too deep",
                ));
            }
            struct V(usize);
            impl<'de> Visitor<'de> for V {
                type Value = Value;
                fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                    f.write_str("bounded JSON without duplicate keys")
                }
                fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Value, E> {
                    Ok(Value::Bool(v))
                }
                fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Value, E> {
                    Ok(v.into())
                }
                fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Value, E> {
                    Ok(v.into())
                }
                fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Value, E> {
                    serde_json::Number::from_f64(v)
                        .map(Value::Number)
                        .ok_or_else(|| E::custom("Invalid JSON number"))
                }
                fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Value, E> {
                    Ok(v.into())
                }
                fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Value, E> {
                    Ok(v.into())
                }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
                    Ok(Value::Null)
                }
                fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
                    Ok(Value::Null)
                }
                fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
                    let mut values = vec![];
                    while let Some(value) = seq.next_element_seed(Unique(self.0 + 1))? {
                        if values.len() >= 256 {
                            return Err(serde::de::Error::custom(
                                "Power-profile array exceeds its bound",
                            ));
                        }
                        values.push(value);
                    }
                    Ok(Value::Array(values))
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
                    let mut values = Map::new();
                    while let Some(key) = map.next_key::<String>()? {
                        if values.contains_key(&key) {
                            return Err(serde::de::Error::custom("Duplicate JSON key"));
                        }
                        if values.len() >= 128 {
                            return Err(serde::de::Error::custom(
                                "Power-profile dictionary exceeds its bound",
                            ));
                        }
                        values.insert(key, map.next_value_seed(Unique(self.0 + 1))?);
                    }
                    Ok(Value::Object(values))
                }
            }
            de.deserialize_any(V(self.0))
        }
    }
    fn json(output: CommandOutput) -> Result<Value, ControlError> {
        if output.status != 0 {
            return Err(output_error(output, "profile_daemon_query_failed"));
        }
        if !output.stderr.is_empty() || output.stdout.len() > LIMIT || output.stderr.len() > LIMIT {
            return Err(output_error(output, "profile_daemon_invalid_reply"));
        }
        let mut de = serde_json::Deserializer::from_str(&output.stdout);
        let value = Unique(0)
            .deserialize(&mut de)
            .map_err(|e| invalid(e.to_string()))?;
        de.end().map_err(|e| invalid(e.to_string()))?;
        Ok(value)
    }
    fn body<'a>(value: &'a Value, signature: &str) -> Result<&'a Value, ControlError> {
        let map = value
            .as_object()
            .ok_or_else(|| invalid("Reply is not a typed dictionary"))?;
        if map.len() != 2
            || map.get("type").and_then(Value::as_str) != Some(signature)
            || !map.contains_key("data")
        {
            return Err(invalid(format!("Expected D-Bus signature {signature}")));
        }
        Ok(&map["data"])
    }
    fn scalar(value: &Value, signature: &str) -> Result<Value, ControlError> {
        let array = body(value, signature)?
            .as_array()
            .ok_or_else(|| invalid("Method reply data is not an array"))?;
        if array.len() != 1 {
            return Err(invalid("Method reply must contain exactly one result"));
        }
        Ok(array[0].clone())
    }
    fn text(value: &Value, maximum: usize) -> Result<String, ControlError> {
        let value = value
            .as_str()
            .ok_or_else(|| invalid("Expected a string variant"))?;
        if value.len() > maximum || value.contains('\0') {
            return Err(invalid(
                "Power-profile string exceeds its bound or contains NUL",
            ));
        }
        Ok(value.into())
    }
    fn variant_text(value: &Value, maximum: usize) -> Result<String, ControlError> {
        text(body(value, "s")?, maximum)
    }
    fn dict_strings(value: &Value) -> Result<BTreeMap<String, String>, ControlError> {
        let map = value
            .as_object()
            .ok_or_else(|| invalid("Profile entry is not a dictionary"))?;
        if map.is_empty() || map.len() > 32 {
            return Err(invalid("Profile dictionary is empty or excessive"));
        }
        map.iter()
            .map(|(key, value)| {
                if key.is_empty() || key.len() > 128 || key.contains('\0') {
                    return Err(invalid("Profile dictionary key is invalid"));
                }
                Ok((key.clone(), variant_text(value, 1024)?))
            })
            .collect()
    }
    fn parse_profile(value: &str) -> Result<PowerProfile, ControlError> {
        PowerProfile::from_str(value)
            .map_err(|_| invalid("Daemon advertised an unknown power profile"))
    }
    fn rank(profile: PowerProfile) -> u8 {
        match profile {
            PowerProfile::PowerSaver => 0,
            PowerProfile::Balanced => 1,
            PowerProfile::Performance => 2,
        }
    }
    fn owner(value: String) -> Result<String, ControlError> {
        let name = value
            .strip_prefix(':')
            .ok_or_else(|| invalid("Daemon owner is not a unique bus name"))?;
        if value.len() > 255
            || name.split('.').count() < 2
            || name.split('.').any(str::is_empty)
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(invalid("Invalid unique daemon owner"));
        }
        Ok(value)
    }
    fn bus_id(io: &mut impl Io) -> Result<String, ControlError> {
        let value = text(&scalar(&json(io.query(Query::BusId)?)?, "s")?, 32)?;
        if value.len() != 32 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("Invalid system-bus identity"));
        }
        Ok(value)
    }
    fn has_owner(io: &mut impl Io, endpoint: Endpoint) -> Result<bool, ControlError> {
        scalar(&json(io.query(Query::HasOwner(endpoint))?)?, "b")?
            .as_bool()
            .ok_or_else(|| invalid("NameHasOwner did not return Boolean"))
    }
    fn unique_owner(io: &mut impl Io, endpoint: Endpoint) -> Result<String, ControlError> {
        owner(text(
            &scalar(&json(io.query(Query::Owner(endpoint))?)?, "s")?,
            255,
        )?)
    }
    #[derive(Clone)]
    struct Observation {
        current: String,
        details: ProfileDetails,
        binding: String,
    }
    fn parse_properties(
        reply: Value,
        daemon: ProfileDaemonIdentity,
    ) -> Result<Observation, ControlError> {
        let dictionary = scalar(&reply, "a{sv}")?;
        let properties = dictionary
            .as_object()
            .ok_or_else(|| invalid("GetAll did not return a property dictionary"))?;
        let get = |name: &str| {
            properties.get(name).ok_or_else(|| {
                invalid(format!("Required power-profile property {name} is missing"))
            })
        };
        let current = variant_text(get("ActiveProfile")?, 32)?;
        parse_profile(&current)?;
        let degraded = variant_text(get("PerformanceDegraded")?, 1024)?;
        let profiles = body(get("Profiles")?, "aa{sv}")?
            .as_array()
            .ok_or_else(|| invalid("Profiles is not a dictionary array"))?;
        if profiles.is_empty() || profiles.len() > 3 {
            return Err(invalid(
                "Profiles is empty or exceeds the three supported profiles",
            ));
        }
        let mut advertised = vec![];
        for value in profiles {
            let mut attributes = dict_strings(value)?;
            let name = attributes
                .remove("Profile")
                .ok_or_else(|| invalid("Advertised profile has no Profile string"))?;
            let profile = parse_profile(&name)?;
            if advertised
                .iter()
                .any(|p: &AdvertisedProfile| p.profile == profile)
            {
                return Err(invalid("Daemon advertised a duplicate profile"));
            }
            if attributes.get("Driver").is_none_or(String::is_empty) {
                return Err(invalid("Advertised profile has no driver identity"));
            }
            advertised.push(AdvertisedProfile {
                profile,
                attributes,
            });
        }
        advertised.sort_by_key(|p| rank(p.profile));
        if !advertised.iter().any(|p| p.profile.to_string() == current) {
            return Err(invalid(
                "ActiveProfile is not one of the advertised profiles",
            ));
        }
        let values = body(get("ActiveProfileHolds")?, "aa{sv}")?
            .as_array()
            .ok_or_else(|| invalid("ActiveProfileHolds is not a dictionary array"))?;
        if values.len() > 64 {
            return Err(invalid("ActiveProfileHolds exceeds the inspection bound"));
        }
        let mut holds = vec![];
        for value in values {
            let mut attributes = dict_strings(value)?;
            let application_id = attributes
                .remove("ApplicationId")
                .ok_or_else(|| invalid("Hold has no ApplicationId"))?;
            if application_id.len() > 256 {
                return Err(invalid("Hold application ID exceeds its bound"));
            }
            let profile = parse_profile(
                &attributes
                    .remove("Profile")
                    .ok_or_else(|| invalid("Hold has no Profile"))?,
            )?;
            if profile == PowerProfile::Balanced || !advertised.iter().any(|p| p.profile == profile)
            {
                return Err(invalid("Hold refers to an unsupported held profile"));
            }
            let reason = attributes
                .remove("Reason")
                .ok_or_else(|| invalid("Hold has no Reason"))?;
            holds.push(ProfileHold {
                application_id,
                profile,
                reason,
                attributes,
            });
        }
        // Hold ordering has no semantic meaning. Preserve duplicates (distinct
        // valid cookies can expose identical public metadata), not their order.
        holds.sort_by_cached_key(|hold| {
            serde_json::to_string(hold).expect("serializable bounded hold")
        });
        let details = ProfileDetails {
            daemon,
            performance_degraded: degraded,
            holds,
            advertised_profiles: advertised,
            holds_will_be_released: false,
        };
        let mut hash = Sha256::new();
        hash.update(b"mechrevo.ppd.binding.v1\0");
        hash.update(serde_json::to_vec(&(&current, &details)).map_err(|e| invalid(e.to_string()))?);
        Ok(Observation {
            current,
            details,
            binding: format!("{:x}", hash.finalize()),
        })
    }
    fn observe(io: &mut impl Io) -> Result<Option<Observation>, ControlError> {
        let bus = bus_id(io)?;
        let endpoint = if has_owner(io, Endpoint::Modern)? {
            Endpoint::Modern
        } else if has_owner(io, Endpoint::Legacy)? {
            Endpoint::Legacy
        } else {
            if bus_id(io)? != bus
                || has_owner(io, Endpoint::Modern)?
                || has_owner(io, Endpoint::Legacy)?
            {
                return Err(failure(
                    "profile_daemon_authority_changed",
                    "A daemon appeared or the bus changed during discovery",
                ));
            }
            return Ok(None);
        };
        let unique = unique_owner(io, endpoint)?;
        let uid = scalar(&json(io.query(Query::User(unique.clone()))?)?, "u")?
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| invalid("Owner UID is not uint32"))?;
        if uid != 0 {
            return Err(failure(
                "profile_daemon_untrusted_owner",
                "The system power-profile service is not owned by UID 0",
            ));
        }
        let reply = json(io.query(Query::Properties(unique.clone(), endpoint))?)?;
        let daemon = ProfileDaemonIdentity {
            bus_id: bus.clone(),
            unique_owner: unique.clone(),
            bus_name: endpoint.name().into(),
            object_path: endpoint.path().into(),
            interface: endpoint.name().into(),
            unix_user: uid,
        };
        let observation = parse_properties(reply, daemon)?;
        if bus_id(io)? != bus
            || unique_owner(io, endpoint)? != unique
            || (endpoint == Endpoint::Legacy && has_owner(io, Endpoint::Modern)?)
        {
            return Err(failure(
                "profile_daemon_authority_changed",
                "The system bus or selected daemon owner changed while reading properties",
            ));
        }
        Ok(Some(observation))
    }
    fn policy(observation: Observation) -> PowerPolicy {
        PowerPolicy {
            scope: SCOPE.into(),
            current: Some(observation.current),
            options: observation
                .details
                .advertised_profiles
                .iter()
                .map(|p| p.profile.to_string())
                .collect(),
            source: Some(EvidenceSource {
                backend: BACKEND.into(),
                path: observation.details.daemon.object_path.clone(),
                device_id: Some(observation.binding),
            }),
            details: Some(observation.details),
        }
    }
    pub(super) fn discover_with(io: &mut impl Io) -> Discovery {
        match observe(io) {
            Ok(None) => Discovery::Absent,
            Ok(Some(value)) => Discovery::Present(Ok(Box::new(policy(value)))),
            Err(e) => Discovery::Present(Err(e)),
        }
    }
    fn unavailable() -> ControlError {
        failure(
            "profile_daemon_unavailable",
            "No running system power-profile service owns either supported endpoint",
        )
    }
    fn plan_from(
        observation: Observation,
        requested: PowerProfile,
    ) -> Result<ProfilePlan, ControlError> {
        let current = observation.current.clone();
        let mut policy = policy(observation);
        let target = requested.to_string();
        if !policy.options.contains(&target) {
            return Err(failure(
                "profile_unavailable",
                "The requested profile is not advertised by the running service",
            ));
        }
        if let Some(details) = policy.details.as_mut() {
            details.holds_will_be_released = target != current && !details.holds.is_empty();
        }
        Ok(ProfilePlan {
            requested,
            target,
            previous: current,
            scope: SCOPE.into(),
            source: policy.source.unwrap(),
            details: policy.details,
        })
    }
    pub(super) fn plan_with(
        io: &mut impl Io,
        requested: PowerProfile,
    ) -> Result<ProfilePlan, ControlError> {
        plan_from(observe(io)?.ok_or_else(unavailable)?, requested)
    }
    fn same_plan(a: &ProfilePlan, b: &ProfilePlan) -> bool {
        a.requested == b.requested
            && a.target == b.target
            && a.previous == b.previous
            && a.scope == b.scope
            && a.source == b.source
            && a.details == b.details
    }
    fn attached(mut error: ControlError, mut receipt: ProfileApplyReceipt) -> ControlError {
        if receipt.primary_error.is_none() {
            receipt.primary_error = Some(error.clone());
        }
        if let Ok(encoded) = serde_json::to_string(&receipt) {
            error
                .details
                .push(format!("{PROFILE_RECEIPT_MARKER}{encoded}"));
        }
        error
    }
    fn output_error(output: CommandOutput, code: &str) -> ControlError {
        let mut error = failure(
            code,
            format!(
                "System power-profile command did not return the expected reply (status {})",
                output.status
            ),
        );
        error.details.push(format!("status={}", output.status));
        for (name, value) in [("stdout", output.stdout), ("stderr", output.stderr)] {
            if !value.is_empty() {
                error.details.push(format!(
                    "{name}: {}",
                    value.chars().take(LIMIT).collect::<String>()
                ));
            }
        }
        error
    }

    // @mark mechrevo.os_power_policy role=implementation
    pub(super) fn apply_with(
        io: &mut impl Io,
        plan: &ProfilePlan,
    ) -> Result<ApplyResult, ControlError> {
        let mut receipt = ProfileApplyReceipt::default();
        if plan.source.backend != BACKEND
            || plan.scope != SCOPE
            || plan.target != plan.requested.to_string()
            || plan.details.is_none()
        {
            return Err(attached(
                failure(
                    "profile_invalid_plan",
                    "The request is not a bound system power-profile plan",
                ),
                receipt,
            ));
        }
        let before = match observe(io) {
            Ok(Some(value)) => value,
            Ok(None) => return Err(attached(unavailable(), receipt)),
            Err(e) => return Err(attached(e, receipt)),
        };
        receipt.holds_before = before.details.holds.clone();
        let fresh = match plan_from(before.clone(), plan.requested) {
            Ok(value) => value,
            Err(e) => return Err(attached(e, receipt)),
        };
        if !same_plan(plan, &fresh) {
            return Err(attached(failure("profile_stale_plan","The service identity, profiles, holds or active profile differ from preview; no setter was submitted"),receipt));
        }
        if plan.target == plan.previous {
            receipt.outcome = ProfileApplyOutcome::Confirmed;
            receipt.stage = ProfileApplyStage::Complete;
            receipt.readback = Some(before.current);
            receipt.authority_unchanged = Some(true);
            receipt.holds_after = Some(before.details.holds);
            return Ok(ApplyResult {
                requested: plan.requested,
                confirmed: plan.target.clone(),
                previous: plan.previous.clone(),
                scope: SCOPE.into(),
                receipt: Some(receipt),
            });
        }
        receipt.stage = ProfileApplyStage::Setter;
        match io.set(&before.details.daemon, &plan.target) {
            Submission::NotSubmitted(error) => return Err(attached(error, receipt)),
            Submission::Submitted(result) => {
                receipt.submitted = true;
                receipt.outcome = ProfileApplyOutcome::Unknown;
                receipt.hold_effect = ProfileHoldEffect::Unknown;
                match result {
                    Ok(output)
                        if output.status == 0
                            && output.stdout.is_empty()
                            && output.stderr.is_empty() =>
                    {
                        receipt.acknowledged = Some(true)
                    }
                    Ok(output) => {
                        receipt.acknowledged = Some(false);
                        receipt.primary_error =
                            Some(output_error(output, "profile_daemon_set_unacknowledged"));
                    }
                    Err(error) => receipt.primary_error = Some(error),
                }
                receipt.writer_may_continue = receipt.acknowledged != Some(true);
            }
        }
        receipt.stage = ProfileApplyStage::Readback;
        match observe(io) {
            Ok(Some(after)) => {
                let stable = after.details.daemon == before.details.daemon
                    && after.details.advertised_profiles == before.details.advertised_profiles;
                receipt.authority_unchanged = Some(stable);
                receipt.readback = Some(after.current.clone());
                receipt.holds_after = Some(after.details.holds.clone());
                if stable && !receipt.holds_before.is_empty() && after.details.holds.is_empty() {
                    receipt.hold_effect = ProfileHoldEffect::ReleasedObserved;
                } else if stable
                    && receipt.holds_before.is_empty()
                    && after.details.holds.is_empty()
                {
                    receipt.hold_effect = ProfileHoldEffect::Unchanged;
                }
                let read_error = if !stable {
                    Some(failure("profile_daemon_authority_changed","Daemon identity or advertised profile implementation changed after submission"))
                } else if after.current != plan.target {
                    Some(failure("profile_readback_mismatch","The independently observed active profile differs from the requested profile"))
                } else {
                    None
                };
                if let Some(error) = read_error {
                    receipt.readback_error = Some(error.clone());
                    if receipt.primary_error.is_none() {
                        receipt.primary_error = Some(error);
                    }
                }
            }
            Ok(None) => {
                receipt.authority_unchanged = Some(false);
                let error = failure(
                    "profile_daemon_disappeared",
                    "The system power-profile service disappeared after submission",
                );
                receipt.readback_error = Some(error.clone());
                if receipt.primary_error.is_none() {
                    receipt.primary_error = Some(error);
                }
            }
            Err(error) => {
                receipt.readback_error = Some(error.clone());
                if receipt.primary_error.is_none() {
                    receipt.primary_error = Some(error);
                }
            }
        }
        if let Some(error) = receipt.primary_error.clone() {
            return Err(attached(error, receipt));
        }
        receipt.outcome = ProfileApplyOutcome::Confirmed;
        receipt.stage = ProfileApplyStage::Complete;
        Ok(ApplyResult {
            requested: plan.requested,
            confirmed: plan.target.clone(),
            previous: plan.previous.clone(),
            scope: SCOPE.into(),
            receipt: Some(receipt),
        })
    }

    fn base_args(timeout: &str) -> Vec<String> {
        [
            "--system",
            "--address=unix:path=/run/dbus/system_bus_socket",
            "--json=short",
            "--no-pager",
            "--auto-start=no",
            "--allow-interactive-authorization=no",
            timeout,
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }
    fn query_args(query: Query) -> Vec<String> {
        let mut args = base_args("--timeout=2s");
        args.push("call".into());
        let daemon = [
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        ];
        match query {
            Query::BusId => {
                args.extend(daemon.map(str::to_owned));
                args.push("GetId".into());
            }
            Query::HasOwner(endpoint) | Query::Owner(endpoint) => {
                let member = if matches!(query, Query::HasOwner(_)) {
                    "NameHasOwner"
                } else {
                    "GetNameOwner"
                };
                args.extend(daemon.map(str::to_owned));
                args.extend([member.into(), "s".into(), endpoint.name().into()]);
            }
            Query::User(owner) => {
                args.extend(daemon.map(str::to_owned));
                args.extend(["GetConnectionUnixUser".into(), "s".into(), owner]);
            }
            Query::Properties(owner, endpoint) => args.extend([
                owner,
                endpoint.path().into(),
                "org.freedesktop.DBus.Properties".into(),
                "GetAll".into(),
                "s".into(),
                endpoint.name().into(),
            ]),
        }
        args
    }
    fn setter_args(identity: &ProfileDaemonIdentity, target: &str) -> Vec<String> {
        let mut args = base_args("--timeout=infinity");
        args.extend([
            "--expect-reply=yes".into(),
            "call".into(),
            identity.unique_owner.clone(),
            identity.object_path.clone(),
            "org.freedesktop.DBus.Properties".into(),
            "Set".into(),
            "ssv".into(),
            identity.interface.clone(),
            "ActiveProfile".into(),
            "s".into(),
            target.into(),
        ]);
        args
    }
    #[cfg(target_os = "linux")]
    pub(super) struct Native;
    #[cfg(target_os = "linux")]
    impl Io for Native {
        fn query(&mut self, query: Query) -> Result<CommandOutput, ControlError> {
            let args = query_args(query);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            crate::command::run_command(
                "/usr/bin/busctl",
                &args,
                std::time::Duration::from_secs(2),
                LIMIT,
            )
        }
        fn set(&mut self, identity: &ProfileDaemonIdentity, target: &str) -> Submission {
            let args = setter_args(identity, target);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            match crate::command::run_control_helper("/usr/bin/busctl", &args, LIMIT) {
                Err(error) if error.code == "command_unavailable" => {
                    Submission::NotSubmitted(error)
                }
                result => Submission::Submitted(result),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        fn injected(code: &str) -> ControlError {
            failure(code, "offline injected failure")
        }
        fn output(value: Value) -> CommandOutput {
            CommandOutput {
                stdout: value.to_string(),
                stderr: String::new(),
                status: 0,
            }
        }
        fn string(value: &str) -> Value {
            json!({"type":"s","data":value})
        }
        fn hold(app: &str) -> Value {
            json!({"ApplicationId":string(app),"Profile":string("performance"),"Reason":string("offline workload")})
        }
        struct SetBehavior {
            submitted: bool,
            apply: bool,
            release: bool,
            reply: Result<CommandOutput, ControlError>,
            third_party: Option<String>,
        }
        impl Default for SetBehavior {
            fn default() -> Self {
                Self {
                    submitted: true,
                    apply: true,
                    release: true,
                    reply: Ok(CommandOutput {
                        stdout: String::new(),
                        stderr: String::new(),
                        status: 0,
                    }),
                    third_party: None,
                }
            }
        }
        struct Mock {
            bus: String,
            owner: String,
            uid: u32,
            modern: bool,
            legacy: bool,
            current: String,
            degraded: String,
            profiles: Vec<Value>,
            holds: Vec<Value>,
            queries: Vec<Query>,
            setters: Vec<(ProfileDaemonIdentity, String)>,
            raw_properties: Option<String>,
            query_failure: Option<Query>,
            fail_readback: bool,
            owner_after_properties: Option<String>,
            bus_after_properties: Option<String>,
            modern_after_properties: bool,
            owner_after_set: Option<String>,
            bus_after_set: Option<String>,
            behavior: Option<SetBehavior>,
        }
        impl Default for Mock {
            fn default() -> Self {
                Self{bus:"0123456789abcdef0123456789abcdef".into(),owner:":1.42".into(),uid:0,modern:true,legacy:true,current:"balanced".into(),degraded:String::new(),profiles:["power-saver","balanced","performance"].into_iter().map(|profile|json!({"Profile":string(profile),"Driver":string("offline-driver"),"CpuDriver":string("offline-cpu")})).collect(),holds:vec![],queries:vec![],setters:vec![],raw_properties:None,query_failure:None,fail_readback:false,owner_after_properties:None,bus_after_properties:None,modern_after_properties:false,owner_after_set:None,bus_after_set:None,behavior:None}
            }
        }
        impl Mock {
            fn properties(&self) -> Value {
                json!({"type":"a{sv}","data":[{"ActiveProfile":string(&self.current),"PerformanceDegraded":string(&self.degraded),"Profiles":{"type":"aa{sv}","data":self.profiles},"ActiveProfileHolds":{"type":"aa{sv}","data":self.holds},"ActionsInfo":{"type":"aa{sv}","data":[{"Enabled":{"type":"b","data":true}}]}}]})
            }
        }
        impl Io for Mock {
            fn query(&mut self, query: Query) -> Result<CommandOutput, ControlError> {
                self.queries.push(query.clone());
                if self.query_failure.as_ref() == Some(&query) {
                    return Err(injected("query_injected"));
                }
                let reply = match query {
                    Query::BusId => json!({"type":"s","data":[self.bus]}),
                    Query::HasOwner(endpoint) => {
                        json!({"type":"b","data":[if endpoint==Endpoint::Modern{self.modern}else{self.legacy}]})
                    }
                    Query::Owner(endpoint) => {
                        if !(if endpoint == Endpoint::Modern {
                            self.modern
                        } else {
                            self.legacy
                        }) {
                            return Err(injected("name_has_no_owner"));
                        }
                        json!({"type":"s","data":[self.owner]})
                    }
                    Query::User(owner) => {
                        assert_eq!(owner, self.owner);
                        json!({"type":"u","data":[self.uid]})
                    }
                    Query::Properties(owner, _) => {
                        assert_eq!(
                            owner, self.owner,
                            "Properties.GetAll must target the observed unique owner"
                        );
                        if self.fail_readback && !self.setters.is_empty() {
                            return Err(injected("readback_transport_failed"));
                        }
                        let reply = self
                            .raw_properties
                            .clone()
                            .unwrap_or_else(|| self.properties().to_string());
                        if let Some(owner) = self.owner_after_properties.take() {
                            self.owner = owner;
                        }
                        if let Some(bus) = self.bus_after_properties.take() {
                            self.bus = bus;
                        }
                        if self.modern_after_properties {
                            self.modern = true;
                        }
                        return Ok(CommandOutput {
                            stdout: reply,
                            stderr: String::new(),
                            status: 0,
                        });
                    }
                };
                Ok(output(reply))
            }
            fn set(&mut self, identity: &ProfileDaemonIdentity, target: &str) -> Submission {
                let behavior = self.behavior.take().unwrap_or_default();
                if !behavior.submitted {
                    return Submission::NotSubmitted(behavior.reply.unwrap_err());
                }
                self.setters.push((identity.clone(), target.into()));
                if behavior.release {
                    self.holds.clear();
                }
                if behavior.apply {
                    self.current = target.into();
                }
                if let Some(profile) = behavior.third_party {
                    self.current = profile;
                }
                if let Some(owner) = self.owner_after_set.take() {
                    self.owner = owner;
                }
                if let Some(bus) = self.bus_after_set.take() {
                    self.bus = bus;
                }
                Submission::Submitted(behavior.reply)
            }
        }
        fn receipt(result: Result<ApplyResult, ControlError>) -> ProfileApplyReceipt {
            error_receipt(&result.unwrap_err())
                .expect("production error must carry its typed receipt")
        }

        // @mark mechrevo.os_power_policy role=verification
        #[test]
        fn real_transaction_seam_confirms_only_ack_and_fresh_target() {
            let mut io = Mock::default();
            let plan = plan_with(&mut io, PowerProfile::Performance).unwrap();
            assert_eq!(plan.source.backend, BACKEND);
            assert_eq!(plan.details.as_ref().unwrap().daemon.unique_owner, ":1.42");
            let result = apply_with(&mut io, &plan).unwrap();
            let receipt = result.receipt.unwrap();
            assert_eq!(receipt.outcome, ProfileApplyOutcome::Confirmed);
            assert!(receipt.submitted);
            assert_eq!(receipt.acknowledged, Some(true));
            assert!(!receipt.writer_may_continue);
            assert_eq!(receipt.readback.as_deref(), Some("performance"));
            assert_eq!(receipt.authority_unchanged, Some(true));
            assert_eq!(io.setters.len(), 1);
            assert_eq!(io.setters[0].0.unique_owner, ":1.42");
            assert!(io.queries.iter().all(|q| !matches!(
                q,
                Query::HasOwner(Endpoint::Legacy) | Query::Owner(Endpoint::Legacy)
            )));
        }
        #[test]
        fn absent_requires_both_names_explicitly_false_and_transport_errors_are_present() {
            let mut io = Mock {
                modern: false,
                legacy: false,
                ..Mock::default()
            };
            assert!(matches!(discover_with(&mut io), Discovery::Absent));
            assert!(io.setters.is_empty());
            let mut io = Mock {
                query_failure: Some(Query::HasOwner(Endpoint::Modern)),
                ..Mock::default()
            };
            assert!(matches!(discover_with(&mut io), Discovery::Present(Err(_))));
            assert!(!io
                .queries
                .iter()
                .any(|q| matches!(q, Query::HasOwner(Endpoint::Legacy))));
        }
        #[test]
        fn legacy_uses_the_complete_legacy_endpoint_only_when_modern_is_unowned() {
            let mut io = Mock {
                modern: false,
                ..Mock::default()
            };
            let plan = plan_with(&mut io, PowerProfile::PowerSaver).unwrap();
            let details = plan.details.as_ref().unwrap();
            assert_eq!(details.daemon.bus_name, Endpoint::Legacy.name());
            assert_eq!(details.daemon.object_path, Endpoint::Legacy.path());
            assert_eq!(details.daemon.interface, Endpoint::Legacy.name());
            apply_with(&mut io, &plan).unwrap();
            assert_eq!(io.setters[0].0.bus_name, Endpoint::Legacy.name());
        }
        #[test]
        fn existing_malformed_modern_service_never_tries_legacy() {
            let mut io = Mock {
                raw_properties: Some("{\"type\":\"a{sv}\",\"data\":[]}".into()),
                ..Mock::default()
            };
            assert!(matches!(discover_with(&mut io), Discovery::Present(Err(_))));
            assert!(!io.queries.iter().any(|q| matches!(
                q,
                Query::HasOwner(Endpoint::Legacy) | Query::Owner(Endpoint::Legacy)
            )));
        }
        #[test]
        fn non_root_owner_is_not_a_usable_authority() {
            let mut io = Mock {
                uid: 1000,
                ..Mock::default()
            };
            let error = match discover_with(&mut io) {
                Discovery::Present(Err(e)) => e,
                _ => panic!("untrusted service was accepted"),
            };
            assert_eq!(error.code, "profile_daemon_untrusted_owner");
            assert!(!io
                .queries
                .iter()
                .any(|q| matches!(q, Query::Properties(..))));
        }
        #[test]
        fn owner_bus_or_endpoint_change_during_getall_is_not_observation() {
            for mode in 0..3 {
                let mut io = Mock::default();
                match mode {
                    0 => io.owner_after_properties = Some(":1.43".into()),
                    1 => io.bus_after_properties = Some("1123456789abcdef0123456789abcdef".into()),
                    _ => {
                        io.modern = false;
                        io.modern_after_properties = true;
                    }
                }
                assert!(matches!(discover_with(&mut io), Discovery::Present(Err(_))));
                assert!(io.setters.is_empty());
            }
        }
        #[test]
        fn advertised_profiles_must_be_unique_known_and_contain_current() {
            for mode in 0..5 {
                let mut io = Mock::default();
                match mode {
                    0 => io.profiles.clear(),
                    1 => io.profiles[2] = io.profiles[0].clone(),
                    2 => io.profiles[0]["Profile"] = string("turbo"),
                    3 => io.profiles.retain(|p| p["Profile"]["data"] != "balanced"),
                    _ => io.profiles[0]["Driver"] = json!({"type":"u","data":1}),
                }
                assert!(matches!(discover_with(&mut io), Discovery::Present(Err(_))));
                assert!(io.setters.is_empty());
            }
        }
        #[test]
        fn empty_missing_malformed_or_ambiguous_variant_replies_are_not_available() {
            for mode in 0..8 {
                let io = Mock::default();
                let mut value = io.properties();
                match mode {
                    0 => value["data"] = json!([]),
                    1 => value["data"] = json!([{}, {}]),
                    2 => value["data"] = json!({}),
                    3 => {
                        value["data"][0]
                            .as_object_mut()
                            .unwrap()
                            .remove("ActiveProfileHolds");
                    }
                    4 => value["data"][0]["ActiveProfileHolds"]["data"] = Value::Null,
                    5 => value["data"][0]["ActiveProfile"]["data"] = json!(["balanced"]),
                    6 => value["data"][0]["Profiles"]["data"][0]["Profile"]["extra"] = json!(1),
                    _ => {
                        value["data"][0]["ActiveProfileHolds"]["data"] = json!([{"ApplicationId":string("app"),"Profile":string("performance"),"Reason":{"type":"b","data":true}}])
                    }
                }
                let mut io = Mock {
                    raw_properties: Some(value.to_string()),
                    ..Mock::default()
                };
                assert!(
                    matches!(discover_with(&mut io), Discovery::Present(Err(_))),
                    "malformed case {mode}"
                );
            }
            for data in [
                r#"{"type":"s","type":"s","data":["x"]}"#,
                r#"{"type":"a{sv}","data":[{"a":{"type":"s","data":"x"},"a":{"type":"s","data":"y"}}]}"#,
                "",
            ] {
                assert!(json(CommandOutput {
                    stdout: data.into(),
                    stderr: String::new(),
                    status: 0
                })
                .is_err());
            }
        }
        #[test]
        fn holds_are_visible_and_allowed_for_an_explicit_different_profile() {
            let mut io = Mock {
                current: "performance".into(),
                holds: vec![hold("org.example.worker")],
                degraded: "future-degradation-reason".into(),
                ..Mock::default()
            };
            let plan = plan_with(&mut io, PowerProfile::Balanced).unwrap();
            let details = plan.details.as_ref().unwrap();
            assert!(details.holds_will_be_released);
            assert_eq!(details.holds[0].application_id, "org.example.worker");
            assert_eq!(details.performance_degraded, "future-degradation-reason");
            let result = apply_with(&mut io, &plan).unwrap();
            let receipt = result.receipt.unwrap();
            assert_eq!(receipt.hold_effect, ProfileHoldEffect::ReleasedObserved);
            assert_eq!(receipt.holds_before.len(), 1);
            assert_eq!(receipt.holds_after, Some(vec![]));
            assert_eq!(io.setters.len(), 1);
        }
        #[test]
        fn same_profile_noop_does_not_release_applications_holds() {
            let mut io = Mock {
                current: "performance".into(),
                holds: vec![hold("app")],
                ..Mock::default()
            };
            let plan = plan_with(&mut io, PowerProfile::Performance).unwrap();
            assert!(!plan.details.as_ref().unwrap().holds_will_be_released);
            let result = apply_with(&mut io, &plan).unwrap();
            let receipt = result.receipt.unwrap();
            assert!(!receipt.submitted);
            assert_eq!(receipt.acknowledged, None);
            assert!(!receipt.writer_may_continue);
            assert_eq!(receipt.hold_effect, ProfileHoldEffect::Unchanged);
            assert_eq!(io.holds.len(), 1);
            assert!(io.setters.is_empty());
        }
        #[test]
        fn public_order_is_normalized_but_duplicate_holds_keep_their_multiplicity() {
            let mut io = Mock {
                holds: vec![hold("b"), hold("a"), hold("a")],
                ..Mock::default()
            };
            let plan = plan_with(&mut io, PowerProfile::Performance).unwrap();
            io.holds.reverse();
            io.profiles.reverse();
            let result = apply_with(&mut io, &plan).unwrap();
            assert_eq!(result.receipt.unwrap().holds_before.len(), 3);
            assert_eq!(io.setters.len(), 1);
        }
        #[test]
        fn changed_owner_bus_profiles_holds_or_metadata_make_the_entire_plan_stale() {
            for mode in 0..7 {
                let mut io = Mock::default();
                let plan = plan_with(&mut io, PowerProfile::Performance).unwrap();
                match mode {
                    0 => io.owner = ":1.43".into(),
                    1 => io.bus = "1123456789abcdef0123456789abcdef".into(),
                    2 => io.current = "power-saver".into(),
                    3 => io.holds.push(hold("new-app")),
                    4 => io.profiles[0]["CpuDriver"] = string("changed-driver"),
                    5 => io.degraded = "lap-detected".into(),
                    _ => io.modern = false,
                }
                let receipt = receipt(apply_with(&mut io, &plan));
                assert_eq!(receipt.outcome, ProfileApplyOutcome::NotSubmitted);
                assert!(!receipt.submitted);
                assert!(io.setters.is_empty(), "stale case {mode}");
            }
        }
        #[test]
        fn changing_one_existing_hold_reason_also_invalidates_preview() {
            let mut io = Mock {
                holds: vec![hold("app")],
                ..Mock::default()
            };
            let plan = plan_with(&mut io, PowerProfile::Performance).unwrap();
            io.holds[0]["Reason"] = string("different reason");
            assert_eq!(
                receipt(apply_with(&mut io, &plan)).outcome,
                ProfileApplyOutcome::NotSubmitted
            );
            assert!(io.setters.is_empty());
        }
        #[test]
        fn tampered_plan_metadata_does_not_select_another_path_or_setter() {
            let mut io = Mock::default();
            let plan = plan_with(&mut io, PowerProfile::Performance).unwrap();
            for mode in 0..7 {
                let mut plan = plan.clone();
                match mode {
                    0 => plan.source.path = "/untrusted/path".into(),
                    1 => plan.source.device_id = Some("other binding".into()),
                    2 => plan.previous = "power-saver".into(),
                    3 => plan.target = "power-saver".into(),
                    4 => plan.details.as_mut().unwrap().daemon.unique_owner = ":1.666".into(),
                    5 => plan.details = None,
                    _ => plan.details.as_mut().unwrap().holds_will_be_released = true,
                }
                let receipt = receipt(apply_with(&mut io, &plan));
                assert!(!receipt.submitted);
                assert!(io.setters.is_empty());
            }
        }
        #[test]
        fn write_error_can_already_have_released_holds_without_changing_profile() {
            let mut io = Mock {
                current: "performance".into(),
                holds: vec![hold("app")],
                behavior: Some(SetBehavior {
                    apply: false,
                    reply: Ok(CommandOutput {
                        stdout: String::new(),
                        stderr: "Failed to activate profile".into(),
                        status: 1,
                    }),
                    ..SetBehavior::default()
                }),
                ..Mock::default()
            };
            let plan = plan_with(&mut io, PowerProfile::Balanced).unwrap();
            let receipt = receipt(apply_with(&mut io, &plan));
            assert_eq!(receipt.outcome, ProfileApplyOutcome::Unknown);
            assert_eq!(receipt.acknowledged, Some(false));
            assert!(receipt.writer_may_continue);
            assert_eq!(receipt.readback.as_deref(), Some("performance"));
            assert_eq!(receipt.hold_effect, ProfileHoldEffect::ReleasedObserved);
            assert_eq!(receipt.holds_after, Some(vec![]));
            assert!(receipt
                .primary_error
                .unwrap()
                .details
                .iter()
                .any(|d| d.contains("Failed to activate profile")));
            assert_eq!(
                io.setters.len(),
                1,
                "no retry or rollback can recreate external holds"
            );
        }
        #[test]
        fn lost_reply_does_not_become_success_even_if_target_is_read_back() {
            let mut io = Mock {
                behavior: Some(SetBehavior {
                    reply: Err(injected("capture_failed_after_set")),
                    ..SetBehavior::default()
                }),
                ..Mock::default()
            };
            let plan = plan_with(&mut io, PowerProfile::Performance).unwrap();
            let receipt = receipt(apply_with(&mut io, &plan));
            assert_eq!(receipt.outcome, ProfileApplyOutcome::Unknown);
            assert_eq!(receipt.acknowledged, None);
            assert!(receipt.writer_may_continue);
            assert_eq!(receipt.readback.as_deref(), Some("performance"));
            assert_eq!(
                receipt.primary_error.unwrap().code,
                "capture_failed_after_set"
            );
            assert_eq!(io.setters.len(), 1);
        }
        #[test]
        fn lost_reply_and_old_readback_do_not_fence_a_possible_daemon_request() {
            let mut io = Mock {
                behavior: Some(SetBehavior {
                    apply: false,
                    release: false,
                    reply: Err(injected("transport_failed")),
                    ..SetBehavior::default()
                }),
                ..Mock::default()
            };
            let plan = plan_with(&mut io, PowerProfile::Performance).unwrap();
            let receipt = receipt(apply_with(&mut io, &plan));
            assert!(receipt.writer_may_continue);
            assert_eq!(receipt.readback.as_deref(), Some("balanced"));
            assert_eq!(receipt.primary_error.unwrap().code, "transport_failed");
            assert_eq!(
                receipt.readback_error.unwrap().code,
                "profile_readback_mismatch"
            );
            assert_eq!(io.setters.len(), 1);
        }
        #[test]
        fn acknowledged_but_third_party_readback_is_never_overwritten() {
            let mut io = Mock {
                behavior: Some(SetBehavior {
                    third_party: Some("power-saver".into()),
                    ..SetBehavior::default()
                }),
                ..Mock::default()
            };
            let plan = plan_with(&mut io, PowerProfile::Performance).unwrap();
            let receipt = receipt(apply_with(&mut io, &plan));
            assert_eq!(receipt.outcome, ProfileApplyOutcome::Unknown);
            assert_eq!(receipt.acknowledged, Some(true));
            assert!(!receipt.writer_may_continue);
            assert_eq!(receipt.readback.as_deref(), Some("power-saver"));
            assert_eq!(io.current, "power-saver");
            assert_eq!(io.setters.len(), 1);
        }
        #[test]
        fn acknowledged_reply_cannot_confirm_a_replaced_owner_or_bus() {
            for bus_change in [false, true] {
                let mut io = Mock::default();
                let plan = plan_with(&mut io, PowerProfile::Performance).unwrap();
                if bus_change {
                    io.bus_after_set = Some("1123456789abcdef0123456789abcdef".into())
                } else {
                    io.owner_after_set = Some(":1.43".into())
                }
                let receipt = receipt(apply_with(&mut io, &plan));
                assert_eq!(receipt.outcome, ProfileApplyOutcome::Unknown);
                assert_eq!(receipt.authority_unchanged, Some(false));
                assert_eq!(receipt.readback.as_deref(), Some("performance"));
                assert_eq!(io.setters.len(), 1);
            }
        }
        #[test]
        fn an_empty_hold_list_from_a_different_authority_does_not_verify_release() {
            let mut io = Mock {
                current: "performance".into(),
                holds: vec![hold("app")],
                ..Mock::default()
            };
            let plan = plan_with(&mut io, PowerProfile::Balanced).unwrap();
            io.owner_after_set = Some(":1.43".into());
            let receipt = receipt(apply_with(&mut io, &plan));
            assert_eq!(receipt.outcome, ProfileApplyOutcome::Unknown);
            assert_eq!(receipt.authority_unchanged, Some(false));
            assert_eq!(receipt.hold_effect, ProfileHoldEffect::Unknown);
            assert_eq!(receipt.holds_after, Some(vec![]));
        }
        #[test]
        fn setter_and_readback_errors_are_retained_independently_without_recursive_marker() {
            let mut io = Mock {
                fail_readback: true,
                behavior: Some(SetBehavior {
                    reply: Err(injected("setter_transport_error")),
                    ..SetBehavior::default()
                }),
                ..Mock::default()
            };
            let plan = plan_with(&mut io, PowerProfile::Performance).unwrap();
            let error = apply_with(&mut io, &plan).unwrap_err();
            let receipt = error_receipt(&error).unwrap();
            assert_eq!(
                receipt.primary_error.unwrap().code,
                "setter_transport_error"
            );
            assert_eq!(
                receipt.readback_error.unwrap().code,
                "readback_transport_failed"
            );
            let encoded = error
                .details
                .iter()
                .find_map(|d| d.strip_prefix(PROFILE_RECEIPT_MARKER))
                .unwrap();
            assert!(!encoded.contains(PROFILE_RECEIPT_MARKER));
            assert_eq!(io.setters.len(), 1);
        }
        #[test]
        fn strict_void_ack_does_not_accept_empty_json_or_stdout_noise() {
            for stdout in ["{\"type\":\"\",\"data\":[]}", "\n", "unexpected"] {
                let mut io = Mock {
                    behavior: Some(SetBehavior {
                        reply: Ok(CommandOutput {
                            stdout: stdout.into(),
                            stderr: String::new(),
                            status: 0,
                        }),
                        ..SetBehavior::default()
                    }),
                    ..Mock::default()
                };
                let plan = plan_with(&mut io, PowerProfile::Performance).unwrap();
                let receipt = receipt(apply_with(&mut io, &plan));
                assert_eq!(receipt.outcome, ProfileApplyOutcome::Unknown);
                assert_eq!(receipt.acknowledged, Some(false));
                assert!(receipt.writer_may_continue);
                assert_eq!(io.setters.len(), 1);
            }
        }
        #[test]
        fn unavailable_profile_and_failed_process_launch_are_not_submitted() {
            let mut io = Mock::default();
            io.profiles
                .retain(|p| p["Profile"]["data"] != "performance");
            assert!(plan_with(&mut io, PowerProfile::Performance).is_err());
            assert!(io.setters.is_empty());
            let mut io = Mock {
                behavior: Some(SetBehavior {
                    submitted: false,
                    reply: Err(injected("command_unavailable")),
                    ..SetBehavior::default()
                }),
                ..Mock::default()
            };
            let plan = plan_with(&mut io, PowerProfile::Performance).unwrap();
            let receipt = receipt(apply_with(&mut io, &plan));
            assert!(!receipt.submitted);
            assert!(!receipt.writer_may_continue);
            assert!(io.setters.is_empty());
        }
        #[test]
        fn fixed_transport_arguments_bind_unique_owner_and_have_no_setter_deadline() {
            let identity = ProfileDaemonIdentity {
                bus_id: "0".repeat(32),
                unique_owner: ":1.42".into(),
                bus_name: Endpoint::Modern.name().into(),
                object_path: Endpoint::Modern.path().into(),
                interface: Endpoint::Modern.name().into(),
                unix_user: 0,
            };
            let args = setter_args(&identity, "power-saver");
            assert!(args
                .iter()
                .any(|a| a == "--address=unix:path=/run/dbus/system_bus_socket"));
            assert!(args.iter().any(|a| a == "--auto-start=no"));
            assert!(args.iter().any(|a| a == "--timeout=infinity"));
            assert!(!args
                .iter()
                .any(|a| a == "--timeout=0" || a == "--timeout=2s"));
            let call = args.iter().position(|a| a == "call").unwrap();
            assert_eq!(&args[call + 1], ":1.42");
            assert!(args
                .windows(4)
                .any(|a| a == ["Set", "ssv", Endpoint::Modern.name(), "ActiveProfile"]));
            let args = query_args(Query::Properties(":1.42".into(), Endpoint::Modern));
            let call = args.iter().position(|a| a == "call").unwrap();
            assert_eq!(&args[call + 1], ":1.42");
            assert!(args.iter().any(|a| a == "--timeout=2s"));
        }
        #[test]
        fn excessive_strings_and_malformed_owner_are_not_discovery() {
            let mut io = Mock {
                owner: Endpoint::Modern.name().into(),
                ..Mock::default()
            };
            assert!(matches!(discover_with(&mut io), Discovery::Present(Err(_))));
            let mut io = Mock {
                degraded: "x".repeat(1025),
                ..Mock::default()
            };
            assert!(matches!(discover_with(&mut io), Discovery::Present(Err(_))));
        }
        #[test]
        fn duplicate_or_malformed_receipt_markers_are_not_typed_receipts() {
            let mut error = injected("error");
            error
                .details
                .push(format!("{PROFILE_RECEIPT_MARKER}not-json"));
            assert!(error_receipt(&error).is_none());
            let mut error = attached(injected("error"), ProfileApplyReceipt::default());
            error.details.push(error.details.last().unwrap().clone());
            assert!(error_receipt(&error).is_none());
        }
    }
}
