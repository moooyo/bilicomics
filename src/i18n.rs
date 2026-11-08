use std::{collections::BTreeMap, sync::OnceLock};

pub fn text(key: &str) -> String {
    static STRINGS: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    STRINGS
        .get_or_init(|| {
            serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/translations.json")))
                .expect("Built-in interface translations")
        })
        .get(key)
        .cloned()
        .unwrap_or_else(|| key.to_owned())
}

pub fn format(key: &str, values: &[(&str, String)]) -> String {
    values.iter().fold(text(key), |value, (name, replacement)| {
        value.replace(&format!("{{{name}}}"), replacement)
    })
}
