//! Retention must not lose semantic roots through duplicate JSON keys or treat
//! unknown values of known fields as a schema it understands. This is stricter
//! than the download parser: a malformed index defers collection, never repair.
use serde::{
    de::{self, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{collections::HashSet, fmt};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Index {
    #[serde(deserialize_with = "unique_objects")]
    pub(super) objects: Vec<Object>,
    #[serde(rename = "virtual", default, deserialize_with = "present_bool")]
    _virtual: Option<bool>,
    #[serde(
        rename = "map_to_resources",
        default,
        deserialize_with = "present_bool"
    )]
    _resources: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Object {
    pub(super) hash: String,
    #[serde(rename = "size", default, deserialize_with = "present_size")]
    _size: Option<u64>,
}

// Omission is historical/optional; an explicitly present null is not a bool/size.
fn present_bool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
    bool::deserialize(deserializer).map(Some)
}

fn present_size<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    u64::deserialize(deserializer).map(Some)
}

fn unique_objects<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Object>, D::Error> {
    struct Objects;
    impl<'de> Visitor<'de> for Objects {
        type Value = Vec<Object>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an asset object map without duplicate names")
        }

        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut names = HashSet::new();
            let mut objects = Vec::new();
            while let Some(name) = map.next_key::<String>()? {
                if !names.insert(name) {
                    return Err(de::Error::custom("duplicate asset object name"));
                }
                objects.push(map.next_value()?);
            }
            Ok(objects)
        }
    }
    deserializer.deserialize_map(Objects)
}
