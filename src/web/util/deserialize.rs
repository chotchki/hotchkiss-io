/// From here: https://github.com/serde-rs/serde/issues/1425#issuecomment-462282398
use serde::de::IntoDeserializer;
use serde::Deserialize;

pub fn empty_string_as_none<'de, D>(de: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(de)?;
    let opt = opt.as_deref();
    match opt {
        None | Some("") => Ok(None),
        Some(s) => String::deserialize(s.into_deserializer()).map(Some),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    #[derive(Debug, Deserialize, PartialEq)]
    pub struct StringTest {
        #[serde(deserialize_with = "empty_string_as_none")]
        pub s: Option<String>,
    }

    #[test]
    fn strings() -> Result<()> {
        let s_none: StringTest = serde_json::from_str("{\"s\": \"\"}")?;
        assert_eq!(s_none, StringTest { s: None });

        let s_some: StringTest = serde_json::from_str("{\"s\": \"foo\"}")?;
        assert_eq!(
            s_some,
            StringTest {
                s: Some("foo".to_string())
            }
        );

        Ok(())
    }
}
