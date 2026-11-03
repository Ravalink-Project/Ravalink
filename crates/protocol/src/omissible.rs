//! A field wrapper that distinguishes an omitted JSON property from an explicit null.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Omissible<T> {
    #[default]
    Omitted,
    Present(T),
}

impl<T> Omissible<T> {
    pub fn is_omitted(&self) -> bool {
        matches!(self, Self::Omitted)
    }

    pub fn as_ref(&self) -> Omissible<&T> {
        match self {
            Self::Omitted => Omissible::Omitted,
            Self::Present(value) => Omissible::Present(value),
        }
    }
}

impl<'de, T> Deserialize<'de> for Omissible<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::Present)
    }
}

impl<T> Serialize for Omissible<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Omitted => serializer.serialize_unit(),
            Self::Present(value) => value.serialize(serializer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Example {
        #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
        value: Omissible<Option<String>>,
    }

    #[test]
    fn distinguishes_missing_from_explicit_null() {
        let missing: Example = serde_json::from_str("{}").unwrap();
        let explicit_null: Example = serde_json::from_str(r#"{"value":null}"#).unwrap();

        assert_eq!(missing.value, Omissible::Omitted);
        assert_eq!(explicit_null.value, Omissible::Present(None));
    }

    #[test]
    fn omitted_fields_are_not_serialized() {
        let value = Example {
            value: Omissible::Omitted,
        };
        assert_eq!(serde_json::to_string(&value).unwrap(), "{}");
    }
}
