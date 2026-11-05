//! Lavalink v4 audio filter request and state models.

use crate::Omissible;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Filters {
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub volume: Omissible<f32>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub equalizer: Omissible<Vec<Band>>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub karaoke: Omissible<Option<Karaoke>>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub timescale: Omissible<Option<Timescale>>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub tremolo: Omissible<Option<Tremolo>>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub vibrato: Omissible<Option<Vibrato>>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub distortion: Omissible<Option<Distortion>>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub rotation: Omissible<Option<Rotation>>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub channel_mix: Omissible<Option<ChannelMix>>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub low_pass: Omissible<Option<LowPass>>,
    /// Plugin filter configurations use Lavalink v4's nested pluginFilters map.
    /// Native playback rejects individual plugin filters until a registered
    /// handler can consume them.
    #[serde(default, rename = "pluginFilters", skip_serializing_if = "HashMap::is_empty")]
    pub plugin_filters: HashMap<String, Value>,
}

impl Filters {
    pub fn invalid_disabled_filters(&self, disabled: &[&str]) -> Vec<String> {
        let mut invalid = Vec::new();
        let standard = [
            ("volume", !self.volume.is_omitted()),
            ("equalizer", !self.equalizer.is_omitted()),
            ("karaoke", !self.karaoke.is_omitted()),
            ("timescale", !self.timescale.is_omitted()),
            ("tremolo", !self.tremolo.is_omitted()),
            ("vibrato", !self.vibrato.is_omitted()),
            ("distortion", !self.distortion.is_omitted()),
            ("rotation", !self.rotation.is_omitted()),
            ("channelMix", !self.channel_mix.is_omitted()),
            ("lowPass", !self.low_pass.is_omitted()),
        ];

        for (name, requested) in standard {
            if requested && disabled.contains(&name) {
                invalid.push(name.to_owned());
            }
        }
        for name in self.plugin_filters.keys() {
            if disabled.contains(&name.as_str()) {
                invalid.push(name.clone());
            }
        }
        invalid
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Band {
    pub band: u8,
    #[serde(default = "one_f32")]
    pub gain: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Karaoke {
    #[serde(default = "one_f32")]
    pub level: f32,
    #[serde(default = "one_f32")]
    pub mono_level: f32,
    #[serde(default = "default_filter_band")]
    pub filter_band: f32,
    #[serde(default = "default_filter_width")]
    pub filter_width: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timescale {
    #[serde(default = "one_f64")]
    pub speed: f64,
    #[serde(default = "one_f64")]
    pub pitch: f64,
    #[serde(default = "one_f64")]
    pub rate: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tremolo {
    #[serde(default = "default_frequency")]
    pub frequency: f32,
    #[serde(default = "half_f32")]
    pub depth: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vibrato {
    #[serde(default = "default_frequency")]
    pub frequency: f32,
    #[serde(default = "half_f32")]
    pub depth: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rotation {
    #[serde(default)]
    pub rotation_hz: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Distortion {
    #[serde(default)]
    pub sin_offset: f32,
    #[serde(default = "one_f32")]
    pub sin_scale: f32,
    #[serde(default)]
    pub cos_offset: f32,
    #[serde(default = "one_f32")]
    pub cos_scale: f32,
    #[serde(default)]
    pub tan_offset: f32,
    #[serde(default = "one_f32")]
    pub tan_scale: f32,
    #[serde(default)]
    pub offset: f32,
    #[serde(default = "one_f32")]
    pub scale: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMix {
    #[serde(default = "one_f32")]
    pub left_to_left: f32,
    #[serde(default)]
    pub left_to_right: f32,
    #[serde(default)]
    pub right_to_left: f32,
    #[serde(default = "one_f32")]
    pub right_to_right: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LowPass {
    #[serde(default = "default_smoothing")]
    pub smoothing: f32,
}

fn one_f32() -> f32 {
    1.0
}
fn half_f32() -> f32 {
    0.5
}
fn one_f64() -> f64 {
    1.0
}
fn default_frequency() -> f32 {
    2.0
}
fn default_filter_band() -> f32 {
    220.0
}
fn default_filter_width() -> f32 {
    100.0
}
fn default_smoothing() -> f32 {
    20.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lavalink_filter_names_and_defaults() {
        let filters: Filters = serde_json::from_str(
            r#"{"timescale":{"speed":1.25},"channelMix":{"leftToRight":0.2}}"#,
        )
        .unwrap();

        assert!(matches!(filters.timescale, Omissible::Present(Some(_))));
        assert_eq!(
            filters.channel_mix,
            Omissible::Present(Some(ChannelMix {
                left_to_left: 1.0,
                left_to_right: 0.2,
                right_to_left: 0.0,
                right_to_right: 1.0,
            }))
        );
    }

    #[test]
    fn round_trips_every_lavalink_v4_standard_filter() {
        let value = serde_json::json!({
            "volume": 1.0,
            "equalizer": [{"band": 0, "gain": 0.2}],
            "karaoke": {
                "level": 1.0,
                "monoLevel": 1.0,
                "filterBand": 220.0,
                "filterWidth": 100.0
            },
            "timescale": {"speed": 1.0, "pitch": 1.0, "rate": 1.0},
            "tremolo": {"frequency": 2.0, "depth": 0.5},
            "vibrato": {"frequency": 2.0, "depth": 0.5},
            "rotation": {"rotationHz": 0.0},
            "distortion": {
                "sinOffset": 0.0,
                "sinScale": 1.0,
                "cosOffset": 0.0,
                "cosScale": 1.0,
                "tanOffset": 0.0,
                "tanScale": 1.0,
                "offset": 0.0,
                "scale": 1.0
            },
            "channelMix": {
                "leftToLeft": 1.0,
                "leftToRight": 0.0,
                "rightToLeft": 0.0,
                "rightToRight": 1.0
            },
            "lowPass": {"smoothing": 20.0}
        });

        let filters: Filters = serde_json::from_value(value.clone()).unwrap();
        assert!(matches!(filters.timescale, Omissible::Present(Some(_))));
        let serialized = serde_json::to_value(filters).unwrap();
        let gain = serialized["equalizer"][0]["gain"].as_f64().unwrap();
        assert!((gain - 0.2).abs() < 1e-6);
        let mut expected = value;
        expected["equalizer"][0]["gain"] = serialized["equalizer"][0]["gain"].clone();
        assert_eq!(serialized, expected);
    }

    #[test]
    fn empty_filter_state_serializes_without_omitted_fields() {
        assert_eq!(
            serde_json::to_value(Filters::default()).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn preserves_unknown_filter_keys_as_plugin_filters() {
        let filters: Filters = serde_json::from_str(
            r#"{"pluginFilters":{"customPlugin":{"strength":0.75}}}"#,
        )
        .unwrap();
        assert_eq!(filters.plugin_filters["customPlugin"]["strength"], 0.75);
        assert_eq!(
            serde_json::to_value(filters).unwrap()["pluginFilters"]["customPlugin"]["strength"],
            0.75
        );
    }

    #[test]
    fn validates_disabled_filters() {
        let filters: Filters = serde_json::from_str(r#"{"volume":0.5,"lowPass":{}}"#).unwrap();
        assert_eq!(
            filters.invalid_disabled_filters(&["volume", "lowPass"]),
            vec!["volume".to_owned(), "lowPass".to_owned()]
        );
    }
}
