//! Typed inputs for the recovered Uniwill RamFan1p5 curve protocol.
//!
//! Protocol validity does not establish that a curve can cool a particular
//! laptop. Device qualification, exclusive ownership and verified restoration
//! belong to the driver transaction, not to this codec. No default curve is
//! inferred from another model. Original hardware bytes are retained verbatim.

use crate::ControlError;
use serde::{Deserialize, Serialize};
use std::io::Read;

pub mod broker;
pub mod client;

#[cfg(target_os = "linux")]
pub mod linux;

pub const MAX_INPUT_BYTES: usize = 8192;
pub const TABLE_BYTES: usize = 96;
pub const OWNED_TABLE_BYTES: usize = 94;
pub const PROTOCOL_PAYLOAD_BYTES: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanCurveLayout {
    UniwillRamFan1p5,
}

/// There are 16 duty stages and 15 boundaries in each direction. The original
/// OEM node format includes two endpoints which its writer ignores; those are
/// deliberately absent here. A raw duty step is one half of a percentage point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FanCurveChannel {
    pub rise_celsius: [u8; 15],
    pub fall_celsius: [u8; 15],
    pub duty_raw: [u8; 16],
}

// @mark mechrevo.fan_curve role=interface contract=ramfan1p5_typed_input
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FanCurve {
    pub schema_version: u32,
    pub layout: FanCurveLayout,
    pub respective: bool,
    pub cpu: FanCurveChannel,
    pub gpu: FanCurveChannel,
}

/// Explicit whole-percent lower bounds applied to a complete user curve.
/// These are command duty values, not measured RPM, acoustic thresholds or a
/// thermal qualification. Absent channel limits leave that channel unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MinimumDutyPolicy {
    pub cpu_min_percent: Option<u8>,
    pub gpu_min_percent: Option<u8>,
    /// Explicit permission to raise zero-valued draft stages as well. Keeping
    /// zero is a byte-level policy, not proof that physical fans stop there.
    pub raise_stopped_stages: bool,
}
impl MinimumDutyPolicy {
    pub fn validate(&self) -> Result<(), ControlError> {
        for (channel, floor) in [("CPU", self.cpu_min_percent), ("GPU", self.gpu_min_percent)] {
            if floor.is_some_and(|percent| !(1..=100).contains(&percent)) {
                return Err(ControlError::new(
                    "fan_minimum_percent_out_of_range",
                    format!("{channel} minimum duty must be a whole percent from 1 through 100"),
                ));
            }
        }
        if self.raise_stopped_stages
            && self.cpu_min_percent.is_none()
            && self.gpu_min_percent.is_none()
        {
            return Err(ControlError::new(
                "fan_minimum_requires_channel",
                "Raising zero-valued stages requires an explicit CPU or GPU minimum",
            ));
        }
        Ok(())
    }
}

/// Can only be constructed after complete protocol validation. It does not
/// implement Deserialize: callers cannot bypass validation at a JSON boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedFanCurve(FanCurve);

impl FanCurve {
    pub fn validate(self) -> Result<ValidatedFanCurve, ControlError> {
        if self.schema_version != 1 {
            return Err(ControlError::new(
                "invalid_curve_schema",
                "Fan curve schema_version must be exactly 1",
            ));
        }
        for (name, channel) in [("cpu", &self.cpu), ("gpu", &self.gpu)] {
            for (stage, duty) in channel.duty_raw.iter().enumerate() {
                if *duty > 200 {
                    return Err(ControlError::new(
                        "fan_curve_duty_out_of_range",
                        format!("{name}.duty_raw[{stage}] exceeds 200 (100 percent)"),
                    ));
                }
            }
        }
        // The recovered OEM enable operation refuses to set 0x07c6 bit 2
        // when CPU end[0] and end[1] are both zero. Reject before any write.
        if self.cpu.rise_celsius[0] == 0 && self.cpu.rise_celsius[1] == 0 {
            return Err(ControlError::new(
                "fan_curve_inactive_cpu",
                "At least one of the first two CPU rise boundaries must be nonzero",
            ));
        }
        // OEM profiles contain repeated 255 tails and fall values greater than
        // the following rise boundary. Do not invent a firmware restriction.
        Ok(ValidatedFanCurve(self))
    }
}

impl ValidatedFanCurve {
    pub fn from_json(input: &[u8]) -> Result<Self, ControlError> {
        if input.len() > MAX_INPUT_BYTES {
            return Err(ControlError::new(
                "curve_input_too_large",
                format!("Fan curve input exceeds {MAX_INPUT_BYTES} bytes"),
            ));
        }
        let curve: FanCurve = serde_json::from_slice(input)
            .map_err(|error| ControlError::new("invalid_curve_json", error.to_string()))?;
        curve.validate()
    }

    /// Bounds the bytes read even when a file grows after its metadata is read.
    pub fn from_reader(reader: impl Read) -> Result<Self, ControlError> {
        let mut input = Vec::new();
        reader
            .take((MAX_INPUT_BYTES + 1) as u64)
            .read_to_end(&mut input)
            .map_err(|error| ControlError::new("curve_read_failed", error.to_string()))?;
        Self::from_json(&input)
    }

    pub fn curve(&self) -> &FanCurve {
        &self.0
    }

    /// Raise selected duty stages in this validated user draft. Temperatures,
    /// channel selection and the source curve remain untouched. Applying a
    /// smaller floor later does not undo previous edits to the draft.
    pub fn with_minimum_duty(&self, policy: MinimumDutyPolicy) -> Result<Self, ControlError> {
        policy.validate()?;
        let mut curve = self.0.clone();
        for (channel, floor) in [
            (&mut curve.cpu, policy.cpu_min_percent),
            (&mut curve.gpu, policy.gpu_min_percent),
        ] {
            if let Some(percent) = floor {
                let raw_floor = percent * 2; // validated 1..=100, exactly 0.5% units
                for duty in &mut channel.duty_raw {
                    if *duty != 0 || policy.raise_stopped_stages {
                        *duty = (*duty).max(raw_floor);
                    }
                }
            }
        }
        curve.validate()
    }

    /// Typed ioctl payload after its versioned 64-byte control prefix. The
    /// final three reserved bytes are always zero, with no native padding.
    pub fn protocol_payload(&self) -> [u8; PROTOCOL_PAYLOAD_BYTES] {
        let mut payload = [0; PROTOCOL_PAYLOAD_BYTES];
        for (fan, channel) in [&self.0.cpu, &self.0.gpu].iter().enumerate() {
            payload[fan * 15..(fan + 1) * 15].copy_from_slice(&channel.rise_celsius);
            payload[30 + fan * 15..30 + (fan + 1) * 15].copy_from_slice(&channel.fall_celsius);
            payload[60 + fan * 16..60 + (fan + 1) * 16].copy_from_slice(&channel.duty_raw);
        }
        payload[92] = u8::from(self.0.respective);
        payload
    }
}

/// An exact 0x0f00..=0x0f5f hardware image. Restoration must use these bytes,
/// even when odd duties, end markers or old values fail new-input policy.
/// There is intentionally no conversion from a raw snapshot to a new curve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFanTable([u8; TABLE_BYTES]);

impl RawFanTable {
    pub fn from_bytes(bytes: [u8; TABLE_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; TABLE_BYTES] {
        &self.0
    }

    /// Produce an expected table without touching the two bytes omitted by
    /// the OEM writer (0x0f10 and 0x0f40). All 16 GPU duties are included.
    pub fn with_curve(&self, curve: &ValidatedFanCurve) -> Self {
        let mut table = self.0;
        for (fan, channel) in [&curve.0.cpu, &curve.0.gpu].iter().enumerate() {
            let base = fan * 48;
            table[base..base + 15].copy_from_slice(&channel.rise_celsius);
            table[base + 15] = 255;
            table[base + 17..base + 32].copy_from_slice(&channel.fall_celsius);
            table[base + 32..base + 48].copy_from_slice(&channel.duty_raw);
        }
        Self(table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn example() -> FanCurve {
        FanCurve {
            schema_version: 1,
            layout: FanCurveLayout::UniwillRamFan1p5,
            respective: true,
            cpu: FanCurveChannel {
                rise_celsius: std::array::from_fn(|i| 35 + i as u8 * 5),
                fall_celsius: std::array::from_fn(|i| 40 + i as u8 * 5),
                duty_raw: std::array::from_fn(|i| 51 + i as u8 * 9),
            },
            gpu: FanCurveChannel {
                rise_celsius: [255; 15],
                fall_celsius: [255; 15],
                duty_raw: [200; 16],
            },
        }
    }

    fn document() -> Value {
        serde_json::to_value(example()).unwrap()
    }

    #[test]
    fn minimum_duty_default_is_exactly_inert_including_half_percent_values() {
        let curve = example().validate().unwrap();
        assert_eq!(
            curve
                .with_minimum_duty(MinimumDutyPolicy::default())
                .unwrap(),
            curve
        );
        let serialized = serde_json::to_value(MinimumDutyPolicy::default()).unwrap();
        assert!(serialized["cpu_min_percent"].is_null());
        assert!(serialized["gpu_min_percent"].is_null());
        assert_eq!(serialized["raise_stopped_stages"], false);
    }

    #[test]
    fn minimum_duty_rejects_invalid_limits_and_unguided_zero_stage_changes() {
        let curve = example().validate().unwrap();
        for percent in [0, 101, 255] {
            for (cpu, gpu) in [(Some(percent), None), (None, Some(percent))] {
                let policy = MinimumDutyPolicy {
                    cpu_min_percent: cpu,
                    gpu_min_percent: gpu,
                    raise_stopped_stages: false,
                };
                assert_eq!(
                    curve.with_minimum_duty(policy).unwrap_err().code,
                    "fan_minimum_percent_out_of_range"
                );
            }
        }
        assert_eq!(
            curve
                .with_minimum_duty(MinimumDutyPolicy {
                    raise_stopped_stages: true,
                    ..Default::default()
                })
                .unwrap_err()
                .code,
            "fan_minimum_requires_channel"
        );
    }

    #[test]
    fn minimum_duty_changes_only_selected_nonzero_duties_and_real_payload() {
        let mut input = example();
        input.respective = false;
        input.cpu.duty_raw = [
            0, 1, 119, 120, 121, 199, 200, 99, 0, 1, 119, 120, 121, 199, 200, 99,
        ];
        input.cpu.rise_celsius[8..].fill(255);
        input.cpu.fall_celsius[5..].fill(255);
        let base = input.clone().validate().unwrap();
        let adjusted = base
            .with_minimum_duty(MinimumDutyPolicy {
                cpu_min_percent: Some(60),
                ..Default::default()
            })
            .unwrap();
        let expected = [
            0, 120, 120, 120, 121, 199, 200, 120, 0, 120, 120, 120, 121, 199, 200, 120,
        ];
        assert_eq!(adjusted.curve().cpu.duty_raw, expected);
        assert_eq!(adjusted.curve().cpu.rise_celsius, input.cpu.rise_celsius);
        assert_eq!(adjusted.curve().cpu.fall_celsius, input.cpu.fall_celsius);
        assert_eq!(adjusted.curve().gpu, input.gpu);
        assert!(!adjusted.curve().respective);
        assert_eq!(base.curve(), &input);
        assert_eq!(&adjusted.protocol_payload()[60..76], &expected);
        assert_eq!(&adjusted.protocol_payload()[76..92], &input.gpu.duty_raw);
    }

    #[test]
    fn minimum_duty_explicit_zero_stage_raise_is_channel_scoped_and_bounded() {
        let mut input = example();
        input.cpu.duty_raw = [0; 16];
        input.gpu.duty_raw = [
            0, 1, 2, 3, 99, 100, 101, 102, 119, 120, 121, 150, 151, 198, 199, 200,
        ];
        let base = input.clone().validate().unwrap();
        let adjusted = base
            .with_minimum_duty(MinimumDutyPolicy {
                gpu_min_percent: Some(100),
                raise_stopped_stages: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(adjusted.curve().gpu.duty_raw, [200; 16]);
        assert_eq!(adjusted.curve().cpu, input.cpu);
        assert_eq!(adjusted.curve().gpu.rise_celsius, input.gpu.rise_celsius);
        assert_eq!(adjusted.curve().gpu.fall_celsius, input.gpu.fall_celsius);
        assert_eq!(
            adjusted
                .with_minimum_duty(MinimumDutyPolicy {
                    gpu_min_percent: Some(1),
                    ..Default::default()
                })
                .unwrap(),
            adjusted
        );
        let original = RawFanTable::from_bytes([253; TABLE_BYTES]);
        let changed = original.with_curve(&adjusted);
        assert_eq!(&changed.as_bytes()[80..96], &[200; 16]);
        assert_eq!(changed.as_bytes()[16], 253);
        assert_eq!(changed.as_bytes()[64], 253);
        assert_eq!(original.as_bytes(), &[253; TABLE_BYTES]);
    }

    #[test]
    fn minimum_duty_one_percent_keeps_higher_half_steps_and_both_channels_separate() {
        let mut input = example();
        input.cpu.duty_raw[..4].copy_from_slice(&[0, 1, 2, 3]);
        input.gpu.duty_raw[..4].copy_from_slice(&[0, 99, 100, 101]);
        let adjusted = input
            .validate()
            .unwrap()
            .with_minimum_duty(MinimumDutyPolicy {
                cpu_min_percent: Some(1),
                gpu_min_percent: Some(50),
                raise_stopped_stages: false,
            })
            .unwrap();
        assert_eq!(&adjusted.curve().cpu.duty_raw[..4], &[0, 2, 2, 3]);
        assert_eq!(&adjusted.curve().gpu.duty_raw[..4], &[0, 100, 100, 101]);
    }

    #[test]
    fn json_roundtrip_retains_half_percent_duties_and_oem_boundary_shapes() {
        let curve = example().validate().unwrap();
        let bytes = serde_json::to_vec(curve.curve()).unwrap();
        assert_eq!(ValidatedFanCurve::from_json(&bytes).unwrap(), curve);
        // This deliberately contains fall > rise and repeated 255 values.
        assert!(curve.curve().cpu.fall_celsius[0] > curve.curve().cpu.rise_celsius[0]);
        assert_eq!(curve.curve().cpu.duty_raw[0], 51);
    }

    #[test]
    fn every_channel_field_has_exact_length_and_integer_domain() {
        for channel in ["cpu", "gpu"] {
            for (field, expected) in [("rise_celsius", 15), ("fall_celsius", 15), ("duty_raw", 16)]
            {
                for length in [0, expected - 1, expected + 1] {
                    let mut value = document();
                    value[channel][field] = json!(vec![50; length]);
                    assert!(
                        ValidatedFanCurve::from_json(&serde_json::to_vec(&value).unwrap()).is_err()
                    );
                }
                for bad in [
                    json!(-1),
                    json!(256),
                    json!(50.5),
                    json!(50.0),
                    json!(true),
                    Value::Null,
                    json!("50"),
                ] {
                    let mut value = document();
                    value[channel][field][0] = bad;
                    assert!(
                        ValidatedFanCurve::from_json(&serde_json::to_vec(&value).unwrap()).is_err()
                    );
                }
            }
        }
    }

    #[test]
    fn missing_unknown_duplicate_and_trailing_json_are_rejected() {
        for channel in ["cpu", "gpu"] {
            let mut value = document();
            value[channel].as_object_mut().unwrap().remove("duty_raw");
            assert!(ValidatedFanCurve::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
            let mut value = document();
            value[channel]["UpT0"] = json!(0);
            assert!(ValidatedFanCurve::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        let valid = serde_json::to_string(&example()).unwrap();
        let duplicate = valid.replacen(
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":1",
            1,
        );
        for invalid in [
            duplicate,
            format!("{valid} {{}}"),
            "null".into(),
            "[]".into(),
        ] {
            assert!(ValidatedFanCurve::from_json(invalid.as_bytes()).is_err());
        }
        let mut value = document();
        value["ignored"] = json!(true);
        assert!(ValidatedFanCurve::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        value.as_object_mut().unwrap().remove("ignored");
        value["layout"] = json!("some_other_laptop");
        assert!(ValidatedFanCurve::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn typed_construction_cannot_bypass_schema_duty_or_enable_checks() {
        let mut curve = example();
        curve.schema_version = 2;
        assert_eq!(curve.validate().unwrap_err().code, "invalid_curve_schema");
        for fan in 0..2 {
            for stage in 0..16 {
                let mut curve = example();
                [&mut curve.cpu, &mut curve.gpu][fan].duty_raw[stage] = 201;
                assert_eq!(
                    curve.validate().unwrap_err().code,
                    "fan_curve_duty_out_of_range"
                );
            }
        }
        let mut curve = example();
        curve.cpu.rise_celsius[..2].fill(0);
        assert_eq!(
            curve.clone().validate().unwrap_err().code,
            "fan_curve_inactive_cpu"
        );
        curve.cpu.rise_celsius[1] = 1;
        assert!(curve.validate().is_ok());
    }

    #[test]
    fn raw_image_preserves_unowned_bytes_and_every_gpu_duty_including_handshake_aliases() {
        let curve = example().validate().unwrap();
        let original = RawFanTable::from_bytes(std::array::from_fn(|i| i as u8));
        let applied = original.with_curve(&curve);
        assert_eq!(applied.0[16], 16);
        assert_eq!(applied.0[64], 64);
        for (fan, channel) in [&curve.curve().cpu, &curve.curve().gpu].iter().enumerate() {
            let base = fan * 48;
            assert_eq!(&applied.0[base..base + 15], &channel.rise_celsius);
            assert_eq!(applied.0[base + 15], 255);
            assert_eq!(&applied.0[base + 17..base + 32], &channel.fall_celsius);
            assert_eq!(&applied.0[base + 32..base + 48], &channel.duty_raw);
        }
        assert_eq!(original.0, std::array::from_fn(|i| i as u8));
        let old_outside_new_policy = RawFanTable::from_bytes([253; TABLE_BYTES]);
        assert_eq!(old_outside_new_policy.as_bytes(), &[253; TABLE_BYTES]);
    }

    #[test]
    fn ioctl_payload_has_exact_fields_and_zero_reserved_bytes() {
        let curve = example().validate().unwrap();
        let payload = curve.protocol_payload();
        assert_eq!(&payload[..15], &curve.curve().cpu.rise_celsius);
        assert_eq!(&payload[15..30], &curve.curve().gpu.rise_celsius);
        assert_eq!(&payload[30..45], &curve.curve().cpu.fall_celsius);
        assert_eq!(&payload[45..60], &curve.curve().gpu.fall_celsius);
        assert_eq!(&payload[60..76], &curve.curve().cpu.duty_raw);
        assert_eq!(&payload[76..92], &curve.curve().gpu.duty_raw);
        assert_eq!(&payload[92..], &[1, 0, 0, 0]);
    }

    #[test]
    fn bounded_reader_reports_oversize_and_io_failures() {
        let bytes = vec![b' '; MAX_INPUT_BYTES + 10];
        assert_eq!(
            ValidatedFanCurve::from_reader(&bytes[..]).unwrap_err().code,
            "curve_input_too_large"
        );
        let mut padded = serde_json::to_vec(&example()).unwrap();
        padded.resize(MAX_INPUT_BYTES, b' ');
        assert!(ValidatedFanCurve::from_reader(&padded[..]).is_ok());
        struct Fails;
        impl Read for Fails {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("source failed"))
            }
        }
        assert_eq!(
            ValidatedFanCurve::from_reader(Fails).unwrap_err().code,
            "curve_read_failed"
        );
    }
}
