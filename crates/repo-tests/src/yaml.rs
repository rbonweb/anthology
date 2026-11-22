//! YAML helpers over yaml-rust2 (MT1): the `yaml.safe_load` of `tests/*.py`.
//!
//! yaml-rust2 reads YAML 1.2 scalars: `on`, `off`, `yes` and `no` stay strings, unlike PyYAML,
//! where `on:` loads as the key `True`. Tests that read a workflow's `on:` therefore index the
//! string `"on"`.

pub use yaml_rust2::Yaml;
use yaml_rust2::YamlLoader;

/// Parses the first document of `text`; `Err` carries the scanner's message.
pub fn parse(text: &str) -> Result<Yaml, String> {
    let mut docs = YamlLoader::load_from_str(text).map_err(|e| e.to_string())?;
    if docs.is_empty() {
        return Ok(Yaml::Null);
    }
    Ok(docs.swap_remove(0))
}

/// `parse`, panicking on bad YAML (a test input that must be valid).
pub fn load(text: &str) -> Yaml {
    parse(text).unwrap_or_else(|e| panic!("invalid YAML: {e}"))
}

/// The repo-relative YAML file `rel`.
pub fn load_file(rel: &str) -> Yaml {
    parse(&crate::repo::read(rel)).unwrap_or_else(|e| panic!("{rel}: invalid YAML: {e}"))
}

/// The keys of a mapping, as strings and in file order; empty for anything else.
pub fn keys(y: &Yaml) -> Vec<String> {
    match y.as_hash() {
        Some(h) => h
            .keys()
            .filter_map(|k| k.as_str().map(str::to_string))
            .collect(),
        None => Vec::new(),
    }
}

/// The items of a sequence; empty for anything else.
pub fn items(y: &Yaml) -> &[Yaml] {
    y.as_vec().map_or(&[], |v| v.as_slice())
}

/// The strings of a sequence of scalars (non-strings are skipped).
pub fn strings(y: &Yaml) -> Vec<String> {
    items(y)
        .iter()
        .filter_map(|i| i.as_str().map(str::to_string))
        .collect()
}

/// Whether the mapping `y` has the key `key`.
pub fn has(y: &Yaml, key: &str) -> bool {
    !y[key].is_badvalue()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappings_and_sequences_read_back() {
        let y = load("a: 1\nb:\n  - x\n  - y\non: true\n");
        assert_eq!(keys(&y), ["a", "b", "on"]);
        assert_eq!(strings(&y["b"]), ["x", "y"]);
        assert_eq!(y["a"].as_i64(), Some(1));
        assert!(has(&y, "on"));
        assert!(!has(&y, "missing"));
        assert!(parse("a: [").is_err());
    }
}
